//! 环境检查与一键修复（spec §11）。
//!
//! - `items`：每个检查项的判定逻辑，只依赖 `Probe`（可注入），单测不碰真实环境。
//! - `install`：修复计划（winget / npm / brew / 登录终端）与执行器，执行器同样可注入。
//! - 这里放数据结构、并发检查、机器人所需项与 `start_bot` 前的就绪检查。

pub mod install;
pub mod items;

use crate::platform::{locate, proc};
use crate::skills::SkillInfo;
use crate::store::config::{self, Bot, Engine};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// 全部检查项，按界面展示顺序。
pub const ALL_IDS: [&str; 13] = [
    "claude",
    "codex",
    "node",
    "python",
    "git",
    "ocr",
    "lark_cli",
    "lark_login",
    "meegle_cli",
    "meegle_login",
    "net_feishu",
    "net_anthropic",
    "system_proxy",
];

/// 单项检查的超时。
pub const ITEM_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Error,
    Checking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    Runtime,
    Feishu,
    Network,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Install,
    Login,
    OpenSettings,
    Fix,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Action {
    pub kind: ActionKind,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnvItem {
    pub id: String,
    pub label: String,
    pub group: Group,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
}

/// 检查项依赖的外部世界。生产用 `SystemProbe`，测试用假实现。全部是同步调用。
pub trait Probe: Send + Sync {
    /// 定位程序（python 要排除 WindowsApps 占位符；只找到占位符时返回占位符路径，由判定逻辑报错）。
    fn find(&self, name: &str) -> Option<PathBuf>;
    /// `program --version` 的第一行。
    fn version(&self, program: &Path) -> Result<String, String>;
    /// 运行 `program args`，返回（退出码，stdout）。10 秒超时。
    fn run(&self, program: &Path, args: &[&str]) -> Result<(i32, String), String>;
    /// lark-cli 用户身份是否已登录（成功即登录，不返回任何账号信息）。
    fn lark_logged_in(&self) -> Result<(), String>;
    fn tcp_reach(&self, host: &str, port: u16) -> bool;
    fn env_var(&self, key: &str) -> Option<String>;
    fn proxy(&self) -> Option<String>;
}

pub struct SystemProbe;

fn extra_search_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let env_dir = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if let Some(p) = env_dir("ProgramFiles") {
        v.push(p.join("nodejs"));
        v.push(p.join("Git").join("cmd"));
    }
    if let Some(l) = env_dir("LOCALAPPDATA") {
        // winget 刚装好的 Python 在 Programs\Python\Python3xx，当前进程的 PATH 里还没有。
        if let Ok(rd) = std::fs::read_dir(l.join("Programs").join("Python")) {
            for e in rd.flatten() {
                v.push(e.path());
            }
        }
    }
    v
}

fn all_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for d in locate::extra_dirs().into_iter().chain(extra_search_dirs()) {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// PATH 里排在前面的 WindowsApps 占位符 python（没有真正的 Python 时用来给出明确提示）。
fn find_python_stub() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .filter(|d| locate::is_store_stub(d))
        .map(|d| d.join("python.exe"))
        .find(|p| p.is_file())
}

pub fn parse_host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("http".to_string(), url),
    };
    let rest = rest.split(['/', '?', '#']).next()?;
    let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    if rest.is_empty() {
        return None;
    }
    let default = if scheme == "https" { 443 } else { 80 };
    match rest.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => Some((h.to_string(), p.parse().ok()?)),
        _ => Some((rest.to_string(), default)),
    }
}

impl Probe for SystemProbe {
    fn find(&self, name: &str) -> Option<PathBuf> {
        locate::find_in(std::ffi::OsStr::new(name), &all_dirs()).or_else(|| {
            if name.starts_with("python") { find_python_stub() } else { None }
        })
    }

    fn version(&self, program: &Path) -> Result<String, String> {
        locate::version(program)
    }

