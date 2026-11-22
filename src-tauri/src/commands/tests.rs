use super::*;
use crate::store::config::{self, AppConfig, Bot, Engine, Group, Project, Trigger};
use crate::store::datasource::{Access, DataSource, DbKind, DirectConn, SkillRef, SshTunnel};
use crate::store::runs::{RunRecord, RunStatus};
use crate::store::secrets;
use crate::test_support::test_home;
use std::collections::HashMap;

fn group(chat_id: &str, projects: &[&str]) -> Group {
    Group {
        chat_id: chat_id.into(),
        name: "测试群".into(),
        slug: format!("slug-{chat_id}"),
        projects: projects.iter().map(|s| s.to_string()).collect(),
        doc_url: None,
        trigger: Trigger::Owner,
        all_projects: false,
    }
}

fn project(repo: &str) -> Project {
    Project::new(repo, "release/*", "AGENTS.md", "示例项目", vec![])
}

fn seed(cfg: AppConfig) {
    config::save(&cfg).unwrap();
}

/// 建一个本地空的裸仓库，`git ls-remote --heads` 能访问它。
fn bare_repo(dir: &std::path::Path) -> String {
    let path = dir.join("remote.git");
    let status = std::process::Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    path.to_string_lossy().into_owned()
}

fn run_record(id: &str, bot_id: &str, started_at: i64) -> RunRecord {
    RunRecord {
        id: id.into(),
        bot_id: bot_id.into(),
        chat_id: "oc_1".into(),
        group_name: "群".into(),
        trigger_message_id: "om_1".into(),
        sender_name: "张三".into(),
        text: "查一下".into(),
        engine: Engine::Claude,
        engine_version: None,
        session_id: None,
        status: RunStatus::Replied,
        started_at,
        finished_at: None,
        final_reply: None,
        error: None,
        sql_count: 0,
        reply_message_id: None,
        sender_union_id: None,
    }
}

/// 测试用唯一 app_id；Drop 时清理凭据库。
struct TestAppId(String);

impl TestAppId {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        Self(format!("cli_cmd_test_{}_{nanos}", std::process::id()))
    }
}

impl Drop for TestAppId {
    fn drop(&mut self) {
        let _ = secrets::delete_app_secret(&self.0);
    }
}

#[test]
#[cfg_attr(not(windows), ignore)]
fn list_bots_view_never_contains_secret() {
    let _home = test_home();
    let id = TestAppId::new();
    let bot = Bot::new_default("b1", "BUG纠察队", &id.0);
    save_bot_impl(bot, Some("TOP-SECRET-VALUE-123".into())).unwrap();
    // 密钥确实进了凭据库
    assert_eq!(secrets::app_secret(&id.0).unwrap(), "TOP-SECRET-VALUE-123");

    let cfg = config::load().unwrap();
    let views = build_bot_views(&cfg, &|_| BotState::Stopped, &|_| false, &HashMap::new());
    let json = serde_json::to_string(&views).unwrap();
    assert!(!json.contains("TOP-SECRET-VALUE-123"), "list_bots 返回里出现了密钥：{json}");
    assert!(!json.to_lowercase().contains("secret"), "返回里不该有 secret 字段：{json}");
    let file = std::fs::read_to_string(crate::platform::paths::config_dir().join("config.json")).unwrap();
    assert!(!file.contains("TOP-SECRET-VALUE-123"), "config.json 里出现了密钥");
}

#[test]
fn bot_view_carries_state_and_today_runs() {
    let mut cfg = AppConfig::default();
    cfg.bots.push(Bot::new_default("b1", "一号", "cli_a"));
    cfg.bots.push(Bot::new_default("b2", "二号", "cli_b"));
    let today = HashMap::from([("b2".to_string(), 3u32)]);
    let views = build_bot_views(
        &cfg,
        &|id| if id == "b1" { BotState::Running } else { BotState::Error("连不上".into()) },
        &|id| id == "b2",
        &today,
    );
    assert_eq!(views[0].state, BotState::Running);
    assert_eq!(views[0].today_runs, 0);
    assert_eq!(views[1].state, BotState::Error("连不上".into()));
    assert_eq!(views[1].today_runs, 3);
    let v = serde_json::to_value(&views[1]).unwrap();
    assert_eq!(v["state"]["kind"], "error");
    assert_eq!(v["bot"]["id"], "b2");
    assert_eq!(v["today_runs"], 3);
    assert_eq!(v["alive"], true, "异常但连接任务还在（重试中）");
    assert_eq!(serde_json::to_value(&views[0]).unwrap()["alive"], false);
}

