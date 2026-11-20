//! 第二三阶段新增的 Tauri 命令（接口表见 docs/superpowers/plans/2026-11-19-phase23-ui-onboarding.md）。
//! 与 `commands.rs` 一样：命令是薄封装，逻辑在 `*_impl` 里，可脱离 Tauri 单测。

use crate::commands::blocking;
use crate::envcheck::{self, EnvItem, Probe, SystemProbe, install};
use crate::runner;
use crate::store::config::{self, Accent, AppearanceMode, Appearance};
use crate::store::runs::{RunRecord, RunStatus, RunStore};
use crate::templates::{self, TemplateVars};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

pub const EVENT_ENV_INSTALL_LOG: &str = "env-install-log";
pub const EVENT_ENV_INSTALL_DONE: &str = "env-install-done";

/// 原始日志最多返回末尾多少字节。
pub const RAW_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;
const BOT_STATS_SCAN_LIMIT: u32 = 2000;

// ───────────────────────── B1：环境检查 ─────────────────────────

#[derive(Debug, Clone, Serialize)]
struct InstallLog<'a> {
    id: &'a str,
    /// 与 `id` 相同；plan 的 Global Constraints 里写的是 `item`，两个字段都给，前端任取其一。
    item: &'a str,
    line: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct InstallDone<'a> {
    id: &'a str,
    ok: bool,
    message: &'a str,
}

#[tauri::command]
pub async fn env_check() -> Result<Vec<EnvItem>, String> {
    blocking(|| {
        let probe: Arc<dyn Probe> = Arc::new(SystemProbe);
        Ok(envcheck::check_all(&probe))
    })
    .await
}

/// 每个机器人「哪些环境项会阻止启动」的规则（基础项 + 所选引擎 + 启用技能的 requires），前端按它和 `env_check`
/// 的结果算阻断项，与后端 `start_bot` / 托盘 / 开机自启的 `ensure_ready` 同一套规则。
#[tauri::command]
pub async fn env_requirements() -> Result<envcheck::EnvRequirements, String> {
    blocking(|| Ok(envcheck::requirements(&config::load()?, &crate::skills::list_all()))).await
}

/// 执行安装 / 登录。过程经 `env-install-log`，结束发 `env-install-done`；失败也是 Ok（失败原因在 done 事件里），
/// 只有传入未知检查项 id 才返回 Err。
#[tauri::command]
pub async fn env_fix(app: AppHandle, id: String) -> Result<(), String> {
    if !envcheck::ALL_IDS.contains(&id.as_str()) {
        return Err(format!("未知的检查项：{id}"));
    }
    blocking(move || {
        let result = install::fix(&id, &SystemProbe, &install::SystemRunner, install::Os::current(), &mut |line| {
            let _ = app.emit(EVENT_ENV_INSTALL_LOG, InstallLog { id: &id, item: &id, line });
        });
        let (ok, message) = match &result {
            Ok(m) => (true, m.as_str()),
            Err(m) => (false, m.as_str()),
        };
        let _ = app.emit(EVENT_ENV_INSTALL_DONE, InstallDone { id: &id, ok, message });
        Ok(())
    })
    .await
}

