//! 以「当前登录用户」的身份调用本机 lark-cli（机器人身份只收发消息，其余都走这里）。
//!
//! Windows 上 `lark-cli` 是 npm 的 `.cmd` 壳，多行参数会被截断，所以定位到它的
//! `run.js` 后直接用 `node run.js` 调用。调用时剔除旧 bridge 注入的环境变量，
//! 否则 lark-cli 会被绑成机器人身份。

use std::path::{Path, PathBuf};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// 旧 lark-channel-bridge 注入的变量：带着它们 lark-cli 会回落到 bot-only 配置。
const BRIDGE_ENV_KEYS: [&str; 4] = [
    "LARK_CHANNEL",
    "LARK_CHANNEL_HOME",
    "LARK_CHANNEL_PROFILE",
    "LARKSUITE_CLI_CONFIG_DIR",
];

/// 由 `lark-cli` 可执行文件（或 `.cmd` 壳）所在位置推出 `run.js`：
/// `<npm 全局目录>/node_modules/@larksuite/cli/scripts/run.js`。
pub fn run_js_path(lark_cli_bin: &Path) -> PathBuf {
    let dir = lark_cli_bin.parent().unwrap_or_else(|| Path::new("."));
    dir.join("node_modules")
        .join("@larksuite")
        .join("cli")
        .join("scripts")
        .join("run.js")
}

/// 去掉 bridge 注入的环境变量（Windows 下变量名不区分大小写，所以忽略大小写比较）。
/// 用 `env::vars_os()` 传入：`env::vars()` 遇到非 UTF-8 的变量会 panic（第一阶段遗留）。
pub fn clean_env<K: AsRef<std::ffi::OsStr>, V>(vars: impl IntoIterator<Item = (K, V)>) -> Vec<(K, V)> {
    vars.into_iter()
        .filter(|(key, _)| {
            let key = key.as_ref().to_string_lossy();
            !BRIDGE_ENV_KEYS.iter().any(|k| k.eq_ignore_ascii_case(&key))
        })
        .collect()
}

/// 单次 lark-cli 调用的超时（防止卡在登录交互里永远不返回）。
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// 带超时执行：stdin 置空，超时杀掉整个进程树。
fn run_command(mut command: Command, timeout: Duration) -> Result<String, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("无法启动 node 调用 lark-cli：{e}"))?;
    // 单独线程读管道，避免输出写满缓冲区把子进程卡住。
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                kill_tree(&mut child);
                let _ = child.wait();
                return Err("lark-cli 调用超时".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("等待 lark-cli 失败：{e}")),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if !status.success() {
        let detail: String = format!(
            "{}{}",
            String::from_utf8_lossy(&stderr),
            String::from_utf8_lossy(&stdout)
        )
        .chars()
        .take(300)
        .collect();
        return Err(format!("lark-cli 调用失败：{}", detail.trim()));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

/// node 可能再拉起 lark-cli 本体，所以 Windows 下用 taskkill /T 结束整棵树。
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .creation_flags(0x0800_0000)
            .output();
    }
    let _ = child.kill();
}

#[derive(Debug, Clone)]
pub struct LarkCli {
    run_js: PathBuf,
}

