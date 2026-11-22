//! 群工作目录准备：每次运行前把模板、技能、禁止规则和运行时配置写进
//! `workspaces_dir()/<bot.id>/<group.slug>/`。
//!
//! Step 0 核实结论（codex-cli 0.167.0，2026-11-19，本机）：
//! - AGENTS.md：Codex 的标准项目说明文件，工作目录下的 `AGENTS.md` 会被读入（官方约定；本机未跑模型实测）。
//! - 项目级 skill 目录：`<项目>/.agents/skills/<name>/SKILL.md`（`~/.agents/skills` 是 Codex / Copilot / Gemini 共用的
//!   跨运行时别名，本机技能文档多处记载）。**未实测 0.167 是否扫描工作目录下的该路径**，所以同时也写 `.claude/skills/`；
//!   Codex 读 AGENTS.md 里的绝对脚本路径同样能跑，技能目录没被扫描也不影响功能。【项目级 skills 目录：未核实】
//! - 禁止规则：`codex execpolicy check --rules <文件> <命令...>` 实测可用，规则语法为
//!   `prefix_rule(pattern=["git","push"], decision="forbidden", justification="...")`，匹配即 `forbidden`。
//!   本模块把 `BUILTIN_DENY` 里的 Bash 规则同源转成 `.codex/rules/beetle.rules`。
//!   【工作目录下的 `.codex/rules` 是否被自动加载：未核实】——Task 9 拉起 Codex 时应显式保证（例如 `-c` 或
//!   用户级 `~/.codex/rules`），并保留 AGENTS.md 里的文字禁令作兜底。
//! - Claude Code：`.claude/settings.json` 的 `permissions.deny` + `additionalDirectories`，规则写法见 `skills::BUILTIN_DENY`。

use crate::platform::paths;
use crate::skills;
use crate::store::config::{AppConfig, Bot, Group};
use crate::templates::{self, TemplateVars};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Interpreters {
    pub python: PathBuf,
    pub node: PathBuf,
}

/// 本次触发的消息信息，写进运行时配置给 p2p_context 等脚本校验用。
#[derive(Debug, Clone)]
pub struct TriggerInfo {
    pub message_id: String,
    /// 机器人应用命名空间的 open_id，和 owner_open_id（lark-cli 命名空间）不可比，只作记录。
    pub sender_open_id: String,
    /// 客户端按 union_id 判定的「触发人是机器人主人」。
    pub is_owner: bool,
}

#[derive(Debug, Clone)]
pub struct PreparedWorkspace {
    pub dir: PathBuf,
    pub runtime_json: PathBuf,
}

/// 写进文件的路径一律用正斜杠（Python / Node / Agent 都认，避免转义问题）。
fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// 技能文档里的 `{{BEETLE_EXE}}`：客户端自己的可执行文件路径（正斜杠），Agent 用 `<它> --db-query <别名>` 直连查库。
/// 取不到时退回 `beetle`（指望在 PATH 里）。
pub(crate) fn beetle_exe() -> String {
    std::env::current_exe().map(|p| fwd(&p)).unwrap_or_else(|_| "beetle".to_string())
}

/// 运行时配置里的 `dbs`：按别名展开成数组（`DataSource::runtime_value`，不含密码、主机、账号），别名去重、保持首次出现的顺序。
pub(crate) fn runtime_dbs<'a>(cfg: &AppConfig, aliases: impl IntoIterator<Item = &'a String>) -> Vec<Value> {
    let mut seen: Vec<&String> = Vec::new();
    let mut out = Vec::new();
    for alias in aliases {
        if seen.contains(&alias) {
            continue;
        }
        seen.push(alias);
        if let Some(ds) = cfg.dbs.get(alias) {
            out.push(ds.runtime_value(alias));
        }
    }
    out
}

fn check_segment(what: &str, v: &str) -> Result<(), String> {
    if v.is_empty() || v == "." || v == ".." || v.contains(['/', '\\', ':']) {
        return Err(format!("{what}不合法：{v:?}"));
    }
    Ok(())
}

fn io<T>(what: &str, r: std::io::Result<T>) -> Result<T, String> {
    r.map_err(|e| format!("{what}失败：{e}"))
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        io(&format!("创建目录 {}", dir.display()), std::fs::create_dir_all(dir))?;
    }
    io(&format!("写入 {}", path.display()), std::fs::write(path, text))
}