// ───────────────────────── B2：模板 / 原始日志 / 外观 / 统计 ─────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TemplateInfo {
    pub name: String,
    pub builtin: bool,
    /// 用户目录里有这份模板（内置模板被改过 / 自定义模板）。
    pub modified: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawLog {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BotStats {
    pub today_runs: u32,
    pub today_replied: u32,
    /// 0–1：已回复 / （已回复 + 失败 + 超时）。取消的运行不算失败；还没有结束的运行时为 0。
    pub success_rate: f64,
    pub avg_duration_ms: u64,
}

fn user_template_path(name: &str) -> std::path::PathBuf {
    templates::user_dir().join(format!("{name}.md"))
}

pub fn list_templates_impl() -> Result<Vec<TemplateInfo>, String> {
    let mut custom: Vec<String> = Vec::new();
    let mut user_files: Vec<String> = Vec::new();
    match std::fs::read_dir(templates::user_dir()) {
        Ok(rd) => {
            for e in rd.flatten() {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "md")
                    && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                    && templates::check_name(stem).is_ok()
                {
                    user_files.push(stem.to_string());
                    if !templates::BUILTIN_NAMES.contains(&stem) {
                        custom.push(stem.to_string());
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("读取模板目录失败：{e}")),
    }
    custom.sort();
    let names = templates::BUILTIN_NAMES.iter().map(|s| s.to_string()).chain(custom);
    Ok(names
        .map(|name| TemplateInfo {
            builtin: templates::builtin(&name).is_some(),
            modified: user_files.contains(&name),
            name,
        })
        .collect())
}

fn sample_vars() -> TemplateVars {
    let mut v = TemplateVars::default();
    for k in templates::VAR_KEYS {
        v.0.insert(k, "x".to_string());
    }
    v
}

/// 模板能否保存：名称合法、不为空，且用全量变量渲染一遍没有未知变量
/// （在保存时报错，而不是等到机器人收到消息才炸）。
pub fn validate_template(name: &str, content: &str) -> Result<(), String> {
    templates::check_name(name)?;
    if content.trim().is_empty() {
        return Err("模板内容不能为空".into());
    }
    templates::render(content, &sample_vars()).map(|_| ())
}

pub fn save_template_impl(name: &str, content: &str) -> Result<(), String> {
    validate_template(name, content)?;
    let dir = templates::user_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建模板目录失败：{e}"))?;
    let path = user_template_path(name);
    let tmp = dir.join(format!("{name}.md.{}.tmp", std::process::id()));
    std::fs::write(&tmp, content).map_err(|e| format!("写入模板失败：{e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("保存模板失败：{e}")
    })
}

/// 删除用户版本，回到内置；没有用户版本时什么都不做。自定义模板（无内置版本）会被直接删除。
pub fn reset_template_impl(name: &str) -> Result<(), String> {
    templates::check_name(name)?;
    match std::fs::remove_file(user_template_path(name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("恢复内置模板失败：{e}")),
    }
}

/// 读文件末尾最多 `max` 字节；被截断时丢掉开头不完整的那一行。文件不存在返回空串。
fn read_tail(path: &Path, max: u64) -> Result<String, String> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(format!("打开日志 {} 失败：{e}", path.display())),
    };
    let len = f.metadata().map_err(|e| format!("读取日志大小失败：{e}"))?.len();
    let truncated = len > max;
    if truncated {
        f.seek(SeekFrom::Start(len - max)).map_err(|e| format!("读取日志失败：{e}"))?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(|e| format!("读取日志失败：{e}"))?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if truncated && let Some(i) = text.find('\n') {
        text.drain(..=i);
    }
    Ok(text)
}

pub fn get_raw_log_impl(run_id: &str) -> Result<RawLog, String> {
    if run_id.is_empty() || !run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("运行 id 不合法：{run_id}"));
    }
    Ok(RawLog {
        stdout: read_tail(&runner::raw_log_path(run_id), RAW_LOG_MAX_BYTES)?,
        stderr: read_tail(&runner::stderr_log_path(run_id), RAW_LOG_MAX_BYTES)?,
    })
}

pub fn set_appearance_impl(a: Appearance) -> Result<(), String> {
    config::update(|cfg| {
        cfg.appearance = a;
        Ok(())
    })
    .map(|_| ())
}

/// 统计 `started_at >= midnight_ms` 的运行。
pub fn bot_stats_from(records: &[RunRecord], midnight_ms: i64) -> BotStats {
    let today: Vec<&RunRecord> = records.iter().filter(|r| r.started_at >= midnight_ms).collect();
    let replied = today.iter().filter(|r| r.status == RunStatus::Replied).count() as u32;
    let bad = today.iter().filter(|r| matches!(r.status, RunStatus::Failed | RunStatus::Timeout)).count() as u32;
    let durations: Vec<u64> = today
        .iter()
        .filter(|r| r.status == RunStatus::Replied)
        .filter_map(|r| r.finished_at.map(|f| (f - r.started_at).max(0) as u64))
        .collect();
    BotStats {
        today_runs: today.len() as u32,
        today_replied: replied,
        success_rate: if replied + bad == 0 { 0.0 } else { replied as f64 / (replied + bad) as f64 },
        avg_duration_ms: if durations.is_empty() { 0 } else { durations.iter().sum::<u64>() / durations.len() as u64 },
    }
}

