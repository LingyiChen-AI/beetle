//! Claude Code 引擎。
//!
//! 事件流（`claude -p --output-format stream-json --verbose`，Claude Code 2.1.327 实录，见
//! tests/fixtures/agent/claude-*.jsonl）：
//! - `system/init`：带 `session_id`；SessionStart 钩子的 `system/hook_*` 事件很多，全部忽略。
//! - `assistant`：`message.content[]` 里有 `text` / `tool_use`。**文本可能是残缺的**（夹具里第一条就是
//!   一段坏掉的工具调用文本，随后被重试），所以文本只记 Thinking 步骤，绝不当最终答案。
//! - `result`：最终结果。`is_error=false` 且 `result` 非空才是 Final；失败时 `is_error=true`，
//!   文字在 `result`（API 错误）或 `errors[]`（如无效 `--resume` id 的 `error_during_execution`）。
//! - 进程被杀 / 崩溃 = 没有 `result` 行 = 没有 Final（claude-no-result.jsonl）。

use super::steps::{classify_command, truncate_chars};
use super::{AgentEngine, AgentEvent, AgentRequest, LaunchSpec, common_env};
use crate::store::config::Engine;
use crate::store::runs::StepKind;
use serde_json::Value;
use std::path::Path;

pub struct Claude;

impl AgentEngine for Claude {
    fn engine(&self) -> Engine {
        Engine::Claude
    }

    fn launch(&self, program: &Path, req: &AgentRequest) -> LaunchSpec {
        let mut args: Vec<String> = [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "bypassPermissions",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(id) = &req.resume_session {
            args.push("--resume".into());
            args.push(id.clone());
        }
        LaunchSpec {
            program: program.to_path_buf(),
            args,
            cwd: req.workspace.clone(),
            env: common_env(req),
            stdin: req.prompt.clone(),
            last_message_file: None,
        }
    }

    fn parse_line(&self, line: &[u8]) -> Vec<AgentEvent> {
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            return vec![];
        };
        match v["type"].as_str().unwrap_or("") {
            "system" => {
                if v["subtype"] == "init"
                    && let Some(id) = v["session_id"].as_str()
                {
                    return vec![AgentEvent::Session(id.to_string())];
                }
                vec![]
            }
            "assistant" => parse_assistant(&v),
            "result" => parse_result(&v),
            _ => vec![],
        }
    }

    fn final_from_files(&self, _spec: &LaunchSpec) -> Option<String> {
        None
    }
}

fn parse_assistant(v: &Value) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    let Some(blocks) = v["message"]["content"].as_array() else {
        return out;
    };
    for b in blocks {
        match b["type"].as_str().unwrap_or("") {
            "text" => {
                let t = b["text"].as_str().unwrap_or("").trim();
                if !t.is_empty() {
                    out.push(AgentEvent::Step {
                        kind: StepKind::Thinking,
                        title: "思考".into(),
                        detail: truncate_chars(t, 200),
                    });
                }
            }
            "tool_use" => out.push(tool_step(
                b["name"].as_str().unwrap_or("工具"),
                &b["input"],
            )),
            _ => {}
        }
    }
    out
}

fn tool_step(name: &str, input: &Value) -> AgentEvent {
    if matches!(name, "Bash" | "PowerShell")
        && let Some(cmd) = input["command"].as_str()
    {
        let (kind, title) = classify_command(cmd);
        return AgentEvent::Step {
            kind,
            title,
            detail: truncate_chars(cmd, 2000),
        };
    }
    let summary = ["file_path", "pattern", "path", "url", "query", "description"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .unwrap_or("");
    let title = if summary.is_empty() {
        name.to_string()
    } else {
        format!("{name} {}", truncate_chars(summary, 120))
    };
    AgentEvent::Step {
        kind: StepKind::Tool,
        title,
        detail: truncate_chars(&input.to_string(), 500),
    }
}

fn parse_result(v: &Value) -> Vec<AgentEvent> {
    let text = v["result"].as_str().unwrap_or("").trim();
    if v["is_error"].as_bool().unwrap_or(false) {
        let errors = v["errors"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str())
                    .collect::<Vec<_>>()
                    .join("；")
            })
            .unwrap_or_default();
        let msg = if !text.is_empty() {
            text.to_string()
        } else if !errors.is_empty() {
            errors
        } else {
            format!("Claude 运行失败（{}）", v["subtype"].as_str().unwrap_or("未知"))
        };
        return vec![AgentEvent::Error(truncate_chars(&msg, 2000))];
    }
    if text.is_empty() {
        return vec![AgentEvent::Error("Claude 没有给出最终结果".into())];
    }
    vec![AgentEvent::Final(text.to_string())]
}