#[test]
fn count_today_counts_only_runs_since_midnight_per_bot() {
    let records = vec![
        run_record("r1", "b1", 1_000),
        run_record("r2", "b1", 5_000),
        run_record("r3", "b2", 6_000),
        run_record("r4", "b2", 500),
    ];
    let counts = count_today(&records, 1_000);
    assert_eq!(counts.get("b1"), Some(&2));
    assert_eq!(counts.get("b2"), Some(&1));
}

#[test]
fn save_bot_keeps_groups_and_owner_registered_meanwhile() {
    let _home = test_home();
    let mut b = Bot::new_default("b1", "旧名", "cli_a");
    b.groups.push(group("oc_auto", &[]));
    seed(AppConfig { bots: vec![b], owner_open_id: Some("ou_owner".into()), ..AppConfig::default() });

    // 界面拿到的是旧快照：不带自动接入的群
    let mut edited = Bot::new_default("b1", "新名", "cli_a");
    edited.timeout_minutes = 45;
    save_bot_impl(edited, None).unwrap();

    let cfg = config::load().unwrap();
    assert_eq!(cfg.bots.len(), 1);
    assert_eq!(cfg.bots[0].name, "新名");
    assert_eq!(cfg.bots[0].timeout_minutes, 45);
    assert_eq!(cfg.bots[0].groups.len(), 1, "群由 save_group 管，save_bot 不能冲掉自动接入的群");
    assert_eq!(cfg.owner_open_id.as_deref(), Some("ou_owner"));
}

/// 审查 B6：只改 App ID、没填新 Secret（新 App ID 在凭据库里也没有）时拒绝保存，配置不变、旧密钥不删。
#[test]
fn save_bot_refuses_app_id_change_without_secret() {
    let _home = test_home();
    let old = TestAppId::new();
    let new = TestAppId(format!("{}_new", old.0));
    seed(AppConfig { bots: vec![Bot::new_default("b1", "一", &old.0)], ..AppConfig::default() });
    let mut edited = Bot::new_default("b1", "一", &new.0);
    edited.name = "改名".into();
    let err = save_bot_impl(edited.clone(), None).unwrap_err();
    assert!(err.contains("更换 App ID 需要同时填写新的 App Secret"), "{err}");
    let err = save_bot_impl(edited, Some("   ".into())).unwrap_err();
    assert!(err.contains("App Secret"), "空白密钥等于没填：{err}");
    let cfg = config::load().unwrap();
    assert_eq!(cfg.bots[0].app_id, old.0, "配置不变");
    assert_eq!(cfg.bots[0].name, "一");
}

/// 改 App ID 同时填了新 Secret：保存成功，新密钥进凭据库。
#[test]
#[cfg_attr(not(windows), ignore)]
fn save_bot_app_id_change_with_secret_saves() {
    let _home = test_home();
    let old = TestAppId::new();
    let new = TestAppId(format!("{}_new", old.0));
    seed(AppConfig { bots: vec![Bot::new_default("b1", "一", &old.0)], ..AppConfig::default() });
    save_bot_impl(Bot::new_default("b1", "一", &new.0), Some("NEW-SECRET".into())).unwrap();
    assert_eq!(config::load().unwrap().bots[0].app_id, new.0);
    assert_eq!(secrets::app_secret(&new.0).unwrap(), "NEW-SECRET");
}

#[test]
fn save_bot_without_id_creates_one_and_appends() {
    let _home = test_home();
    let mut b = Bot::new_default("", "新机器人", "cli_new");
    b.id = String::new();
    let saved = save_bot_impl(b, None).unwrap();
    assert!(!saved.id.is_empty());
    assert_eq!(config::load().unwrap().bots[0].id, saved.id);
}

