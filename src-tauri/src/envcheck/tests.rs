use super::install::{self, Os, Plan, Runner};
use super::*;
use crate::skills::SkillInfo;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct FakeProbe {
    found: HashMap<String, PathBuf>,
    versions: HashMap<String, String>,
    /// 程序名 → `run` 的结果。
    runs: HashMap<String, Result<(i32, String), String>>,
    lark_login: bool,
    reachable: Vec<String>,
    env: HashMap<String, String>,
    proxy: Option<String>,
    delay: Option<Duration>,
}

impl FakeProbe {
    fn with(mut self, name: &str, path: &str, version: &str) -> Self {
        self.found.insert(name.into(), PathBuf::from(path));
        self.versions.insert(path.into(), version.into());
        self
    }
}

impl Probe for FakeProbe {
    fn find(&self, name: &str) -> Option<PathBuf> {
        if let Some(d) = self.delay {
            std::thread::sleep(d);
        }
        self.found.get(name).cloned()
    }
    fn version(&self, program: &Path) -> Result<String, String> {
        self.versions
            .get(program.to_str().unwrap())
            .cloned()
            .ok_or_else(|| "运行失败".to_string())
    }
    fn run(&self, program: &Path, _args: &[&str]) -> Result<(i32, String), String> {
        let name = program.file_stem().unwrap().to_string_lossy().into_owned();
        self.runs.get(&name).cloned().unwrap_or(Err("未配置".into()))
    }
    fn lark_logged_in(&self) -> Result<(), String> {
        if self.lark_login { Ok(()) } else { Err("未登录".into()) }
    }
    fn tcp_reach(&self, host: &str, _port: u16) -> bool {
        self.reachable.iter().any(|h| h == host)
    }
    fn env_var(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }
    fn proxy(&self) -> Option<String> {
        self.proxy.clone()
    }
}

#[test]
fn missing_program_is_error_with_install_action() {
    let it = items::check("node", &FakeProbe::default());
    assert_eq!(it.status, Status::Error);
    assert_eq!(it.action.as_ref().unwrap().kind, ActionKind::Install);
    assert_eq!(it.group, Group::Runtime);
}

#[test]
fn store_stub_python_is_error() {
    let stub = r"C:\Users\a\AppData\Local\Microsoft\WindowsApps\python.exe";
    let name = if cfg!(windows) { "python" } else { "python3" };
    let p = FakeProbe::default().with(name, stub, "Python 3.12.1");
    let it = items::check("python", &p);
    assert_eq!(it.status, Status::Error);
    assert!(it.detail.contains("占位"));
    assert_eq!(it.action.unwrap().kind, ActionKind::Install);
}

#[test]
fn old_versions_warn_and_new_versions_pass() {
    let name = if cfg!(windows) { "python" } else { "python3" };
    let old = FakeProbe::default().with(name, "C:/py/python.exe", "Python 3.9.7");
    assert_eq!(items::check("python", &old).status, Status::Warn);
    let ok = FakeProbe::default().with(name, "C:/py/python.exe", "Python 3.12.1");
    let it = items::check("python", &ok);
    assert_eq!(it.status, Status::Ok);
    assert_eq!(it.version.as_deref(), Some("Python 3.12.1"));

    let old_node = FakeProbe::default().with("node", "C:/n/node.exe", "v16.20.0");
    assert_eq!(items::check("node", &old_node).status, Status::Warn);
    let new_node = FakeProbe::default().with("node", "C:/n/node.exe", "v24.22.1");
    assert_eq!(items::check("node", &new_node).status, Status::Ok);
}

#[test]
fn parse_version_handles_prefixes() {
    assert_eq!(items::parse_version("v24.22.1"), Some((24, 22)));
    assert_eq!(items::parse_version("git version 2.43.0.windows.1"), Some((2, 43)));
    assert_eq!(items::parse_version("Python 3.10"), Some((3, 10)));
    assert_eq!(items::parse_version("none"), None);
}

#[test]
fn engine_login_states() {
    let mut p = FakeProbe::default().with("claude", "C:/c/claude.exe", "2.1.0");
    p.runs.insert("claude".into(), Ok((0, "{\n  \"loggedIn\": true\n}".into())));
    let it = items::check("claude", &p);
    assert_eq!((it.status, it.action.is_none()), (Status::Ok, true));

    p.runs.insert("claude".into(), Ok((0, "{\"loggedIn\": false}".into())));
    let it = items::check("claude", &p);
    assert_eq!(it.status, Status::Warn);
    assert_eq!(it.action.unwrap().kind, ActionKind::Login);

    let mut c = FakeProbe::default().with("codex", "C:/c/codex.exe", "0.1.0");
    c.runs.insert("codex".into(), Ok((1, String::new())));
    assert_eq!(items::check("codex", &c).status, Status::Warn);
}