pub fn bot_stats_impl(store: &RunStore, bot_id: &str, midnight_ms: i64) -> Result<BotStats, String> {
    Ok(bot_stats_from(&store.list_runs(Some(bot_id), BOT_STATS_SCAN_LIMIT)?, midnight_ms))
}

#[tauri::command]
pub async fn list_templates() -> Result<Vec<TemplateInfo>, String> {
    blocking(list_templates_impl).await
}

#[tauri::command]
pub async fn get_template(name: String) -> Result<String, String> {
    blocking(move || templates::load(&name)).await
}

#[tauri::command]
pub async fn save_template(name: String, content: String) -> Result<(), String> {
    blocking(move || save_template_impl(&name, &content)).await
}

#[tauri::command]
pub async fn reset_template(name: String) -> Result<(), String> {
    blocking(move || reset_template_impl(&name)).await
}

#[tauri::command]
pub async fn get_raw_log(run_id: String) -> Result<RawLog, String> {
    blocking(move || get_raw_log_impl(&run_id)).await
}

#[tauri::command]
pub async fn get_appearance() -> Result<Appearance, String> {
    blocking(|| Ok(config::load()?.appearance)).await
}

/// 两种传法都接受：`{appearance: {mode, accent}}` 或直接 `{mode, accent}`（缺的字段沿用当前值）。
#[tauri::command]
pub async fn set_appearance(
    appearance: Option<Appearance>,
    mode: Option<AppearanceMode>,
    accent: Option<Accent>,
) -> Result<(), String> {
    blocking(move || {
        let current = config::load()?.appearance;
        let next = appearance.unwrap_or(Appearance {
            mode: mode.unwrap_or(current.mode),
            accent: accent.unwrap_or(current.accent),
        });
        set_appearance_impl(next)
    })
    .await
}

// ───────────────────────── 设置页「运行」：代理、代码缓存目录、日志保留天数 ─────────────────────────

/// 设置页读写的运行设置。空字符串 / None 表示用默认值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSettings {
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub code_cache_dir: Option<String>,
    pub log_retention_days: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuntimeSettingsView {
    pub proxy: Option<String>,
    pub code_cache_dir: Option<String>,
    pub log_retention_days: u32,
    /// 不填代码缓存目录时实际使用的目录（界面占位提示）
    pub default_code_cache_dir: String,
    pub min_retention_days: u32,
    pub max_retention_days: u32,
}

pub fn runtime_settings_view(cfg: &config::AppConfig) -> RuntimeSettingsView {
    RuntimeSettingsView {
        proxy: cfg.proxy.clone().filter(|p| !p.trim().is_empty()),
        code_cache_dir: cfg.code_cache_dir.clone().filter(|p| !p.trim().is_empty()),
        log_retention_days: cfg.log_retention_days(),
        default_code_cache_dir: crate::platform::paths::code_cache_dir().to_string_lossy().replace('\\', "/"),
        min_retention_days: config::LOG_RETENTION_RANGE.0,
        max_retention_days: config::LOG_RETENTION_RANGE.1,
    }
}