    fn run(&self, program: &Path, args: &[&str]) -> Result<(i32, String), String> {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = proc::spawn_hidden(&mut cmd).map_err(|e| format!("无法运行 {}：{e}", program.display()))?;
        let out = proc::wait_output_timeout(child, ITEM_TIMEOUT)?;
        Ok((out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned()))
    }

    fn lark_logged_in(&self) -> Result<(), String> {
        crate::feishu::lark_cli::LarkCli::detect()?.current_user().map(|_| ())
    }

    fn tcp_reach(&self, host: &str, port: u16) -> bool {
        use std::net::ToSocketAddrs;
        let Ok(addrs) = (host, port).to_socket_addrs() else { return false };
        addrs.into_iter().any(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_secs(5)).is_ok())
    }

    fn env_var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.trim().is_empty())
    }

    fn proxy(&self) -> Option<String> {
        config::load().ok()?.proxy.filter(|p| !p.trim().is_empty())
    }
}

/// 并发检查 `ids` 里的项（保持传入顺序）。每项独立线程，超过 `timeout` 记为错误。
pub fn check_ids(probe: &Arc<dyn Probe>, ids: &[&str], timeout: Duration) -> Vec<EnvItem> {
    let receivers: Vec<_> = ids
        .iter()
        .map(|id| {
            let (tx, rx) = std::sync::mpsc::channel();
            let (probe, id) = (probe.clone(), id.to_string());
            std::thread::spawn(move || {
                let _ = tx.send(items::check(&id, probe.as_ref()));
            });
            rx
        })
        .collect();
    ids.iter()
        .zip(receivers)
        .map(|(id, rx)| {
            rx.recv_timeout(timeout).unwrap_or_else(|_| {
                let mut it = items::base(id);
                it.status = Status::Error;
                it.detail = format!("检查超过 {} 秒未完成", timeout.as_secs());
                it
            })
        })
        .collect()
}

pub fn check_all(probe: &Arc<dyn Probe>) -> Vec<EnvItem> {
    check_ids(probe, &ALL_IDS, ITEM_TIMEOUT)
}

/// 每个机器人都需要的基础项（不含引擎）。首次引导只拦这些项 + 「至少一个引擎可用」。
pub const BASE_IDS: [&str; 6] = ["node", "python", "git", "lark_cli", "lark_login", "net_feishu"];
/// 引擎检查项：机器人只需要自己所选的那一个。
pub const ENGINE_IDS: [&str; 2] = ["claude", "codex"];

/// 前端算「哪些环境项会阻止启动」用的规则（`env_requirements` 命令返回），与 `ensure_ready` 同一套：
/// 每个机器人的所需项 = 基础项 + 所选引擎 + 启用技能的 requires；所需项里状态为「错误」的阻止启动。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnvRequirements {
    pub base: Vec<&'static str>,
    pub engines: Vec<&'static str>,
    /// 机器人 id → 所需检查项（按 `ALL_IDS` 顺序）
    pub bots: std::collections::BTreeMap<String, Vec<&'static str>>,
}

pub fn requirements(cfg: &config::AppConfig, skills: &[SkillInfo]) -> EnvRequirements {
    EnvRequirements {
        base: BASE_IDS.to_vec(),
        engines: ENGINE_IDS.to_vec(),
        bots: cfg.bots.iter().map(|b| (b.id.clone(), required_ids_for(b, skills, cfg))).collect(),
    }
}

/// 阻止启动的项：`ids` 里状态为「错误」的（警告、未登录的启发式判断不拦）。
pub fn blockers<'a>(items: &'a [EnvItem], ids: &[&str]) -> Vec<&'a EnvItem> {
    items.iter().filter(|i| i.status == Status::Error && ids.contains(&i.id.as_str())).collect()
}