#[test]
fn lark_login_requires_cli_then_login() {
    let none = items::check("lark_login", &FakeProbe::default());
    assert_eq!(none.status, Status::Error);
    assert_eq!(none.action.unwrap().kind, ActionKind::Install);

    let mut p = FakeProbe::default().with("lark-cli", "C:/l/lark-cli.cmd", "1.0.0");
    let it = items::check("lark_login", &p);
    assert_eq!((it.status, it.action.unwrap().kind), (Status::Error, ActionKind::Login));
    p.lark_login = true;
    assert_eq!(items::check("lark_login", &p).status, Status::Ok);
}

/// 没装 ocr：错误 + 一键安装（npm 全局装 open-code-review）；装了就是正常，归在运行环境组。
#[test]
fn ocr_missing_offers_npm_install() {
    let it = items::check("ocr", &FakeProbe::default());
    assert_eq!((it.status, it.group), (Status::Error, Group::Runtime));
    assert_eq!(it.action.unwrap().kind, ActionKind::Install);
    let plan = install::plan_for("ocr", ActionKind::Install, Os::Windows).unwrap();
    assert!(
        matches!(&plan, Plan::Run { program, args, manual_hint } if program == "npm"
            && args == &["install", "-g", "@alibaba-group/open-code-review"]
            && manual_hint.contains("Node.js")),
        "{plan:?}"
    );
    let ok = FakeProbe::default().with("ocr", "C:/n/ocr.cmd", "0.3.1");
    let it = items::check("ocr", &ok);
    assert_eq!((it.status, it.version.as_deref()), (Status::Ok, Some("0.3.1")));
}

/// 用 pr-review 技能的机器人需要 ocr（技能清单的 requires 写了 ocr 也认）；查 bug 机器人不需要。
#[test]
fn review_bot_requires_ocr() {
    let review = Bot::new_review("r", "审查", "cli_r");
    assert!(required_ids(&review, &[skill("pr-review", &["python", "git"])]).contains(&"ocr"));
    let mut other = Bot::new_default("b", "n", "cli_x");
    other.skills = vec!["my-review".into()];
    assert!(required_ids(&other, &[skill("my-review", &["ocr"])]).contains(&"ocr"));
    let plain = Bot::new_default("c", "n", "cli_y");
    assert!(!required_ids(&plain, &[skill("db-query", &[])]).contains(&"ocr"));
}

#[test]
fn network_and_proxy_items() {
    let mut p = FakeProbe::default();
    assert_eq!(items::check("net_feishu", &p).status, Status::Error);
    p.reachable.push("open.feishu.cn".into());
    assert_eq!(items::check("net_feishu", &p).status, Status::Ok);

    // 有代理时检查的是代理地址，不是 api.anthropic.com。
    assert_eq!(items::check("net_anthropic", &p).status, Status::Warn);
    p.proxy = Some("http://127.0.0.1:7890".into());
    p.reachable.push("127.0.0.1".into());
    assert_eq!(items::check("net_anthropic", &p).status, Status::Ok);

    assert_eq!(items::check("system_proxy", &p).status, Status::Ok);
    p.env.insert("HTTPS_PROXY".into(), "http://x".into());
    let it = items::check("system_proxy", &p);
    assert_eq!(it.status, Status::Warn);
    assert_eq!(it.action.unwrap().kind, ActionKind::Fix);
}

fn skill(name: &str, requires: &[&str]) -> SkillInfo {
    SkillInfo {
        name: name.into(),
        display_name: name.into(),
        description: String::new(),
        requires: requires.iter().map(|s| s.to_string()).collect(),
        builtin: true,
    }
}

#[test]
fn required_ids_follow_engine_and_skills() {
    let mut bot = Bot::new_default("b", "n", "cli_x");
    bot.skills = vec!["chat-context".into()];
    let skills = [skill("chat-context", &["node", "lark-cli"])];
    let ids = required_ids(&bot, &skills);
    assert!(ids.contains(&"claude") && !ids.contains(&"codex"));
    assert!(ids.contains(&"lark_cli") && ids.contains(&"lark_login"));
    assert!(!ids.contains(&"net_anthropic") && !ids.contains(&"system_proxy"));

    bot.engine = Engine::Codex;
    let ids = required_ids(&bot, &skills);
    assert!(ids.contains(&"codex") && !ids.contains(&"claude"));
    // 按展示顺序且不重复。
    let mut sorted = ids.clone();
    sorted.dedup();
    assert_eq!(sorted, ids);
}