fn clean(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// 代理只接受 `http://` `https://` `socks5://` `socks5h://` 加 主机[:端口]。
fn validate_proxy(p: &str) -> Result<(), String> {
    let scheme = p.split_once("://").map(|(s, _)| s.to_ascii_lowercase());
    let ok_scheme = scheme.as_deref().is_some_and(|s| matches!(s, "http" | "https" | "socks5" | "socks5h"));
    if !ok_scheme || envcheck::parse_host_port(p).is_none() {
        return Err(format!("代理地址不合法：{p}（例如 http://127.0.0.1:7890）"));
    }
    Ok(())
}

/// 校验并保存运行设置（走 `config::update`）。代码缓存目录必须是绝对路径，保存前先建好目录确认可写。
pub fn set_runtime_settings_impl(s: RuntimeSettings) -> Result<(), String> {
    let proxy = clean(s.proxy);
    if let Some(p) = &proxy {
        validate_proxy(p)?;
    }
    let dir = clean(s.code_cache_dir);
    if let Some(d) = &dir {
        let path = Path::new(d);
        if !path.is_absolute() {
            return Err(format!("代码缓存目录必须是完整路径（例如 D:/beetle/code）：{d}"));
        }
        std::fs::create_dir_all(path).map_err(|e| format!("无法创建代码缓存目录 {d}：{e}"))?;
    }
    let (min, max) = config::LOG_RETENTION_RANGE;
    if !(min..=max).contains(&s.log_retention_days) {
        return Err(format!("日志保留天数要在 {min}～{max} 之间"));
    }
    config::update(|cfg| {
        cfg.proxy = proxy.clone();
        cfg.code_cache_dir = dir.clone().map(|d| d.replace('\\', "/"));
        cfg.log_retention_days = Some(s.log_retention_days);
        Ok(())
    })
    .map(|_| ())
}

#[tauri::command]
pub async fn get_runtime_settings() -> Result<RuntimeSettingsView, String> {
    blocking(|| Ok(runtime_settings_view(&config::load()?))).await
}

/// 保存后立刻按新的保留天数清理一次（客户端每次启动也会按它清理）。代理、代码缓存目录从机器人下一次运行起生效。
#[tauri::command]
pub async fn set_runtime_settings(state: tauri::State<'_, crate::commands::AppState>, settings: RuntimeSettings) -> Result<(), String> {
    let store = state.store.clone();
    blocking(move || {
        let days = settings.log_retention_days;
        set_runtime_settings_impl(settings)?;
        store.purge_older_than(days)?;
        runner::purge_raw_logs(days);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn bot_stats(state: tauri::State<'_, crate::commands::AppState>, bot_id: String) -> Result<BotStats, String> {
    let store = state.store.clone();
    blocking(move || bot_stats_impl(&store, &bot_id, crate::commands::local_midnight_ms())).await
}

// ───────────────────────── B3：导入导出、旧版迁移、导入技能 ─────────────────────────

#[tauri::command]
pub async fn export_config(path: String) -> Result<(), String> {
    blocking(move || crate::portable::export_config_impl(Path::new(&path))).await
}

#[tauri::command]
pub async fn preview_import(path: String) -> Result<crate::portable::ImportPreview, String> {
    blocking(move || crate::portable::preview_import_impl(Path::new(&path))).await
}

#[tauri::command]
pub async fn apply_import(path: String, select: crate::portable::ImportSelect) -> Result<(), String> {
    blocking(move || crate::portable::apply_import_impl(Path::new(&path), &select)).await
}

#[tauri::command]
pub async fn detect_legacy() -> Result<crate::migrate::LegacyInfo, String> {
    blocking(|| {
        let files = crate::migrate::LegacyFiles::from_home()?;
        crate::migrate::detect_legacy_impl(&files, &crate::platform::legacy::SystemLegacy)
    })
    .await
}

/// `stop_bridge` 对应前端的 `stopBridge`（Tauri 自动把 camelCase 转成 snake_case）。
#[tauri::command]
pub async fn migrate_legacy(app: AppHandle, stop_bridge: bool) -> Result<crate::migrate::MigrateResult, String> {
    let result = blocking(move || {
        let files = crate::migrate::LegacyFiles::from_home()?;
        let getter = crate::platform::legacy::BridgeSecretGetter { script: files.getter_script() };
        crate::migrate::migrate_legacy_impl(
            &files,
            &crate::platform::legacy::SystemLegacy,
            &getter,
            &crate::store::secrets::set_app_secret,
            stop_bridge,
        )
    })
    .await;
    crate::tray::refresh(&app);
    result
}

#[tauri::command]
pub async fn import_local_skill(dir: String) -> Result<crate::commands::SkillInfoView, String> {
    blocking(move || {
        let s = crate::skills::import_local(Path::new(&dir))?;
        Ok(crate::commands::SkillInfoView {
            name: s.name,
            display_name: s.display_name,
            description: s.description,
            requires: s.requires,
            builtin: s.builtin,
        })
    })
    .await
}

#[cfg(test)]
mod tests;
