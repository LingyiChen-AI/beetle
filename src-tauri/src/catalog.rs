//! 技能中心：本产品自己做的、可安装到本机 Claude Code / Codex 的技能。
//!
//! 技能随客户端打包在 `resources/catalog/<name>/`，每个技能有 `catalog.json`（名称、显示名、说明、版本、依赖、配置类型）、
//! Claude / Codex 通用的 `SKILL.md` 以及脚本。以 `_` / `.` 开头的目录不是技能。以后加技能只需往 catalog 里加目录。
//!
//! 安装：复制到 `~/.claude/skills/<name>` 或 `$CODEX_HOME/skills/<name>`（默认 `~/.codex/skills/<name>`），
//! 渲染 SKILL.md 里的 `{{SKILL_DIR}}` `{{PYTHON}}` `{{NODE}}`，`config == "beetle"` 的技能再写一份
//! `beetle.runtime.json`（来自客户端配置），最后写标记文件 `.beetle-installed.json`。
//! 只覆盖 / 删除带这个标记的目录，用户自己放的同名技能一律不动。
//! 改名前（BUG纠察队）装的技能带的是 `.bug-patrol-installed.json` / source `bug-patrol`，同样认作本客户端安装的。
//!
//! 与机器人运行时（`resources/skills/` + `workspace.rs`）完全独立：这里不读也不改那边的任何文件。

use crate::commands::blocking;
use crate::platform::{locate, paths};
use crate::store::config::{self, AppConfig};
use crate::templates::{self, TemplateVars};
use crate::workspace::{self, Interpreters};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static CATALOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 安装标记文件：有它（且 source 为本客户端）才算本客户端安装的目录。
pub const MARKER_FILE: &str = ".beetle-installed.json";
/// 改名前的安装标记文件名，只读不写。
pub const LEGACY_MARKER_FILE: &str = ".bug-patrol-installed.json";
/// 独立使用时脚本读取的运行时配置（见 `catalog/bug-hunt/_shared/bp_common.py`）。
pub const RUNTIME_FILE: &str = "beetle.runtime.json";
/// 改名前的运行时配置文件名：旧版本装的脚本只认它，目录里有这个文件时同步配置也一并刷新。
pub const LEGACY_RUNTIME_FILE: &str = "bug-patrol.runtime.json";
const MANIFEST_FILE: &str = "catalog.json";
const MARKER_SOURCE: &str = "beetle";
/// 改名前写进标记的 source，识别为本客户端安装的。
const LEGACY_MARKER_SOURCE: &str = "bug-patrol";
/// `catalog.json` 里 `config` 为这个值（或旧值 `bug-patrol`）的技能，安装时写运行时配置。
const CONFIG_BEETLE: &str = "beetle";
const LEGACY_CONFIG: &str = "bug-patrol";

fn needs_runtime(m: &Manifest) -> bool {
    matches!(m.config.as_deref(), Some(CONFIG_BEETLE) | Some(LEGACY_CONFIG))
}

/// 启动时由 `lib.rs` 传入 Tauri 的 resource 目录（其下 `catalog/`）。只有第一次调用生效。
pub fn init(resource_dir: &Path) {
    let dir = resource_dir.join("catalog");
    if dir.is_dir() {
        let _ = CATALOG_DIR.set(dir);
    }
}