#[test]
fn ensure_ready_lists_blocking_items_and_ignores_warnings() {
    let mut bot = Bot::new_default("b", "n", "cli_x");
    bot.skills = vec![];
    let probe: Arc<dyn Probe> = Arc::new(FakeProbe::default());
    let err = ensure_ready_with(&probe, &bot, &[], ITEM_TIMEOUT).unwrap_err();
    assert!(err.starts_with("环境未就绪："), "{err}");
    assert!(err.contains("Node.js") && err.contains("Git") && err.contains("Claude Code CLI"), "{err}");

    // 只有警告（版本低 / 未登录）不拦启动。
    let name = if cfg!(windows) { "python" } else { "python3" };
    let mut p = FakeProbe::default()
        .with("node", "C:/n/node.exe", "v16.0.0")
        .with(name, "C:/py/python.exe", "Python 3.12.0")
        .with("git", "C:/g/git.exe", "git version 2.40.0")
        .with("lark-cli", "C:/l/lark-cli.cmd", "1.0")
        .with("claude", "C:/c/claude.exe", "2.1.0");
    p.lark_login = true;
    p.reachable.push("open.feishu.cn".into());
    p.runs.insert("claude".into(), Ok((0, "{\"loggedIn\": false}".into())));
    let probe: Arc<dyn Probe> = Arc::new(p);
    assert!(ensure_ready_with(&probe, &bot, &[], ITEM_TIMEOUT).is_ok());
}

fn item(id: &str, status: Status) -> EnvItem {
    let mut it = items::base(id);
    it.status = status;
    it
}

/// 审查 B1：只装了 Claude Code 时 codex 一项是错误，但它不是 Claude 机器人的所需项，不能拦启动。
#[test]
fn requirements_per_bot_ignore_unused_engine() {
    let mut claude_bot = Bot::new_default("b1", "一", "cli_a");
    claude_bot.skills = vec!["pr-review".into()];
    let mut codex_bot = Bot::new_default("b2", "二", "cli_b");
    codex_bot.engine = Engine::Codex;
    codex_bot.skills = vec![];
    let cfg = config::AppConfig { bots: vec![claude_bot, codex_bot], ..Default::default() };
    let req = requirements(&cfg, &[skill("pr-review", &["python", "git"])]);
    assert_eq!(req.base, BASE_IDS.to_vec());
    assert_eq!(req.engines, vec!["claude", "codex"]);
    assert!(req.bots["b1"].contains(&"claude") && !req.bots["b1"].contains(&"codex"));
    assert!(req.bots["b1"].contains(&"ocr"));
    assert!(req.bots["b2"].contains(&"codex") && !req.bots["b2"].contains(&"ocr"));

    let env = vec![item("claude", Status::Ok), item("codex", Status::Error), item("git", Status::Ok), item("system_proxy", Status::Error)];
    assert!(blockers(&env, &req.bots["b1"]).is_empty(), "未选用的引擎和无关检查项不拦");
    let b2: Vec<&str> = blockers(&env, &req.bots["b2"]).iter().map(|i| i.id.as_str()).collect();
    assert_eq!(b2, vec!["codex"]);
    // 警告不拦
    let env = vec![item("git", Status::Warn)];
    assert!(blockers(&env, &req.bots["b1"]).is_empty());
}

#[test]
fn requirements_serialize_for_frontend() {
    let cfg = config::AppConfig { bots: vec![Bot::new_default("b1", "一", "cli_a")], ..Default::default() };
    let v = serde_json::to_value(requirements(&cfg, &[])).unwrap();
    assert_eq!(v["base"][0], "node");
    assert_eq!(v["engines"], serde_json::json!(["claude", "codex"]));
    assert!(v["bots"]["b1"].as_array().unwrap().iter().any(|x| x == "claude"));
}

#[test]
fn slow_item_times_out_without_blocking_others() {
    let p = FakeProbe { delay: Some(Duration::from_millis(500)), ..Default::default() };
    let probe: Arc<dyn Probe> = Arc::new(p);
    let started = std::time::Instant::now();
    let out = check_ids(&probe, &["node", "git"], Duration::from_millis(50));
    assert!(started.elapsed() < Duration::from_millis(400));
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|i| i.status == Status::Error && i.detail.contains("未完成")));
}

