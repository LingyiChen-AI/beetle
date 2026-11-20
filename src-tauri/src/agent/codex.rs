//! Codex 引擎（codex-cli 0.167.0，2026-11-19 本机实测）。
//!
//! 启动参数：`exec --json --skip-git-repo-check -C <ws> -o <ws>/.beetle-last.txt
//! -c approval_policy="never" -s workspace-write -c sandbox_workspace_write.network_access=true -`
//! 全部被真实 CLI 接受，与任务书一致，无需修正。续聊是 `exec [以上选项] resume <id> -`：
//! `exec resume` 子命令自己没有 `-C` / `-s`，所以这些选项必须写在 `resume` 之前（实测可续上同一 thread）。
//! prompt 走 stdin（参数 `-`）。
//!
//! 沙箱与额外可写目录（实测）：`-s workspace-write` 只允许写工作目录（及系统临时目录），code-sync 写的代码缓存
//! 在工作区外，所以对 `AgentRequest.extra_writable_dirs` 的每个目录追加
//! `--add-dir <dir>`（写在 `resume` 之前）。在 `C:/Users/…/bp-sbx/{ws,ok,no}`（非临时目录）里实测：
//! `-C <ws> --add-dir <ok>` 时 `ok/t.txt` 创建成功，未加的 `no/` 被沙箱拒绝（访问被拒绝）；
//! `exec … --add-dir <ok> resume <id> -` 被接受并续上同一 thread，`ok/t2.txt` 创建成功。
//! 路径格式：`-C`、`-o`、`--add-dir` 传**正斜杠** Windows 路径（`C:/Users/...`）均被接受并生效，故统一转成正斜杠。
//! 注意系统临时目录（%TEMP%）本来就可写，拿它测「未加目录被拒」会得到假阴性。
//!
//! 项目级加载实测（临时目录里放 `AGENTS.md`、`.agents/skills/<x>/SKILL.md`、`.codex/rules/*.rules`，
//! 用上面的参数 + `-C <dir>` 跑 `codex exec`，**不加任何额外 `-c`**）：
//! - AGENTS.md：会被加载（模型答出了其中的暗号）。
//! - 项目级 skill：`<ws>/.agents/skills/<x>/SKILL.md` 可见，模型能列出并读取该技能。
//! - 规则文件：`<ws>/.codex/rules/*.rules` 自动生效，命令在执行前被拒绝，模型转述的原因正是规则里的 justification。
//!   因此不需要额外的 `-c` 或用户级 rules；不要给 codex 传 `--ignore-rules`。
//!
//! 事件流（见 tests/fixtures/agent/codex-*.jsonl）：`thread.started{thread_id}`、`item.started/completed`
//! （`command_execution` / `agent_message` / `reasoning` / `file_change` / `mcp_tool_call` / `web_search` / `error`）、
//! `turn.completed`、`turn.failed{error.message}`、顶层 `error`。命令在 `item.started` 时登记一步，
//! `item.completed` 不再重复。`item.completed` 的 `type=error` 只是警告（如模型元数据缺失），不是失败。
//! `agent_message` 只记 Thinking；最终答案只读 `-o` 文件（`final_from_files`）。
//! 无效 `exec resume <id>` 时 stdout 一行都没有，只有 stderr 报错且进程非 0 退出——由进程层按「无 Final」处理。
//! 注意：Codex 在 Windows 上把命令包成 `powershell.exe -Command "<原命令>"`，分类时已先剥掉这层。

use super::steps::{classify_command, truncate_chars};
use super::{AgentEngine, AgentEvent, AgentRequest, LaunchSpec, common_env};
use crate::store::config::Engine;
use crate::store::runs::StepKind;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Codex;

const LAST_MESSAGE_NAME: &str = ".beetle-last.txt";

impl AgentEngine for Codex {
    fn engine(&self) -> Engine {
        Engine::Codex
    }

