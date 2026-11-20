//! Tauri 命令：把存储、飞书注册、supervisor 暴露给前端。
//!
//! 结构：每个命令只是薄封装，真正的逻辑在 `*_impl` 纯函数里（不依赖 Tauri，可直接单测）。
//! 所有配置写入一律走 `config::update`（锁内重读 → 修改 → 保存），否则会冲掉自动接入登记的群、主人 open_id 等。

use crate::platform::{paths, proc};
use crate::skills;
use crate::store::config::{self, AppConfig, Bot, Group, Project};
use crate::store::datasource::{Access, DataSource, DbKind};
use crate::store::runs::{RunRecord, RunStore, StepRecord};
use crate::store::secrets;
use crate::supervisor::{
    BotState, BotStateEvent, EVENT_BOT_STATE, EVENT_RUN_STEP, EVENT_RUN_UPDATED, EventSink, RunStepEvent, RunUpdatedEvent, Supervisor,
};
use crate::tray;
use serde::Serialize;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

/// 校验仓库地址的超时（brief：30 秒）。
const REPO_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// 「今日运行数」最多往回看多少条记录。
const TODAY_SCAN_LIMIT: u32 = 2000;
const RUNS_LIMIT_MAX: u32 = 500;

pub struct AppState {
    pub supervisor: Supervisor,
    pub store: Arc<RunStore>,
}

/// 机器人在列表里的视图。**不含 App Secret**（密钥只在系统凭据库里，`Bot` 结构体里也没有这个字段）。
#[derive(Debug, Serialize)]
pub struct BotView {
    pub bot: Bot,
    pub state: BotState,
    pub today_runs: u32,
    /// 连接任务是否还在：异常状态下为 true 表示「连接失败，正在重试」，界面显示「停止」而不是「启动」。
    pub alive: bool,
}

#[derive(Debug, Serialize)]
pub struct ProjectsView {
    pub projects: std::collections::BTreeMap<String, Project>,
    pub dbs: std::collections::BTreeMap<String, DataSource>,
    pub default_projects: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RunDetail {
    pub run: RunRecord,
    pub steps: Vec<StepRecord>,
}

#[derive(Debug, Serialize)]
pub struct SkillInfoView {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub requires: Vec<String>,
    pub builtin: bool,
}

/// 本机已装的技能（给数据源「查询技能」下拉用）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LocalSkill {
    pub name: String,
    /// `claude` = `~/.claude/skills`，`codex` = `~/.codex/skills`。
    pub location: &'static str,
    pub description: String,
}

/// 技能设置表单：字段定义（来自技能自带的 `settings_schema.fields`）+ 当前保存的值。
#[derive(Debug, Serialize)]
pub struct SkillSettings {
    pub fields: Vec<serde_json::Value>,
    pub value: serde_json::Value,
}

// ───────────────────────── 纯逻辑 ─────────────────────────

pub fn build_bot_views(
    cfg: &AppConfig,
    state_of: &dyn Fn(&str) -> BotState,
    alive_of: &dyn Fn(&str) -> bool,
    today: &HashMap<String, u32>,
) -> Vec<BotView> {
    cfg.bots
        .iter()
        .map(|b| BotView {
            bot: b.clone(),
            state: state_of(&b.id),
            today_runs: today.get(&b.id).copied().unwrap_or(0),
            alive: alive_of(&b.id),
        })
        .collect()
}

/// 统计 `started_at >= midnight_ms` 的运行数，按机器人分组。
pub fn count_today(records: &[RunRecord], midnight_ms: i64) -> HashMap<String, u32> {
    let mut out: HashMap<String, u32> = HashMap::new();
    for r in records.iter().filter(|r| r.started_at >= midnight_ms) {
        *out.entry(r.bot_id.clone()).or_default() += 1;
    }
    out
}

pub(crate) fn local_midnight_ms() -> i64 {
    use chrono::{Duration as Span, Local};
    Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(Local).earliest())
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|| (Local::now() - Span::hours(24)).timestamp_millis())
}

fn bot_index(cfg: &AppConfig, id: &str) -> Result<usize, String> {
    cfg.bots.iter().position(|b| b.id == id).ok_or_else(|| format!("找不到机器人 {id}"))
}

fn other_bot_uses_app_id(cfg: &AppConfig, except_id: &str, app_id: &str) -> bool {
    cfg.bots.iter().any(|b| b.id != except_id && b.app_id == app_id)
}