#[test]
fn save_bot_rejects_invalid_fields_with_chinese_errors() {
    let _home = test_home();
    let mut b = Bot::new_default("b1", "  ", "cli_a");
    assert!(save_bot_impl(b.clone(), None).unwrap_err().contains("名称"));
    b.name = "ok".into();
    b.app_id = " ".into();
    assert!(save_bot_impl(b.clone(), None).unwrap_err().contains("App ID"));
    b.app_id = "cli_a".into();
    b.concurrency = 0;
    assert!(save_bot_impl(b.clone(), None).unwrap_err().contains("并发"));
    b.concurrency = 2;
    b.timeout_minutes = 0;
    assert!(save_bot_impl(b, None).unwrap_err().contains("超时"));
    assert!(config::load().unwrap().bots.is_empty());
}

#[test]
fn delete_bot_removes_it_and_leaves_others() {
    let _home = test_home();
    seed(AppConfig {
        bots: vec![Bot::new_default("b1", "一", "cli_unit_del_a"), Bot::new_default("b2", "二", "cli_unit_del_b")],
        ..AppConfig::default()
    });
    delete_bot_impl("b1").unwrap();
    let cfg = config::load().unwrap();
    assert_eq!(cfg.bots.len(), 1);
    assert_eq!(cfg.bots[0].id, "b2");
    assert!(delete_bot_impl("nope").unwrap_err().contains("找不到"));
}

#[test]
fn set_start_with_client_flips_flag_only() {
    let _home = test_home();
    seed(AppConfig { bots: vec![Bot::new_default("b1", "一", "cli_a")], ..AppConfig::default() });
    set_start_with_client_impl("b1", true).unwrap();
    assert!(config::load().unwrap().bots[0].start_with_client);
    set_start_with_client_impl("b1", false).unwrap();
    assert!(!config::load().unwrap().bots[0].start_with_client);
}

#[test]
fn save_group_upserts_by_chat_id_and_remove_group_deletes() {
    let _home = test_home();
    let mut cfg = AppConfig { bots: vec![Bot::new_default("b1", "一", "cli_a")], ..AppConfig::default() };
    cfg.projects.insert("proj-a".into(), project("https://x/a.git"));
    seed(cfg);

    save_group_impl("b1", group("oc_1", &["proj-a"])).unwrap();
    let mut g = group("oc_1", &["proj-a"]);
    g.name = "改名后".into();
    g.trigger = Trigger::All;
    save_group_impl("b1", g).unwrap();
    save_group_impl("b1", group("oc_2", &[])).unwrap();
    let groups = config::load().unwrap().bots[0].groups.clone();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].name, "改名后");
    assert_eq!(groups[0].trigger, Trigger::All);

    remove_group_impl("b1", "oc_1").unwrap();
    let groups = config::load().unwrap().bots[0].groups.clone();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].chat_id, "oc_2");
}

#[test]
fn save_group_rejects_unknown_project_and_unsafe_slug() {
    let _home = test_home();
    seed(AppConfig { bots: vec![Bot::new_default("b1", "一", "cli_a")], ..AppConfig::default() });
    let err = save_group_impl("b1", group("oc_1", &["不存在"])).unwrap_err();
    assert!(err.contains("不存在"), "{err}");
    let mut g = group("oc_1", &[]);
    g.slug = "../evil".into();
    assert!(save_group_impl("b1", g).unwrap_err().contains("目录名"));
    let mut g = group("", &[]);
    g.slug = "ok".into();
    assert!(save_group_impl("b1", g).unwrap_err().contains("chat_id"));
    save_group_impl("b1", group("oc_a", &[])).unwrap();
    let mut dup = group("oc_b", &[]);
    dup.slug = "slug-oc_a".into();
    assert!(save_group_impl("b1", dup).unwrap_err().contains("已被"), "两个群不能共用一个工作目录");
    assert_eq!(config::load().unwrap().bots[0].groups.len(), 1);
    assert!(save_group_impl("nope", group("oc_1", &[])).unwrap_err().contains("找不到"));
}

