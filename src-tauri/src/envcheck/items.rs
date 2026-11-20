//! 各检查项的判定（spec §11）。只通过 `Probe` 接触外部环境。

use super::{Action, ActionKind, EnvItem, Group, Probe, Status};
use crate::platform::locate;
use std::path::Path;

pub fn label(id: &str) -> &'static str {
    match id {
        "claude" => "Claude Code CLI",
        "codex" => "Codex CLI",
        "node" => "Node.js",
        "python" => "Python",
        "git" => "Git",
        "lark_cli" => "lark-cli",
        "lark_login" => "lark-cli 登录",
        "ocr" => "ocr（代码审查）",
        "net_feishu" => "网络：飞书",
        "net_anthropic" => "网络：Anthropic API",
        "system_proxy" => "系统代理变量",
        _ => "未知项",
    }
}

fn group(id: &str) -> Group {
    match id {
        "lark_cli" | "lark_login" => Group::Feishu,
        "net_feishu" | "net_anthropic" | "system_proxy" => Group::Network,
        _ => Group::Runtime,
    }
}

/// 空白结果（状态先记为错误，调用方覆盖）。
pub fn base(id: &str) -> EnvItem {
    EnvItem {
        id: id.to_string(),
        label: label(id).to_string(),
        group: group(id),
        status: Status::Error,
        version: None,
        detail: String::new(),
        action: None,
    }
}

fn act(kind: ActionKind, label: &str) -> Option<Action> {
    Some(Action { kind, label: label.to_string() })
}

fn set(mut it: EnvItem, status: Status, detail: impl Into<String>, action: Option<Action>) -> EnvItem {
    it.status = status;
    it.detail = detail.into();
    it.action = action;
    it
}

/// 取版本号里第一个 `主.次`，如 `v24.22.1` → (24, 22)，`Python 3.12.1` → (3, 12)。
pub fn parse_version(s: &str) -> Option<(u32, u32)> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let mut parts = s[start..].split('.');
    let major: u32 = parts.next()?.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()?;
    let minor: u32 = parts
        .next()
        .map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>())
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    Some((major, minor))
}

pub fn check(id: &str, p: &dyn Probe) -> EnvItem {
    let it = base(id);
    match id {
        "claude" => check_engine(it, p, "claude", "claude auth login 或 /login"),
        "codex" => check_engine(it, p, "codex", "codex login"),
        "node" => check_runtime(it, p, "node", Some((18, 0)), "Node.js"),
        "python" => check_python(it, p),
        "git" => check_runtime(it, p, "git", None, "Git"),
        "lark_cli" => check_lark_cli(it, p),
        "lark_login" => check_lark_login(it, p),
        "ocr" => check_runtime(it, p, "ocr", None, "ocr"),
        "net_feishu" => check_net_feishu(it, p),
        "net_anthropic" => check_net_anthropic(it, p),
        "system_proxy" => check_system_proxy(it, p),
        _ => set(it, Status::Error, "未知的检查项", None),
    }
}

/// 找程序 + 取版本。失败时返回已填好的错误结果。
fn locate_with_version(it: EnvItem, p: &dyn Probe, name: &str) -> Result<(EnvItem, std::path::PathBuf, String), EnvItem> {
    let Some(path) = p.find(name) else {
        let text = format!("没有找到 {name} 命令");
        return Err(set(it, Status::Error, text, act(ActionKind::Install, "一键安装")));
    };
    match p.version(&path) {
        Ok(v) => Ok((it, path, v)),
        Err(e) => Err(set(it, Status::Error, format!("找到了 {} 但无法运行：{e}", path.display()), act(ActionKind::Install, "重新安装"))),
    }
}

fn check_engine(it: EnvItem, p: &dyn Probe, name: &str, _login_hint: &str) -> EnvItem {
    let (mut it, path, version) = match locate_with_version(it, p, name) {
        Ok(r) => r,
        Err(e) => return e,
    };
    it.version = Some(version);
    let logged_in = login_state(p, &path, name);
    match logged_in {
        Some(true) => set(it, Status::Ok, format!("已安装并登录（{}）", path.display()), None),
        Some(false) => set(it, Status::Warn, "已安装，但还没有登录", act(ActionKind::Login, "去登录")),
        None => set(it, Status::Warn, "已安装，但无法确认登录状态", act(ActionKind::Login, "去登录")),
    }
}