pub const APP_ID_NEEDS_SECRET: &str = "更换 App ID 需要同时填写新的 App Secret（原 App ID 的密钥不会被删除）";

/// 保存机器人（新增或修改）。App Secret 只作为参数进入凭据库，不进配置、不进返回值。
/// 群列表由 `save_group` / `remove_group` / 自动接入管理：这里总是沿用配置里最新的群，
/// 避免界面拿着旧快照把自动接入的群冲掉。
pub fn save_bot_impl(mut bot: Bot, secret: Option<String>) -> Result<Bot, String> {
    bot.name = bot.name.trim().to_string();
    bot.app_id = bot.app_id.trim().to_string();
    if bot.name.is_empty() {
        return Err("机器人名称不能为空".into());
    }
    if bot.app_id.is_empty() {
        return Err("App ID 不能为空".into());
    }
    if bot.concurrency == 0 {
        return Err("并发数至少为 1".into());
    }
    if bot.timeout_minutes == 0 {
        return Err("超时时间至少为 1 分钟".into());
    }
    if bot.id.trim().is_empty() {
        bot.id = uuid::Uuid::new_v4().to_string();
    }
    let secret = secret.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    // 审查 B6：改了 App ID 却没填新 Secret 时，新 App ID 在凭据库里必须已经有密钥，否则保存后机器人永远启动不了、
    // 旧密钥还会被下面清掉。先在锁外查一次凭据库（慢操作不进配置锁）。
    let app_id_changed = config::load().is_ok_and(|c| c.bots.iter().any(|b| b.id == bot.id && b.app_id != bot.app_id));
    let new_id_has_secret = secret.is_some() || (app_id_changed && secrets::app_secret(&bot.app_id).is_ok());
    if app_id_changed && !new_id_has_secret {
        return Err(APP_ID_NEEDS_SECRET.into());
    }
    if let Some(s) = secret.as_deref() {
        secrets::set_app_secret(&bot.app_id, s)?;
    }
    let mut old_app_id = None;
    let mut saved = bot.clone();
    let cfg = config::update(|cfg| {
        match cfg.bots.iter().position(|b| b.id == bot.id) {
            Some(i) => {
                if cfg.bots[i].app_id != bot.app_id && !new_id_has_secret {
                    return Err(APP_ID_NEEDS_SECRET.into());
                }
                old_app_id = Some(cfg.bots[i].app_id.clone());
                bot.groups = cfg.bots[i].groups.clone();
                cfg.bots[i] = bot.clone();
            }
            None => cfg.bots.push(bot.clone()),
        }
        saved = bot.clone();
        Ok(())
    })?;
    if let Some(old) = old_app_id
        && old != saved.app_id
        && !other_bot_uses_app_id(&cfg, &saved.id, &old)
        && let Err(e) = secrets::delete_app_secret(&old)
    {
        eprintln!("beetle: 清理旧 App ID 的凭据失败：{e}");
    }
    Ok(saved)
}

/// 删除机器人及它的 App Secret（别的机器人还在用同一个 App ID 时保留密钥）。
pub fn delete_bot_impl(id: &str) -> Result<(), String> {
    let mut app_id = String::new();
    let cfg = config::update(|cfg| {
        let i = bot_index(cfg, id)?;
        app_id = cfg.bots.remove(i).app_id;
        Ok(())
    })?;
    if !other_bot_uses_app_id(&cfg, id, &app_id) {
        secrets::delete_app_secret(&app_id).map_err(|e| format!("机器人已删除，但清除 App Secret 失败：{e}"))?;
    }
    Ok(())
}

pub fn set_start_with_client_impl(id: &str, on: bool) -> Result<(), String> {
    config::update(|cfg| {
        let i = bot_index(cfg, id)?;
        cfg.bots[i].start_with_client = on;
        Ok(())
    })
    .map(|_| ())
}

/// 群在磁盘上的工作目录名：不能含路径分隔符。
fn check_slug(slug: &str) -> Result<(), String> {
    if slug.is_empty() || slug == "." || slug == ".." || slug.contains(['/', '\\', ':']) {
        return Err(format!("群的目录名不合法：{slug:?}（不能为空，也不能含 / \\ :）"));
    }
    Ok(())
}