#[test]
fn check_repo_accepts_reachable_and_rejects_unreachable_in_chinese() {
    let dir = tempfile::tempdir().unwrap();
    let repo = bare_repo(dir.path());
    check_repo(&repo, Duration::from_secs(30)).unwrap();

    let missing = dir.path().join("不存在.git").to_string_lossy().into_owned();
    let err = check_repo(&missing, Duration::from_secs(30)).unwrap_err();
    assert!(err.contains("无法访问仓库"), "{err}");
    assert!(check_repo("--upload-pack=evil", Duration::from_secs(30)).unwrap_err().contains("不合法"));
    assert!(check_repo("  ", Duration::from_secs(30)).unwrap_err().contains("不合法"));
}

#[test]
fn save_project_validates_repo_before_writing() {
    let _home = test_home();
    let dir = tempfile::tempdir().unwrap();
    let ok = bare_repo(dir.path());
    save_project_impl("proj-a", project(&ok)).unwrap();
    assert_eq!(config::load().unwrap().projects["proj-a"].repo, ok);

    let bad = dir.path().join("nope.git").to_string_lossy().into_owned();
    let err = save_project_impl("proj-b", project(&bad)).unwrap_err();
    assert!(err.contains("无法访问仓库"), "{err}");
    assert!(!config::load().unwrap().projects.contains_key("proj-b"), "校验失败不能写入配置");
}

#[test]
fn save_project_rejects_blank_name_and_unknown_db() {
    let _home = test_home();
    let dir = tempfile::tempdir().unwrap();
    let ok = bare_repo(dir.path());
    assert!(save_project_impl(" ", project(&ok)).unwrap_err().contains("名称"));
    let mut p = project(&ok);
    p.dbs = vec!["缺失库".into()];
    assert!(save_project_impl("proj-a", p).unwrap_err().contains("缺失库"));
}

/// 记录方式：勾了建 issue 但推不出平台、token 变量名不合法都拒绝；issue 仓库 / 变量名去空白，空白当没填。
#[test]
fn save_project_validates_and_normalizes_record_fields() {
    let _home = test_home();
    let dir = tempfile::tempdir().unwrap();
    let ok = bare_repo(dir.path());
    let mut p = project(&ok);
    p.record.issue = true;
    assert!(save_project_impl("proj-a", p.clone()).unwrap_err().contains("建 issue"));

    p.issue_repo = Some("  https://gitee.com/team/issues  ".into());
    p.token_env = Some("BAD NAME".into());
    assert!(save_project_impl("proj-a", p.clone()).unwrap_err().contains("环境变量名"));

    p.token_env = Some("   ".into());
    save_project_impl("proj-a", p).unwrap();
    let saved = config::load().unwrap().projects["proj-a"].clone();
    assert!(saved.record.issue && saved.record.feishu_doc);
    assert_eq!(saved.issue_repo.as_deref(), Some("https://gitee.com/team/issues"));
    assert_eq!(saved.token_env, None);
}

#[test]
fn remove_project_also_clears_references() {
    let _home = test_home();
    let mut b = Bot::new_default("b1", "一", "cli_a");
    b.groups.push(group("oc_1", &["proj-a", "proj-b"]));
    let mut cfg = AppConfig { bots: vec![b], default_projects: vec!["proj-a".into(), "proj-b".into()], ..AppConfig::default() };
    cfg.projects.insert("proj-a".into(), project("r1"));
    cfg.projects.insert("proj-b".into(), project("r2"));
    seed(cfg);

    remove_project_impl("proj-a").unwrap();
    let cfg = config::load().unwrap();
    assert!(!cfg.projects.contains_key("proj-a"));
    assert_eq!(cfg.default_projects, ["proj-b"]);
    assert_eq!(cfg.bots[0].groups[0].projects, ["proj-b"]);
}

fn direct_ds(db: &str) -> DataSource {
    DataSource {
        kind: DbKind::Mysql,
        description: " 订单库 ".into(),
        access: Access::Direct(DirectConn {
            host: " 127.0.0.1 ".into(),
            port: 3306,
            user: "demo_reader".into(),
            database: db.into(),
            ssh: None,
            timeout_secs: 5,
            max_rows: 200,
        }),
    }
}

