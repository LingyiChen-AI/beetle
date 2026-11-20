//! 定位 claude / codex / python 等命令行程序，并读取版本号。
//! 参考 MemDock `extract/runner.rs` 的 `find_program` / `known_cli_dirs`。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Windows 应用商店的 python 占位符（`...\WindowsApps\python.exe`）：存在但不是真正的解释器，运行只会弹商店。
pub fn is_store_stub(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case("WindowsApps"))
}

/// 在 `dirs` 里按顺序找可执行文件 `name`。Windows 优先 `.exe`，其次 npm 的 `.cmd` / `.bat`
/// （同目录下无扩展名的那个是 shell 脚本，运行不了）。返回的路径一定是绝对路径。
pub fn find_in(name: &OsStr, dirs: &[PathBuf]) -> Option<PathBuf> {
    #[cfg(windows)]
    let exts: &[&str] = &["exe", "cmd", "bat"];
    #[cfg(not(windows))]
    let exts: &[&str] = &[""];
    let is_python = name.to_string_lossy().to_ascii_lowercase().starts_with("python");
    for d in dirs {
        for e in exts {
            let mut f = name.to_os_string();
            if !e.is_empty() {
                f.push(".");
                f.push(e);
            }
            let p = d.join(f);
            if !p.is_file() || (is_python && is_store_stub(&p)) {
                continue;
            }
            return Some(std::path::absolute(&p).unwrap_or(p));
        }
    }
    None
}

/// 除 PATH 外常见的 CLI 安装目录（桌面应用从开始菜单启动时 PATH 可能不全）。
pub fn extra_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let env_dir = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if let Some(a) = env_dir("APPDATA") {
        v.push(a.join("npm"));
    }
    if let Some(h) = env_dir("USERPROFILE").or_else(dirs::home_dir) {
        v.push(h.join(".claude").join("local"));
        v.push(h.join(".local").join("bin"));
    }
    if let Some(l) = env_dir("LOCALAPPDATA") {
        v.push(l.join("Programs"));
    }
    v
}

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for d in extra_dirs() {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// 定位程序：PATH + `%APPDATA%\npm` + `%USERPROFILE%\.claude\local` + `%LOCALAPPDATA%\Programs`。
pub fn find(name: &str) -> Option<PathBuf> {
    find_in(OsStr::new(name), &search_dirs())
}

/// 运行 `program --version`（隐藏窗口，10 秒超时），返回输出的第一行。
pub fn version(program: &Path) -> Result<String, String> {
    use std::process::{Command, Stdio};
    use std::time::Duration;
    let mut cmd = Command::new(program);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = super::proc::spawn_hidden(&mut cmd).map_err(|e| format!("无法运行 {}：{e}", program.display()))?;
    let out = super::proc::wait_output_timeout(child, Duration::from_secs(10))
        .map_err(|e| format!("{} --version 失败：{e}", program.display()))?;
    if !out.status.success() {
        return Err(format!("{} --version 退出码 {:?}", program.display(), out.status.code()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().map(str::trim).find(|l| !l.is_empty());
    match first {
        Some(l) => Ok(l.to_string()),
        None => Err("--version 没有输出".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_skips_windowsapps_python_stub() {
        assert!(is_store_stub(Path::new(
            r"C:\Users\a\AppData\Local\Microsoft\WindowsApps\python.exe"
        )));
        assert!(is_store_stub(Path::new(
            r"c:\users\a\appdata\local\microsoft\windowsapps\python3.exe"
        )));
        assert!(!is_store_stub(Path::new(r"C:\Python312\python.exe")));
    }

    #[test]
    fn find_in_skips_stub_dir_for_python_but_not_others() {
        let t = tempfile::tempdir().unwrap();
        let stub = t.path().join("WindowsApps");
        let real = t.path().join("real");
        std::fs::create_dir_all(&stub).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        let ext = if cfg!(windows) { ".exe" } else { "" };
        for d in [&stub, &real] {
            std::fs::write(d.join(format!("python{ext}")), b"x").unwrap();
        }
        std::fs::write(stub.join(format!("tool{ext}")), b"x").unwrap();
        let dirs = vec![stub.clone(), real.clone()];
        let py = find_in(OsStr::new("python"), &dirs).unwrap();
        assert!(py.starts_with(&real), "{py:?}");
        let tool = find_in(OsStr::new("tool"), &dirs).unwrap();
        assert!(tool.starts_with(&stub), "非 python 不排除：{tool:?}");
    }

    #[cfg(windows)]
    #[test]
    fn find_in_prefers_exe_over_cmd_and_returns_absolute() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("claude.cmd"), b"x").unwrap();
        std::fs::write(t.path().join("claude"), b"x").unwrap();
        let p = find_in(OsStr::new("claude"), &[t.path().to_path_buf()]).unwrap();
        assert_eq!(p.file_name().unwrap(), "claude.cmd");
        std::fs::write(t.path().join("claude.exe"), b"x").unwrap();
        let p = find_in(OsStr::new("claude"), &[t.path().to_path_buf()]).unwrap();
        assert_eq!(p.file_name().unwrap(), "claude.exe");
        assert!(p.is_absolute());
    }

    #[test]
    fn find_in_returns_none_when_missing() {
        let t = tempfile::tempdir().unwrap();
        assert!(find_in(OsStr::new("nope-xyz"), &[t.path().to_path_buf()]).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn version_runs_real_program() {
        let cargo = find("cargo").expect("cargo 应在 PATH 里");
        assert!(cargo.is_absolute());
        let v = version(&cargo).unwrap();
        assert!(v.starts_with("cargo "), "{v}");
    }

    #[test]
    fn version_reports_missing_program() {
        let e = version(Path::new("Z:/definitely/not/here.exe")).unwrap_err();
        assert!(e.contains("无法运行"), "{e}");
    }
}