pub fn save_group_impl(bot_id: &str, mut group: Group) -> Result<(), String> {
    group.chat_id = group.chat_id.trim().to_string();
    group.slug = group.slug.trim().to_string();
    if group.chat_id.is_empty() {
        return Err("群的 chat_id 不能为空".into());
    }
    check_slug(&group.slug)?;
    config::update(|cfg| {
        if let Some(missing) = group.projects.iter().find(|p| !cfg.projects.contains_key(*p)) {
            return Err(format!("项目「{missing}」不存在，请先在「项目与库」里添加"));
        }
        let i = bot_index(cfg, bot_id)?;
        let groups = &mut cfg.bots[i].groups;
        if groups.iter().any(|g| g.slug == group.slug && g.chat_id != group.chat_id) {
            return Err(format!("目录名 {} 已被同一机器人的另一个群使用", group.slug));
        }
        match groups.iter().position(|g| g.chat_id == group.chat_id) {
            Some(j) => groups[j] = group.clone(),
            None => groups.push(group.clone()),
        }
        Ok(())
    })
    .map(|_| ())
}

pub fn remove_group_impl(bot_id: &str, chat_id: &str) -> Result<(), String> {
    config::update(|cfg| {
        let i = bot_index(cfg, bot_id)?;
        cfg.bots[i].groups.retain(|g| g.chat_id != chat_id);
        Ok(())
    })
    .map(|_| ())
}

/// `git ls-remote --heads <repo>`（隐藏窗口，不弹凭据输入框）校验仓库能访问。失败返回中文错误。
pub fn check_repo(repo: &str, timeout: Duration) -> Result<(), String> {
    let repo = repo.trim();
    // 以 - 开头会被 git 当成选项
    if repo.is_empty() || repo.starts_with('-') {
        return Err(format!("仓库地址不合法：{repo:?}"));
    }
    let mut cmd = Command::new("git");
    cmd.args(["ls-remote", "--heads", "--", repo])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o ConnectTimeout=15");
    }
    let child = proc::spawn_hidden(&mut cmd).map_err(|e| format!("无法运行 git：{e}（请确认已安装 git）"))?;
    let out = proc::wait_output_timeout(child, timeout)
        .map_err(|e| format!("无法访问仓库 {}：{e}", crate::redact::redact(repo)))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reason = stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("git 返回错误");
    Err(format!("无法访问仓库 {}：{}", crate::redact::redact(repo), crate::redact::redact(reason)))
}

fn validate_project(cfg: &AppConfig, name: &str, project: &Project) -> Result<(), String> {
    if name.is_empty() {
        return Err("项目名称不能为空".into());
    }
    if project.repo.trim().is_empty() {
        return Err("仓库地址不能为空".into());
    }
    if let Some(missing) = project.dbs.iter().find(|d| !cfg.dbs.contains_key(*d)) {
        return Err(format!("数据源「{missing}」不存在，请先在「数据源」页添加"));
    }
    if let Some(env) = project.token_env.as_deref()
        && !crate::issue::valid_env_name(env)
    {
        return Err(format!("token 环境变量名不合法：{env}（只能用字母、数字和下划线）"));
    }
    if project.record.issue && crate::issue::target(project).is_none() {
        return Err("勾选了「建 issue」，但仓库地址不是 gitee.com / github.com：请填写 issue 仓库（完整地址）或取消勾选".into());
    }
    Ok(())
}

/// 保存项目：先校验仓库能访问（慢操作，在配置锁外做），再写配置。
pub fn save_project_impl(name: &str, mut project: Project) -> Result<(), String> {
    let name = name.trim();
    project.repo = project.repo.trim().to_string();
    let blank_none = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    project.issue_repo = blank_none(project.issue_repo.take());
    project.token_env = blank_none(project.token_env.take());
    validate_project(&config::load()?, name, &project)?;
    check_repo(&project.repo, REPO_CHECK_TIMEOUT)?;
    config::update(|cfg| {
        validate_project(cfg, name, &project)?;
        cfg.projects.insert(name.to_string(), project.clone());
        Ok(())
    })
    .map(|_| ())
}

/// 删除项目，同时从默认项目和各群的项目列表里去掉（否则准备工作区时会引用到不存在的项目）。
pub fn remove_project_impl(name: &str) -> Result<(), String> {
    config::update(|cfg| {
        cfg.projects.remove(name);
        cfg.default_projects.retain(|p| p != name);
        for bot in &mut cfg.bots {
            for g in &mut bot.groups {
                g.projects.retain(|p| p != name);
            }
        }
        Ok(())
    })
    .map(|_| ())
}