fn skill_ds(db: &str) -> DataSource {
    DataSource {
        kind: DbKind::Mysql,
        description: String::new(),
        access: Access::Skill(SkillRef { skill: " my-sql-skill ".into(), database: db.into(), note: "库名写 demo".into() }),
    }
}

#[test]
fn alias_rules() {
    for ok in ["demo", "demo_orders", "demo-1", "A9", &"a".repeat(64)] {
        assert!(valid_alias(ok), "{ok}");
    }
    for bad in ["", "订单库", "a b", "a/b", "a.b", "../x", &"a".repeat(65)] {
        assert!(!valid_alias(bad), "{bad}");
    }
}

#[test]
fn save_datasource_validates_and_trims() {
    let _home = test_home();
    // 直连、不带密码：不碰 keyring
    save_datasource_impl(" demo_orders ", direct_ds("demo_orders"), None).unwrap();
    let ds = &config::load().unwrap().dbs["demo_orders"];
    assert_eq!(ds.description, "订单库");
    assert!(matches!(&ds.access, Access::Direct(c) if c.host == "127.0.0.1" && c.database == "demo_orders"));

    assert!(save_datasource_impl("订单库", direct_ds("d"), None).unwrap_err().contains("别名不合法"));
    let mut bad = direct_ds(" ");
    assert!(save_datasource_impl("x", bad.clone(), None).unwrap_err().contains("库名"));
    bad = direct_ds("d");
    if let Access::Direct(c) = &mut bad.access {
        c.host = " ".into();
    }
    assert!(save_datasource_impl("x", bad, None).unwrap_err().contains("主机"));
    let mut other = direct_ds("d");
    other.kind = DbKind::Other;
    assert!(save_datasource_impl("x", other, None).unwrap_err().contains("技能方式"));
    let mut ssh = direct_ds("d");
    if let Access::Direct(c) = &mut ssh.access {
        c.ssh = Some(SshTunnel { host: "jump".into(), port: 22, user: "".into(), key_path: "C:/k".into() });
    }
    assert!(save_datasource_impl("x", ssh, None).unwrap_err().contains("SSH"));
    let mut no_skill = skill_ds("d");
    if let Access::Skill(r) = &mut no_skill.access {
        r.skill = " ".into();
    }
    assert!(save_datasource_impl("x", no_skill, None).unwrap_err().contains("查询技能"));
    // 范围与前端一致：超时 1～300 秒、最多行数 1～10000、端口 1～65535
    for (timeout, rows, port, want) in [(0, 200, 3306, "超时"), (301, 200, 3306, "超时"), (5, 0, 3306, "行数"), (5, 10001, 3306, "行数"), (5, 200, 0, "端口")] {
        let mut ds = direct_ds("d");
        if let Access::Direct(c) = &mut ds.access {
            (c.timeout_secs, c.max_rows, c.port) = (timeout, rows, port);
        }
        assert!(save_datasource_impl("x", ds, None).unwrap_err().contains(want), "{timeout}/{rows}/{port}");
    }
    let mut edge = direct_ds("d");
    if let Access::Direct(c) = &mut edge.access {
        (c.timeout_secs, c.max_rows, c.port) = (300, 10000, 65535);
    }
    save_datasource_impl("edge", edge, None).unwrap();
    assert!(!config::load().unwrap().dbs.contains_key("x"), "校验失败不写配置");
    assert!(config::load().unwrap().dbs.contains_key("edge"), "边界值可以保存");
}

#[test]
fn save_skill_datasource_keeps_skill_and_note() {
    let _home = test_home();
    save_datasource_impl("demo_skill", skill_ds("demo"), Some(String::new())).unwrap();
    let ds = &config::load().unwrap().dbs["demo_skill"];
    assert_eq!(
        ds.access,
        Access::Skill(SkillRef { skill: "my-sql-skill".into(), database: "demo".into(), note: "库名写 demo".into() })
    );
}