#[test]
fn check_all_returns_every_item_in_order() {
    let probe: Arc<dyn Probe> = Arc::new(FakeProbe::default());
    let ids: Vec<String> = check_all(&probe).into_iter().map(|i| i.id).collect();
    assert_eq!(ids, ALL_IDS);
}

#[test]
fn item_serializes_to_the_contract_shape() {
    let it = items::check("node", &FakeProbe::default());
    let v = serde_json::to_value(&it).unwrap();
    assert_eq!(v["status"], "error");
    assert_eq!(v["group"], "runtime");
    assert_eq!(v["action"]["kind"], "install");
    assert!(v.get("version").is_none());
    let v = serde_json::to_value(items::check("ocr", &FakeProbe::default())).unwrap();
    assert_eq!((v["action"]["kind"].as_str(), v["group"].as_str()), (Some("install"), Some("runtime")));
}

#[test]
fn plans_use_expected_packages() {
    let node = install::plan_for("node", ActionKind::Install, Os::Windows).unwrap();
    assert!(matches!(&node, Plan::Run { program, args, .. } if program == "winget" && args.contains(&"OpenJS.NodeJS.LTS".to_string())));
    let py = install::plan_for("python", ActionKind::Install, Os::Windows).unwrap();
    assert!(matches!(&py, Plan::Run { args, .. } if args.contains(&"Python.Python.3.12".to_string())));
    let git = install::plan_for("git", ActionKind::Install, Os::Mac).unwrap();
    assert!(matches!(&git, Plan::Run { program, .. } if program == "brew"));
    let claude = install::plan_for("claude", ActionKind::Install, Os::Windows).unwrap();
    assert!(matches!(&claude, Plan::Run { program, args, .. } if program == "npm" && args[2] == "@anthropic-ai/claude-code"));
    let login = install::plan_for("codex", ActionKind::Login, Os::Windows).unwrap();
    assert_eq!(login, Plan::Terminal { args: vec!["codex".into(), "login".into()] });
    assert!(matches!(install::plan_for("system_proxy", ActionKind::Fix, Os::Windows).unwrap(), Plan::Manual(_)));
    assert!(install::plan_for("net_feishu", ActionKind::Fix, Os::Windows).is_err());
}

#[derive(Default)]
struct FakeRunner {
    calls: Mutex<Vec<String>>,
    code: i32,
}

impl Runner for FakeRunner {
    fn run(&self, program: &str, args: &[String], on_line: &mut dyn FnMut(&str)) -> Result<i32, String> {
        self.calls.lock().unwrap().push(format!("{program} {}", args.join(" ")));
        on_line("正在下载");
        Ok(self.code)
    }
    fn open_terminal(&self, args: &[String]) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("terminal {}", args.join(" ")));
        Ok(())
    }
}

#[test]
fn fix_runs_injected_runner_and_streams_lines() {
    let runner = FakeRunner::default();
    let mut lines = Vec::new();
    let msg = install::fix("git", &FakeProbe::default(), &runner, Os::Windows, &mut |l| lines.push(l.to_string())).unwrap();
    assert!(msg.contains("安装完成"));
    assert!(runner.calls.lock().unwrap()[0].starts_with("winget install"));
    assert!(lines.iter().any(|l| l == "正在下载"));

    let failing = FakeRunner { code: 1, ..Default::default() };
    let err = install::fix("git", &FakeProbe::default(), &failing, Os::Windows, &mut |_| {}).unwrap_err();
    assert!(err.contains("退出码 1") && err.contains("git-scm.com"), "{err}");
}

#[test]
fn fix_opens_terminal_for_login_and_skips_healthy_items() {
    let mut p = FakeProbe::default().with("claude", "C:/c/claude.exe", "2.1.0");
    p.runs.insert("claude".into(), Ok((0, "{\"loggedIn\": false}".into())));
    let runner = FakeRunner::default();
    install::fix("claude", &p, &runner, Os::Windows, &mut |_| {}).unwrap();
    assert_eq!(runner.calls.lock().unwrap()[0], "terminal claude auth login");

    let mut ok = FakeProbe::default().with("git", "C:/g/git.exe", "git version 2.40.0");
    ok.lark_login = true;
    let runner = FakeRunner::default();
    let msg = install::fix("git", &ok, &runner, Os::Windows, &mut |_| {}).unwrap();
    assert!(msg.contains("不需要修复"));
    assert!(runner.calls.lock().unwrap().is_empty());
}