/// 数据源别名：字母、数字、下划线、短横线，1～64 个字符（会出现在 `--db-query <别名>` 命令行和 keyring 用户名里）。
pub fn valid_alias(alias: &str) -> bool {
    (1..=64).contains(&alias.len()) && alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn check_alias(alias: &str) -> Result<(), String> {
    if valid_alias(alias) {
        Ok(())
    } else {
        Err(format!("数据源别名不合法：{alias:?}（只能用字母、数字、下划线、短横线，最长 64 个字符）"))
    }
}

/// 直连超时（秒）、最多行数的允许范围：和前端表单同一组数。端口是 u16，只需排除 0。
pub const TIMEOUT_SECS_RANGE: std::ops::RangeInclusive<u32> = 1..=300;
pub const MAX_ROWS_RANGE: std::ops::RangeInclusive<u32> = 1..=10000;

/// 去掉首尾空白并校验数据源字段。不碰密码。
pub fn normalize_datasource(ds: &mut DataSource) -> Result<(), String> {
    ds.description = ds.description.trim().to_string();
    match &mut ds.access {
        Access::Direct(c) => {
            if ds.kind == DbKind::Other {
                return Err("「其他」类型的库不支持直连，请改用技能方式查询".into());
            }
            c.host = c.host.trim().to_string();
            c.user = c.user.trim().to_string();
            c.database = c.database.trim().to_string();
            if c.host.is_empty() {
                return Err("主机不能为空".into());
            }
            if c.port == 0 {
                return Err("端口要在 1～65535 之间".into());
            }
            if c.user.is_empty() {
                return Err("账号不能为空".into());
            }
            if c.database.is_empty() {
                return Err("库名不能为空".into());
            }
            if !TIMEOUT_SECS_RANGE.contains(&c.timeout_secs) {
                return Err(format!("超时要在 {}～{} 秒之间", TIMEOUT_SECS_RANGE.start(), TIMEOUT_SECS_RANGE.end()));
            }
            if !MAX_ROWS_RANGE.contains(&c.max_rows) {
                return Err(format!("最多行数要在 {}～{} 之间", MAX_ROWS_RANGE.start(), MAX_ROWS_RANGE.end()));
            }
            if let Some(t) = &mut c.ssh {
                t.host = t.host.trim().to_string();
                t.user = t.user.trim().to_string();
                t.key_path = t.key_path.trim().to_string();
                if t.host.is_empty() || t.user.is_empty() || t.key_path.is_empty() {
                    return Err("SSH 跳板的主机、账号、私钥路径都要填写".into());
                }
                if t.port == 0 {
                    return Err("SSH 端口要在 1～65535 之间".into());
                }
            }
        }
        Access::Skill(r) => {
            r.skill = r.skill.trim().to_string();
            r.database = r.database.trim().to_string();
            r.note = r.note.trim().to_string();
            if r.skill.is_empty() {
                return Err("查询技能不能为空".into());
            }
        }
    }
    Ok(())
}

/// 空串视同没填。
fn non_empty(pw: Option<String>) -> Option<String> {
    pw.filter(|p| !p.is_empty())
}

/// 保存数据源（新增或修改）。密码只进 keyring：`password` 为 None / 空串时保留原密码；
/// 改成技能方式时删掉已保存的密码（技能方式用不到）。
pub fn save_datasource_impl(alias: &str, mut ds: DataSource, password: Option<String>) -> Result<(), String> {
    let alias = alias.trim();
    check_alias(alias)?;
    normalize_datasource(&mut ds)?;
    let password = non_empty(password);
    let direct = matches!(ds.access, Access::Direct(_));
    // 先存密码再写配置：密码存失败时配置不变
    if direct && let Some(pw) = password.as_deref() {
        secrets::set_db_password(alias, pw)?;
    }
    config::update(|cfg| {
        cfg.dbs.insert(alias.to_string(), ds.clone());
        Ok(())
    })?;
    if !direct && let Err(e) = secrets::delete_db_password(alias) {
        eprintln!("beetle: 清理数据源 {alias} 的密码失败：{e}");
    }
    Ok(())
}

/// 删除数据源：被项目引用时拒绝；删掉配置后同时删 keyring 里的密码。
pub fn delete_datasource_impl(alias: &str) -> Result<(), String> {
    config::update(|cfg| {
        if let Some((name, _)) = cfg.projects.iter().find(|(_, p)| p.dbs.iter().any(|d| d == alias)) {
            return Err(format!("数据源 {alias} 被项目 {name} 使用，先在项目里取消关联"));
        }
        if cfg.dbs.remove(alias).is_none() {
            return Err(format!("找不到数据源 {alias}"));
        }
        Ok(())
    })?;
    secrets::delete_db_password(alias).map_err(|e| format!("数据源已删除，但清除密码失败：{e}"))
}

/// 测试连接要用的数据源和密码：只支持直连；`password` 为空时用 `alias` 在 keyring 里已保存的密码。
pub fn prepare_test_datasource(
    mut ds: DataSource,
    alias: Option<String>,
    password: Option<String>,
) -> Result<(DataSource, Option<String>), String> {
    if !matches!(ds.access, Access::Direct(_)) {
        return Err("技能方式的数据源由技能自己连库，客户端无法测试连接".into());
    }
    normalize_datasource(&mut ds)?;
    let password = match non_empty(password) {
        Some(p) => Some(p),
        None => match alias.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) => secrets::db_password(a)?,
            None => None,
        },
    };
    Ok((ds, password))
}