/// `claude auth status` / `codex login status`：退出码 0 且没有明确写未登录才算已登录。
fn login_state(p: &dyn Probe, path: &Path, name: &str) -> Option<bool> {
    let args: &[&str] = if name == "claude" { &["auth", "status"] } else { &["login", "status"] };
    match p.run(path, args) {
        Ok((0, out)) => {
            let compact: String = out.chars().filter(|c| !c.is_whitespace()).collect();
            Some(!compact.contains("\"loggedIn\":false") && !out.to_lowercase().contains("not logged"))
        }
        Ok(_) => Some(false),
        Err(_) => None,
    }
}

fn check_runtime(it: EnvItem, p: &dyn Probe, name: &str, min: Option<(u32, u32)>, shown: &str) -> EnvItem {
    match p.find(name) {
        Some(path) => check_runtime_at(it, p, &path, min, shown),
        None => set(it, Status::Error, format!("没有找到 {name} 命令"), act(ActionKind::Install, "一键安装")),
    }
}

fn check_python(it: EnvItem, p: &dyn Probe) -> EnvItem {
    let name = if cfg!(windows) { "python" } else { "python3" };
    let Some(path) = p.find(name) else {
        return set(it, Status::Error, "没有找到 Python", act(ActionKind::Install, "一键安装"));
    };
    if locate::is_store_stub(&path) {
        let text = "只找到 Windows 应用商店的 python 占位程序（不是真正的 Python），请安装";
        return set(it, Status::Error, text, act(ActionKind::Install, "一键安装"));
    }
    check_runtime_at(it, p, &path, Some((3, 10)), "Python")
}

fn check_runtime_at(mut it: EnvItem, p: &dyn Probe, path: &Path, min: Option<(u32, u32)>, shown: &str) -> EnvItem {
    let version = match p.version(path) {
        Ok(v) => v,
        Err(e) => return set(it, Status::Error, format!("找到了 {} 但无法运行：{e}", path.display()), act(ActionKind::Install, "重新安装")),
    };
    it.version = Some(version.clone());
    if let Some(min) = min
        && parse_version(&version).is_none_or(|v| v < min)
    {
        let text = format!("{shown} 版本过低，需要 {}.{} 及以上", min.0, min.1);
        return set(it, Status::Warn, text, act(ActionKind::Install, "升级"));
    }
    set(it, Status::Ok, format!("已安装（{}）", path.display()), None)
}

fn check_lark_cli(it: EnvItem, p: &dyn Probe) -> EnvItem {
    match locate_with_version(it, p, "lark-cli") {
        Ok((mut it, path, version)) => {
            it.version = Some(version);
            set(it, Status::Ok, format!("已安装（{}）", path.display()), None)
        }
        Err(e) => e,
    }
}

fn check_lark_login(it: EnvItem, p: &dyn Probe) -> EnvItem {
    if p.find("lark-cli").is_none() {
        return set(it, Status::Error, "需要先安装 lark-cli", act(ActionKind::Install, "安装 lark-cli"));
    }
    match p.lark_logged_in() {
        Ok(()) => set(it, Status::Ok, "用户身份已登录", None),
        Err(_) => set(it, Status::Error, "用户身份未登录或登录已过期", act(ActionKind::Login, "去登录")),
    }
}

fn check_net_feishu(it: EnvItem, p: &dyn Probe) -> EnvItem {
    if p.tcp_reach("open.feishu.cn", 443) {
        set(it, Status::Ok, "open.feishu.cn 可直连", None)
    } else {
        set(it, Status::Error, "无法连接 open.feishu.cn:443，请检查网络", act(ActionKind::OpenSettings, "设置代理"))
    }
}

fn check_net_anthropic(it: EnvItem, p: &dyn Probe) -> EnvItem {
    let (host, port, via) = match p.proxy().and_then(|u| super::parse_host_port(&u)) {
        Some((h, port)) => (h, port, "代理"),
        None => ("api.anthropic.com".to_string(), 443, "直连"),
    };
    if p.tcp_reach(&host, port) {
        set(it, Status::Ok, format!("{via}可达（{host}:{port}）"), None)
    } else {
        set(it, Status::Warn, format!("{via}不可达（{host}:{port}），Claude 可能无法联网，请检查代理设置"), act(ActionKind::OpenSettings, "设置代理"))
    }
}

fn check_system_proxy(it: EnvItem, p: &dyn Probe) -> EnvItem {
    let keys = ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "https_proxy", "http_proxy", "all_proxy"];
    let found: Vec<&str> = keys.iter().copied().filter(|k| p.env_var(k).is_some()).collect();
    if found.is_empty() {
        set(it, Status::Ok, "未设置系统级代理变量", None)
    } else {
        let text = format!("检测到环境变量 {}，可能让飞书长连接走错路径；如不需要请在系统环境变量里删除", found.join("、"));
        set(it, Status::Warn, text, act(ActionKind::Fix, "查看说明"))
    }
}
