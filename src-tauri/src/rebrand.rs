//! 改名（BUG纠察队 → Beetle）后的启动迁移，以及按配置版本号做的一次性配置升级。
//!
//! 迁移（只做一次，幂等）：旧配置目录 `%APPDATA%\BugPatrol` 存在、新目录 `%APPDATA%\Beetle` 不存在时，
//! 复制 `config.json`、`runs.db`（含 `-wal` / `-shm`）、`templates/`、用户导入的 `skills/`，
//! 以及旧本地数据目录 `%LOCALAPPDATA%\BugPatrol\runs` 下的原始日志；`workspaces/`、`code/` 可重建，不复制。
//! 先复制到临时目录再整体改名成新目录：中途失败不会留下半套新目录，下次启动会重试。
//! App Secret 不再从旧凭据库复制（旧 service 名不内置），需要时用 `beetle --import-secret <app_id>` 重新写入。
//! 旧目录原样保留，不删除。迁移结果写进 `<新配置目录>/migration.log`。

use crate::platform::paths;
use crate::store::config;
use std::path::{Path, PathBuf};

/// 当前配置格式版本。
/// - 1：已有机器人补上 issue-record 技能（启用了 feishu-doc-record 而没有 issue-record 的）。
/// - 2：机器人技能里的 sql-window（已移除的内置技能）换成通用的 db-query（保持位置、去重）。
///   旧数据源 `{kind, database}` 在反序列化时就转成「用 sql-window 技能查」（见 `DataSource`），这里不用再改；
///   `skill_settings["sql-window"]` 留着不删（用户本机的 sql-window 技能可能还要参考），运行时不再输出。
pub const CONFIG_VERSION: u32 = 2;

/// 配置目录里要复制的文件（不存在的跳过）。
const CONFIG_FILES: [&str; 4] = ["config.json", "runs.db", "runs.db-wal", "runs.db-shm"];
/// 配置目录里要复制的子目录：自定义模板、用户导入的技能。
const CONFIG_SUBDIRS: [&str; 2] = ["templates", "skills"];
/// 本地数据目录里要复制的子目录：运行原始日志。
const DATA_SUBDIRS: [&str; 1] = ["runs"];

const LOG_FILE: &str = "migration.log";

#[derive(Debug, Default, PartialEq)]
pub struct MigrationReport {
    /// 是否做了迁移（false = 没有旧目录或新目录已存在，什么都没动）。
    pub migrated: bool,
    /// 复制了的文件 / 目录（相对名）。
    pub copied: Vec<String>,
    /// 复制了 App Secret 的 app_id。
    pub secrets_copied: Vec<String>,
    /// 不影响启动的问题：日志复制失败、凭据读不到等。
    pub warnings: Vec<String>,
}

impl MigrationReport {
    fn lines(&self, old: &Path, new: &Path) -> Vec<String> {
        let mut out = vec![format!("从 {} 迁移到 {}", old.display(), new.display())];
        out.push(format!("复制：{}", if self.copied.is_empty() { "（无）".to_string() } else { self.copied.join("、") }));
        out.push(format!(
            "App Secret：{}",
            if self.secrets_copied.is_empty() { "（无）".to_string() } else { self.secrets_copied.join("、") }
        ));
        out.extend(self.warnings.iter().map(|w| format!("注意：{w}")));
        out
    }
}

fn io<T>(what: &str, r: std::io::Result<T>) -> Result<T, String> {
    r.map_err(|e| format!("{what}失败：{e}"))
}

/// 把 `src` 目录下的文件补到 `dst`（已存在的同名文件不覆盖），返回复制了几个文件。
fn merge_dir(src: &Path, dst: &Path) -> Result<usize, String> {
    io(&format!("创建目录 {}", dst.display()), std::fs::create_dir_all(dst))?;
    let mut n = 0;
    for e in io(&format!("读取目录 {}", src.display()), std::fs::read_dir(src))?.flatten() {
        let from = e.path();
        let to = dst.join(e.file_name());
        if from.is_dir() {
            n += merge_dir(&from, &to)?;
        } else if !to.exists() {
            io(&format!("复制 {}", from.display()), std::fs::copy(&from, &to))?;
            n += 1;
        }
    }
    Ok(n)
}

