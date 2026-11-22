//! 旧版 bug-bot（lark-channel-bridge + Claude Code）迁移（spec §14）。
//!
//! 读取：`~/.claude/skills/bug-bot/config.json`（项目、库、群、默认项目）、
//! `~/.lark-channel/config.json`（App ID、引擎、白名单群）、`~/.codex/sql-window/connection.json`（SQL 执行窗口）；
//! 密钥经旧 bridge 的 `secrets-getter.cmd` 读出后直接写进系统凭据库，全程不打印、不记录、不进配置。
//! 系统操作（查 / 停进程和计划任务、读密钥、写凭据库）都经 trait / 闭包注入，测试不碰真实环境。

use crate::platform::legacy::{DEFAULT_TASK, LegacySys, SecretGetter};
use crate::store::config::{self, Bot, Engine, Group, Project, Trigger};
use crate::store::datasource::DataSource;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct LegacyFiles {
    pub home: PathBuf,
}

impl LegacyFiles {
    pub fn from_home() -> Result<Self, String> {
        dirs::home_dir().map(|home| LegacyFiles { home }).ok_or_else(|| "找不到用户主目录".to_string())
    }
    fn bug_bot_config(&self) -> PathBuf {
        self.home.join(".claude").join("skills").join("bug-bot").join("config.json")
    }
    fn lark_config(&self) -> PathBuf {
        self.home.join(".lark-channel").join("config.json")
    }
    pub fn getter_script(&self) -> PathBuf {
        self.home.join(".lark-channel").join("secrets-getter.cmd")
    }
    fn sql_window_connection(&self) -> PathBuf {
        self.home.join(".codex").join("sql-window").join("connection.json")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LegacyGroupInfo {
    pub chat_id: String,
    pub name: String,
    pub doc_url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LegacyInfo {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    pub groups: Vec<LegacyGroupInfo>,
    pub projects: Vec<String>,
    pub bridge_running: bool,
    pub task_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MigrateResult {
    pub bot_id: String,
    pub warnings: Vec<String>,
}

/// 解析后的旧版配置。
struct Legacy {
    app_id: String,
    engine: Engine,
    groups: Vec<Group>,
    projects: BTreeMap<String, Project>,
    dbs: BTreeMap<String, DataSource>,
    default_projects: Vec<String>,
    sql_window: Option<Value>,
    task: String,
    warnings: Vec<String>,
}

fn read_json(path: &std::path::Path) -> Result<Option<Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取 {} 失败：{e}", path.display())),
    };
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map(Some)
        .map_err(|e| format!("旧版配置 {} 解析失败：{e}", path.display()))
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn strings_of(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn short_slug(chat_id: &str) -> String {
    let tail: String = chat_id.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
    format!("bug-bot-{tail}")
}

/// 找不到旧版配置（任意一个必需文件缺失）返回 `Ok(None)`。
fn read_legacy(files: &LegacyFiles) -> Result<Option<Legacy>, String> {
    let (Some(bot_cfg), Some(lark)) = (read_json(&files.bug_bot_config())?, read_json(&files.lark_config())?) else {
        return Ok(None);
    };
    let mut warnings = Vec::new();

    // bridge 的 profile：bug-bot 配置里指定的，其次 lark-channel 的 activeProfile，最后 claude。
    let profile = bot_cfg
        .pointer("/bridge/profile")
        .and_then(Value::as_str)
        .or_else(|| lark.get("activeProfile").and_then(Value::as_str))
        .unwrap_or("claude")
        .to_string();
    let prof = lark.pointer(&format!("/profiles/{profile}"));
    let Some(app_id) = prof.and_then(|p| p.pointer("/accounts/app/id")).and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let engine = match prof.and_then(|p| p.get("agentKind")).and_then(Value::as_str) {
        Some("codex") => Engine::Codex,
        _ => Engine::Claude,
    };
    let allowed: Vec<String> = prof.and_then(|p| p.pointer("/access/allowedChats")).map(|a| strings_of(&serde_json::json!({"x": a}), "x")).unwrap_or_default();

    let mut projects = BTreeMap::new();
    if let Some(m) = bot_cfg.get("projects").and_then(Value::as_object) {
        for (name, v) in m {
            let Some(repo) = str_of(v, "repo") else {
                warnings.push(format!("旧版项目 {name} 没有仓库地址，已跳过"));
                continue;
            };
            projects.insert(
                name.clone(),
                Project::new(
                    repo,
                    str_of(v, "branch_glob").unwrap_or("release/*"),
                    str_of(v, "rules_file").unwrap_or("AGENTS.md"),
                    str_of(v, "description").unwrap_or_default(),
                    strings_of(v, "dbs"),
                ),
            );
        }
    }
    let mut dbs = BTreeMap::new();
    if let Some(m) = bot_cfg.get("dbs").and_then(Value::as_object) {
        for (alias, v) in m {
            // 旧版只有库名、靠 sql-window 查：按旧格式交给 DataSource 的兼容反序列化，转成「用 sql-window 技能查」
            let old = serde_json::json!({
                "kind": str_of(v, "kind").unwrap_or("mysql"),
                "database": str_of(v, "database").unwrap_or(alias),
            });
            match serde_json::from_value::<DataSource>(old) {
                Ok(ds) => {
                    dbs.insert(alias.clone(), ds);
                }
                Err(e) => warnings.push(format!("旧版库 {alias} 无法转换（{e}），已跳过，请在「数据源」页重新添加")),
            }
        }
    }
    let default_projects = strings_of(&bot_cfg, "default_projects");

    // 群：旧版的白名单（谁都能 @）对应新版「所有人可触发」。
    let mut groups: Vec<Group> = Vec::new();
    if let Some(m) = bot_cfg.get("groups").and_then(Value::as_object) {
        for (chat_id, v) in m {
            let mut ps = strings_of(v, "projects");
            if ps.is_empty() {
                ps = default_projects.clone();
            }
            groups.push(Group {
                chat_id: chat_id.clone(),
                name: str_of(v, "name").unwrap_or(chat_id).to_string(),
                slug: str_of(v, "slug").map(str::to_string).unwrap_or_else(|| short_slug(chat_id)),
                projects: ps,
                doc_url: str_of(v, "doc_url").map(str::to_string),
                trigger: Trigger::All,
                all_projects: false,
            });
        }
    }
    for chat_id in allowed {
        if !groups.iter().any(|g| g.chat_id == chat_id) {
            warnings.push(format!("白名单里的群 {chat_id} 在旧版 bug-bot 里没有登记（没有群名和记录文档），已按默认项目导入，请在群设置里补充"));
            groups.push(Group {
                name: chat_id.clone(),
                slug: short_slug(&chat_id),
                projects: default_projects.clone(),
                doc_url: None,
                trigger: Trigger::All,
                chat_id,
                all_projects: false,
            });
        }
    }

    let sql_window = read_json(&files.sql_window_connection())?.and_then(|v| {
        str_of(&v, "chatId")?;
        let mut m = serde_json::Map::new();
        for key in ["chatId", "botId", "groupName", "botName", "status"] {
            if let Some(s) = str_of(&v, key) {
                m.insert(key.to_string(), Value::String(s.to_string()));
            }
        }
        Some(Value::Object(m))
    });
    let task = bot_cfg.pointer("/bridge/scheduled_task").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_TASK).to_string();

    Ok(Some(Legacy { app_id: app_id.to_string(), engine, groups, projects, dbs, default_projects, sql_window, task, warnings }))
}

pub fn detect_legacy_impl(files: &LegacyFiles, sys: &dyn LegacySys) -> Result<LegacyInfo, String> {
    let Some(l) = read_legacy(files)? else { return Ok(LegacyInfo::default()) };
    Ok(LegacyInfo {
        found: true,
        groups: l
            .groups
            .iter()
            .map(|g| LegacyGroupInfo { chat_id: g.chat_id.clone(), name: g.name.clone(), doc_url: g.doc_url.clone().unwrap_or_default() })
            .collect(),
        projects: l.projects.keys().cloned().collect(),
        bridge_running: sys.bridge_running(),
        task_enabled: sys.task_enabled(&l.task),
        app_id: Some(l.app_id),
    })
}

/// 导入机器人、群、项目、库、默认项目、SQL 执行窗口连接；`stop_bridge` 为 true 时导入成功后停旧 bridge。
/// 已存在同 App ID 的机器人时不重复创建，只补上缺的群。已有的项目 / 库 / 技能设置不覆盖。
pub fn migrate_legacy_impl(
    files: &LegacyFiles,
    sys: &dyn LegacySys,
    getter: &dyn SecretGetter,
    store_secret: &dyn Fn(&str, &str) -> Result<(), String>,
    stop_bridge: bool,
) -> Result<MigrateResult, String> {
    let Some(legacy) = read_legacy(files)? else {
        return Err("没有检测到旧版 bug-bot 配置（需要 ~/.claude/skills/bug-bot/config.json 和 ~/.lark-channel/config.json）".into());
    };
    let mut warnings = legacy.warnings.clone();

    // 密钥放在配置更新之前读，且不进入任何错误 / 警告文字。
    match getter.get(&legacy.app_id) {
        Ok(secret) => {
            if let Err(e) = store_secret(&legacy.app_id, &secret) {
                warnings.push(format!("读到了 App Secret 但保存到系统凭据库失败（{e}），请在机器人设置里手动填写"));
            }
        }
        Err(e) => warnings.push(format!("没能从旧 bridge 读到 App Secret（{e}），请在机器人设置里手动填写")),
    }

    let mut bot_id = String::new();
    config::update(|cfg| {
        for (k, v) in &legacy.projects {
            cfg.projects.entry(k.clone()).or_insert_with(|| v.clone());
        }
        for (k, v) in &legacy.dbs {
            cfg.dbs.entry(k.clone()).or_insert_with(|| v.clone());
        }
        if cfg.default_projects.is_empty() {
            cfg.default_projects = legacy.default_projects.clone();
        }
        if let Some(sw) = &legacy.sql_window {
            cfg.skill_settings.entry("sql-window".to_string()).or_insert_with(|| sw.clone());
        }
        match cfg.bots.iter_mut().find(|b| b.app_id == legacy.app_id) {
            Some(bot) => {
                bot_id = bot.id.clone();
                for g in &legacy.groups {
                    if !bot.groups.iter().any(|x| x.chat_id == g.chat_id) {
                        bot.groups.push(g.clone());
                    }
                }
            }
            None => {
                let mut bot = Bot::new_default(&uuid::Uuid::new_v4().to_string(), "查 bug 机器人", &legacy.app_id);
                bot.engine = legacy.engine;
                bot.groups = legacy.groups.clone();
                bot_id = bot.id.clone();
                cfg.bots.push(bot);
            }
        }
        Ok(())
    })?;

    if stop_bridge {
        warnings.extend(sys.stop_bridge(&legacy.task));
    }
    Ok(MigrateResult { bot_id, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_home;
    use std::cell::RefCell;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeSys {
        running: bool,
        enabled: bool,
        stopped_tasks: Mutex<Vec<String>>,
        stop_warnings: Vec<String>,
    }
    impl LegacySys for FakeSys {
        fn bridge_running(&self) -> bool {
            self.running
        }
        fn task_enabled(&self, _task: &str) -> bool {
            self.enabled
        }
        fn stop_bridge(&self, task: &str) -> Vec<String> {
            self.stopped_tasks.lock().unwrap().push(task.to_string());
            self.stop_warnings.clone()
        }
    }

    struct FakeGetter(Result<&'static str, &'static str>);
    impl SecretGetter for FakeGetter {
        fn get(&self, _app_id: &str) -> Result<String, String> {
            self.0.map(String::from).map_err(String::from)
        }
    }

    fn write(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// 构造一份旧版目录，内容照真实机器上的结构。
    fn legacy_home(dir: &std::path::Path) -> LegacyFiles {
        let files = LegacyFiles { home: dir.join("legacy-home") };
        write(
            files.bug_bot_config(),
            r#"{
              "bridge": {"profile": "claude", "scheduled_task": "LarkChannelBridge.Bot.claude"},
              "projects": {
                "demo-service": {"repo": "git@x:demo.git", "branch_glob": "release/*", "rules_file": "AGENTS.md",
                                 "description": "示例项目", "dbs": ["demo_config", "demo_core"]},
                "broken": {"description": "没有仓库"}
              },
              "default_projects": ["demo-service"],
              "dbs": {"demo_config": {"kind": "mysql", "database": "demo_config"}, "demo_core": {"database": "demo_core"}},
              "groups": {
                "oc_aaa11111": {"name": "示例项目群", "slug": "demo-bug-bot", "projects": ["demo-service"], "doc_url": "https://x.feishu.cn/docx/a"},
                "oc_bbb22222": {"name": "测试群", "slug": "qc-bug-bot", "doc_url": ""}
              }
            }"#,
        );
        write(
            files.lark_config(),
            r#"{"activeProfile": "claude", "secrets": {"provider": "bridge"},
                "profiles": {"claude": {"agentKind": "claude",
                  "accounts": {"app": {"id": "cli_legacy", "secret": "不应被读取", "tenant": "feishu"}},
                  "access": {"allowedChats": ["oc_aaa11111", "oc_ccc33333"]}}}}"#,
        );
        write(
            files.sql_window_connection(),
            r#"{"status": "verified", "groupName": "SQL 群", "botName": "窗口", "chatId": "oc_sql", "botId": "ou_sqlbot", "botAppId": "cli_other", "note": "x"}"#,
        );
        files
    }

    #[test]
    fn detect_reports_groups_projects_and_bridge_state() {
        let home = test_home();
        let files = legacy_home(home.path());
        let sys = FakeSys { running: true, enabled: true, ..Default::default() };
        let info = detect_legacy_impl(&files, &sys).unwrap();
        assert!(info.found && info.bridge_running && info.task_enabled);
        assert_eq!(info.app_id.as_deref(), Some("cli_legacy"));
        assert_eq!(info.projects, ["demo-service"]);
        assert_eq!(info.groups.len(), 3);
        let g = info.groups.iter().find(|g| g.chat_id == "oc_aaa11111").unwrap();
        assert_eq!((g.name.as_str(), g.doc_url.as_str()), ("示例项目群", "https://x.feishu.cn/docx/a"));
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["groups"][0].as_object().unwrap().len(), 3);
    }

    #[test]
    fn detect_not_found_when_files_missing() {
        let home = test_home();
        let files = LegacyFiles { home: home.path().join("nobody") };
        let info = detect_legacy_impl(&files, &FakeSys::default()).unwrap();
        assert_eq!(info, LegacyInfo::default());
        assert!(serde_json::to_value(&info).unwrap().get("app_id").is_none());
    }

    #[test]
    fn migrate_imports_everything_and_keeps_secret_out_of_config() {
        let home = test_home();
        let files = legacy_home(home.path());
        let sys = FakeSys::default();
        let stored: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let store = |app: &str, secret: &str| {
            stored.borrow_mut().push((app.into(), secret.into()));
            Ok(())
        };
        let res = migrate_legacy_impl(&files, &sys, &FakeGetter(Ok("TOP-SECRET-VALUE")), &store, false).unwrap();

        assert_eq!(stored.into_inner(), [("cli_legacy".to_string(), "TOP-SECRET-VALUE".to_string())]);
        let cfg = config::load().unwrap();
        let text = std::fs::read_to_string(crate::platform::paths::config_dir().join("config.json")).unwrap();
        assert!(!text.contains("TOP-SECRET-VALUE") && !text.contains("不应被读取"));
        assert!(!res.warnings.iter().any(|w| w.contains("TOP-SECRET-VALUE")));

        assert_eq!(cfg.bots.len(), 1);
        let bot = &cfg.bots[0];
        assert_eq!((bot.id.as_str(), bot.app_id.as_str(), bot.engine), (res.bot_id.as_str(), "cli_legacy", Engine::Claude));
        assert_eq!(bot.groups.len(), 3);
        assert!(bot.groups.iter().all(|g| g.trigger == Trigger::All), "白名单 → 所有人可触发");
        let demo = bot.groups.iter().find(|g| g.chat_id == "oc_aaa11111").unwrap();
        assert_eq!((demo.slug.as_str(), demo.doc_url.as_deref()), ("demo-bug-bot", Some("https://x.feishu.cn/docx/a")));
        let qc = bot.groups.iter().find(|g| g.chat_id == "oc_bbb22222").unwrap();
        assert_eq!((qc.projects.clone(), qc.doc_url.clone()), (vec!["demo-service".to_string()], None));
        let extra = bot.groups.iter().find(|g| g.chat_id == "oc_ccc33333").unwrap();
        assert_eq!(extra.slug, "bug-bot-ccc33333");

        assert_eq!(cfg.projects.keys().collect::<Vec<_>>(), ["demo-service"]);
        assert_eq!(cfg.projects["demo-service"].dbs, ["demo_config", "demo_core"]);
        {
            use crate::store::datasource::{Access, DbKind, SkillRef};
            assert_eq!(cfg.dbs["demo_core"].kind, DbKind::Mysql, "旧版没写类型按 mysql");
            assert_eq!(
                cfg.dbs["demo_core"].access,
                Access::Skill(SkillRef { skill: "sql-window".into(), database: "demo_core".into(), note: String::new() }),
                "旧版的库转成用 sql-window 技能查"
            );
        }
        assert_eq!(cfg.default_projects, ["demo-service"]);
        assert_eq!(cfg.skill_settings["sql-window"]["chatId"], "oc_sql");
        assert!(cfg.skill_settings["sql-window"].get("note").is_none());
        assert!(res.warnings.iter().any(|w| w.contains("broken")));
        assert!(res.warnings.iter().any(|w| w.contains("oc_ccc33333")));
        assert!(sys.stopped_tasks.lock().unwrap().is_empty(), "stopBridge=false 不碰旧 bridge");
    }

    #[test]
    fn migrate_warns_when_secret_unavailable_and_still_imports() {
        let home = test_home();
        let files = legacy_home(home.path());
        let store = |_: &str, _: &str| -> Result<(), String> { panic!("取不到密钥时不应写凭据库") };
        let res = migrate_legacy_impl(&files, &FakeSys::default(), &FakeGetter(Err("脚本超时")), &store, false).unwrap();
        assert!(res.warnings.iter().any(|w| w.contains("手动填写") && w.contains("脚本超时")));
        assert_eq!(config::load().unwrap().bots.len(), 1);
    }

    #[test]
    fn stop_bridge_only_when_asked_and_after_import_with_warnings_passed_through() {
        let home = test_home();
        let files = legacy_home(home.path());
        let sys = FakeSys { stop_warnings: vec!["需要管理员权限".into()], ..Default::default() };
        let res = migrate_legacy_impl(&files, &sys, &FakeGetter(Ok("s")), &|_, _| Ok(()), true).unwrap();
        assert_eq!(*sys.stopped_tasks.lock().unwrap(), ["LarkChannelBridge.Bot.claude"]);
        assert!(res.warnings.contains(&"需要管理员权限".to_string()));
        assert_eq!(config::load().unwrap().bots.len(), 1);
    }

    #[test]
    fn migrate_twice_does_not_duplicate_and_keeps_existing_settings() {
        let home = test_home();
        let files = legacy_home(home.path());
        config::update(|c| {
            c.projects.insert(
                "demo-service".into(),
                Project::new("mine", "x", "r", "手改过", vec![]),
            );
            c.default_projects = vec!["mine".into()];
            Ok(())
        })
        .unwrap();
        let first = migrate_legacy_impl(&files, &FakeSys::default(), &FakeGetter(Ok("s")), &|_, _| Ok(()), false).unwrap();
        let second = migrate_legacy_impl(&files, &FakeSys::default(), &FakeGetter(Ok("s")), &|_, _| Ok(()), false).unwrap();
        assert_eq!(first.bot_id, second.bot_id);
        let cfg = config::load().unwrap();
        assert_eq!(cfg.bots.len(), 1);
        assert_eq!(cfg.bots[0].groups.len(), 3);
        assert_eq!(cfg.projects["demo-service"].description, "手改过");
        assert_eq!(cfg.default_projects, ["mine"]);
    }

    #[test]
    fn migrate_without_legacy_files_is_a_chinese_error() {
        let home = test_home();
        let files = LegacyFiles { home: home.path().join("none") };
        let err = migrate_legacy_impl(&files, &FakeSys::default(), &FakeGetter(Ok("s")), &|_, _| Ok(()), false).unwrap_err();
        assert!(err.contains("没有检测到旧版"));
        assert!(config::load().unwrap().bots.is_empty());
    }

    #[test]
    fn codex_profile_maps_to_codex_engine() {
        let home = test_home();
        let files = legacy_home(home.path());
        let text = std::fs::read_to_string(files.lark_config()).unwrap().replace("\"agentKind\": \"claude\"", "\"agentKind\": \"codex\"");
        std::fs::write(files.lark_config(), text).unwrap();
        migrate_legacy_impl(&files, &FakeSys::default(), &FakeGetter(Ok("s")), &|_, _| Ok(()), false).unwrap();
        assert_eq!(config::load().unwrap().bots[0].engine, Engine::Codex);
    }
}