/// 扫 `~/.claude/skills/*/SKILL.md`、`~/.codex/skills/*/SKILL.md`，按位置、名称排序。读不到的目录跳过。
pub fn list_local_skills_impl() -> Vec<LocalSkill> {
    let mut out = Vec::new();
    for (target, location) in [(crate::catalog::Target::Claude, "claude"), (crate::catalog::Target::Codex, "codex")] {
        let Ok(rd) = std::fs::read_dir(target.skills_root()) else { continue };
        let mut found: Vec<LocalSkill> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let md = std::fs::read_to_string(e.path().join("SKILL.md")).ok()?;
                Some(LocalSkill {
                    name: e.file_name().to_string_lossy().into_owned(),
                    location,
                    description: skills::frontmatter_description(&md),
                })
            })
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        out.extend(found);
    }
    out
}

pub fn set_default_projects_impl(names: Vec<String>) -> Result<(), String> {
    config::update(|cfg| {
        if let Some(missing) = names.iter().find(|n| !cfg.projects.contains_key(*n)) {
            return Err(format!("项目「{missing}」不存在"));
        }
        cfg.default_projects = names.clone();
        Ok(())
    })
    .map(|_| ())
}

fn require_skill(name: &str) -> Result<(), String> {
    if skills::list_all().iter().any(|s| s.name == name) {
        Ok(())
    } else {
        Err(format!("找不到技能 {name}"))
    }
}

/// 读技能清单里的 `settings_schema.fields`；没有该段的技能返回空列表。
fn schema_fields(name: &str) -> Vec<serde_json::Value> {
    let Some(dir) = skills::skill_dir(name) else { return Vec::new() };
    let Some(text) = skills::read_manifest_text(&dir) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else {
        return Vec::new();
    };
    v.pointer("/settings_schema/fields").and_then(|f| f.as_array()).cloned().unwrap_or_default()
}

pub fn skill_settings_impl(name: &str) -> Result<SkillSettings, String> {
    require_skill(name)?;
    let cfg = config::load()?;
    Ok(SkillSettings {
        fields: schema_fields(name),
        value: cfg.skill_settings.get(name).cloned().unwrap_or_else(|| serde_json::json!({})),
    })
}

pub fn save_skill_settings_impl(name: &str, value: serde_json::Value) -> Result<(), String> {
    require_skill(name)?;
    if !value.is_object() {
        return Err("技能设置必须是一个对象".into());
    }
    config::update(|cfg| {
        cfg.skill_settings.insert(name.to_string(), value.clone());
        Ok(())
    })
    .map(|_| ())
}

/// 首次运行（配置里还没有自启标记）默认开启开机自启，并记下来；用户关掉后是 `Some(false)`，不会再被打开。
/// 返回 true 表示这次需要去系统里启用自启。
pub fn first_run_autostart(cfg: &mut AppConfig) -> bool {
    if cfg.autostart.is_none() {
        cfg.autostart = Some(true);
        true
    } else {
        false
    }
}

/// 首次运行去系统里开启自启失败时调用：撤回「已初始化」标记，下次启动再试（第一阶段遗留：失败后不再重试）。
/// 用户自己关掉的（`Some(false)`）不动。
pub fn autostart_enable_failed(cfg: &mut AppConfig) {
    if cfg.autostart == Some(true) {
        cfg.autostart = None;
    }
}