/// 机器人所需的检查项 = 基础项 + 所选引擎 + 启用技能 `requires`（spec §11），按 `ALL_IDS` 顺序。
pub fn required_ids(bot: &Bot, skills: &[SkillInfo]) -> Vec<&'static str> {
    let mut need: Vec<&str> = BASE_IDS.to_vec();
    need.push(match bot.engine {
        Engine::Claude => "claude",
        Engine::Codex => "codex",
    });
    for name in &bot.skills {
        let Some(skill) = skills.iter().find(|s| &s.name == name) else { continue };
        for r in &skill.requires {
            match r.as_str() {
                "node" => need.push("node"),
                "python" => need.push("python"),
                "git" => need.push("git"),
                "ocr" => need.push("ocr"),
                "lark-cli" => {
                    need.push("lark_cli");
                    need.push("lark_login");
                }
                _ => {}
            }
        }
        // 代码审查走 `ocr delegate`：技能清单没写 requires 也要检查
        if name == "pr-review" {
            need.push("ocr");
        }
    }
    ALL_IDS.iter().copied().filter(|id| need.contains(id)).collect()
}

/// 机器人会建飞书项目工单：启用了 issue-record，且它能查到的项目里有勾「建飞书项目工单」的。
/// 能查到的项目 = 各群关联的项目；有群开了「全部项目」或没有群时（私聊可查全部项目），看全部项目。
pub fn needs_meegle(bot: &Bot, cfg: &config::AppConfig) -> bool {
    if !bot.skills.iter().any(|s| s == "issue-record") {
        return false;
    }
    if bot.groups.is_empty() || bot.groups.iter().any(|g| g.all_projects) {
        return cfg.projects.values().any(|p| p.record.meegle);
    }
    bot.groups
        .iter()
        .flat_map(|g| g.scope(&cfg.projects))
        .any(|(_, p, _)| p.record.meegle)
}

/// `required_ids` 再加上配置决定的项（建飞书项目工单 → meegle CLI 与登录）。
pub fn required_ids_for(bot: &Bot, skills: &[SkillInfo], cfg: &config::AppConfig) -> Vec<&'static str> {
    let mut need = required_ids(bot, skills);
    if needs_meegle(bot, cfg) {
        need.extend(["meegle_cli", "meegle_login"]);
    }
    ALL_IDS.iter().copied().filter(|id| need.contains(id)).collect()
}

/// 只有状态为「错误」的所需项会阻止启动（警告、未登录的启发式判断不拦）。
pub fn ensure_ready_with(probe: &Arc<dyn Probe>, bot: &Bot, skills: &[SkillInfo], timeout: Duration) -> Result<(), String> {
    let cfg = config::load().unwrap_or_default();
    let ids = required_ids_for(bot, skills, &cfg);
    let items = check_ids(probe, &ids, timeout);
    let bad: Vec<String> = blockers(&items, &ids).into_iter().map(|i| i.label.clone()).collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!("环境未就绪：{}", bad.join("、")))
    }
}

/// 所有启动路径（界面 `start_bot`、开机自启 / 随客户端启动、托盘点击）启动前都调用（经 `Supervisor::start_checked`）。
/// 找不到机器人时放行（由 supervisor 报「找不到机器人」）。
pub fn ensure_ready(bot_id: &str) -> Result<(), String> {
    let cfg = config::load()?;
    let Some(bot) = cfg.bots.iter().find(|b| b.id == bot_id) else { return Ok(()) };
    let probe: Arc<dyn Probe> = Arc::new(SystemProbe);
    ensure_ready_with(&probe, bot, &crate::skills::list_all(), ITEM_TIMEOUT)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod proxy_tests {
    use super::parse_host_port;

    #[test]
    fn parses_proxy_urls() {
        assert_eq!(parse_host_port("http://127.0.0.1:7890"), Some(("127.0.0.1".into(), 7890)));
        assert_eq!(parse_host_port("https://u:p@proxy.local/x"), Some(("proxy.local".into(), 443)));
        assert_eq!(parse_host_port("proxy.local"), Some(("proxy.local".into(), 80)));
        assert_eq!(parse_host_port(""), None);
    }
}