    fn launch(&self, program: &Path, req: &AgentRequest) -> LaunchSpec {
        let ws = req.workspace.to_string_lossy().replace('\\', "/");
        let last = format!("{}/{LAST_MESSAGE_NAME}", ws.trim_end_matches('/'));
        let mut args: Vec<String> = [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "-C",
            &ws,
            "-o",
            &last,
            "-c",
            "approval_policy=\"never\"",
            "-s",
            "workspace-write",
            "-c",
            "sandbox_workspace_write.network_access=true",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for d in &req.extra_writable_dirs {
            args.push("--add-dir".into());
            args.push(d.to_string_lossy().replace('\\', "/"));
        }
        if let Some(id) = &req.resume_session {
            args.push("resume".into());
            args.push(id.clone());
        }
        args.push("-".into());
        LaunchSpec {
            program: program.to_path_buf(),
            args,
            cwd: req.workspace.clone(),
            env: common_env(req),
            stdin: req.prompt.clone(),
            last_message_file: Some(PathBuf::from(last)),
        }
    }

    fn parse_line(&self, line: &[u8]) -> Vec<AgentEvent> {
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            return vec![];
        };
        match v["type"].as_str().unwrap_or("") {
            "thread.started" => v["thread_id"]
                .as_str()
                .map(|id| vec![AgentEvent::Session(id.to_string())])
                .unwrap_or_default(),
            "turn.failed" => {
                let m = v["error"]["message"].as_str().unwrap_or("Codex 运行失败");
                vec![AgentEvent::Error(truncate_chars(m, 2000))]
            }
            "error" => vec![step(
                StepKind::Error,
                "Codex 报告错误",
                v["message"].as_str().unwrap_or(""),
            )],
            "item.started" => parse_started(&v["item"]),
            "item.completed" => parse_completed(&v["item"]),
            _ => vec![],
        }
    }

    fn final_from_files(&self, spec: &LaunchSpec) -> Option<String> {
        let f = spec.last_message_file.as_ref()?;
        let text = std::fs::read(f).ok()?;
        let text = String::from_utf8_lossy(&text);
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }
}

fn step(kind: StepKind, title: &str, detail: &str) -> AgentEvent {
    AgentEvent::Step {
        kind,
        title: title.to_string(),
        detail: truncate_chars(detail, 2000),
    }
}

fn parse_started(item: &Value) -> Vec<AgentEvent> {
    match item["type"].as_str().unwrap_or("") {
        "command_execution" => {
            let cmd = item["command"].as_str().unwrap_or("");
            let (kind, title) = classify_command(cmd);
            vec![step(kind, &title, cmd)]
        }
        "mcp_tool_call" => {
            let name = format!(
                "{}.{}",
                item["server"].as_str().unwrap_or("mcp"),
                item["tool"].as_str().unwrap_or("tool")
            );
            vec![step(StepKind::Tool, &name, &item["arguments"].to_string())]
        }
        "web_search" => vec![step(
            StepKind::Tool,
            &format!("搜索 {}", truncate_chars(item["query"].as_str().unwrap_or(""), 120)),
            "",
        )],
        _ => vec![],
    }
}