// ───────────────────────── 事件出口 ─────────────────────────

/// supervisor 事件 → 前端事件；机器人状态变化时顺带刷新托盘菜单。
pub struct TauriSink {
    pub app: AppHandle,
}

impl EventSink for TauriSink {
    fn bot_state(&self, event: BotStateEvent) {
        let _ = self.app.emit(EVENT_BOT_STATE, &event);
        // 菜单重建放到主线程异步做：回调里不同步读 supervisor 状态 / 配置，也不阻塞发事件的线程
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || tray::refresh(&app));
    }

    fn run_updated(&self, event: RunUpdatedEvent) {
        let _ = self.app.emit(EVENT_RUN_UPDATED, &event);
    }

    fn run_step(&self, event: RunStepEvent) {
        let _ = self.app.emit(EVENT_RUN_STEP, &event);
    }
}

// ───────────────────────── Tauri 命令 ─────────────────────────
// Tauri v2 的同步命令跑在主线程；凭据库、SQLite、git 都可能阻塞，所以命令一律 async，
// 阻塞部分放进 `blocking`（spawn_blocking）里，不卡界面事件循环。

/// 把阻塞操作放到阻塞线程池执行。
pub(crate) async fn blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| format!("后台任务异常：{e}"))?
}

#[tauri::command]
pub async fn list_bots(state: State<'_, AppState>) -> Result<Vec<BotView>, String> {
    let (supervisor, store) = (state.supervisor.clone(), state.store.clone());
    blocking(move || {
        let cfg = config::load()?;
        let recent = store.list_runs(None, TODAY_SCAN_LIMIT)?;
        let today = count_today(&recent, local_midnight_ms());
        Ok(build_bot_views(&cfg, &|id| supervisor.state(id), &|id| supervisor.is_alive(id), &today))
    })
    .await
}

/// `app_secret`：手填的密钥；`attempt`：扫码创建应用时的尝试 id（密钥由后端暂存，不经过界面）。
#[tauri::command]
pub async fn save_bot(app: AppHandle, bot: Bot, app_secret: Option<String>, attempt: Option<String>) -> Result<Bot, String> {
    let saved = blocking(move || {
        let typed = app_secret.filter(|s| !s.trim().is_empty());
        let secret = match (typed, attempt) {
            (Some(s), _) => Some(s),
            (None, Some(a)) if !a.is_empty() => Some(crate::feishu::registration::registered_secret(&a, bot.app_id.trim())?),
            _ => None,
        };
        save_bot_impl(bot, secret)
    })
    .await?;
    tray::refresh(&app);
    Ok(saved)
}

#[tauri::command]
pub async fn delete_bot(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let supervisor = state.supervisor.clone();
    let result = blocking(move || {
        supervisor.stop(&id);
        delete_bot_impl(&id)
    })
    .await;
    tray::refresh(&app);
    result
}

#[tauri::command]
pub async fn start_bot(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let supervisor = state.supervisor.clone();
    // 环境不满足（缺 Node / Git、飞书未登录……）时不启动，直接把缺的项说清楚。托盘、开机自启走同一个入口。
    blocking(move || supervisor.start_checked(&id, &crate::envcheck::ensure_ready)).await
}

