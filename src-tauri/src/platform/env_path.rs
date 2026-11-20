//! 修 GUI 启动时残缺的 PATH。
//!
//! macOS / Linux 上从 Finder、Dock、launchd 启动的应用，拿到的 PATH 来自 launchd 而不是用户的 shell：
//! `launchctl getenv PATH` 没设过时就只有 `/usr/bin:/bin:/usr/sbin:/sbin`。于是 nvm / homebrew /
//! volta 装的 node、npm 全局装的 lark-cli、conda 的 python3 统统找不到，环境检查一片红，
//! 「一键安装」调 brew / npm 也直接 `No such file or directory`——而从终端 `pnpm tauri dev`
//! 起的同一份代码全都正常，因为它继承了 shell 的 PATH。
//!
//! 修法分两步，因为问登录 shell 很慢（rc 里有 nvm / conda / 插件时实测要几秒，不能卡在启动路径上）：
//!
//! 1. `init()`：启动时立刻合并「上次探到的 shell PATH（缓存）+ 当前 PATH + 一组常见安装目录」。
//!    只读文件和 stat，微秒级。常见的 nvm / fnm / volta / homebrew / conda / pyenv 装法靠兜底目录就够了。
//! 2. `spawn_cache_refresh()`：窗口起来之后在后台线程问一次登录 shell，把结果写进缓存，供下次启动用。
//!    兜底目录没枚举到的冷门装法，第二次启动起生效。
//!
//! 改的是本进程的 PATH，派生的 Agent / lark-cli / brew 子进程一并继承。
//! Windows 的 GUI 进程正常继承系统 PATH，不做任何处理。

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

/// 登录 shell 探测的超时。rc 文件卡住（比如等输入）时宁可放弃，反正还有兜底目录。
#[cfg(unix)]
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 把 PATH 从 shell 的其它输出里框出来（rc 文件可能自己打印东西）。
#[cfg(unix)]
const MARK: &str = "__BEETLE_PATH__";

/// Windows 不需要修（GUI 进程正常继承系统 PATH）。
#[cfg(windows)]
pub fn init() {}

/// 启动时调用一次，修好本进程的 PATH。只读缓存和 stat，不跑 shell，不阻塞启动。
/// 内部用 `std::env::set_var`，**必须在派生任何线程之前调用**（见 `lib.rs::run` 的第一行）。
#[cfg(unix)]
pub fn init() {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let fixed = repair(&current, read_cache().as_deref(), &fallback_dirs());
    if fixed == current {
        return;
    }
    // Safety：只在 run() 的最开头调用，此时进程还是单线程，没有并发读写 env 的线程。
    unsafe { std::env::set_var("PATH", &fixed) };
}

#[cfg(windows)]
pub fn spawn_cache_refresh() {}

/// 窗口起来之后调用：后台问一次登录 shell，把 PATH 写进缓存给下次启动用。
/// 不动本进程的 PATH——此时已经有别的线程在跑了，`set_var` 不再安全。
#[cfg(unix)]
pub fn spawn_cache_refresh() {
    std::thread::spawn(|| {
        let Some(path) = login_shell_path() else { return };
        if read_cache().as_deref() == Some(path.as_os_str()) {
            return;
        }
        let file = super::paths::shell_path_cache();
        let written = file
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::write(&file, path.as_encoded_bytes()));
        if let Err(e) = written {
            eprintln!("beetle: 写 PATH 缓存失败（{}）：{e}", file.display());
        }
    });
}

#[cfg(unix)]
fn read_cache() -> Option<std::ffi::OsString> {
    let raw = std::fs::read(super::paths::shell_path_cache()).ok()?;
    let text = String::from_utf8(raw).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.into())
}

/// 合并出修好的 PATH：登录 shell 的目录在前（和用户在终端里敲命令的结果一致），
/// 其次是本进程原有的目录（一个都不丢），最后是兜底目录。按出现顺序去重。
pub fn repair(current: &std::ffi::OsStr, shell: Option<&std::ffi::OsStr>, fallback: &[PathBuf]) -> std::ffi::OsString {
    let split = |v: &std::ffi::OsStr| std::env::split_paths(v).collect::<Vec<_>>();
    let groups = [shell.map(split).unwrap_or_default(), split(current), fallback.to_vec()];
    let merged = dedup(groups.iter().flatten());
    // join_paths 只在目录名里有 `:`（Windows 是 `;`）时失败，这种目录本来也进不了 PATH，丢掉重来。
    std::env::join_paths(&merged).unwrap_or_else(|_| {
        let clean: Vec<PathBuf> = merged.into_iter().filter(|d| std::env::join_paths([d]).is_ok()).collect();
        std::env::join_paths(&clean).unwrap_or_default()
    })
}