fn parse_completed(item: &Value) -> Vec<AgentEvent> {
    match item["type"].as_str().unwrap_or("") {
        // 助手文本只进运行日志，绝不当最终答案
        "agent_message" | "reasoning" => {
            let t = item["text"].as_str().unwrap_or("").trim();
            if t.is_empty() {
                vec![]
            } else {
                vec![step(StepKind::Thinking, "思考", &truncate_chars(t, 200))]
            }
        }
        "file_change" => vec![step(StepKind::Tool, "修改文件", &item["changes"].to_string())],
        "error" => vec![step(
            StepKind::Error,
            "Codex 提示",
            item["message"].as_str().unwrap_or(""),
        )],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::super::{fixture, parse_all};
    use super::*;
    use crate::store::runs::StepKind;
    use std::path::PathBuf;

    fn req(resume: Option<&str>) -> AgentRequest {
        AgentRequest {
            workspace: PathBuf::from("C:/ws/bot/g"),
            runtime_json: PathBuf::from("C:/ws/bot/g/beetle.runtime.json"),
            prompt: "查一下".into(),
            resume_session: resume.map(String::from),
            proxy: None,
            extra_writable_dirs: vec![],
        }
    }

    #[test]
    fn codex_ok_fixture_yields_session_and_final_from_file() {
        let ev = parse_all(&Codex, &fixture("codex-ok.jsonl"));
        assert!(matches!(ev.first(), Some(AgentEvent::Session(s)) if s == "01a1f239-9494-7390-aab9-3564643216ff"));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Step { kind: StepKind::Tool, .. })));
        // agent_message 只是 Thinking，事件流里没有 Final
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Final(_))), "{ev:?}");
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Step { kind: StepKind::Thinking, .. })));
        let dir = tempfile::tempdir().unwrap();
        let spec = Codex.launch(Path::new("codex"), &AgentRequest { workspace: dir.path().into(), ..req(None) });
        let f = spec.last_message_file.clone().unwrap();
        assert_eq!(Codex.final_from_files(&spec), None, "文件不存在时没有最终答案");
        std::fs::write(&f, "OK\n").unwrap();
        assert_eq!(Codex.final_from_files(&spec), Some("OK".to_string()));
        std::fs::write(&f, "  \n").unwrap();
        assert_eq!(Codex.final_from_files(&spec), None, "空文件不算最终答案");
    }

    #[test]
    fn codex_error_fixture_yields_error_not_final() {
        let ev = parse_all(&Codex, &fixture("codex-error.jsonl"));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Error(m) if m.contains("bogus-model-x"))));
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Final(_))));
        // 「Model metadata ... not found」只是警告，不是失败
        assert_eq!(ev.iter().filter(|e| matches!(e, AgentEvent::Error(_))).count(), 1, "{ev:?}");
    }

    #[test]
    fn codex_resume_fixture_yields_same_session() {
        let ev = parse_all(&Codex, &fixture("codex-resume.jsonl"));
        assert!(matches!(ev.first(), Some(AgentEvent::Session(s)) if s == "01a1f239-9494-7390-aab9-3564643216ff"));
    }

    #[test]
    fn codex_launch_args_exact() {
        let spec = Codex.launch(Path::new("codex"), &req(None));
        let ws = "C:/ws/bot/g";
        let expect: Vec<String> = [
            "exec", "--json", "--skip-git-repo-check", "-C", ws, "-o", "C:/ws/bot/g/.beetle-last.txt",
            "-c", "approval_policy=\"never\"", "-s", "workspace-write",
            "-c", "sandbox_workspace_write.network_access=true", "-",
        ]
        .iter().map(|s| s.to_string()).collect();
        assert_eq!(spec.args, expect);
        assert_eq!(spec.stdin, "查一下");
        assert_eq!(spec.cwd, PathBuf::from(ws));
        assert_eq!(spec.last_message_file, Some(PathBuf::from("C:/ws/bot/g/.beetle-last.txt")));
    }

    #[test]
    fn codex_resume_args_put_options_before_subcommand() {
        let spec = Codex.launch(Path::new("codex"), &req(Some("sid-9")));
        let n = spec.args.len();
        assert_eq!(&spec.args[n - 3..], ["resume", "sid-9", "-"]);
        assert_eq!(spec.args[0], "exec");
        assert!(spec.args.contains(&"-C".to_string()));
    }

    #[test]
    fn codex_extra_writable_dirs_become_add_dir_before_resume() {
        let dirs = vec![PathBuf::from(r"C:\Users\u\code-cache"), PathBuf::from("C:/data/extra")];
        let r = |resume| AgentRequest { extra_writable_dirs: dirs.clone(), ..req(resume) };
        let adds = |a: &[String]| {
            a.windows(2).filter(|w| w[0] == "--add-dir").map(|w| w[1].clone()).collect::<Vec<_>>()
        };
        let spec = Codex.launch(Path::new("codex"), &r(None));
        assert_eq!(adds(&spec.args), ["C:/Users/u/code-cache", "C:/data/extra"]);
        assert_eq!(spec.args.last().unwrap(), "-");
        let spec = Codex.launch(Path::new("codex"), &r(Some("sid-9")));
        assert_eq!(adds(&spec.args), ["C:/Users/u/code-cache", "C:/data/extra"]);
        let resume_at = spec.args.iter().position(|a| a == "resume").unwrap();
        let last_add = spec.args.iter().rposition(|a| a == "--add-dir").unwrap();
        assert!(last_add + 1 < resume_at, "--add-dir 必须在 resume 之前：{:?}", spec.args);
        // 没有额外目录时不出现 --add-dir
        assert!(!Codex.launch(Path::new("codex"), &req(None)).args.contains(&"--add-dir".to_string()));
    }

    #[test]
    fn codex_command_item_is_classified_once() {
        let started = br#"{"type":"item.started","item":{"id":"i","type":"command_execution","command":"powershell.exe -Command \"python C:/x/sync_code.py --repo a\"","status":"in_progress"}}"#;
        let done = br#"{"type":"item.completed","item":{"id":"i","type":"command_execution","command":"powershell.exe -Command \"python C:/x/sync_code.py --repo a\"","exit_code":0,"status":"completed"}}"#;
        assert!(matches!(&Codex.parse_line(started)[..], [AgentEvent::Step { kind: StepKind::SyncCode, .. }]));
        assert!(Codex.parse_line(done).is_empty());
    }
}