#[tauri::command]
pub async fn stop_bot(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let supervisor = state.supervisor.clone();
    blocking(move || {
        supervisor.stop(&id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn set_start_with_client(id: String, on: bool) -> Result<(), String> {
    blocking(move || set_start_with_client_impl(&id, on)).await
}

#[tauri::command]
pub async fn save_group(app: AppHandle, bot_id: String, group: Group) -> Result<(), String> {
    blocking(move || save_group_impl(&bot_id, group)).await?;
    tray::refresh(&app);
    Ok(())
}

#[tauri::command]
pub async fn remove_group(app: AppHandle, bot_id: String, chat_id: String) -> Result<(), String> {
    blocking(move || remove_group_impl(&bot_id, &chat_id)).await?;
    tray::refresh(&app);
    Ok(())
}

#[tauri::command]
pub async fn list_projects() -> Result<ProjectsView, String> {
    blocking(|| {
        let cfg = config::load()?;
        Ok(ProjectsView { projects: cfg.projects, dbs: cfg.dbs, default_projects: cfg.default_projects })
    })
    .await
}

/// `git ls-remote` 最长 30 秒，不卡界面。
#[tauri::command]
pub async fn save_project(name: String, project: Project) -> Result<(), String> {
    blocking(move || save_project_impl(&name, project)).await
}

#[tauri::command]
pub async fn remove_project(name: String) -> Result<(), String> {
    blocking(move || remove_project_impl(&name)).await
}

/// `password`：None / 空串 = 保留已保存的密码。
#[tauri::command]
pub async fn save_datasource(alias: String, ds: DataSource, password: Option<String>) -> Result<(), String> {
    blocking(move || save_datasource_impl(&alias, ds, password)).await
}

#[tauri::command]
pub async fn delete_datasource(alias: String) -> Result<(), String> {
    blocking(move || delete_datasource_impl(&alias)).await
}

/// 跑 `SELECT 1`（Mongo `ping`），成功返回「连接成功，耗时 x ms」。读 keyring 放阻塞线程，连库本身是异步的。
#[tauri::command]
pub async fn test_datasource(ds: DataSource, alias: Option<String>, password: Option<String>) -> Result<String, String> {
    let (ds, password) = blocking(move || prepare_test_datasource(ds, alias, password)).await?;
    crate::dbquery::test_connection(&ds, password.as_deref()).await
}

#[tauri::command]
pub async fn list_local_skills() -> Result<Vec<LocalSkill>, String> {
    blocking(|| Ok(list_local_skills_impl())).await
}

#[tauri::command]
pub async fn set_default_projects(names: Vec<String>) -> Result<(), String> {
    blocking(move || set_default_projects_impl(names)).await
}

#[tauri::command]
pub async fn list_runs(state: State<'_, AppState>, bot_id: Option<String>, limit: u32) -> Result<Vec<RunRecord>, String> {
    let store = state.store.clone();
    blocking(move || store.list_runs(bot_id.as_deref(), limit.clamp(1, RUNS_LIMIT_MAX))).await
}

#[tauri::command]
pub async fn get_run(state: State<'_, AppState>, id: String) -> Result<RunDetail, String> {
    let store = state.store.clone();
    blocking(move || {
        let (run, steps) = store.get_run(&id)?.ok_or_else(|| format!("找不到运行记录 {id}"))?;
        Ok(RunDetail { run, steps })
    })
    .await
}

#[tauri::command]
pub async fn cancel_run(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let supervisor = state.supervisor.clone();
    blocking(move || {
        if supervisor.cancel_run(&id) {
            Ok(())
        } else {
            Err("这次运行已经结束，或不在本机的队列里，无法取消".into())
        }
    })
    .await
}

/// 失败时的中文原因（例如「只能重试本次客户端启动后收到的运行」）原样交给界面显示。
#[tauri::command]
pub async fn retry_run(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let supervisor = state.supervisor.clone();
    blocking(move || supervisor.retry_run(&id)).await
}

#[tauri::command]
pub async fn list_skills() -> Result<Vec<SkillInfoView>, String> {
    blocking(|| {
        Ok(skills::list_all()
            .into_iter()
            .map(|s| SkillInfoView {
                name: s.name,
                display_name: s.display_name,
                description: s.description,
                requires: s.requires,
                builtin: s.builtin,
            })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn get_skill_settings(name: String) -> Result<SkillSettings, String> {
    blocking(move || skill_settings_impl(&name)).await
}

#[tauri::command]
pub async fn save_skill_settings(name: String, value: serde_json::Value) -> Result<(), String> {
    blocking(move || save_skill_settings_impl(&name, value)).await
}

#[tauri::command]
pub async fn get_autostart() -> Result<bool, String> {
    blocking(|| Ok(config::load()?.autostart.unwrap_or(false))).await
}

#[tauri::command]
pub async fn set_autostart(app: AppHandle, on: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    blocking(move || {
        let launcher = app.autolaunch();
        if on {
            launcher.enable().map_err(|e| format!("开启开机自启失败：{e}"))?;
        } else {
            launcher.disable().map_err(|e| format!("关闭开机自启失败：{e}"))?;
        }
        config::update(|cfg| {
            cfg.autostart = Some(on);
            Ok(())
        })
        .map(|_| ())
    })
    .await
}

/// 运行记录库路径（`runs.db`）。
pub fn runs_db_path() -> std::path::PathBuf {
    paths::config_dir().join("runs.db")
}

#[cfg(test)]
mod tests;