/// 编辑数据源不填密码（None 或空串）时保留已保存的密码；填了才覆盖；删数据源时一起删掉。
#[test]
#[cfg_attr(not(windows), ignore)]
fn save_without_password_keeps_existing() {
    let _home = test_home();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let alias = format!("unit_test_ds_{}_{nanos}", std::process::id());
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = secrets::delete_db_password(&self.0);
        }
    }
    let _cleanup = Cleanup(alias.clone());

    save_datasource_impl(&alias, direct_ds("demo"), Some("pw-1".into())).unwrap();
    assert_eq!(secrets::db_password(&alias).unwrap().as_deref(), Some("pw-1"));
    save_datasource_impl(&alias, direct_ds("demo2"), None).unwrap();
    save_datasource_impl(&alias, direct_ds("demo3"), Some(String::new())).unwrap();
    assert_eq!(secrets::db_password(&alias).unwrap().as_deref(), Some("pw-1"), "不填密码不清掉已保存的");
    let text = std::fs::read_to_string(crate::platform::paths::config_dir().join("config.json")).unwrap();
    assert!(!text.contains("pw-1"), "密码不进配置文件");

    // 测试连接：不填密码时用已保存的；填了用填的
    let (_, pw) = prepare_test_datasource(direct_ds("demo"), Some(alias.clone()), None).unwrap();
    assert_eq!(pw.as_deref(), Some("pw-1"));
    let (_, pw) = prepare_test_datasource(direct_ds("demo"), Some(alias.clone()), Some("typed".into())).unwrap();
    assert_eq!(pw.as_deref(), Some("typed"));

    save_datasource_impl(&alias, direct_ds("demo"), Some("pw-2".into())).unwrap();
    assert_eq!(secrets::db_password(&alias).unwrap().as_deref(), Some("pw-2"));

    delete_datasource_impl(&alias).unwrap();
    assert!(!config::load().unwrap().dbs.contains_key(&alias));
    assert_eq!(secrets::db_password(&alias).unwrap(), None, "删数据源同时删密码");
}

#[test]
fn delete_datasource_refuses_when_project_uses_it() {
    let _home = test_home();
    let mut cfg = AppConfig::default();
    let mut p = project("r1");
    p.dbs = vec!["demo_orders".into()];
    cfg.projects.insert("proj-a".into(), p);
    cfg.dbs.insert("demo_orders".into(), skill_ds("demo_orders"));
    seed(cfg);
    let err = delete_datasource_impl("demo_orders").unwrap_err();
    assert_eq!(err, "数据源 demo_orders 被项目 proj-a 使用，先在项目里取消关联");
    assert!(config::load().unwrap().dbs.contains_key("demo_orders"));
    assert!(delete_datasource_impl("nope").unwrap_err().contains("找不到"));
}

#[test]
fn test_datasource_only_for_direct() {
    let err = prepare_test_datasource(skill_ds("d"), None, None).unwrap_err();
    assert!(err.contains("技能方式"), "{err}");
    let (ds, pw) = prepare_test_datasource(direct_ds("d"), None, Some("typed".into())).unwrap();
    assert_eq!(pw.as_deref(), Some("typed"));
    assert!(matches!(&ds.access, Access::Direct(c) if c.host == "127.0.0.1"), "测试前也去空白");
    let (_, pw) = prepare_test_datasource(direct_ds("d"), Some("  ".into()), Some(String::new())).unwrap();
    assert_eq!(pw, None, "没填密码也没有别名：不带密码去连");
}