pub(crate) fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    io(&format!("创建目录 {}", dst.display()), std::fs::create_dir_all(dst))?;
    for entry in io(&format!("读取目录 {}", src.display()), std::fs::read_dir(src))? {
        let entry = io("读取目录项", entry)?;
        let name = entry.file_name();
        if name == "__pycache__" {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(&name));
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            io(&format!("复制 {}", from.display()), std::fs::copy(&from, &to))?;
        }
    }
    Ok(())
}

/// 内置禁止清单并上机器人追加项，保持顺序、去重。
fn deny_rules(bot: &Bot) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in skills::BUILTIN_DENY.iter().copied().chain(bot.extra_deny.iter().map(String::as_str)) {
        if !out.iter().any(|x| x == r) {
            out.push(r.to_string());
        }
    }
    out
}

/// 转成 Claude Code 权限规则里的绝对路径写法（2026-11-19 本机实测，claude 2.1.327）：
/// `C:/x/y` → `//c/x/y`（盘符小写、去冒号）有效；`//C:/x/y` 无效（实测未拦截）；POSIX `/a/b` → `//a/b`。
fn claude_abs_path(p: &str) -> String {
    let b = p.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/' {
        format!("//{}{}", (b[0] as char).to_ascii_lowercase(), &p[2..])
    } else {
        format!("/{p}")
    }
}

/// 路径限定的写保护：agent 不能改自己的规则 / 说明 / 运行时配置，也不能改代码缓存。
/// 语法（实测）：`/xxx` 相对项目根（工作目录），`//c/...` 为绝对路径。只有 `Edit(...)` 规则有效，
/// Claude Code 会提示 `Write(path)` 规则不被文件权限检查匹配，Edit 规则已覆盖所有文件编辑工具。
fn path_rules(code_cache: &Path) -> Vec<String> {
    let cache = format!("{}/**", claude_abs_path(&fwd(code_cache)));
    [
        "/.claude/**",
        "/.codex/**",
        "/.agents/**",
        "/CLAUDE.md",
        "/AGENTS.md",
        "/beetle.runtime.json",
    ]
    .iter()
    .map(|t| format!("Edit({t})"))
    .chain(std::iter::once(format!("Edit({cache})")))
    .collect()
}

/// `Bash(git push:*)` →`["git", "push"]`。只转换 Bash 的前缀规则；带通配符或引号的跳过。
fn bash_prefix(rule: &str) -> Option<Vec<&str>> {
    let inner = rule.strip_prefix("Bash(")?.strip_suffix(":*)")?;
    let tokens: Vec<&str> = inner.split_whitespace().collect();
    if tokens.is_empty() || tokens.iter().any(|t| t.contains(['*', '"', '\\'])) {
        return None;
    }
    Some(tokens)
}

/// Codex execpolicy 规则文件内容（语法已用 `codex execpolicy check` 核对）。
fn codex_rules(deny: &[String]) -> String {
    let mut out = String::from("# Beetle 自动生成，每次运行前覆盖，请勿手改。\n");
    for rule in deny {
        if let Some(tokens) = bash_prefix(rule) {
            let pattern = tokens.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(", ");
            out.push_str(&format!(
                "prefix_rule(pattern=[{pattern}], decision=\"forbidden\", justification=\"Beetle 禁止清单\")\n"
            ));
        }
    }
    out
}

/// 追加到 CLAUDE.md / AGENTS.md 末尾的文字禁令（Codex 规则文件未必被加载时的兜底）。
fn deny_section(deny: &[String]) -> String {
    let cmds: Vec<String> = deny
        .iter()
        .filter_map(|r| bash_prefix(r))
        .map(|t| format!("`{}`", t.join(" ")))
        .collect();
    format!(
        "\n\n## 客户端禁止清单\n\n以下命令被客户端禁止，不要执行（会被拦截或视为违规）：{}。\n删除文件（rm、Remove-Item、del 等）也被禁止。群里发消息、回复只由客户端做；代码目录只读。\n",
        cmds.join("、")
    )
}

