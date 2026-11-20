//! 配置导出 / 导入（spec §12）：把项目、数据源、模板、技能设置打包成一个 JSON 文件给同事用。
//! **不含任何密钥**（数据源密码只在本机 keyring，导入后要在「数据源」页重新填），也不含 owner_open_id；机器人只导出名称 / App ID / 引擎 / 模板 / 技能（预览用，导入时不创建机器人）。

use crate::store::config::{self, AppConfig, Engine, Project};
use crate::store::datasource::DataSource;
use crate::templates;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub const FORMAT_VERSION: u32 = 1;
/// 导入文件大小上限，防止误选了别的大文件。
const MAX_IMPORT_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportBot {
    pub name: String,
    pub app_id: String,
    pub engine: Engine,
    pub template: String,
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportFile {
    pub version: u32,
    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
    #[serde(default)]
    /// 旧版导出文件里的 `{kind, database}` 读进来是「用 sql-window 技能查」（见 `DataSource` 的兼容反序列化）。
    pub dbs: BTreeMap<String, DataSource>,
    #[serde(default)]
    pub templates: BTreeMap<String, String>,
    #[serde(default)]
    pub skills: BTreeMap<String, Value>,
    #[serde(default)]
    pub bots: Vec<ExportBot>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BotBrief {
    pub name: String,
    pub app_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImportPreview {
    pub projects: Vec<String>,
    pub dbs: Vec<String>,
    pub templates: Vec<String>,
    pub skills: Vec<String>,
    pub bots: Vec<BotBrief>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ImportSelect {
    pub projects: Vec<String>,
    pub dbs: Vec<String>,
    pub templates: Vec<String>,
    pub skills: Vec<String>,
}

fn looks_secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["secret", "token", "password", "passwd", "apikey", "api_key", "credential"].iter().any(|w| k.contains(w))
}

/// 递归去掉看起来是密钥的字段（技能设置是自由格式的 JSON）。
pub fn strip_secrets(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter().filter(|(k, _)| !looks_secret(k)).map(|(k, v)| (k.clone(), strip_secrets(v))).collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip_secrets).collect()),
        other => other.clone(),
    }
}

/// 去掉仓库地址里内嵌的凭据（审查 B9）：`https://user:token@host/x` → `https://host/x`。
/// http / https 的整段 userinfo 都去掉（令牌常常就写在用户名位置）；ssh 等其它协议只去掉密码、保留用户名（`ssh://git@host`）。
/// scp 写法 `git@host:path` 没有密码，原样保留。
pub fn strip_repo_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(host_end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else { return url.to_string() };
    let http = matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https");
    let keep = if http { None } else { userinfo.split(':').next().filter(|u| !u.is_empty()) };
    match keep {
        Some(user) => format!("{scheme}://{user}@{host}{tail}"),
        None => format!("{scheme}://{host}{tail}"),
    }
}

/// 读用户目录里的模板（内置模板没改过就不导出）。
pub fn read_user_templates() -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let rd = match std::fs::read_dir(templates::user_dir()) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(format!("读取模板目录失败：{e}")),
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().is_some_and(|x| x == "md")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && templates::check_name(stem).is_ok()
        {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("读取模板 {stem} 失败：{e}"))?;
            out.insert(stem.to_string(), text);
        }
    }
    Ok(out)
}

pub fn build_export(cfg: &AppConfig, user_templates: BTreeMap<String, String>) -> ExportFile {
    ExportFile {
        version: FORMAT_VERSION,
        projects: cfg
            .projects
            .iter()
            .map(|(k, p)| (k.clone(), Project { repo: strip_repo_credentials(&p.repo), ..p.clone() }))
            .collect(),
        dbs: cfg.dbs.clone(),
        templates: user_templates,
        skills: cfg.skill_settings.iter().map(|(k, v)| (k.clone(), strip_secrets(v))).collect(),
        bots: cfg
            .bots
            .iter()
            .map(|b| ExportBot {
                name: b.name.clone(),
                app_id: b.app_id.clone(),
                engine: b.engine,
                template: b.template.clone(),
                skills: b.skills.clone(),
            })
            .collect(),
    }
}