#[cfg(test)]
mod tests {
    use super::super::{fixture, parse_all};
    use super::*;
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
    fn claude_ok_fixture_yields_session_steps_and_final() {
        let ev = parse_all(&Claude, &fixture("claude-ok.jsonl"));
        assert!(matches!(ev.first(), Some(AgentEvent::Session(s)) if s == "5acf6c3a-0918-4494-aa50-2422a8eefbdd"));
        assert!(ev.iter().any(|e| matches!(
            e,
            AgentEvent::Step { kind: StepKind::Tool | StepKind::ReadChat, title, .. } if title == "ls"
        )));
        assert!(matches!(ev.last(), Some(AgentEvent::Final(t)) if t.contains("OK")));
        // 只有一个 Final，且助手文本（含坏掉的那段）没有变成 Final
        assert_eq!(ev.iter().filter(|e| matches!(e, AgentEvent::Final(_))).count(), 1);
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Step { kind: StepKind::Thinking, .. })));
    }

    #[test]
    fn claude_error_result_yields_error_not_final() {
        let ev = parse_all(&Claude, &fixture("claude-error.jsonl"));
        assert!(ev.iter().any(|e| matches!(e, AgentEvent::Error(m) if m.contains("bogus-model-x"))));
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Final(_))), "{ev:?}");
    }

    #[test]
    fn claude_resume_error_reads_errors_array() {
        let ev = parse_all(&Claude, &fixture("claude-error-resume.jsonl"));
        assert!(matches!(&ev[..], [AgentEvent::Error(m)] if m.contains("No conversation found")), "{ev:?}");
    }

    #[test]
    fn claude_no_result_yields_no_final() {
        let ev = parse_all(&Claude, &fixture("claude-no-result.jsonl"));
        assert!(!ev.is_empty());
        assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Final(_) | AgentEvent::Error(_))), "{ev:?}");
    }

    #[test]
    fn claude_bash_tool_use_is_classified() {
        let line = br#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"python C:/x/chat-context/scripts/chat_context.py --limit 20"}},{"type":"tool_use","name":"Read","input":{"file_path":"C:/ws/a.txt"}}]}}"#;
        let ev = Claude.parse_line(line);
        assert!(matches!(&ev[0], AgentEvent::Step { kind: StepKind::ReadChat, .. }));
        assert!(matches!(&ev[1], AgentEvent::Step { kind: StepKind::Tool, title, .. } if title == "Read C:/ws/a.txt"));
    }

    #[test]
    fn claude_garbage_line_is_ignored() {
        assert!(Claude.parse_line(b"not json").is_empty());
        assert!(Claude.parse_line(b"").is_empty());
    }

    #[test]
    fn claude_launch_args_exact() {
        let spec = Claude.launch(Path::new("claude"), &req(None));
        assert_eq!(
            spec.args,
            ["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", "bypassPermissions"]
        );
        assert_eq!(spec.cwd, PathBuf::from("C:/ws/bot/g"));
        assert_eq!(spec.stdin, "查一下");
        assert_eq!(spec.last_message_file, None);
        assert_eq!(spec.program, PathBuf::from("claude"));

        let with_dirs = AgentRequest { extra_writable_dirs: vec![PathBuf::from("C:/x")], ..req(None) };
        assert_eq!(Claude.launch(Path::new("claude"), &with_dirs).args, spec.args, "Claude 忽略 extra_writable_dirs");

        let spec = Claude.launch(Path::new("claude"), &req(Some("sid-1")));
        assert_eq!(&spec.args[spec.args.len() - 2..], ["--resume", "sid-1"]);
        assert_eq!(spec.args.len(), 8);
    }
}