/// 按出现顺序去重，空目录丢掉。
fn dedup<'a>(dirs: impl Iterator<Item = &'a PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if !d.as_os_str().is_empty() && !out.contains(d) {
            out.push(d.clone());
        }
    }
    out
}

/// 跑用户的登录 shell 取 PATH。
///
/// 先试交互式登录（`-ilc`）：nvm、conda、pyenv 的初始化多半写在 `.zshrc` / `.bashrc` 里，
/// 只有交互式 shell 才会 source 它们；失败或没问出来再退回非交互的 `-lc`。
#[cfg(unix)]
fn login_shell_path() -> Option<std::ffi::OsString> {
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
    let script = format!("printf '{MARK}%s{MARK}' \"$PATH\"");
    for flags in ["-ilc", "-lc"] {
        let Some(out) = run_shell(&shell, flags, &script) else { continue };
        if let Some(p) = parse_probe_output(&out) {
            return Some(p.into());
        }
    }
    None
}

#[cfg(unix)]
fn run_shell(shell: &std::ffi::OsStr, flags: &str, script: &str) -> Option<String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(shell);
    cmd.arg(flags).arg(script).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let child = super::proc::spawn_hidden(&mut cmd).ok()?;
    let out = super::proc::wait_output_timeout(child, PROBE_TIMEOUT).ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 取两个标记之间的内容。shell 自己打印的提示符、欢迎语都在标记外面。
#[cfg(unix)]
fn parse_probe_output(out: &str) -> Option<String> {
    let (_, rest) = out.split_once(MARK)?;
    let (path, _) = rest.split_once(MARK)?;
    let path = path.trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// 登录 shell 探测失败时的兜底：常见包管理器 / 版本管理器的 bin 目录，只留真实存在的。
/// Windows 上的兜底在 `locate::extra_dirs`，两边各管各的。
#[cfg(unix)]
pub fn fallback_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin", "/usr/local/sbin"]
        .iter()
        .map(PathBuf::from)
        .collect();
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".local").join("bin"));
        v.push(h.join(".claude").join("local"));
        // node 的几种版本管理器
        v.extend(version_dirs(&h.join(".nvm").join("versions").join("node"), Path::new("bin")));
        v.extend(version_dirs(
            &h.join(".local").join("share").join("fnm").join("node-versions"),
            &Path::new("installation").join("bin"),
        ));
        v.extend(version_dirs(
            &h.join("Library").join("Application Support").join("fnm").join("node-versions"),
            &Path::new("installation").join("bin"),
        ));
        v.push(h.join(".volta").join("bin"));
        v.push(h.join(".bun").join("bin"));
        v.push(h.join(".deno").join("bin"));
        v.push(h.join(".cargo").join("bin"));
        v.push(h.join("go").join("bin"));
        // python
        v.push(h.join(".pyenv").join("shims"));
        for conda in ["miniconda3", "anaconda3", "miniforge3"] {
            v.push(h.join(conda).join("bin"));
        }
    }
    v.retain(|d| d.is_dir());
    v
}

/// 扫版本目录（`<root>/<版本>/<suffix>`），版本号大的排前面，解析不出版本号的按名字兜底排在后面。
#[cfg(unix)]
fn version_dirs(root: &Path, suffix: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut found: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let bin = e.path().join(suffix);
            bin.is_dir().then(|| (e.file_name().to_string_lossy().into_owned(), bin))
        })
        .collect();
    found.sort_by(|(a, _), (b, _)| version_key(b).cmp(&version_key(a)).then_with(|| a.cmp(b)));
    found.into_iter().map(|(_, p)| p).collect()
}