/// 运行时配置：技能脚本通过环境变量 `BEETLE_RUNTIME`（兼容旧名 `BUG_PATROL_RUNTIME`）读取。字段见 `_shared/bp_common.py`。
/// `projects` / `dbs` 按本次可查的项目（`Group::scope`）生成：本群常用项目在前，开了「全部项目」时含全部项目；
/// `dbs` 是这些项目关联的数据源（数组，见 `runtime_dbs`），每个项目的 `dbs` 也展开成同样的对象数组。
/// `preferred_projects` 是本群常用项目，`all_projects` 表示由 AI 按问题自行定位项目。
fn runtime_value(cfg: &AppConfig, group: &Group, trigger: &TriggerInfo) -> Value {
    let mut projects = serde_json::Map::new();
    let mut aliases: Vec<&String> = Vec::new();
    let mut preferred: Vec<&String> = Vec::new();
    for (name, p, is_preferred) in group.scope(&cfg.projects) {
        projects.insert(name.clone(), p.runtime_value(&cfg.dbs));
        aliases.extend(p.dbs.iter());
        if is_preferred {
            preferred.push(name);
        }
    }
    let dbs = runtime_dbs(cfg, aliases);
    json!({
        "chat_id": group.chat_id,
        "group_name": group.name,
        "doc_url": group.doc_url,
        "owner_open_id": cfg.owner_open_id,
        "code_root": fwd(&cfg.code_cache_dir()),
        "review_state_dir": fwd(&crate::review_card::state_root()),
        "projects": projects,
        "preferred_projects": preferred,
        "all_projects": group.all_projects,
        "dbs": dbs,
        "trigger_message_id": trigger.message_id,
        "trigger_sender_open_id": trigger.sender_open_id,
        "trigger_is_owner": trigger.is_owner,
    })
}

/// 把启用的技能和 `_shared` 复制到 `<root>`，SKILL.md 里的占位符渲染掉。先清空 `<root>`，所以停用的技能会被删掉。
fn install_skills(root: &Path, enabled: &[String], interp: &Interpreters) -> Result<(), String> {
    if root.exists() {
        io(&format!("清理 {}", root.display()), std::fs::remove_dir_all(root))?;
    }
    io(&format!("创建 {}", root.display()), std::fs::create_dir_all(root))?;
    let src_root = skills::builtin_dir();
    copy_dir(&src_root.join("_shared"), &root.join("_shared"))?;
    for name in enabled {
        let dst = root.join(name);
        copy_dir(&skills::skill_dir(name).ok_or_else(|| format!("找不到技能：{name}"))?, &dst)?;
        let md_path = dst.join("SKILL.md");
        let md = io(&format!("读取 {}", md_path.display()), std::fs::read_to_string(&md_path))?;
        let mut vars = TemplateVars::default();
        vars.0.insert("SKILL_DIR", fwd(&dst));
        vars.0.insert("PYTHON", fwd(&interp.python));
        vars.0.insert("NODE", fwd(&interp.node));
        vars.0.insert("BEETLE_EXE", beetle_exe());
        let md = templates::render(&md, &vars).map_err(|e| format!("技能 {name} 的 SKILL.md：{e}"))?;
        write_file(&md_path, &md)?;
    }
    Ok(())
}

