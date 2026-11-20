//! 一键修复：把「检查项 + 动作」映射成修复计划，再用可注入的执行器执行。
//! 外部命令只在用户点按钮后执行；输出逐行回调给界面。

use super::{ActionKind, Probe, items};
use crate::platform::{locate, proc};
use std::io::Read;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Other
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// 后台执行并推送输出（winget / npm / brew）。
    Run { program: String, args: Vec<String>, manual_hint: String },
    /// 打开新的可见终端窗口执行（登录类，需要用户交互）。
    Terminal { args: Vec<String> },
    /// 不执行命令，只把说明回给用户。
    Manual(String),
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn pkg_manager_plan(os: Os, winget_id: &str, brew_name: &str, hint: &str) -> Plan {
    match os {
        Os::Windows => Plan::Run {
            program: "winget".into(),
            args: s(&["install", "-e", "--id", winget_id, "--accept-package-agreements", "--accept-source-agreements"]),
            manual_hint: hint.into(),
        },
        Os::Mac => Plan::Run { program: "brew".into(), args: s(&["install", brew_name]), manual_hint: hint.into() },
        Os::Other => Plan::Manual(format!("请手动安装：{hint}")),
    }
}

fn npm_plan(package: &str) -> Plan {
    Plan::Run {
        program: "npm".into(),
        args: s(&["install", "-g", package]),
        manual_hint: "需要先安装 Node.js（https://nodejs.org），再执行 npm install -g ".to_string() + package,
    }
}

/// 修复计划。`lark_login` 在 lark-cli 还没装时动作是「安装」，按装 lark-cli 处理。
pub fn plan_for(id: &str, kind: ActionKind, os: Os) -> Result<Plan, String> {
    Ok(match (id, kind) {
        ("node", ActionKind::Install) => {
            pkg_manager_plan(os, "OpenJS.NodeJS.LTS", "node", "https://nodejs.org/zh-cn/download")
        }
        ("python", ActionKind::Install) => {
            let name = if os == Os::Mac { "python@3.12" } else { "" };
            pkg_manager_plan(os, "Python.Python.3.12", name, "https://www.python.org/downloads/")
        }
        ("git", ActionKind::Install) => pkg_manager_plan(os, "Git.Git", "git", "https://git-scm.com/downloads"),
        ("claude", ActionKind::Install) => npm_plan("@anthropic-ai/claude-code"),
        ("codex", ActionKind::Install) => npm_plan("@openai/codex"),
        ("lark_cli" | "lark_login", ActionKind::Install) => npm_plan("@larksuite/cli"),
        ("ocr", ActionKind::Install) => npm_plan("@alibaba-group/open-code-review"),
        ("claude", ActionKind::Login) => Plan::Terminal { args: s(&["claude", "auth", "login"]) },
        ("codex", ActionKind::Login) => Plan::Terminal { args: s(&["codex", "login"]) },
        ("lark_login", ActionKind::Login) => Plan::Terminal { args: s(&["lark-cli", "auth", "login"]) },
        ("system_proxy", ActionKind::Fix) => Plan::Manual(
            "客户端不会改动你的环境变量。如需移除：Windows 打开「系统属性 - 环境变量」删除 HTTPS_PROXY / HTTP_PROXY / ALL_PROXY；macOS 检查 shell 配置文件和 launchctl getenv。".into(),
        ),
        (_, ActionKind::OpenSettings) => Plan::Manual("请在客户端的设置或技能页里完成配置".into()),
        _ => return Err(format!("检查项 {id} 没有可执行的修复动作")),
    })
}

/// 外部命令执行器（可注入）。
pub trait Runner: Send + Sync {
    /// 执行并逐行回调输出（stdout 与 stderr 合并），返回退出码。
    fn run(&self, program: &str, args: &[String], on_line: &mut dyn FnMut(&str)) -> Result<i32, String>;
    fn open_terminal(&self, args: &[String]) -> Result<(), String>;
}

pub struct SystemRunner;

/// 按 `\n` 或 `\r` 切行（winget 的进度条用 `\r` 刷新），空行丢弃。
fn pump<R: Read>(mut r: R, tx: std::sync::mpsc::Sender<String>) {
    let mut buf = [0u8; 4096];
    let mut line: Vec<u8> = Vec::new();
    let flush = |line: &mut Vec<u8>| {
        let text = String::from_utf8_lossy(line).trim().to_string();
        line.clear();
        if !text.is_empty() {
            let _ = tx.send(text);
        }
    };
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                flush(&mut line);
            } else if line.len() < 8192 {
                line.push(b);
            }
        }
    }
    flush(&mut line);
}

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[String], on_line: &mut dyn FnMut(&str)) -> Result<i32, String> {
        let exe = locate::find(program).unwrap_or_else(|| program.into());
        let mut cmd = Command::new(&exe);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = proc::spawn_hidden(&mut cmd).map_err(|e| format!("无法运行 {program}：{e}"))?;
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(o) = child.stdout.take() {
            let tx = tx.clone();
            std::thread::spawn(move || pump(o, tx));
        }
        if let Some(e) = child.stderr.take() {
            let tx = tx.clone();
            std::thread::spawn(move || pump(e, tx));
        }
        drop(tx);
        for line in rx {
            on_line(&line);
        }
        let status = child.wait().map_err(|e| format!("等待 {program} 结束失败：{e}"))?;
        Ok(status.code().unwrap_or(-1))
    }

    fn open_terminal(&self, args: &[String]) -> Result<(), String> {
        #[cfg(windows)]
        {
            // 不设 CREATE_NO_WINDOW：就是要弹出可见窗口；`start` 开新控制台，`cmd /k` 让窗口保留。
            let mut cmd = Command::new("cmd");
            cmd.args(["/c", "start", "", "cmd", "/k"]).args(args);
            cmd.spawn().map(|_| ()).map_err(|e| format!("打开终端失败：{e}"))
        }
        #[cfg(not(windows))]
        {
            let line = args.join(" ");
            let script = format!("tell application \"Terminal\" to do script \"{}\"", line.replace('"', "'"));
            Command::new("osascript")
                .args(["-e", &script, "-e", "tell application \"Terminal\" to activate"])
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("打开终端失败：{e}"))
        }
    }
}

/// 执行修复：先重新检查拿到当前动作，再按计划执行。返回给界面的结束说明；Err 同样是说明文字。
pub fn fix(
    id: &str,
    probe: &dyn Probe,
    runner: &dyn Runner,
    os: Os,
    on_line: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let item = items::check(id, probe);
    let Some(action) = item.action else {
        return Ok(format!("{} 当前不需要修复", item.label));
    };
    match plan_for(id, action.kind, os)? {
        Plan::Manual(text) => Ok(text),
        Plan::Terminal { args } => {
            on_line(&format!("已打开终端窗口：{}，请在窗口里完成登录，完成后回到这里点「重新检查」", args.join(" ")));
            runner.open_terminal(&args)?;
            Ok("已打开登录终端，完成登录后请重新检查".into())
        }
        Plan::Run { program, args, manual_hint } => {
            on_line(&format!("> {program} {}", args.join(" ")));
            match runner.run(&program, &args, on_line) {
                Ok(0) => Ok(format!("{} 安装完成，请重新检查", item.label)),
                Ok(code) => Err(format!("安装失败（退出码 {code}），原始输出见上方。可手动安装：{manual_hint}")),
                Err(e) => Err(format!("{e}。可手动安装：{manual_hint}")),
            }
        }
    }
}