/// 技能中心根目录；没调用过 `init`（开发 / 测试）时指向源码里的 `src-tauri/resources/catalog`。
pub fn catalog_dir() -> PathBuf {
    CATALOG_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join("catalog"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub version: String,
    #[serde(default)]
    pub requires: Vec<String>,
    /// `"beetle"`（旧值 `"bug-patrol"` 同义）：安装时写运行时配置；`null`：不需要客户端配置。
    #[serde(default)]
    pub config: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Claude,
    Codex,
}

impl Target {
    pub const ALL: [Target; 2] = [Target::Claude, Target::Codex];

    pub fn parse(s: &str) -> Result<Target, String> {
        match s {
            "claude" => Ok(Target::Claude),
            "codex" => Ok(Target::Codex),
            other => Err(format!("未知的安装位置：{other}（只能是 claude 或 codex）")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::Claude => "Claude Code",
            Target::Codex => "Codex",
        }
    }

    /// 技能根目录：Claude `~/.claude/skills`，Codex `$CODEX_HOME/skills`（默认 `~/.codex/skills`）。
    pub fn skills_root(self) -> PathBuf {
        match self {
            Target::Claude => user_home().join(".claude").join("skills"),
            Target::Codex => codex_home().join("skills"),
        }
    }
}

/// 用户主目录。单测里改到 `BEETLE_HOME` 下的 `data/user-home`，不碰真实的 `~/.claude`、`~/.codex`。
fn user_home() -> PathBuf {
    if cfg!(test) {
        return paths::data_dir().join("user-home");
    }
    dirs::home_dir().unwrap_or_else(std::env::temp_dir)
}

/// Codex 主目录：`CODEX_HOME` 环境变量优先（单测里忽略，避免读到开发机的设置）。
fn codex_home() -> PathBuf {
    if !cfg!(test)
        && let Some(v) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty())
    {
        return PathBuf::from(v);
    }
    user_home().join(".codex")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    version: String,
    #[serde(default)]
    installed_at: String,
    source: String,
}

/// 先读新标记文件，没有再读改名前的旧标记文件；source 为 `beetle` 或旧的 `bug-patrol` 都算本客户端安装的。
fn read_marker(dir: &Path) -> Option<Marker> {
    [MARKER_FILE, LEGACY_MARKER_FILE].iter().find_map(|f| {
        let text = std::fs::read_to_string(dir.join(f)).ok()?;
        let m: Marker = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
        (m.source == MARKER_SOURCE || m.source == LEGACY_MARKER_SOURCE).then_some(m)
    })
}

/// 写进文件的路径一律用正斜杠（Python / Node / Agent 都认，避免转义问题）。
fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && !n.starts_with(['_', '.']) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 列出技能中心的技能，按名称排序。`catalog.json` 缺失、损坏或 name 与目录名不一致的跳过。
pub fn list_manifests() -> Vec<Manifest> {
    let Ok(rd) = std::fs::read_dir(catalog_dir()) else {
        return Vec::new();
    };
    let mut out: Vec<Manifest> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let dir_name = e.file_name().to_string_lossy().into_owned();
            if !valid_name(&dir_name) || !e.path().join("SKILL.md").is_file() {
                return None;
            }
            let text = std::fs::read_to_string(e.path().join(MANIFEST_FILE)).ok()?;
            let m: Manifest = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
            (m.name == dir_name).then_some(m)
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn manifest(name: &str) -> Result<Manifest, String> {
    list_manifests()
        .into_iter()
        .find(|m| m.name == name)
        .ok_or_else(|| format!("技能中心里没有技能 {name}"))
}

/// `available` 是否比 `installed` 新。都是 `1.2.3` 这类数字版本时按段比较，否则不相同就算有更新。
pub fn is_newer(available: &str, installed: &str) -> bool {
    fn parts(s: &str) -> Option<Vec<u64>> {
        s.trim().trim_start_matches(['v', 'V']).split('.').map(|p| p.parse().ok()).collect()
    }
    match (parts(available), parts(installed)) {
        (Some(a), Some(b)) => a > b,
        _ => available.trim() != installed.trim(),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PerTarget<T> {
    pub claude: T,
    pub codex: T,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CatalogEntry {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub requires: Vec<String>,
    /// 已安装的版本（本客户端安装的才算）；没装为 null。
    pub installed: PerTarget<Option<String>>,
    pub update_available: PerTarget<bool>,
}

/// 某个技能在某个位置由本客户端安装的版本。
pub fn installed_version(name: &str, target: Target) -> Option<String> {
    read_marker(&target.skills_root().join(name)).map(|m| m.version)
}

pub fn list() -> Vec<CatalogEntry> {
    list_manifests()
        .into_iter()
        .map(|m| {
            let claude = installed_version(&m.name, Target::Claude);
            let codex = installed_version(&m.name, Target::Codex);
            let upd = |v: &Option<String>| v.as_deref().is_some_and(|v| is_newer(&m.version, v));
            let update_available = PerTarget { claude: upd(&claude), codex: upd(&codex) };
            CatalogEntry {
                name: m.name,
                display_name: m.display_name,
                description: m.description,
                version: m.version,
                requires: m.requires,
                installed: PerTarget { claude, codex },
                update_available,
            }
        })
        .collect()
}

/// 独立代码缓存：和机器人的 `code_cache_dir()` 分开，用户会话里同步代码（强制切分支）不会影响正在跑的机器人。
pub fn standalone_code_root() -> PathBuf {
    paths::data_dir().join("skill-code")
}

/// 技能写报告的目录（SKILL.md 里的 `{{REPORTS_DIR}}`）：放在技能目录之外，更新 / 重装 / 卸载技能都不会删掉用户的报告。
pub fn reports_dir(name: &str) -> PathBuf {
    paths::data_dir().join("skill-reports").join(name)
}

/// 旧版本把报告写在 `<技能目录>/reports/`：更新 / 卸载前把里面的文件搬到 `reports_dir`（重名的改名搬过去，不覆盖、不丢）。
fn migrate_old_reports(dst: &Path, name: &str) -> Result<(), String> {
    let old = dst.join("reports");
    let Ok(rd) = std::fs::read_dir(&old) else { return Ok(()) };
    let target = reports_dir(name);
    io(&format!("创建报告目录 {}", target.display()), std::fs::create_dir_all(&target))?;
    for e in rd.flatten() {
        let to = target.join(e.file_name());
        if to.exists() {
            // 同名报告已在新目录：给旧的换个名字，绝不覆盖、也不丢
            let stem = e.file_name().to_string_lossy().into_owned();
            let alt = target.join(format!("{stem}.旧版-{}", std::process::id()));
            io(&format!("迁移报告 {}", e.path().display()), std::fs::rename(e.path(), alt))?;
        } else {
            io(&format!("迁移报告 {}", e.path().display()), std::fs::rename(e.path(), to))?;
        }
    }
    Ok(())
}

/// 独立使用时的运行时配置：全部项目和数据源（没有 chat_id / doc_url / trigger 字段，群和文档由调用参数给出）。
/// 不含任何密钥，数据源也不含主机、账号（见 `DataSource::runtime_value`）。
pub fn runtime_value(cfg: &AppConfig) -> Value {
    let mut projects = serde_json::Map::new();
    for (name, p) in &cfg.projects {
        projects.insert(name.clone(), p.runtime_value(&cfg.dbs));
    }
    json!({
        "owner_open_id": cfg.owner_open_id,
        "code_root": fwd(&standalone_code_root()),
        "review_state_dir": fwd(&crate::review_card::state_root()),
        "projects": projects,
        "dbs": workspace::runtime_dbs(cfg, cfg.dbs.keys()),
    })
}

fn io<T>(what: &str, r: std::io::Result<T>) -> Result<T, String> {
    r.map_err(|e| format!("{what}失败：{e}"))
}

fn write_runtime(dir: &Path, cfg: &AppConfig) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&runtime_value(cfg)).map_err(|e| format!("序列化运行时配置失败：{e}"))?;
    let path = dir.join(RUNTIME_FILE);
    io(&format!("写入 {}", path.display()), std::fs::write(&path, &text))?;
    // 改名前装的技能：旧脚本只读旧文件名，有就一起刷新，直到用户点「更新」重装
    let legacy = dir.join(LEGACY_RUNTIME_FILE);
    if legacy.is_file() {
        io(&format!("写入 {}", legacy.display()), std::fs::write(&legacy, &text))?;
    }
    Ok(())
}

/// 把 `staging` 里复制好的技能准备完整：渲染 SKILL.md（`{{SKILL_DIR}}` 写最终目录 `dst`）、写运行时配置和标记。
fn prepare_staging(m: &Manifest, staging: &Path, dst: &Path, cfg: &AppConfig, interp: &Interpreters) -> Result<(), String> {
    let md_path = staging.join("SKILL.md");
    let md = io(&format!("读取 {}", md_path.display()), std::fs::read_to_string(&md_path))?;
    let mut vars = TemplateVars::default();
    vars.0.insert("SKILL_DIR", fwd(dst));
    vars.0.insert("PYTHON", fwd(&interp.python));
    vars.0.insert("NODE", fwd(&interp.node));
    vars.0.insert("BEETLE_EXE", workspace::beetle_exe());
    let reports = reports_dir(&m.name);
    io(&format!("创建报告目录 {}", reports.display()), std::fs::create_dir_all(&reports))?;
    vars.0.insert("REPORTS_DIR", fwd(&reports));
    let md = templates::render(&md, &vars).map_err(|e| format!("技能 {} 的 SKILL.md：{e}", m.name))?;
    io(&format!("写入 {}", md_path.display()), std::fs::write(&md_path, md))?;
    if needs_runtime(m) {
        write_runtime(staging, cfg)?;
    }
    let marker = json!({
        "version": m.version,
        "installed_at": chrono::Local::now().to_rfc3339(),
        "source": MARKER_SOURCE,
    });
    let text = serde_json::to_string_pretty(&marker).map_err(|e| format!("序列化安装标记失败：{e}"))?;
    io("写入安装标记", std::fs::write(staging.join(MARKER_FILE), text))
}

fn foreign_error(t: Target, dst: &Path) -> String {
    format!("{}：已存在同名技能（不是本客户端安装的），请先手动处理：{}", t.label(), dst.display())
}

fn install_one(m: &Manifest, src: &Path, t: Target, cfg: &AppConfig, interp: &Interpreters) -> Result<(), String> {
    let root = t.skills_root();
    io(&format!("创建目录 {}", root.display()), std::fs::create_dir_all(&root))?;
    let dst = root.join(&m.name);
    let staging = root.join(format!(".beetle-tmp-{}-{}", m.name, std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let cleanup = |e: String| {
        let _ = std::fs::remove_dir_all(&staging);
        e
    };
    workspace::copy_dir(src, &staging).map_err(cleanup)?;
    // 源目录里万一带了开发时留下的运行时配置 / 标记，不能当成安装结果带过去
    for f in [RUNTIME_FILE, LEGACY_RUNTIME_FILE, MARKER_FILE, LEGACY_MARKER_FILE] {
        let _ = std::fs::remove_file(staging.join(f));
    }
    prepare_staging(m, &staging, &dst, cfg, interp).map_err(cleanup)?;
    if dst.exists() {
        // 复制期间可能有人放了同名目录：再确认一次是本客户端装的才删
        if read_marker(&dst).is_none() {
            return Err(cleanup(foreign_error(t, &dst)));
        }
        // 审查 B8：旧版本写在技能目录里的报告先搬出去，再删旧目录
        migrate_old_reports(&dst, &m.name).map_err(cleanup)?;
        std::fs::remove_dir_all(&dst).map_err(|e| cleanup(format!("删除旧版本 {} 失败：{e}（可能有程序正在使用，关掉后重试）", dst.display())))?;
    }
    std::fs::rename(&staging, &dst).map_err(|e| cleanup(format!("安装到 {} 失败：{e}", dst.display())))
}

/// 安装 / 更新到选中的位置。先检查全部目标：任何一个是别人的同名目录就整个不装。
pub fn install(name: &str, targets: &[Target], cfg: &AppConfig, interp: &Interpreters) -> Result<(), String> {
    let mut uniq: Vec<Target> = Vec::new();
    for t in targets {
        if !uniq.contains(t) {
            uniq.push(*t);
        }
    }
    if uniq.is_empty() {
        return Err("请至少选择一个安装位置（Claude Code 或 Codex）".into());
    }
    let m = manifest(name)?;
    for t in &uniq {
        let dst = t.skills_root().join(&m.name);
        if dst.exists() && read_marker(&dst).is_none() {
            return Err(foreign_error(*t, &dst));
        }
    }
    let src = catalog_dir().join(&m.name);
    for t in uniq {
        install_one(&m, &src, t, cfg, interp)?;
    }
    Ok(())
}

/// 卸载：只删除带本客户端标记的目录。没装过视为成功。
pub fn uninstall(name: &str, t: Target) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("技能名不合法：{name}"));
    }
    let dst = t.skills_root().join(name);
    if !dst.exists() {
        return Ok(());
    }
    if read_marker(&dst).is_none() {
        return Err(format!("{}：{} 不是本客户端安装的，不能卸载：{}", t.label(), name, dst.display()));
    }
    migrate_old_reports(&dst, name)?;
    io(&format!("删除 {}", dst.display()), std::fs::remove_dir_all(&dst))
}

/// 把客户端当前配置（项目、数据源）重新写进所有已安装、需要配置的技能。返回刷新了几处。
pub fn refresh_config(cfg: &AppConfig) -> Result<u32, String> {
    let mut n = 0;
    for m in list_manifests().into_iter().filter(needs_runtime) {
        for t in Target::ALL {
            let dir = t.skills_root().join(&m.name);
            if read_marker(&dir).is_some() {
                write_runtime(&dir, cfg)?;
                n += 1;
            }
        }
    }
    Ok(n)
}

fn interpreters() -> Interpreters {
    let py = if cfg!(windows) { "python" } else { "python3" };
    Interpreters {
        python: locate::find(py).unwrap_or_else(|| py.into()),
        node: locate::find("node").unwrap_or_else(|| "node".into()),
    }
}

// ───────────────────────── Tauri 命令 ─────────────────────────

#[tauri::command]
pub async fn catalog_list() -> Result<Vec<CatalogEntry>, String> {
    blocking(|| Ok(list())).await
}

/// `targets`：`"claude"` / `"codex"`。
#[tauri::command]
pub async fn catalog_install(name: String, targets: Vec<String>) -> Result<(), String> {
    blocking(move || {
        let targets = targets.iter().map(|t| Target::parse(t)).collect::<Result<Vec<_>, _>>()?;
        let cfg = config::load()?;
        install(&name, &targets, &cfg, &interpreters())
    })
    .await
}

#[tauri::command]
pub async fn catalog_uninstall(name: String, target: String) -> Result<(), String> {
    blocking(move || uninstall(&name, Target::parse(&target)?)).await
}

/// 「同步配置」：改了项目 / 数据源后，把最新配置写进已安装的技能。
#[tauri::command]
pub async fn catalog_refresh_config() -> Result<u32, String> {
    blocking(|| refresh_config(&config::load()?)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::config::Project;
    use crate::test_support::test_home;
    use crate::store::datasource::{Access, DataSource, DbKind, SkillRef};

    /// 技能方式的数据源（测试里不需要直连信息）。
    fn skill_ds(db: &str) -> DataSource {
        DataSource {
            kind: DbKind::Mysql,
            description: String::new(),
            access: Access::Skill(SkillRef { skill: "my-sql-skill".into(), database: db.into(), note: String::new() }),
        }
    }

    fn cfg() -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.owner_open_id = Some("ou_owner".into());
        cfg.projects.insert(
            "demo".into(),
            Project::new("git@gitee.com:a/demo.git", "release/*", "AGENTS.md", "示例服务", vec!["demo_config".into()]),
        );
        cfg.dbs.insert("demo_config".into(), skill_ds("demo_config"));
        // 旧配置留下的 sql-window 设置：不再写进运行时
        cfg.skill_settings.insert("sql-window".into(), json!({"chatId": "oc_demo", "status": "verified"}));
        cfg
    }

    fn interp() -> Interpreters {
        Interpreters { python: PathBuf::from("C:\\Py\\python.exe"), node: PathBuf::from("C:\\node\\node.exe") }
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    fn rt(dir: &Path) -> Value {
        serde_json::from_str(&read(&dir.join(RUNTIME_FILE))).unwrap()
    }

    #[test]
    fn catalog_lists_bug_hunt_with_manifest_fields() {
        let ms = list_manifests();
        let m = ms.iter().find(|m| m.name == "bug-hunt").expect("技能中心里应有 bug-hunt");
        assert_eq!(m.display_name, "查 bug");
        assert_eq!(m.requires, ["python", "lark_cli", "git"]);
        assert_eq!(m.config.as_deref(), Some("beetle"));
        assert!(!m.version.is_empty() && !m.description.is_empty());
        let src = catalog_dir().join("bug-hunt");
        for f in [RUNTIME_FILE, LEGACY_RUNTIME_FILE, MARKER_FILE, LEGACY_MARKER_FILE] {
            assert!(!src.join(f).exists(), "源目录不能带安装产物 {f}");
        }
        assert!(!src.join("scripts").join("p2p_context.py").exists(), "私聊脚本依赖机器人运行时，不放进独立技能");
    }

    #[test]
    fn target_roots_are_under_isolated_home_in_tests() {
        let home = test_home();
        let base = home.path().join("data").join("user-home");
        assert_eq!(Target::Claude.skills_root(), base.join(".claude").join("skills"));
        assert_eq!(Target::Codex.skills_root(), base.join(".codex").join("skills"));
        assert_eq!(Target::parse("claude"), Ok(Target::Claude));
        assert_eq!(Target::parse("codex"), Ok(Target::Codex));
        assert!(Target::parse("cursor").unwrap_err().contains("未知的安装位置"));
    }

    /// 代码审查技能：脚本与公共模块齐全、占位渲染干净，运行时配置带上一轮结果目录（和机器人同一个位置）。
    #[test]
    fn install_code_review_skill_with_shared_modules_and_review_dir() {
        let _home = test_home();
        let m = manifest("code-review").expect("技能中心里应有 code-review");
        assert_eq!((m.display_name.as_str(), m.config.as_deref()), ("代码审查", Some("beetle")));
        install("code-review", &[Target::Claude], &cfg(), &interp()).unwrap();
        let dst = Target::Claude.skills_root().join("code-review");
        let md = read(&dst.join("SKILL.md"));
        assert!(!md.contains("{{") && !md.contains('\\'), "占位要渲染干净、路径用正斜杠");
        assert!(md.starts_with("---\nname: code-review\n") || md.starts_with("---\r\nname: code-review"));
        assert!(md.contains(&format!("{}/scripts/pr_fetch.py", fwd(&dst))));
        assert!(md.contains(&fwd(&reports_dir("code-review"))));
        for f in ["scripts/pr_fetch.py", "scripts/pr_review_state.py", "scripts/sync_code.py", "_shared/bp_common.py", "_shared/bp_issue.py", "_shared/bp_pr.py"] {
            assert!(dst.join(f).is_file(), "缺少 {f}");
        }
        let r = rt(&dst);
        assert_eq!(r["review_state_dir"], fwd(&crate::review_card::state_root()));
        assert_eq!(r["projects"]["demo"]["repo"], "git@gitee.com:a/demo.git");
    }

    #[test]
    fn install_to_both_targets_renders_placeholders_and_writes_runtime_and_marker() {
        let _home = test_home();
        install("bug-hunt", &[Target::Claude, Target::Codex], &cfg(), &interp()).unwrap();
        for t in Target::ALL {
            let dst = t.skills_root().join("bug-hunt");
            let md = read(&dst.join("SKILL.md"));
            assert!(!md.contains("{{"), "{} 的 SKILL.md 有未渲染的占位", t.label());
            assert!(md.starts_with("---\nname: bug-hunt\n") || md.starts_with("---\r\nname: bug-hunt"), "frontmatter 要保留");
            assert!(md.contains(&workspace::beetle_exe()), "SKILL.md 里的 {{{{BEETLE_EXE}}}} 渲染成客户端路径");
            assert!(md.contains("C:/Py/python.exe"));
            assert!(!md.contains('\\'), "路径一律正斜杠");
            for f in ["scripts/chat_context.py", "scripts/sync_code.py", "scripts/append_bug_doc.py", "scripts/create_issue.py", "_shared/bp_common.py", "_shared/bp_issue.py"] {
                assert!(dst.join(f).is_file(), "缺少 {f}");
            }
            let r = rt(&dst);
            assert_eq!(r["projects"]["demo"]["repo"], "git@gitee.com:a/demo.git");
            assert_eq!(r["projects"]["demo"]["record"], json!({"issue": true, "feishu_doc": true, "meegle": false, "feishu_task": false}));
            assert_eq!(r["projects"]["demo"]["dbs"][0]["alias"], "demo_config");
            assert_eq!(r["dbs"][0], json!({"alias": "demo_config", "kind": "mysql", "description": "", "access": "skill",
                                           "skill": "my-sql-skill", "database": "demo_config", "note": ""}));
            assert!(r.get("sql_window").is_none() && r.get("requests_dir").is_none(), "sql-window 相关字段已移除");
            assert!(!read(&dst.join(RUNTIME_FILE)).contains("oc_demo"));
            assert_eq!(r["owner_open_id"], "ou_owner");
            assert_eq!(r["code_root"], fwd(&standalone_code_root()));
            assert_ne!(r["code_root"], fwd(&paths::code_cache_dir()), "不能和机器人共用代码缓存");
            assert!(r.get("chat_id").is_none() && r.get("trigger_message_id").is_none() && r.get("doc_url").is_none());
            let marker: Value = serde_json::from_str(&read(&dst.join(MARKER_FILE))).unwrap();
            assert_eq!(marker["source"], "beetle");
            assert!(!dst.join(LEGACY_MARKER_FILE).exists() && !dst.join(LEGACY_RUNTIME_FILE).exists(), "新装的不写旧文件名");
            assert_eq!(marker["version"], manifest("bug-hunt").unwrap().version);
            assert!(marker["installed_at"].as_str().is_some_and(|s| !s.is_empty()));
        }
        assert!(!paths::data_dir().join("sql-requests").exists(), "不再创建 SQL 请求目录");
        let e = list().into_iter().find(|e| e.name == "bug-hunt").unwrap();
        assert_eq!(e.installed.claude, Some(e.version.clone()));
        assert_eq!(e.installed.codex, Some(e.version.clone()));
        assert_eq!(e.update_available, PerTarget { claude: false, codex: false });
        // 没有留下临时目录
        let leftovers = std::fs::read_dir(Target::Claude.skills_root())
            .unwrap()
            .flatten()
            .filter(|d| d.file_name().to_string_lossy().starts_with('.'))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn install_refuses_foreign_dir_and_touches_nothing() {
        let _home = test_home();
        let foreign = Target::Codex.skills_root().join("bug-hunt");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("SKILL.md"), "用户自己的").unwrap();
        let err = install("bug-hunt", &[Target::Claude, Target::Codex], &cfg(), &interp()).unwrap_err();
        assert!(err.contains("已存在同名技能（不是本客户端安装的），请先手动处理"), "{err}");
        assert!(err.contains("Codex"));
        assert_eq!(read(&foreign.join("SKILL.md")), "用户自己的");
        assert!(!Target::Claude.skills_root().join("bug-hunt").exists(), "有一个目标冲突就整个不装");

        // 标记 source 不是本客户端的也算别人的
        std::fs::write(foreign.join(MARKER_FILE), r#"{"version":"1.0.0","installed_at":"","source":"other"}"#).unwrap();
        assert!(install("bug-hunt", &[Target::Codex], &cfg(), &interp()).is_err());
        assert_eq!(installed_version("bug-hunt", Target::Codex), None);
    }

    #[test]
    fn reinstall_replaces_old_files_when_marker_present() {
        let _home = test_home();
        install("bug-hunt", &[Target::Claude], &cfg(), &interp()).unwrap();
        let dst = Target::Claude.skills_root().join("bug-hunt");
        std::fs::write(dst.join("stale.txt"), "旧文件").unwrap();
        let mut c = cfg();
        c.dbs.insert("demo_worker".into(), skill_ds("demo_worker"));
        install("bug-hunt", &[Target::Claude, Target::Claude], &c, &interp()).unwrap();
        assert!(!dst.join("stale.txt").exists(), "覆盖更新要先删旧目录");
        let aliases: Vec<Value> = rt(&dst)["dbs"].as_array().unwrap().iter().map(|d| d["alias"].clone()).collect();
        assert_eq!(aliases, [json!("demo_config"), json!("demo_worker")], "独立技能写全部数据源");
    }

    /// 审查 B8：报告目录在技能目录之外，更新 / 重装不会删掉报告；旧版本写在技能目录里的报告会被搬出来。
    #[test]
    fn reinstall_keeps_reports_outside_skill_dir() {
        let _home = test_home();
        install("bug-hunt", &[Target::Claude], &cfg(), &interp()).unwrap();
        let dst = Target::Claude.skills_root().join("bug-hunt");
        let reports = reports_dir("bug-hunt");
        assert!(reports.is_dir());
        assert!(!reports.starts_with(&dst), "报告目录不能在技能目录里");
        let md = read(&dst.join("SKILL.md"));
        assert!(md.contains(&format!("{}/", fwd(&reports))), "SKILL.md 里的报告路径指向技能目录之外");
        assert!(!md.contains(&format!("{}/reports/", fwd(&dst))));
        std::fs::write(reports.join("2026-11-19-a.md"), "新报告").unwrap();
        // 模拟旧版本留在技能目录里的报告（含一份重名的）
        std::fs::create_dir_all(dst.join("reports")).unwrap();
        std::fs::write(dst.join("reports").join("2026-11-01-old.md"), "旧报告").unwrap();
        std::fs::write(dst.join("reports").join("2026-11-19-a.md"), "旧的同名报告").unwrap();

        install("bug-hunt", &[Target::Claude], &cfg(), &interp()).unwrap();
        assert_eq!(read(&reports.join("2026-11-19-a.md")), "新报告", "不覆盖已有报告");
        assert_eq!(read(&reports.join("2026-11-01-old.md")), "旧报告", "旧报告搬到新目录");
        let renamed = std::fs::read_dir(&reports)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("2026-11-19-a.md.旧版-"));
        assert!(renamed, "重名的旧报告改名保留");
        uninstall("bug-hunt", Target::Claude).unwrap();
        assert_eq!(read(&reports.join("2026-11-19-a.md")), "新报告", "卸载也不删报告");
    }

    #[test]
    fn install_rejects_empty_targets_and_unknown_skill() {
        let _home = test_home();
        assert!(install("bug-hunt", &[], &cfg(), &interp()).unwrap_err().contains("至少选择"));
        assert!(install("nope", &[Target::Claude], &cfg(), &interp()).unwrap_err().contains("没有技能"));
    }

    #[test]
    fn uninstall_only_removes_client_installed_dirs() {
        let _home = test_home();
        install("bug-hunt", &[Target::Claude], &cfg(), &interp()).unwrap();
        let foreign = Target::Codex.skills_root().join("bug-hunt");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("SKILL.md"), "用户自己的").unwrap();

        assert!(uninstall("bug-hunt", Target::Codex).unwrap_err().contains("不是本客户端安装的"));
        assert!(foreign.join("SKILL.md").is_file());
        uninstall("bug-hunt", Target::Claude).unwrap();
        assert!(!Target::Claude.skills_root().join("bug-hunt").exists());
        // 没装过视为成功；非法名拒绝
        uninstall("bug-hunt", Target::Claude).unwrap();
        assert!(uninstall("../x", Target::Claude).is_err());
        assert!(uninstall("", Target::Claude).is_err());
    }

    #[test]
    fn update_available_when_installed_version_is_older() {
        let _home = test_home();
        install("bug-hunt", &[Target::Claude, Target::Codex], &cfg(), &interp()).unwrap();
        let marker = Target::Codex.skills_root().join("bug-hunt").join(MARKER_FILE);
        std::fs::write(&marker, r#"{"version":"0.0.1","installed_at":"x","source":"beetle"}"#).unwrap();
        let e = list().into_iter().find(|e| e.name == "bug-hunt").unwrap();
        assert_eq!(e.installed.codex.as_deref(), Some("0.0.1"));
        assert_eq!(e.update_available, PerTarget { claude: false, codex: true });

        assert!(is_newer("1.0.10", "1.0.9"));
        assert!(is_newer("v2.0.0", "1.9.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.2.0"), "已装的更新不提示");
        assert!(is_newer("1.0.0-beta", "1.0.0-alpha"), "不是纯数字版本时不同即更新");
    }

    #[test]
    fn refresh_config_rewrites_runtime_of_installed_skills_only() {
        let _home = test_home();
        install("bug-hunt", &[Target::Claude], &cfg(), &interp()).unwrap();
        let mut c = cfg();
        c.dbs.insert("demo_new".into(), skill_ds("demo_new"));
        c.projects.clear();
        assert_eq!(refresh_config(&c).unwrap(), 1);
        let r = rt(&Target::Claude.skills_root().join("bug-hunt"));
        assert_eq!(r["dbs"].as_array().unwrap().len(), 2);
        assert!(r["projects"].as_object().unwrap().is_empty());
        assert!(!Target::Codex.skills_root().join("bug-hunt").exists(), "没装的位置不创建");
    }

    /// 改名前装的技能（旧标记文件 + source bug-patrol + 旧运行时文件名）：仍算本客户端安装的，
    /// 同步配置时新旧两个运行时文件都刷新，更新时可以覆盖，卸载时可以删除。
    #[test]
    fn legacy_bug_patrol_install_is_recognized() {
        let _home = test_home();
        let dst = Target::Claude.skills_root().join("bug-hunt");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("SKILL.md"), "旧版").unwrap();
        std::fs::write(dst.join(LEGACY_MARKER_FILE), r#"{"version":"0.0.1","installed_at":"x","source":"bug-patrol"}"#).unwrap();
        std::fs::write(dst.join(LEGACY_RUNTIME_FILE), "{}").unwrap();
        assert_eq!(installed_version("bug-hunt", Target::Claude).as_deref(), Some("0.0.1"));
        let e = list().into_iter().find(|e| e.name == "bug-hunt").unwrap();
        assert!(e.update_available.claude);

        assert_eq!(refresh_config(&cfg()).unwrap(), 1);
        assert_eq!(rt(&dst)["dbs"][0]["alias"], "demo_config");
        let legacy: Value = serde_json::from_str(&read(&dst.join(LEGACY_RUNTIME_FILE))).unwrap();
        assert_eq!(legacy, rt(&dst), "旧脚本读的旧文件也要刷新");

        install("bug-hunt", &[Target::Claude], &cfg(), &interp()).unwrap();
        assert!(dst.join(MARKER_FILE).is_file());
        assert!(!dst.join(LEGACY_MARKER_FILE).exists() && !dst.join(LEGACY_RUNTIME_FILE).exists(), "重装后只留新文件名");
        uninstall("bug-hunt", Target::Claude).unwrap();
        assert!(!dst.exists());
    }
}