pub fn prepare(
    cfg: &AppConfig,
    bot: &Bot,
    group: &Group,
    interp: &Interpreters,
    trigger: &TriggerInfo,
) -> Result<PreparedWorkspace, String> {
    check_segment("机器人 id", &bot.id)?;
    check_segment("群标识 slug", &group.slug)?;
    let available = skills::list_all();
    for name in &bot.skills {
        if !available.iter().any(|s| &s.name == name) {
            return Err(format!("找不到内置技能：{name}"));
        }
    }

    let dir = paths::workspaces_dir().join(&bot.id).join(&group.slug);
    io("创建工作目录", std::fs::create_dir_all(dir.join("reports")))?;

    let claude_skills = dir.join(".claude").join("skills");
    install_skills(&claude_skills, &bot.skills, interp)?;
    install_skills(&dir.join(".agents").join("skills"), &bot.skills, interp)?;

    let deny = deny_rules(bot);
    let mut vars = TemplateVars::default();
    vars.0.insert("GROUP_NAME", group.name.clone());
    vars.0.insert("CHAT_ID", group.chat_id.clone());
    vars.0.insert("PROJECTS_BLOCK", templates::projects_block(group, &cfg.projects));
    vars.0.insert("DOC_URL", group.doc_url.clone().unwrap_or_else(|| "未配置".to_string()));
    vars.0.insert("SKILLS_DIR", fwd(&claude_skills));
    vars.0.insert("PYTHON", fwd(&interp.python));
    vars.0.insert("NODE", fwd(&interp.node));
    vars.0.insert("BEETLE_EXE", beetle_exe());
    let mut text = templates::render(&templates::load(&bot.template)?, &vars)?;
    text.push_str(&deny_section(&deny));
    write_file(&dir.join("CLAUDE.md"), &text)?;
    write_file(&dir.join("AGENTS.md"), &text)?;

    let mut claude_deny = deny.clone();
    claude_deny.extend(path_rules(&cfg.code_cache_dir()));
    let settings = json!({
        "permissions": {
            "deny": claude_deny,
            "additionalDirectories": [fwd(&cfg.code_cache_dir())],
        }
    });
    write_file(
        &dir.join(".claude").join("settings.json"),
        &serde_json::to_string_pretty(&settings).map_err(|e| format!("序列化 settings.json 失败：{e}"))?,
    )?;
    write_file(&dir.join(".codex").join("rules").join("beetle.rules"), &codex_rules(&deny))?;
    // 改名前生成的同名文件留在旧工作区里：删掉，免得 Codex 同时加载两份规则、Agent 读到过期配置
    let _ = std::fs::remove_file(dir.join(".codex").join("rules").join("bug-patrol.rules"));
    let _ = std::fs::remove_file(dir.join("bug-patrol.runtime.json"));

    let runtime_json = dir.join("beetle.runtime.json");
    write_file(
        &runtime_json,
        &serde_json::to_string_pretty(&runtime_value(cfg, group, trigger))
            .map_err(|e| format!("序列化运行时配置失败：{e}"))?,
    )?;
    Ok(PreparedWorkspace { dir, runtime_json })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::BUILTIN_DENY;
    use crate::store::config::{Project, Trigger};
    use crate::store::datasource::{Access, DataSource, DbKind, DirectConn, SkillRef};
    use crate::test_support::test_home;

    fn fixture() -> (AppConfig, Bot, Group, Interpreters, TriggerInfo) {
        let mut cfg = AppConfig::default();
        cfg.owner_open_id = Some("ou_owner".into());
        cfg.projects.insert(
            "demo".into(),
            Project::new("git@gitee.com:a/demo.git", "release/*", "AGENTS.md", "示例服务", vec!["demo_config".into()]),
        );
        cfg.projects.insert(
            "other".into(),
            Project::new("git@x:a/other.git", "release/*", "AGENTS.md", "不相关项目", vec!["other_db".into()]),
        );
        cfg.dbs.insert("demo_config".into(), direct_ds("demo_config"));
        cfg.dbs.insert(
            "other_db".into(),
            DataSource {
                kind: DbKind::Mysql,
                description: "不相关的库".into(),
                access: Access::Skill(SkillRef { skill: "my-sql-skill".into(), database: "other_db".into(), note: String::new() }),
            },
        );
        let group = Group {
            chat_id: "oc_abc".into(),
            name: "测试群".into(),
            slug: "test-group".into(),
            projects: vec!["demo".into()],
            doc_url: Some("https://x.feishu.cn/docx/abc".into()),
            trigger: Trigger::Owner,
            all_projects: false,
        };
        let bot = Bot::new_default("b1", "机器人", "cli_x");
        let interp = Interpreters { python: PathBuf::from("C:\\Py\\python.exe"), node: PathBuf::from("node") };
        let trig = TriggerInfo { message_id: "om_1".into(), sender_open_id: "ou_bot_ns".into(), is_owner: true };
        (cfg, bot, group, interp, trig)
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    /// 直连数据源（主机、账号只在配置里，运行时配置里不能出现）。
    fn direct_ds(db: &str) -> DataSource {
        DataSource {
            kind: DbKind::Mysql,
            description: "示例配置库".into(),
            access: Access::Direct(DirectConn {
                host: "db-host.example".into(),
                port: 3306,
                user: "demo_reader".into(),
                database: db.into(),
                ssh: None,
                timeout_secs: 5,
                max_rows: 200,
            }),
        }
    }

    #[test]
    fn prepare_writes_both_instruction_files_identical() {
        let _h = test_home();
        let (cfg, bot, group, interp, trig) = fixture();
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        assert_eq!(ws.dir, paths::workspaces_dir().join("b1").join("test-group"));
        let claude = read(&ws.dir.join("CLAUDE.md"));
        assert_eq!(claude, read(&ws.dir.join("AGENTS.md")));
        assert!(claude.contains("测试群") && claude.contains("oc_abc"));
        assert!(claude.contains("https://x.feishu.cn/docx/abc"));
        assert!(!claude.contains("{{"));
        assert!(!claude.contains('\\'), "模板里的路径应全是正斜杠");
        assert!(claude.contains("C:/Py/python.exe"));
        assert!(ws.dir.join("reports").is_dir());
    }

    #[test]
    fn missing_doc_url_renders_placeholder_and_null() {
        let _h = test_home();
        let (cfg, bot, mut group, interp, trig) = fixture();
        group.doc_url = None;
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        assert!(read(&ws.dir.join("CLAUDE.md")).contains("未配置"));
        let rt: Value = serde_json::from_str(&read(&ws.runtime_json)).unwrap();
        assert!(rt["doc_url"].is_null());
    }

    #[test]
    fn prepare_copies_only_enabled_skills_and_renders_placeholders() {
        let _h = test_home();
        let (cfg, mut bot, group, interp, trig) = fixture();
        bot.skills = vec!["code-sync".into()];
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        for root in [".claude/skills", ".agents/skills"] {
            let base = ws.dir.join(root);
            let md = read(&base.join("code-sync/SKILL.md"));
            assert!(!md.contains("{{"), "{root}");
            assert!(md.contains("C:/Py/python.exe"), "{root}");
            assert!(md.contains(&format!("{}/code-sync/scripts/sync_code.py", base.to_string_lossy().replace('\\', "/"))), "{root}");
            assert!(base.join("code-sync/scripts/sync_code.py").is_file());
            assert!(!base.join("chat-context").exists(), "{root}");
            assert!(base.join("_shared/bp_common.py").is_file(), "{root}");
            assert!(!base.join("_tests").exists(), "{root}");
        }
    }

    #[test]
    fn unknown_skill_is_an_error() {
        let _h = test_home();
        let (cfg, mut bot, group, interp, trig) = fixture();
        bot.skills = vec!["../evil".into()];
        assert!(prepare(&cfg, &bot, &group, &interp, &trig).is_err());
        bot.skills = vec!["nope".into()];
        assert!(prepare(&cfg, &bot, &group, &interp, &trig).unwrap_err().contains("nope"));
    }

    #[test]
    fn deny_rules_include_all_builtin_plus_extra() {
        let _h = test_home();
        let (cfg, mut bot, group, interp, trig) = fixture();
        bot.extra_deny = vec!["Bash(curl:*)".into(), "Bash(git push:*)".into()];
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        let s: Value = serde_json::from_str(&read(&ws.dir.join(".claude/settings.json"))).unwrap();
        let deny: Vec<&str> = s["permissions"]["deny"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        for r in BUILTIN_DENY {
            assert!(deny.contains(r), "缺少 {r}");
        }
        assert!(deny.contains(&"Bash(curl:*)"));
        assert_eq!(deny.len(), BUILTIN_DENY.len() + 1 + path_rules(&paths::code_cache_dir()).len(), "重复的追加项应去重");
        let cache = paths::code_cache_dir().to_string_lossy().replace('\\', "/");
        for t in ["/.claude/**", "/.codex/**", "/.agents/**", "/CLAUDE.md", "/AGENTS.md", "/beetle.runtime.json"] {
            let r = format!("Edit({t})");
            assert!(deny.contains(&r.as_str()), "缺少 {r}");
        }
        let cache_rule = format!("Edit({}/**)", claude_abs_path(&cache));
        assert!(deny.contains(&cache_rule.as_str()), "缺少 {cache_rule}");
        assert!(!cache_rule.contains('\\'));
        assert!(deny.iter().all(|r| !r.starts_with("Write(")), "Write(path) 规则无效，不应出现");
        let dirs = s["permissions"]["additionalDirectories"].as_array().unwrap();
        assert!(dirs.iter().any(|d| d.as_str().unwrap() == paths::code_cache_dir().to_string_lossy().replace('\\', "/")));
    }

    #[test]
    fn claude_abs_path_converts_drive_and_posix_paths() {
        assert_eq!(claude_abs_path("C:/Users/A/code"), "//c/Users/A/code");
        assert_eq!(claude_abs_path("d:/x"), "//d/x");
        assert_eq!(claude_abs_path("/home/a/code"), "//home/a/code");
    }

    #[test]
    fn codex_rules_file_forbids_builtin_bash_prefixes() {
        let _h = test_home();
        let (cfg, mut bot, group, interp, trig) = fixture();
        bot.extra_deny = vec!["Bash(curl:*)".into()];
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        let rules = read(&ws.dir.join(".codex/rules/beetle.rules"));
        assert!(rules.contains(r#"pattern=["git", "push"]"#));
        assert!(rules.contains(r#"pattern=["lark-cli", "im"]"#));
        assert!(rules.contains(r#"pattern=["mysql"]"#));
        assert!(rules.contains(r#"pattern=["curl"]"#));
        assert!(rules.contains(r#"decision="forbidden""#));
        assert!(!rules.contains("PowerShell"));
        // AGENTS.md 兜底：文字禁令
        assert!(read(&ws.dir.join("AGENTS.md")).contains("git push"));
    }

    #[test]
    fn runtime_json_contains_group_projects_and_no_secrets() {
        let _h = test_home();
        let (cfg, bot, group, interp, trig) = fixture();
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        let text = read(&ws.runtime_json);
        assert!(!text.to_lowercase().contains("secret"));
        assert!(!text.contains('\\'), "运行时配置里的路径应全是正斜杠");
        let rt: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(rt["chat_id"], "oc_abc");
        assert_eq!(rt["group_name"], "测试群");
        assert_eq!(rt["doc_url"], "https://x.feishu.cn/docx/abc");
        assert_eq!(rt["owner_open_id"], "ou_owner");
        assert_eq!(rt["trigger_message_id"], "om_1");
        assert_eq!(rt["trigger_sender_open_id"], "ou_bot_ns");
        assert_eq!(rt["trigger_is_owner"], true);
        assert_eq!(rt["code_root"], paths::code_cache_dir().to_string_lossy().replace('\\', "/"));
        assert!(rt.get("requests_dir").is_none() && rt.get("sql_window").is_none(), "sql-window 相关字段已移除");
        assert_eq!(rt["review_state_dir"], paths::data_dir().join("pr-reviews").to_string_lossy().replace('\\', "/"));
        let projects = rt["projects"].as_object().unwrap();
        assert_eq!(projects.keys().collect::<Vec<_>>(), ["demo"]);
        assert_eq!(projects["demo"]["repo"], "git@gitee.com:a/demo.git");
        // 记录方式：gitee 仓库默认建 issue + 写飞书文档；issue 仓库与 token 变量名未改时为 null（脚本按默认推断）
        assert_eq!(projects["demo"]["record"], json!({"issue": true, "feishu_doc": true, "meegle": false, "feishu_task": false}));
        assert!(projects["demo"]["issue_repo"].is_null() && projects["demo"]["token_env"].is_null());
        assert_eq!(projects["demo"]["rules_file"], "AGENTS.md");
        let dbs = rt["dbs"].as_array().unwrap();
        assert_eq!(dbs.len(), 1, "只含本群项目关联的数据源");
        assert_eq!(dbs[0]["alias"], "demo_config");
        assert_eq!(dbs[0]["kind"], "mysql");
        assert_eq!(dbs[0]["access"], "direct");
        assert_eq!(dbs[0]["database"], "demo_config");
        assert_eq!(projects["demo"]["dbs"], json!([dbs[0].clone()]), "项目的 dbs 展开成同样的对象");
        // 不泄露密码和连接信息
        assert!(!text.contains("\"password\""));
        assert!(!text.contains("\"host\""));
        assert!(!text.contains("db-host.example") && !text.contains("demo_reader"));
    }

    /// 技能方式的数据源：运行时写明用哪个技能查、库名和补充说明；一个库被多个项目关联时只出现一次。
    #[test]
    fn runtime_dbs_skill_access_and_dedup() {
        let _h = test_home();
        let (mut cfg, bot, mut group, interp, trig) = fixture();
        cfg.projects.get_mut("other").unwrap().dbs = vec!["other_db".into(), "demo_config".into()];
        group.projects = vec!["demo".into(), "other".into()];
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        let rt: Value = serde_json::from_str(&read(&ws.runtime_json)).unwrap();
        let dbs = rt["dbs"].as_array().unwrap();
        let aliases: Vec<&str> = dbs.iter().map(|d| d["alias"].as_str().unwrap()).collect();
        assert_eq!(aliases, ["demo_config", "other_db"]);
        assert_eq!(dbs[1]["access"], "skill");
        assert_eq!(dbs[1]["skill"], "my-sql-skill");
        assert_eq!(dbs[1]["database"], "other_db");
    }

    /// 开了「全部项目」：运行时含全部项目和它们的库，常用项目列在 preferred_projects；CLAUDE.md 带定位说明。
    #[test]
    fn all_projects_scope_in_runtime_and_rules() {
        let _h = test_home();
        let (cfg, bot, mut group, interp, trig) = fixture();
        let rt_of = |g: &Group| -> Value {
            let ws = prepare(&cfg, &bot, g, &interp, &trig).unwrap();
            serde_json::from_str(&read(&ws.runtime_json)).unwrap()
        };
        let rt = rt_of(&group);
        assert_eq!(rt["projects"].as_object().unwrap().keys().collect::<Vec<_>>(), ["demo"]);
        assert_eq!((rt["all_projects"].clone(), rt["preferred_projects"].clone()), (json!(false), json!(["demo"])));

        group.all_projects = true;
        let rt = rt_of(&group);
        assert_eq!(rt["projects"].as_object().unwrap().keys().collect::<Vec<_>>(), ["demo", "other"]);
        let aliases: Vec<&str> = rt["dbs"].as_array().unwrap().iter().map(|d| d["alias"].as_str().unwrap()).collect();
        assert_eq!(aliases, ["demo_config", "other_db"]);
        assert_eq!((rt["all_projects"].clone(), rt["preferred_projects"].clone()), (json!(true), json!(["demo"])));
        let dir = paths::workspaces_dir().join(&bot.id).join(&group.slug);
        let md = read(&dir.join("CLAUDE.md"));
        assert!(md.contains(crate::templates::AUTO_LOCATE_HINT) && md.contains("`demo`【本群常用】") && md.contains("`other`："));
    }

    /// 技能文档里的 `{{BEETLE_EXE}}` 渲染成客户端可执行文件的绝对路径（正斜杠）。
    #[test]
    fn beetle_exe_placeholder_is_forward_slash_path() {
        let exe = beetle_exe();
        assert!(!exe.contains('\\'));
        assert_eq!(exe, std::env::current_exe().unwrap().to_string_lossy().replace('\\', "/"));
        let mut vars = TemplateVars::default();
        vars.0.insert("BEETLE_EXE", exe.clone());
        assert_eq!(templates::render("{{BEETLE_EXE}} --db-query demo", &vars).unwrap(), format!("{exe} --db-query demo"));
    }

    /// 设置页改了代码缓存目录：运行时 code_root、写保护规则、additionalDirectories 都跟着改。
    #[test]
    fn custom_code_cache_dir_is_used_everywhere() {
        let h = test_home();
        let (mut cfg, bot, group, interp, trig) = fixture();
        let custom = h.path().join("my-cache");
        cfg.code_cache_dir = Some(custom.to_string_lossy().into_owned());
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        let want = custom.to_string_lossy().replace('\\', "/");
        let rt: Value = serde_json::from_str(&read(&ws.runtime_json)).unwrap();
        assert_eq!(rt["code_root"], want.as_str());
        let s: Value = serde_json::from_str(&read(&ws.dir.join(".claude/settings.json"))).unwrap();
        assert_eq!(s["permissions"]["additionalDirectories"][0], want.as_str());
        let rule = format!("Edit({}/**)", claude_abs_path(&want));
        assert!(s["permissions"]["deny"].as_array().unwrap().iter().any(|r| r == rule.as_str()), "缺少 {rule}");
    }

    /// 记录方式多选：项目里改过的 record / issue_repo / token_env 原样进运行时配置；默认技能带上 issue-record。
    #[test]
    fn runtime_json_contains_record_config_and_issue_skill() {
        let _h = test_home();
        let (mut cfg, bot, group, interp, trig) = fixture();
        let p = cfg.projects.get_mut("demo").unwrap();
        p.record = crate::store::config::RecordOpts { issue: true, feishu_doc: false, meegle: false, feishu_task: false };
        p.issue_repo = Some("team/issues".into());
        p.token_env = Some("MY_GITEE_TOKEN".into());
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        let rt: Value = serde_json::from_str(&read(&ws.runtime_json)).unwrap();
        let demo = &rt["projects"]["demo"];
        assert_eq!(demo["record"], json!({"issue": true, "feishu_doc": false, "meegle": false, "feishu_task": false}));
        assert!(demo["meegle"].is_null() && demo["feishu_task"].is_null() && demo["record_rule"].is_null());
        assert_eq!(demo["issue_repo"], "team/issues");
        assert_eq!(demo["token_env"], "MY_GITEE_TOKEN");
        for root in [".claude/skills", ".agents/skills"] {
            let base = ws.dir.join(root);
            assert!(base.join("issue-record/scripts/create_issue.py").is_file(), "{root}");
            for f in ["issue-record/scripts/create_work_item.py", "issue-record/scripts/create_task.py", "_shared/bp_meegle.py", "_shared/bp_task.py"] {
                assert!(base.join(f).is_file(), "{root} 缺少 {f}");
            }
            assert!(base.join("_shared/bp_issue.py").is_file(), "{root}");
            assert!(!read(&base.join("issue-record/SKILL.md")).contains("{{"), "{root}");
        }
        let md = read(&ws.dir.join("CLAUDE.md"));
        assert!(md.contains("issue-record/scripts/create_issue.py --project"), "模板第 5 步要调建 issue 脚本");
        assert!(md.contains("记录方式：建 issue（Gitee · team/issues）"), "项目清单里写明记录方式");
    }

    /// 旧配置里留着的 sql-window 设置不再写进运行时配置。
    #[test]
    fn runtime_ignores_legacy_sql_window_settings() {
        let _h = test_home();
        let (mut cfg, bot, group, interp, trig) = fixture();
        cfg.skill_settings.insert("sql-window".into(), json!({"chatId": "oc_demo", "status": "verified"}));
        cfg.owner_open_id = None;
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        let text = read(&ws.runtime_json);
        let rt: Value = serde_json::from_str(&text).unwrap();
        assert!(rt.get("sql_window").is_none() && !text.contains("oc_demo"));
        assert!(rt["owner_open_id"].is_null());
    }

    #[test]
    fn prepare_is_idempotent_and_removes_disabled_skills() {
        let _h = test_home();
        let (cfg, mut bot, group, interp, trig) = fixture();
        let ws1 = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        assert!(ws1.dir.join(".claude/skills/chat-context/SKILL.md").is_file());
        let first = read(&ws1.dir.join("CLAUDE.md"));
        bot.skills.retain(|s| s != "chat-context");
        let ws2 = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        assert_eq!(ws1.dir, ws2.dir);
        assert_eq!(first, read(&ws2.dir.join("CLAUDE.md")));
        for root in [".claude/skills", ".agents/skills"] {
            assert!(!ws2.dir.join(root).join("chat-context").exists(), "{root}");
            assert!(ws2.dir.join(root).join("code-sync/SKILL.md").is_file(), "{root}");
        }
    }

    #[test]
    fn reports_dir_keeps_existing_files() {
        let _h = test_home();
        let (cfg, bot, group, interp, trig) = fixture();
        let ws = prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        std::fs::write(ws.dir.join("reports/a.md"), "keep").unwrap();
        prepare(&cfg, &bot, &group, &interp, &trig).unwrap();
        assert_eq!(read(&ws.dir.join("reports/a.md")), "keep");
    }

    #[test]
    fn unsafe_group_slug_is_rejected() {
        let _h = test_home();
        let (cfg, bot, mut group, interp, trig) = fixture();
        group.slug = "../x".into();
        assert!(prepare(&cfg, &bot, &group, &interp, &trig).is_err());
    }
}