pub fn export_config_impl(path: &Path) -> Result<(), String> {
    let cfg = config::load()?;
    let file = build_export(&cfg, read_user_templates()?);
    let text = serde_json::to_string_pretty(&file).map_err(|e| format!("序列化导出文件失败：{e}"))?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    std::fs::write(path, text).map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

pub fn read_export(path: &Path) -> Result<ExportFile, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    if meta.len() > MAX_IMPORT_BYTES {
        return Err("文件太大，不像是 Beetle 的配置文件".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    let file: ExportFile = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("不是有效的配置文件：{e}"))?;
    if file.version != FORMAT_VERSION {
        return Err(format!("不支持的配置文件版本 {}（当前支持 {FORMAT_VERSION}）", file.version));
    }
    Ok(file)
}

pub fn preview_import_impl(path: &Path) -> Result<ImportPreview, String> {
    let f = read_export(path)?;
    Ok(ImportPreview {
        projects: f.projects.keys().cloned().collect(),
        dbs: f.dbs.keys().cloned().collect(),
        templates: f.templates.keys().cloned().collect(),
        skills: f.skills.keys().cloned().collect(),
        bots: f.bots.into_iter().map(|b| BotBrief { name: b.name, app_id: b.app_id }).collect(),
    })
}

/// 导入勾选的项；同名的覆盖，没勾选的不动。模板先全部校验（名称、变量），任何一项不合格就整体不导入。
pub fn apply_import_impl(path: &Path, select: &ImportSelect) -> Result<(), String> {
    let f = read_export(path)?;
    let tpls: Vec<(&String, &String)> = f.templates.iter().filter(|(k, _)| select.templates.contains(k)).collect();
    for (name, content) in &tpls {
        crate::commands_ext::validate_template(name, content).map_err(|e| format!("模板 {name}：{e}"))?;
    }
    config::update(|cfg| {
        for (k, v) in f.projects.iter().filter(|(k, _)| select.projects.contains(k)) {
            cfg.projects.insert(k.clone(), v.clone());
        }
        for (k, v) in f.dbs.iter().filter(|(k, _)| select.dbs.contains(k)) {
            if !crate::commands::valid_alias(k) {
                return Err(format!("数据源别名不合法：{k:?}"));
            }
            cfg.dbs.insert(k.clone(), v.clone());
        }
        for (k, v) in f.skills.iter().filter(|(k, _)| select.skills.contains(k)) {
            cfg.skill_settings.insert(k.clone(), strip_secrets(v));
        }
        Ok(())
    })?;
    for (name, content) in tpls {
        crate::commands_ext::save_template_impl(name, content)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::config::Bot;
    use crate::store::datasource::{Access, DbKind, DirectConn, SkillRef};
    use crate::test_support::test_home;

    fn direct(db: &str) -> DataSource {
        DataSource {
            kind: DbKind::Mysql,
            description: "示例库".into(),
            access: Access::Direct(DirectConn {
                host: "127.0.0.1".into(),
                port: 3306,
                user: "demo_reader".into(),
                database: db.into(),
                ssh: None,
                timeout_secs: 5,
                max_rows: 200,
            }),
        }
    }

    fn project() -> Project {
        Project::new("git@x:a/b.git", "release/*", "AGENTS.md", "项目", vec!["demo".into()])
    }

    fn seed() {
        config::update(|c| {
            c.owner_open_id = Some("ou_owner_secret_id".into());
            c.owner_union_id = Some("on_owner_secret_id".into());
            c.projects.insert("demo".into(), project());
            c.dbs.insert("demo".into(), direct("demo_config"));
            c.skill_settings.insert(
                "demo-skill".into(),
                serde_json::json!({"chatId": "oc_1", "status": "verified", "appSecret": "xx", "nested": {"apiKey": "k", "ok": 1}}),
            );
            c.bots.push(Bot::new_default("b1", "查 bug", "cli_a"));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn export_contains_no_secret_or_owner_and_matches_format() {
        let home = test_home();
        seed();
        std::fs::create_dir_all(templates::user_dir()).unwrap();
        std::fs::write(templates::user_dir().join("mine.md"), "我的 {{GROUP_NAME}}").unwrap();
        let out = home.path().join("export.json");
        export_config_impl(&out).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        let lower = text.to_lowercase();
        assert!(!lower.contains("secret") && !lower.contains("apikey") && !text.contains("owner_open_id"), "{text}");
        assert!(!text.contains("ou_owner") && !text.contains("on_owner") && !text.contains("owner_union_id"));
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["projects"]["demo"]["repo"], "git@x:a/b.git");
        assert_eq!(v["templates"]["mine"], "我的 {{GROUP_NAME}}");
        assert_eq!(v["skills"]["demo-skill"]["status"], "verified");
        assert_eq!(v["skills"]["demo-skill"]["nested"]["ok"], 1);
        assert!(!lower.contains("password"), "导出不带数据源密码");
        assert_eq!(v["dbs"]["demo"]["access"], "direct");
        assert_eq!(v["dbs"]["demo"]["database"], "demo_config");
        assert_eq!(v["bots"][0], serde_json::json!({"name": "查 bug", "app_id": "cli_a", "engine": "claude", "template": "bug-hunt", "skills": ["chat-context", "code-sync", "db-query", "issue-record", "feishu-doc-record"]}));
    }

    #[test]
    fn strip_repo_credentials_removes_userinfo() {
        assert_eq!(strip_repo_credentials("https://user:tok123@gitee.com/a/b.git"), "https://gitee.com/a/b.git");
        assert_eq!(strip_repo_credentials("https://ghp_token@github.com/a/b.git"), "https://github.com/a/b.git");
        assert_eq!(strip_repo_credentials("HTTP://u:p@host:8080/x?y=1"), "HTTP://host:8080/x?y=1");
        assert_eq!(strip_repo_credentials("ssh://git:pw@host/a.git"), "ssh://git@host/a.git");
        assert_eq!(strip_repo_credentials("ssh://git@host/a.git"), "ssh://git@host/a.git");
        assert_eq!(strip_repo_credentials("git@gitee.com:a/b.git"), "git@gitee.com:a/b.git");
        assert_eq!(strip_repo_credentials("https://gitee.com/a/b.git"), "https://gitee.com/a/b.git");
        // 路径里的 @ 不是 userinfo
        assert_eq!(strip_repo_credentials("https://host/a@b/c.git"), "https://host/a@b/c.git");
    }

    /// 审查 B9：导出文件里不能带仓库地址里的令牌；本机配置不受影响。
    #[test]
    fn export_strips_repo_credentials() {
        let home = test_home();
        config::update(|c| {
            c.projects.insert("p".into(), Project { repo: "https://me:SECRET_TOKEN@gitee.com/a/b.git".into(), ..project() });
            Ok(())
        })
        .unwrap();
        let out = home.path().join("e.json");
        export_config_impl(&out).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(!text.contains("SECRET_TOKEN") && !text.contains("me:"), "{text}");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["projects"]["p"]["repo"], "https://gitee.com/a/b.git");
        assert_eq!(config::load().unwrap().projects["p"].repo, "https://me:SECRET_TOKEN@gitee.com/a/b.git");
    }

    #[test]
    fn preview_lists_names_and_bots() {
        let home = test_home();
        seed();
        let out = home.path().join("e.json");
        export_config_impl(&out).unwrap();
        let p = preview_import_impl(&out).unwrap();
        assert_eq!((p.projects, p.dbs, p.skills), (vec!["demo".to_string()], vec!["demo".to_string()], vec!["demo-skill".to_string()]));
        assert_eq!(p.bots, vec![BotBrief { name: "查 bug".into(), app_id: "cli_a".into() }]);
    }

    #[test]
    fn apply_import_only_touches_selected_items_and_keeps_others() {
        let home = test_home();
        let file = home.path().join("in.json");
        let mut p2 = project();
        p2.description = "新的".into();
        let payload = ExportFile {
            version: 1,
            projects: BTreeMap::from([("demo".to_string(), p2), ("other".to_string(), project())]),
            dbs: BTreeMap::from([("d1".to_string(), direct("d1"))]),
            templates: BTreeMap::from([("t1".to_string(), "模板 {{CHAT_ID}}".to_string())]),
            skills: BTreeMap::from([("demo-skill".to_string(), serde_json::json!({"status": "verified", "token": "no"}))]),
            bots: vec![],
        };
        std::fs::write(&file, serde_json::to_string(&payload).unwrap()).unwrap();
        config::update(|c| {
            c.projects.insert("demo".into(), project());
            c.proxy = Some("http://p".into());
            Ok(())
        })
        .unwrap();
        let sel = ImportSelect { projects: vec!["demo".into()], templates: vec!["t1".into()], skills: vec!["demo-skill".into()], dbs: vec![] };
        apply_import_impl(&file, &sel).unwrap();
        let cfg = config::load().unwrap();
        assert_eq!(cfg.projects["demo"].description, "新的");
        assert!(!cfg.projects.contains_key("other"), "没勾选的不导入");
        assert!(cfg.dbs.is_empty());
        assert_eq!(cfg.skill_settings["demo-skill"], serde_json::json!({"status": "verified"}));
        assert_eq!(cfg.proxy.as_deref(), Some("http://p"));
        assert_eq!(templates::load("t1").unwrap(), "模板 {{CHAT_ID}}");
    }

    /// 旧版导出文件的库 `{kind, database}`：导入后是「用 sql-window 技能查」。
    #[test]
    fn import_old_db_entry_becomes_skill_access() {
        let home = test_home();
        let f = home.path().join("old.json");
        std::fs::write(&f, r#"{"version":1,"dbs":{"demo_a":{"kind":"mysql","database":"demo_a"}}}"#).unwrap();
        apply_import_impl(&f, &ImportSelect { dbs: vec!["demo_a".into()], ..Default::default() }).unwrap();
        let ds = &config::load().unwrap().dbs["demo_a"];
        assert_eq!(
            ds.access,
            Access::Skill(SkillRef { skill: "sql-window".into(), database: "demo_a".into(), note: String::new() })
        );
    }

    #[test]
    fn bad_template_aborts_everything() {
        let home = test_home();
        let file = home.path().join("in.json");
        let payload = ExportFile {
            version: 1,
            projects: BTreeMap::from([("p".to_string(), project())]),
            dbs: BTreeMap::new(),
            templates: BTreeMap::from([("bad".to_string(), "{{UNKNOWN}}".to_string())]),
            skills: BTreeMap::new(),
            bots: vec![],
        };
        std::fs::write(&file, serde_json::to_string(&payload).unwrap()).unwrap();
        let sel = ImportSelect { projects: vec!["p".into()], templates: vec!["bad".into()], ..Default::default() };
        let err = apply_import_impl(&file, &sel).unwrap_err();
        assert!(err.contains("bad") && err.contains("UNKNOWN"), "{err}");
        assert!(config::load().unwrap().projects.is_empty(), "校验失败时配置不变");
    }

    #[test]
    fn rejects_wrong_version_garbage_and_traversal_names() {
        let home = test_home();
        let f = home.path().join("x.json");
        std::fs::write(&f, r#"{"version": 2}"#).unwrap();
        assert!(read_export(&f).unwrap_err().contains("不支持"));
        std::fs::write(&f, "not json").unwrap();
        assert!(read_export(&f).unwrap_err().contains("不是有效"));
        std::fs::write(&f, r#"{"version":1,"templates":{"../evil":"x"}}"#).unwrap();
        let sel = ImportSelect { templates: vec!["../evil".into()], ..Default::default() };
        assert!(apply_import_impl(&f, &sel).is_err());
        assert!(read_export(&home.path().join("missing.json")).is_err());
    }
}