#[test]
fn list_local_skills_scans_claude_and_codex_dirs() {
    let _home = test_home();
    assert!(list_local_skills_impl().is_empty());
    let mk = |root: std::path::PathBuf, name: &str, md: Option<&str>| {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        if let Some(md) = md {
            std::fs::write(d.join("SKILL.md"), md).unwrap();
        }
    };
    let claude = crate::catalog::Target::Claude.skills_root();
    let codex = crate::catalog::Target::Codex.skills_root();
    mk(claude.clone(), "zeta", Some("---\nname: zeta\ndescription: \"查库技能\"\n---\n正文"));
    mk(claude.clone(), "alpha", Some("没有 front matter"));
    mk(claude, "no-md", None);
    mk(codex, "my-sql", Some("---\nname: my-sql\ndescription: Codex 里的\n---\n"));
    let got = list_local_skills_impl();
    let brief: Vec<(&str, &str, &str)> = got.iter().map(|s| (s.name.as_str(), s.location, s.description.as_str())).collect();
    assert_eq!(brief, [("alpha", "claude", ""), ("zeta", "claude", "查库技能"), ("my-sql", "codex", "Codex 里的")]);
    let v = serde_json::to_value(&got[0]).unwrap();
    assert_eq!(v, serde_json::json!({"name": "alpha", "location": "claude", "description": ""}));
}

#[test]
fn default_projects_validate() {
    let _home = test_home();
    let mut cfg = AppConfig::default();
    cfg.projects.insert("proj-a".into(), project("r1"));
    seed(cfg);

    set_default_projects_impl(vec!["proj-a".into()]).unwrap();
    assert_eq!(config::load().unwrap().default_projects, ["proj-a"]);
    assert!(set_default_projects_impl(vec!["没有".into()]).unwrap_err().contains("没有"));
    assert_eq!(config::load().unwrap().default_projects, ["proj-a"], "校验失败不改配置");
}

/// 技能设置表单：字段来自技能清单的 `settings_schema.fields`（用一个导入的技能测，内置技能都没有设置项）。
#[test]
fn skill_settings_returns_schema_fields_and_saved_value() {
    let _home = test_home();
    let dir = crate::skills::user_dir().join("demo-settings");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "---\nname: demo-settings\ndescription: 示例\n---\n").unwrap();
    std::fs::write(
        dir.join(crate::skills::MANIFEST_FILE),
        r#"{"name":"demo-settings","display_name":"示例","requires":[],
            "settings_schema":{"fields":[{"key":"endpoint","label":"地址","type":"text","required":true},{"key":"status","label":"状态","type":"text","required":false}]}}"#,
    )
    .unwrap();
    let view = skill_settings_impl("demo-settings").unwrap();
    let keys: Vec<&str> = view.fields.iter().filter_map(|f| f["key"].as_str()).collect();
    assert_eq!(keys, ["endpoint", "status"]);
    assert_eq!(view.value, serde_json::json!({}));

    save_skill_settings_impl("demo-settings", serde_json::json!({"endpoint": "http://demo", "status": "verified"})).unwrap();
    let view = skill_settings_impl("demo-settings").unwrap();
    assert_eq!(view.value["endpoint"], "http://demo");
    assert!(skill_settings_impl("不是技能").unwrap_err().contains("技能"));
    assert!(save_skill_settings_impl("demo-settings", serde_json::json!("字符串")).unwrap_err().contains("对象"));
    assert!(save_skill_settings_impl("不是技能", serde_json::json!({})).is_err());
}

#[test]
fn skill_without_settings_schema_has_no_fields() {
    let _home = test_home();
    assert!(skill_settings_impl("code-sync").unwrap().fields.is_empty());
}

#[test]
fn first_run_enables_autostart_once_and_respects_user_choice() {
    let mut cfg = AppConfig::default();
    assert!(first_run_autostart(&mut cfg), "首次运行默认开启");
    assert_eq!(cfg.autostart, Some(true));
    assert!(!first_run_autostart(&mut cfg), "已初始化不再处理");
    cfg.autostart = Some(false);
    assert!(!first_run_autostart(&mut cfg), "用户关掉后不再自动打开");
    assert_eq!(cfg.autostart, Some(false));
}

#[test]
fn failed_autostart_enable_is_retried_next_launch() {
    let mut cfg = AppConfig::default();
    assert!(first_run_autostart(&mut cfg));
    autostart_enable_failed(&mut cfg);
    assert_eq!(cfg.autostart, None, "撤回标记");
    assert!(first_run_autostart(&mut cfg), "下次启动再试");
    cfg.autostart = Some(false);
    autostart_enable_failed(&mut cfg);
    assert_eq!(cfg.autostart, Some(false), "用户关掉的不动");
}