/// `v24.21.0` → (24, 21, 0)；认不出来的算 (0, 0, 0)。
#[cfg(unix)]
fn version_key(name: &str) -> (u32, u32, u32) {
    let digits = name.trim_start_matches(['v', 'V']);
    let mut it = digits.split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>());
    let mut next = || it.next().and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
    (next(), next(), next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn paths(v: &[&str]) -> Vec<PathBuf> {
        v.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn repair_puts_shell_dirs_first_and_keeps_everything() {
        let current = std::env::join_paths(paths(&["/usr/bin", "/bin"])).unwrap();
        let shell = std::env::join_paths(paths(&["/opt/homebrew/bin", "/usr/bin"])).unwrap();
        let fixed = repair(&current, Some(&shell), &paths(&["/fallback/bin"]));
        let got: Vec<PathBuf> = std::env::split_paths(&fixed).collect();
        assert_eq!(got, paths(&["/opt/homebrew/bin", "/usr/bin", "/bin", "/fallback/bin"]), "shell 在前、原有的不丢、去重");
    }

    #[test]
    fn repair_without_shell_probe_still_appends_fallback() {
        let current = std::env::join_paths(paths(&["/usr/bin"])).unwrap();
        let fixed = repair(&current, None, &paths(&["/opt/homebrew/bin"]));
        let got: Vec<PathBuf> = std::env::split_paths(&fixed).collect();
        assert_eq!(got, paths(&["/usr/bin", "/opt/homebrew/bin"]));
    }

    #[test]
    fn repair_drops_empty_entries() {
        let current = OsString::from(if cfg!(windows) { "C:\\bin;;C:\\bin" } else { "/usr/bin::/usr/bin" });
        let fixed = repair(&current, None, &[]);
        assert_eq!(std::env::split_paths(&fixed).count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn version_dirs_sorts_newest_first() {
        let t = tempfile::tempdir().unwrap();
        for v in ["v18.20.4", "v24.9.0", "v24.21.0", "不是版本号"] {
            std::fs::create_dir_all(t.path().join(v).join("bin")).unwrap();
        }
        // 没有 bin 子目录的不算
        std::fs::create_dir_all(t.path().join("v30.0.0")).unwrap();
        let got: Vec<String> = version_dirs(t.path(), Path::new("bin"))
            .iter()
            .map(|p| p.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(got, ["v24.21.0", "v24.9.0", "v18.20.4", "不是版本号"]);
    }

    #[cfg(unix)]
    #[test]
    fn version_key_parses_nvm_style() {
        assert_eq!(version_key("v24.21.0"), (24, 21, 0));
        assert_eq!(version_key("20.1"), (20, 1, 0));
        assert_eq!(version_key("system"), (0, 0, 0));
    }

    #[cfg(unix)]
    #[test]
    fn cached_shell_path_is_used_on_next_start() {
        let home = crate::test_support::test_home();
        assert!(read_cache().is_none(), "还没写过缓存");
        let file = crate::platform::paths::shell_path_cache();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "/opt/homebrew/bin:/usr/bin\n").unwrap();
        assert_eq!(read_cache().unwrap(), OsString::from("/opt/homebrew/bin:/usr/bin"), "行尾换行要去掉");
        std::fs::write(&file, "   \n").unwrap();
        assert!(read_cache().is_none(), "空内容当没有缓存");
        drop(home);
    }

    #[cfg(unix)]
    #[test]
    fn parse_probe_output_ignores_rc_file_noise() {
        let out = format!("欢迎回来\n{MARK}/opt/homebrew/bin:/usr/bin{MARK}\n(eval):1: 某个插件的抱怨");
        assert_eq!(parse_probe_output(&out).unwrap(), "/opt/homebrew/bin:/usr/bin");
        assert!(parse_probe_output("什么都没有").is_none());
        assert!(parse_probe_output(&format!("{MARK}   {MARK}")).is_none(), "空 PATH 当没问出来");
    }

    /// 本条就是这次的 bug：GUI 启动只有 launchd 的默认 PATH 时，修完必须能找回用户装的 node。
    #[cfg(unix)]
    #[test]
    fn gui_launch_path_finds_user_installed_node() {
        use crate::platform::locate;
        if locate::find("node").is_none() {
            return; // 这台机器压根没装 node，无从验证
        }
        let launchd = OsString::from("/usr/bin:/bin:/usr/sbin:/sbin");
        let bare: Vec<PathBuf> = std::env::split_paths(&launchd).collect();
        if locate::find_in(std::ffi::OsStr::new("node"), &bare).is_some() {
            return; // node 就装在系统目录里，这台机器复现不了这个 bug
        }
        let fixed = repair(&launchd, login_shell_path().as_deref(), &fallback_dirs());
        let dirs: Vec<PathBuf> = std::env::split_paths(&fixed).collect();
        for d in std::env::split_paths(&launchd) {
            assert!(dirs.contains(&d), "原有目录 {d:?} 不能丢");
        }
        assert!(
            locate::find_in(std::ffi::OsStr::new("node"), &dirs).is_some(),
            "修好的 PATH 里应该能找到 node：{dirs:?}"
        );
    }
}