/// 配置里所有机器人的 app_id（按原始 JSON 读，配置格式以后变了也不影响迁移）。
fn bot_app_ids(config_json: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(config_json) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = v
        .get("bots")
        .and_then(|b| b.as_array())
        .into_iter()
        .flatten()
        .filter_map(|b| b.get("app_id").and_then(|a| a.as_str()))
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(String::from)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// 旧配置里开机自启是开着的：改成「未初始化」，新程序首次启动时按新名字（Beetle）重新登记系统自启项。
/// 旧的自启项登记的是旧程序，新程序里查不到它，不重置的话设置页显示「已开启」但开机不会启动新程序。
fn reset_autostart(config_json: &Path) -> Result<bool, String> {
    let Ok(text) = std::fs::read_to_string(config_json) else { return Ok(false) };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else {
        return Ok(false);
    };
    if v.get("autostart").and_then(|a| a.as_bool()) != Some(true) {
        return Ok(false);
    }
    v["autostart"] = serde_json::Value::Null;
    let text = serde_json::to_string_pretty(&v).map_err(|e| format!("序列化配置失败：{e}"))?;
    io(&format!("写入 {}", config_json.display()), std::fs::write(config_json, text)).map(|()| true)
}

/// 迁移本体。目录和「复制一个 App Secret」都由参数给出，单测不碰真实目录和系统凭据库。
/// `copy_secret(app_id)`：Ok(true) 复制了；Ok(false) 旧的没有（或新的已有）；Err 读写凭据库出错。
/// 返回 Err 只在配置目录没迁成时（下次启动重试）；其余问题记进 `warnings`。
pub fn migrate_legacy_dirs(
    old_config: &Path,
    old_data: &Path,
    new_config: &Path,
    new_data: &Path,
    copy_secret: &dyn Fn(&str) -> Result<bool, String>,
) -> Result<MigrationReport, String> {
    let mut report = MigrationReport::default();
    if !old_config.is_dir() || new_config.exists() {
        return Ok(report);
    }
    let parent = new_config.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let dir_name = new_config.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Beetle".into());
    let staging = parent.join(format!(".{dir_name}.migrating-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let cleanup = |e: String| {
        let _ = std::fs::remove_dir_all(&staging);
        e
    };
    io(&format!("创建目录 {}", staging.display()), std::fs::create_dir_all(&staging)).map_err(cleanup)?;
    for f in CONFIG_FILES {
        let from = old_config.join(f);
        if from.is_file() {
            io(&format!("复制 {}", from.display()), std::fs::copy(&from, staging.join(f))).map_err(cleanup)?;
            report.copied.push(f.to_string());
        }
    }
    for d in CONFIG_SUBDIRS {
        let from = old_config.join(d);
        if from.is_dir() {
            crate::workspace::copy_dir(&from, &staging.join(d)).map_err(cleanup)?;
            report.copied.push(format!("{d}/"));
        }
    }
    if reset_autostart(&staging.join("config.json")).map_err(cleanup)? {
        report.warnings.push("开机自启：按新名称重新登记（旧的「BUG纠察队」自启项随旧版卸载删除）".into());
    }
    // 原始日志：不影响使用，复制失败只记一条注意，不阻止迁移
    for d in DATA_SUBDIRS {
        let from = old_data.join(d);
        if from.is_dir() {
            match merge_dir(&from, &new_data.join(d)) {
                Ok(n) => report.copied.push(format!("{d}/（{n} 个文件）")),
                Err(e) => report.warnings.push(format!("原始日志 {d}/ 没有复制完整：{e}")),
            }
        }
    }
    io(
        &format!("把 {} 改名为 {}", staging.display(), new_config.display()),
        std::fs::rename(&staging, new_config),
    )
    .map_err(cleanup)?;
    report.migrated = true;

    for app_id in bot_app_ids(&new_config.join("config.json")) {
        match copy_secret(&app_id) {
            Ok(true) => report.secrets_copied.push(app_id),
            Ok(false) => report.warnings.push(format!("{app_id}：旧凭据库里没有 App Secret（或新凭据库已有），跳过")),
            Err(e) => report.warnings.push(format!("{app_id}：复制 App Secret 失败，跳过（{e}），请在客户端里重新填写")),
        }
    }
    Ok(report)
}

/// 配置升级：按 `config_version` 补做各版本的一次性改动，最后把版本号写成当前版本。
/// 返回本次改了技能列表的机器人 id（按机器人顺序、不重复）。版本号已是最新时什么都不改。
pub fn upgrade_config(cfg: &mut config::AppConfig) -> Vec<String> {
    let mut changed: Vec<String> = Vec::new();
    if cfg.config_version >= CONFIG_VERSION {
        return changed;
    }
    if cfg.config_version < 1 {
        for b in &mut cfg.bots {
            let has = |n: &str| b.skills.iter().any(|s| s == n);
            if has("feishu-doc-record") && !has("issue-record") {
                // 放在 feishu-doc-record 前面，和新建机器人的默认顺序一致
                let pos = b.skills.iter().position(|s| s == "feishu-doc-record").unwrap_or(b.skills.len());
                b.skills.insert(pos, "issue-record".to_string());
                changed.push(b.id.clone());
            }
        }
    }
    if cfg.config_version < 2 {
        for b in &mut cfg.bots {
            if replace_skill(&mut b.skills, "sql-window", "db-query") && !changed.contains(&b.id) {
                changed.push(b.id.clone());
            }
        }
    }
    cfg.config_version = CONFIG_VERSION;
    // 按机器人顺序输出
    let order: Vec<String> = cfg.bots.iter().map(|b| b.id.clone()).collect();
    changed.sort_by_key(|id| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
    changed
}

/// 把技能列表里的 `from` 换成 `to`，保持位置；`to` 已在列表里时只删 `from`。返回是否改了。
fn replace_skill(skills: &mut Vec<String>, from: &str, to: &str) -> bool {
    let Some(pos) = skills.iter().position(|s| s == from) else { return false };
    if !skills.iter().any(|s| s == to) {
        skills[pos] = to.to_string();
    }
    // 剩下的 `from`（`to` 已存在时的原位，或手改配置留下的重复项）全部去掉
    skills.retain(|s| s != from);
    true
}

fn write_log(lines: &[String]) {
    for l in lines {
        eprintln!("beetle: {l}");
    }
    let path = paths::config_dir().join(LOG_FILE);
    let stamp = chrono::Local::now().to_rfc3339();
    let text: String = lines.iter().map(|l| format!("[{stamp}] {l}\n")).collect();
    let r = std::fs::create_dir_all(paths::config_dir()).and_then(|()| {
        use std::io::Write;
        std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(text.as_bytes())
    });
    if let Err(e) = r {
        eprintln!("beetle: 写迁移日志 {} 失败：{e}", path.display());
    }
}

/// 读配置并按需升级；配置损坏时不动它（交给后续正常的报错流程）。
fn upgrade_config_file() -> Result<Option<Vec<String>>, String> {
    if config::load()?.config_version >= CONFIG_VERSION {
        return Ok(None);
    }
    let mut changed = Vec::new();
    config::update(|cfg| {
        changed = upgrade_config(cfg);
        Ok(())
    })?;
    Ok(Some(changed))
}

/// 启动时调用（在读配置、打开运行记录库之前）：先迁移旧目录，再升级配置。出错只记日志，不阻止启动。
pub fn run_startup_migrations() {
    let (old_config, new_config) = (paths::legacy_config_dir(), paths::config_dir());
    match migrate_legacy_dirs(&old_config, &paths::legacy_data_dir(), &new_config, &paths::data_dir(), &|_id| {
        // 旧版凭据库 service 名带有内部标识，开源版不再内置；内部用户早已完成迁移，这里不再复制 App Secret。
        Ok(false)
    }) {
        Ok(r) if r.migrated => write_log(&r.lines(&old_config, &new_config)),
        Ok(_) => {}
        Err(e) => eprintln!("beetle: 从 {} 迁移失败，下次启动重试：{e}", old_config.display()),
    }
    match upgrade_config_file() {
        Ok(Some(changed)) if !changed.is_empty() => {
            write_log(&[format!(
                "配置升级到版本 {CONFIG_VERSION}：机器人 {} 的技能列表已更新（补 issue-record / sql-window 换成 db-query）",
                changed.join("、")
            )])
        }
        Ok(_) => {}
        Err(e) => eprintln!("beetle: 配置升级失败：{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::config::{AppConfig, Bot};
    use crate::test_support::test_home;
    use std::cell::RefCell;

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    /// 在 test home 下构造旧目录：配置、运行记录库（含 wal/shm）、模板、导入的技能、原始日志、可重建的工作区和代码缓存。
    fn seed_legacy() {
        let c = paths::legacy_config_dir();
        let d = paths::legacy_data_dir();
        write(&c.join("config.json"), r#"{"bots":[{"app_id":"cli_old_a"},{"app_id":"cli_old_b"},{"app_id":"cli_old_a"}],"autostart":true}"#);
        write(&c.join("runs.db"), "db");
        write(&c.join("runs.db-wal"), "wal");
        write(&c.join("runs.db-shm"), "shm");
        write(&c.join("templates").join("bug-hunt.md"), "自定义模板");
        write(&c.join("skills").join("my-skill").join("SKILL.md"), "导入的技能");
        write(&d.join("runs").join("r1.jsonl"), "日志1");
        write(&d.join("runs").join("r1.stderr.txt"), "错误输出");
        write(&d.join("workspaces").join("b1").join("g").join("CLAUDE.md"), "工作区");
        write(&d.join("code").join("repo").join("a.txt"), "代码");
    }

    fn migrate(copy: &dyn Fn(&str) -> Result<bool, String>) -> Result<MigrationReport, String> {
        migrate_legacy_dirs(&paths::legacy_config_dir(), &paths::legacy_data_dir(), &paths::config_dir(), &paths::data_dir(), copy)
    }

    #[test]
    fn legacy_dirs_live_under_test_home() {
        let home = test_home();
        assert_eq!(paths::legacy_config_dir(), home.path().join("legacy").join("config"));
        assert_eq!(paths::legacy_data_dir(), home.path().join("legacy").join("data"));
        assert_ne!(paths::legacy_config_dir(), paths::config_dir());
    }

    #[test]
    fn migrates_config_runs_templates_and_logs_but_not_workspaces() {
        let _home = test_home();
        seed_legacy();
        let asked = RefCell::new(Vec::new());
        let r = migrate(&|id| {
            asked.borrow_mut().push(id.to_string());
            Ok(id == "cli_old_a")
        })
        .unwrap();
        assert!(r.migrated);
        let c = paths::config_dir();
        let new_cfg: serde_json::Value = serde_json::from_str(&read(&c.join("config.json"))).unwrap();
        let old_cfg: serde_json::Value = serde_json::from_str(&read(&paths::legacy_config_dir().join("config.json"))).unwrap();
        assert_eq!(new_cfg["bots"], old_cfg["bots"]);
        assert_eq!(old_cfg["autostart"], true, "旧目录原样保留");
        assert!(new_cfg["autostart"].is_null(), "开机自启重置为未初始化，新程序按新名字重新登记");
        assert_eq!(read(&c.join("runs.db")), "db");
        assert_eq!(read(&c.join("runs.db-wal")), "wal");
        assert_eq!(read(&c.join("runs.db-shm")), "shm");
        assert_eq!(read(&c.join("templates").join("bug-hunt.md")), "自定义模板");
        assert_eq!(read(&c.join("skills").join("my-skill").join("SKILL.md")), "导入的技能");
        let d = paths::data_dir();
        assert_eq!(read(&d.join("runs").join("r1.jsonl")), "日志1");
        assert_eq!(read(&d.join("runs").join("r1.stderr.txt")), "错误输出");
        assert!(!d.join("workspaces").exists() && !d.join("code").exists(), "工作区和代码缓存可重建，不复制");
        // 凭据：每个 app_id 只问一次；读不到的记注意，不算失败
        assert_eq!(*asked.borrow(), ["cli_old_a", "cli_old_b"]);
        assert_eq!(r.secrets_copied, ["cli_old_a"]);
        assert!(r.warnings.iter().any(|w| w.contains("cli_old_b")), "{:?}", r.warnings);
        // 旧目录保留；没有留下临时目录
        assert!(paths::legacy_config_dir().join("config.json").is_file());
        let leftovers = std::fs::read_dir(c.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".migrating-"))
            .count();
        assert_eq!(leftovers, 0);
        let lines = r.lines(&paths::legacy_config_dir(), &c);
        assert!(lines.iter().any(|l| l.contains("runs.db-wal") && l.contains("templates/")), "{lines:?}");
    }

    #[test]
    fn migration_runs_only_once() {
        let _home = test_home();
        seed_legacy();
        assert!(migrate(&|_| Ok(true)).unwrap().migrated);
        // 新目录里改过的配置不会被第二次迁移覆盖
        write(&paths::config_dir().join("config.json"), "{}");
        write(&paths::legacy_config_dir().join("config.json"), r#"{"bots":[]}"#);
        let calls = RefCell::new(0);
        let r = migrate(&|_| {
            *calls.borrow_mut() += 1;
            Ok(true)
        })
        .unwrap();
        assert_eq!(r, MigrationReport::default());
        assert_eq!(read(&paths::config_dir().join("config.json")), "{}");
        assert_eq!(*calls.borrow(), 0);
    }

    #[test]
    fn no_legacy_dir_means_nothing_to_do() {
        let _home = test_home();
        let r = migrate(&|_: &str| -> Result<bool, String> { panic!("没有旧目录不该碰凭据库") }).unwrap();
        assert!(!r.migrated);
        assert!(!paths::config_dir().exists(), "没有旧目录时不创建新目录");
    }

    #[test]
    fn new_dir_already_exists_skips_migration() {
        let _home = test_home();
        seed_legacy();
        write(&paths::config_dir().join("config.json"), r#"{"bots":[]}"#);
        let r = migrate(&|_: &str| -> Result<bool, String> { panic!("新目录已存在不该迁移") }).unwrap();
        assert!(!r.migrated);
        assert!(!paths::config_dir().join("runs.db").exists());
    }

    #[test]
    fn secret_errors_are_warnings_not_failures() {
        let _home = test_home();
        seed_legacy();
        let r = migrate(&|_| Err("凭据库打不开".into())).unwrap();
        assert!(r.migrated);
        assert!(r.secrets_copied.is_empty());
        assert_eq!(r.warnings.iter().filter(|w| w.contains("凭据库打不开")).count(), 2);
    }

    #[test]
    fn startup_migration_then_upgrade_end_to_end() {
        let _home = test_home();
        let c = paths::legacy_config_dir();
        let mut old = Bot::new_default("b1", "查 bug", "cli_e2e_never_in_keyring");
        old.skills = ["chat-context", "code-sync", "sql-window", "feishu-doc-record"].map(String::from).to_vec();
        let old_cfg = AppConfig { bots: vec![old], ..AppConfig::default() };
        write(&c.join("config.json"), &serde_json::to_string(&old_cfg).unwrap());
        // 用注入的凭据闭包走迁移（不碰系统凭据库），再走真实的配置升级
        assert!(migrate(&|_| Ok(false)).unwrap().migrated);
        assert_eq!(upgrade_config_file().unwrap(), Some(vec!["b1".to_string()]));
        let cfg = config::load().unwrap();
        assert_eq!(cfg.config_version, CONFIG_VERSION);
        assert_eq!(cfg.bots[0].skills, ["chat-context", "code-sync", "db-query", "issue-record", "feishu-doc-record"]);
        // 已是最新版本：不再改
        assert_eq!(upgrade_config_file().unwrap(), None);
    }

    #[test]
    fn upgrade_adds_issue_record_once_only_where_doc_record_enabled() {
        let mut with_doc = Bot::new_default("b1", "一", "cli_a");
        with_doc.skills = ["chat-context", "feishu-doc-record", "my-skill"].map(String::from).to_vec();
        let mut without_doc = Bot::new_default("b2", "二", "cli_b");
        without_doc.skills = ["chat-context", "code-sync"].map(String::from).to_vec();
        let already = Bot::new_default("b3", "三", "cli_c");
        let mut cfg = AppConfig { bots: vec![with_doc, without_doc, already], ..AppConfig::default() };
        assert_eq!(cfg.config_version, 0, "旧配置没有版本号字段时为 0");

        assert_eq!(upgrade_config(&mut cfg), ["b1"]);
        assert_eq!(cfg.config_version, CONFIG_VERSION);
        assert_eq!(cfg.bots[0].skills, ["chat-context", "issue-record", "feishu-doc-record", "my-skill"]);
        assert_eq!(cfg.bots[1].skills, ["chat-context", "code-sync"], "没启用写文档的不补");
        assert_eq!(cfg.bots[2].skills.iter().filter(|s| *s == "issue-record").count(), 1, "已有的不重复加");

        // 用户升级后又去掉了 issue-record：版本号已是最新，不再补回来
        cfg.bots[0].skills.retain(|s| s != "issue-record");
        assert!(upgrade_config(&mut cfg).is_empty());
        assert!(!cfg.bots[0].skills.iter().any(|s| s == "issue-record"));
    }

    /// 版本 2：sql-window 原位换成 db-query；已有 db-query 的只删 sql-window；重复项去掉；没有的不动。
    #[test]
    fn upgrade_v2_replaces_sql_window_skill() {
        let bot = |id: &str, skills: &[&str]| {
            let mut b = Bot::new_default(id, id, "cli_x");
            b.skills = skills.iter().map(|s| s.to_string()).collect();
            b
        };
        let mut cfg = AppConfig {
            bots: vec![
                bot("b1", &["chat-context", "code-sync", "sql-window", "issue-record", "feishu-doc-record"]),
                bot("b2", &["sql-window", "db-query", "code-sync"]),
                bot("b3", &["pr-review", "code-sync"]),
                bot("b4", &["sql-window", "chat-context", "sql-window"]),
            ],
            config_version: 1,
            ..AppConfig::default()
        };
        assert_eq!(upgrade_config(&mut cfg), ["b1", "b2", "b4"]);
        assert_eq!(cfg.config_version, 2);
        assert_eq!(cfg.bots[0].skills, ["chat-context", "code-sync", "db-query", "issue-record", "feishu-doc-record"]);
        assert_eq!(cfg.bots[1].skills, ["db-query", "code-sync"]);
        assert_eq!(cfg.bots[2].skills, ["pr-review", "code-sync"]);
        assert_eq!(cfg.bots[3].skills, ["db-query", "chat-context"]);
        // 已是最新版本：用户之后手动加回 sql-window 也不再改
        cfg.bots[2].skills.push("sql-window".into());
        assert!(upgrade_config(&mut cfg).is_empty());
        assert!(cfg.bots[2].skills.iter().any(|s| s == "sql-window"));
    }

    /// 旧配置的库 `{kind, database}`（没有 access）读出来是「用 sql-window 技能查」，现有的库升级后还能查；
    /// sql-window 的设置保留不删。
    #[test]
    fn old_db_entry_becomes_skill_sql_window() {
        use crate::store::datasource::{Access, DbKind, SkillRef};
        let _home = test_home();
        write(
            &paths::config_dir().join("config.json"),
            r#"{"bots":[],"dbs":{"demo_a":{"kind":"mysql","database":"demo_a"},"demo_b":{"kind":"weird","database":"demo_b"}},
               "skill_settings":{"sql-window":{"status":"verified"}}}"#,
        );
        let cfg = config::load().unwrap();
        let a = &cfg.dbs["demo_a"];
        assert_eq!(a.kind, DbKind::Mysql);
        assert_eq!(
            a.access,
            Access::Skill(SkillRef { skill: "sql-window".into(), database: "demo_a".into(), note: String::new() })
        );
        assert_eq!(cfg.dbs["demo_b"].kind, DbKind::Other, "未知类型按 other");
        assert_eq!(upgrade_config_file().unwrap(), Some(vec![]));
        let cfg = config::load().unwrap();
        assert_eq!(cfg.config_version, CONFIG_VERSION);
        assert!(matches!(&cfg.dbs["demo_a"].access, Access::Skill(s) if s.skill == "sql-window" && s.database == "demo_a"));
        assert_eq!(cfg.skill_settings["sql-window"]["status"], "verified");
    }

    #[test]
    fn old_config_json_without_version_field_loads_as_zero() {
        let _home = test_home();
        write(&paths::config_dir().join("config.json"), r#"{"bots":[]}"#);
        assert_eq!(config::load().unwrap().config_version, 0);
    }
}
