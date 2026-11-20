//! 测试用假 Agent：在临时目录里生成一个 `.cmd` 批处理，`RunnerDeps.locate` 指向它。
//! 真实引擎（Claude / Codex）照常生成启动参数，`proc::spawn` 照常拉起（Rust 对 `.cmd` 走 cmd.exe），
//! 批处理忽略参数，按模式回放夹具、睡眠或直接退出。`--version` 一律秒回，避免 `locate::version` 被睡眠模式卡住。
//! 每次被调用都把收到的参数写进同目录的 `args.txt`，测试据此检查 `--resume` 等参数。

use std::path::{Path, PathBuf};

pub enum Mode<'a> {
    /// 原样输出夹具文件的每一行，然后以 `code` 退出。
    Replay { fixture: &'a Path, code: i32 },
    /// 先输出一行 init，再睡 60 秒（孙进程 ping，被杀时一并结束）。
    Sleep,
    /// 先把 `text` 写进 `file`（模拟 Codex 的 `-o` 最后消息文件），再回放夹具并以 `code` 退出。
    WriteThenReplay { file: &'a Path, text: &'a str, fixture: &'a Path, code: i32 },
    /// 往 stderr 写 `text` 后以 `code` 退出（stdout 无输出）。
    StderrExit { text: &'a str, code: i32 },
}

pub fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agent").join(name)
}

pub fn args_file(dir: &Path) -> PathBuf {
    dir.join("args.txt")
}

pub fn make(dir: &Path, mode: Mode) -> PathBuf {
    let mut s = String::from("@echo off\r\nif \"%~1\"==\"--version\" (echo fake-agent 1.0& exit /b 0)\r\n");
    s.push_str(&format!("echo %*> \"{}\"\r\n", args_file(dir).display()));
    match mode {
        Mode::Replay { fixture, code } => {
            s.push_str(&format!("type \"{}\"\r\nexit /b {code}\r\n", fixture.display()));
        }
        Mode::Sleep => {
            s.push_str("echo {\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-sleep\"}\r\n");
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
    let p = dir.join("fake-agent.cmd");
    std::fs::write(&p, s).unwrap();
    p
}