impl LarkCli {
    /// 在 PATH 里找 `lark-cli` 并定位其 `run.js`。
    pub fn detect() -> Result<Self, String> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let names: &[&str] = if cfg!(windows) {
            &["lark-cli.cmd", "lark-cli.exe", "lark-cli"]
        } else {
            &["lark-cli"]
        };
        let bin = std::env::split_paths(&path)
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .find(|candidate| candidate.is_file())
            .ok_or("找不到 lark-cli，请先安装并登录（npm i -g @larksuite/cli）")?;
        let run_js = run_js_path(&bin);
        if !run_js.is_file() {
            return Err(format!("找到 lark-cli 但缺少 {}", run_js.display()));
        }
        Ok(Self { run_js })
    }

    /// 执行 lark-cli，返回 stdout（UTF-8）。非 0 退出码返回带 stderr 摘要的错误；
    /// 超过 `CALL_TIMEOUT` 杀掉进程并报错（同步接口，调用方应放进 spawn_blocking）。
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut command = Command::new("node");
        command
            .arg(&self.run_js)
            .args(args)
            .env_clear()
            .envs(clean_env(std::env::vars_os()));
        run_command(command, CALL_TIMEOUT)
    }

    fn run_json(&self, args: &[&str]) -> Result<serde_json::Value, String> {
        let out = self.run(args)?;
        serde_json::from_str(&out).map_err(|e| format!("lark-cli 输出不是 JSON：{e}"))
    }

    /// 当前登录用户（即机器人主人）的身份：lark-cli 应用下的 open_id + 跨应用一致的 union_id。
    pub fn current_user(&self) -> Result<UserIds, String> {
        let value = self.run_json(&["contact", "+get-user", "--as", "user"])?;
        Ok(UserIds {
            open_id: pick(&value, "/data/user/open_id", "当前登录用户的 open_id")?,
            union_id: pick(&value, "/data/user/union_id", "当前登录用户的 union_id")?,
        })
    }

    pub fn chat_name(&self, chat_id: &str) -> Result<String, String> {
        let value = self.run_json(&["im", "chats", "get", "--chat-id", chat_id, "--as", "user"])?;
        pick(&value, "/data/name", "群名称")
    }

    /// 按 union_id 查姓名。机器人事件里的 open_id 属于机器人应用，拿到 lark-cli 应用里查会报 99992361，
    /// union_id 跨应用一致（参数已用 `lark-cli contact +get-user --help` 核对：`--user-id-type union_id`）。
    pub fn user_name(&self, union_id: &str) -> Result<String, String> {
        let value = self.run_json(&user_name_args(union_id))?;
        pick(&value, "/data/user/name", "用户姓名")
    }

    /// 取一条消息的文本（被引用消息进 prompt 用）。
    pub fn message_text(&self, message_id: &str) -> Result<String, String> {
        let value = self.run_json(&["im", "+messages-mget", "--message-ids", message_id, "--as", "user"])?;
        pick(&value, "/data/messages/0/content", "消息内容")
    }

    /// 我能看到的任务清单（建飞书任务时选清单用），最多一页 50 个。
    pub fn tasklists(&self) -> Result<Vec<(String, String)>, String> {
        let v = self.run_json(&["task", "tasklists", "list", "--as", "user", "--params", r#"{"page_size":50}"#])?;
        Ok(v.pointer("/data/items")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| Some((t["guid"].as_str()?.to_string(), t["name"].as_str().unwrap_or_default().to_string())))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// 按姓名 / 拼音 / 邮箱搜通讯录：(open_id, 姓名, 部门)。
    pub fn search_users(&self, query: &str) -> Result<Vec<(String, String, String)>, String> {
        let v = self.run_json(&["contact", "+search-user", "--as", "user", "--query", query, "--page-size", "10"])?;
        let strip = |s: &str| s.replace("<h>", "").replace("</h>", "");
        Ok(v.pointer("/data/users")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|u| {
                        Some((
                            u["open_id"].as_str()?.to_string(),
                            strip(u["localized_name"].as_str().or(u["name"].as_str()).unwrap_or_default()),
                            strip(u["department"].as_str().unwrap_or_default()),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// 以用户身份新建一份飞书文档，返回文档链接（新群自动接入时建记录文档用）。
    pub fn create_doc(&self, title: &str) -> Result<String, String> {
        let value = self.run_json(&[
            "docs",
            "+create",
            "--as",
            "user",
            "--doc-format",
            "markdown",
            "--title",
            title,
            "--content",
            DOC_INTRO,
        ])?;
        pick(&value, "/data/document/url", "新文档链接")
    }
}

fn user_name_args(union_id: &str) -> [&str; 8] {
    ["contact", "+get-user", "--user-id", union_id, "--user-id-type", "union_id", "--as", "user"]
}

/// 用户身份。open_id 按应用区分（lark-cli 与机器人应用各是一套），union_id 同一企业内跨应用一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserIds {
    pub open_id: String,
    pub union_id: String,
}

/// 新建记录文档的正文（单行，避免多行参数问题）。
const DOC_INTRO: &str = "本文档由 Beetle 自动创建，用于记录本群的问题排查结论与修复建议。";

/// 上层（supervisor / onboard）用到的 lark-cli 能力。抽成 trait 便于测试替换；全部是同步调用，
/// 在异步上下文里要放进 `spawn_blocking`。
pub trait LarkOps: Send + Sync {
    fn current_user(&self) -> Result<UserIds, String>;
    fn chat_name(&self, chat_id: &str) -> Result<String, String>;
    /// 按 union_id 查用户姓名。
    fn user_name(&self, union_id: &str) -> Result<String, String>;
    fn message_text(&self, message_id: &str) -> Result<String, String>;
    fn create_doc(&self, title: &str) -> Result<String, String>;
}

impl LarkOps for LarkCli {
    fn current_user(&self) -> Result<UserIds, String> {
        LarkCli::current_user(self)
    }
    fn chat_name(&self, chat_id: &str) -> Result<String, String> {
        LarkCli::chat_name(self, chat_id)
    }
    fn user_name(&self, union_id: &str) -> Result<String, String> {
        LarkCli::user_name(self, union_id)
    }
    fn message_text(&self, message_id: &str) -> Result<String, String> {
        LarkCli::message_text(self, message_id)
    }
    fn create_doc(&self, title: &str) -> Result<String, String> {
        LarkCli::create_doc(self, title)
    }
}

/// 生产用：每次调用时现找本机 lark-cli（客户端运行中途才装好 / 登录也能用上）。
pub struct SystemLark;

impl LarkOps for SystemLark {
    fn current_user(&self) -> Result<UserIds, String> {
        LarkCli::detect()?.current_user()
    }
    fn chat_name(&self, chat_id: &str) -> Result<String, String> {
        LarkCli::detect()?.chat_name(chat_id)
    }
    fn user_name(&self, union_id: &str) -> Result<String, String> {
        LarkCli::detect()?.user_name(union_id)
    }
    fn message_text(&self, message_id: &str) -> Result<String, String> {
        LarkCli::detect()?.message_text(message_id)
    }
    fn create_doc(&self, title: &str) -> Result<String, String> {
        LarkCli::detect()?.create_doc(title)
    }
}

fn pick(value: &serde_json::Value, pointer: &str, what: &str) -> Result<String, String> {
    value
        .pointer(pointer)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("lark-cli 没有返回{what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_js_sits_next_to_the_shim_in_node_modules() {
        let bin = Path::new("C:/npm/global/lark-cli.cmd");
        assert_eq!(
            run_js_path(bin),
            Path::new("C:/npm/global/node_modules/@larksuite/cli/scripts/run.js")
        );
    }

    #[test]
    fn clean_env_drops_bridge_variables_only() {
        let vars = [
            ("PATH", "x"),
            ("LARK_CHANNEL", "1"),
            ("LARK_CHANNEL_HOME", "h"),
            ("lark_channel_profile", "p"), // Windows 下大小写不敏感
            ("LARKSUITE_CLI_CONFIG_DIR", "d"),
            ("LARK_OTHER", "keep"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
        let kept = clean_env(vars);
        let keys: Vec<&str> = kept.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["PATH", "LARK_OTHER"]);
    }

    /// 非 UTF-8 的环境变量不会让调用 panic（OsString 原样透传）。
    #[test]
    fn clean_env_accepts_os_strings() {
        use std::ffi::OsString;
        let vars = vec![
            (OsString::from("LARK_CHANNEL"), OsString::from("1")),
            (OsString::from("KEEP"), OsString::from("v")),
        ];
        let kept = clean_env(vars);
        assert_eq!(kept, vec![(OsString::from("KEEP"), OsString::from("v"))]);
        // 真实环境变量也能直接传
        let _ = clean_env(std::env::vars_os());
    }

    #[test]
    fn user_name_looks_up_by_union_id() {
        assert_eq!(
            user_name_args("on_abc"),
            ["contact", "+get-user", "--user-id", "on_abc", "--user-id-type", "union_id", "--as", "user"]
        );
    }

    #[test]
    fn pick_reports_missing_field_in_chinese() {
        let value = serde_json::json!({"data": {"name": ""}});
        assert_eq!(pick(&value, "/data/name", "群名称").unwrap_err(), "lark-cli 没有返回群名称");
    }

    #[test]
    fn hanging_command_times_out_and_is_killed() {
        let mut command = Command::new("node");
        command.args(["-e", "setTimeout(() => {}, 20000)"]);
        let started = Instant::now();
        let err = run_command(command, Duration::from_millis(500)).unwrap_err();
        assert_eq!(err, "lark-cli 调用超时");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn finished_command_returns_stdout_and_stdin_is_closed() {
        let mut command = Command::new("node");
        command.args(["-e", "process.stdin.on('end', () => console.log('关闭')); process.stdin.resume()"]);
        let out = run_command(command, Duration::from_secs(10)).unwrap();
        assert_eq!(out.trim(), "关闭");
    }
}
