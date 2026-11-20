//! 测试用假 Agent：在临时目录里生成一个可执行脚本，`RunnerDeps.locate` 指向它。
//! Windows 是 `.cmd` 批处理（Rust 对 `.cmd` 走 cmd.exe），其它平台是 `#!/bin/sh` 脚本并置上可执行位。
//! 真实引擎（Claude / Codex）照常生成启动参数，`proc::spawn` 照常拉起，脚本忽略参数，
//! 按模式回放夹具、睡眠或直接退出。`--version` 一律秒回，避免 `locate::version` 被睡眠模式卡住。
//! 每次被调用都把收到的参数写进同目录的 `args.txt`，测试据此检查 `--resume` 等参数。
//!
//! 两边行为要一致：`cat` 对应 `type`，`sleep 60` 对应 `ping -n 61`（都是独立的孙进程，
//! 用来验证取消 / 超时时整棵进程树都被结束）。

use std::path::{Path, PathBuf};

pub enum Mode<'a> {
    /// 原样输出夹具文件的每一行，然后以 `code` 退出。
    Replay { fixture: &'a Path, code: i32 },
    /// 先输出一行 init，再睡 60 秒（孙进程，被杀时一并结束）。
    Sleep,
    /// 先把 `text` 写进 `file`（模拟 Codex 的 `-o` 最后消息文件），再回放夹具并以 `code` 退出。
    WriteThenReplay { file: &'a Path, text: &'a str, fixture: &'a Path, code: i32 },
    /// 往 stderr 写 `text` 后以 `code` 退出（stdout 无输出）。
    StderrExit { text: &'a str, code: i32 },
}

/// Sleep 模式先吐的那行，测试据此确认 Agent 真的起来了。
const INIT_LINE: &str = r#"{"type":"system","subtype":"init","session_id":"s-sleep"}"#;

pub fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agent").join(name)
}

pub fn args_file(dir: &Path) -> PathBuf {
    dir.join("args.txt")
}

pub fn make(dir: &Path, mode: Mode) -> PathBuf {
    #[cfg(windows)]
    let (name, script) = ("fake-agent.cmd", cmd_script(dir, mode));
    #[cfg(not(windows))]
    let (name, script) = ("fake-agent", sh_script(dir, mode));
    let p = dir.join(name);
    std::fs::write(&p, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

#[cfg(windows)]
fn cmd_script(dir: &Path, mode: Mode) -> String {
    let mut s = String::from("@echo off\r\nif \"%~1\"==\"--version\" (echo fake-agent 1.0& exit /b 0)\r\n");
    s.push_str(&format!("echo %*> \"{}\"\r\n", args_file(dir).display()));
    match mode {
        Mode::Replay { fixture, code } => {
            s.push_str(&format!("type \"{}\"\r\nexit /b {code}\r\n", fixture.display()));
        }
        Mode::Sleep => {
            s.push_str(&format!("echo {INIT_LINE}\r\n"));
            s.push_str("ping -n 61 127.0.0.1 >nul\r\nexit /b 0\r\n");
        }
        Mode::WriteThenReplay { file, text, fixture, code } => {
            s.push_str(&format!("echo {text}> \"{}\"\r\n", file.display()));
            s.push_str(&format!("type \"{}\"\r\nexit /b {code}\r\n", fixture.display()));
        }
        Mode::StderrExit { text, code } => {
            s.push_str(&format!("echo {text} 1>&2\r\nexit /b {code}\r\n"));
        }
    }
    s
}

#[cfg(not(windows))]
fn sh_script(dir: &Path, mode: Mode) -> String {
    let path = |p: &Path| quote(&p.to_string_lossy());
    let mut s = String::from("#!/bin/sh\n");
    s.push_str("if [ \"$1\" = \"--version\" ]; then echo 'fake-agent 1.0'; exit 0; fi\n");
    s.push_str(&format!("echo \"$@\" > {}\n", path(&args_file(dir))));
    match mode {
        Mode::Replay { fixture, code } => {
            s.push_str(&format!("cat {}\nexit {code}\n", path(fixture)));
        }
        Mode::Sleep => {
            s.push_str(&format!("echo {}\n", quote(INIT_LINE)));
            s.push_str("sleep 60\nexit 0\n");
        }
        Mode::WriteThenReplay { file, text, fixture, code } => {
            s.push_str(&format!("printf '%s\\n' {} > {}\n", quote(text), path(file)));
            s.push_str(&format!("cat {}\nexit {code}\n", path(fixture)));
        }
        Mode::StderrExit { text, code } => {
            s.push_str(&format!("printf '%s\\n' {} 1>&2\nexit {code}\n", quote(text)));
        }
    }
    s
}

/// 单引号包起来；内部的单引号按 shell 惯例拆成 `'\''`。
#[cfg(not(windows))]
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
