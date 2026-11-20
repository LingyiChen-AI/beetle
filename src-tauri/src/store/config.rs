//! 应用配置：`<config_dir>/config.json`。不含任何密钥（密钥见 `secrets`）。

use crate::platform::paths;
use crate::store::datasource::DataSource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Owner,
    All,
}

/// 机器人在群里怎样被触发：`mention` = @ 机器人（默认，旧配置没有这个字段时也是它）；
/// `pr_link` = 群里任何消息带 Gitee / GitHub 的 PR 链接就触发，不用 @；`both` = 两种都行。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerMode {
    #[default]
    Mention,
    PrLink,
    Both,
}

impl TriggerMode {
    pub fn accepts_mention(self) -> bool {
        matches!(self, TriggerMode::Mention | TriggerMode::Both)
    }

    pub fn accepts_pr_link(self) -> bool {
        matches!(self, TriggerMode::PrLink | TriggerMode::Both)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub chat_id: String,
    pub name: String,
    pub slug: String,
    pub projects: Vec<String>,
    pub doc_url: Option<String>,
    pub trigger: Trigger,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub app_id: String,
    pub engine: Engine,
    pub start_with_client: bool,
    pub timeout_minutes: u32,
    pub concurrency: u32,
    pub template: String,
    pub skills: Vec<String>,
    pub extra_deny: Vec<String>,
    pub auto_onboard: bool,
    pub groups: Vec<Group>,
    /// 触发方式；旧配置没有这个字段 = 只认 @。
    #[serde(default)]
    pub trigger_mode: TriggerMode,
}

/// 新机器人的默认技能列表。
pub const DEFAULT_SKILLS: [&str; 5] = ["chat-context", "code-sync", "db-query", "issue-record", "feishu-doc-record"];

/// 代码审查模板名：选它的机器人最终回复是结构化 JSON，客户端渲染成卡片发到群里。
pub const CODE_REVIEW_TEMPLATE: &str = "code-review";

impl Bot {
    pub fn new_default(id: &str, name: &str, app_id: &str) -> Self {
        Bot {
            id: id.to_string(),
            name: name.to_string(),
            app_id: app_id.to_string(),
            engine: Engine::Claude,
            start_with_client: false,
            timeout_minutes: 30,
            concurrency: 2,
            template: "bug-hunt".to_string(),
            skills: DEFAULT_SKILLS
                .map(String::from)
                .to_vec(),
            extra_deny: Vec::new(),
            auto_onboard: true,
            groups: Vec::new(),
            trigger_mode: TriggerMode::Mention,
        }
    }

    /// 代码审查机器人的默认值：群里贴 PR 链接自动触发，技能用 PR 审查 + 代码同步。
    pub fn new_review(id: &str, name: &str, app_id: &str) -> Self {
        Bot {
            template: CODE_REVIEW_TEMPLATE.to_string(),
            skills: ["pr-review", "code-sync"].map(String::from).to_vec(),
            trigger_mode: TriggerMode::PrLink,
            ..Bot::new_default(id, name, app_id)
        }
    }

    pub fn is_code_review(&self) -> bool {
        self.template == CODE_REVIEW_TEMPLATE
    }
}

fn yes() -> bool {
    true
}

/// 查 bug 找到问题后的记录方式，可同时选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordOpts {
    /// 在项目仓库（Gitee / GitHub）建 issue；同标题已存在则追加评论。
    #[serde(default)]
    pub issue: bool,
    /// 追加到群配置的飞书文档。
    #[serde(default = "yes")]
    pub feishu_doc: bool,
    /// 用飞书项目 CLI（meegle）在配置好的空间里建工单；同名工单已存在则追加评论。
    #[serde(default)]
    pub meegle: bool,
    /// 用 lark-cli 以机器人主人的身份建飞书任务（可选放进任务清单、指定负责人）。
    #[serde(default)]
    pub feishu_task: bool,
}

impl RecordOpts {
    /// 没配置过时的默认值：仓库在 gitee.com / github.com 就建 issue；写飞书文档保持旧行为，默认开。
    pub fn default_for(repo: &str) -> Self {
        RecordOpts { issue: crate::issue::platform_of(repo).is_some(), feishu_doc: true, meegle: false, feishu_task: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "ProjectRaw")]
pub struct Project {
    pub repo: String,
    pub branch_glob: String,
    pub rules_file: String,
    pub description: String,
    pub dbs: Vec<String>,
    /// 记录方式；旧配置没有这个字段时按仓库地址推断（见 `RecordOpts::default_for`）。
    pub record: RecordOpts,
    /// 建 issue 的仓库 `owner/repo`；None = 从仓库地址解析。
    pub issue_repo: Option<String>,
    /// 读 token 的环境变量名；None = Gitee 用 `GITEE_TOKEN`、GitHub 用 `GITHUB_TOKEN`。token 本身不进配置。
    pub token_env: Option<String>,
    /// 飞书项目工单配置；`record.meegle` 为 true 时用。
    pub meegle: Option<MeegleTarget>,
    /// 飞书任务配置；`record.feishu_task` 为 true 时用，None = 默认（不进清单、不指定负责人）。
    pub feishu_task: Option<FeishuTaskTarget>,
    /// 记录规则（自然语言）：勾了多种记录方式时，按问题类型挑着执行，如「调研类建飞书任务；bug 建 issue 和飞书项目工单再写文档」。
    /// None = 勾选的方式全部执行。
    pub record_rule: Option<String>,
}

/// 飞书项目工单的建单配置：在项目页登录飞书项目 CLI 后，从 CLI 拉空间、工单类型、必填字段选出来。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeegleTarget {
    /// 空间 project_key
    pub project_key: String,
    /// 空间短名（拼工单链接用），没有时用 project_key
    #[serde(default)]
    pub simple_name: String,
    #[serde(default)]
    pub space_name: String,
    /// 工单类型 key，默认 issue（缺陷）
    #[serde(default = "default_work_item_type")]
    pub work_item_type: String,
    #[serde(default)]
    pub work_item_type_name: String,
    /// 建单时固定带上的字段（必填下拉等），值已是 CLI 建单要的格式（选项 id，或多选的 JSON 数组字符串）
    #[serde(default)]
    pub fields: Vec<MeegleField>,
    /// 建单时由 AI 按问题判断的字段（如优先级、涉及端、业务线）：存可选项，机器人选好用 `--field 名称=选项` 传给脚本
    #[serde(default)]
    pub ai_fields: Vec<MeegleAiField>,
    /// 默认处理人：放进 `role` 这个角色
    #[serde(default)]
    pub assignee: Option<MeegleUser>,
    #[serde(default)]
    pub role: Option<MeegleRole>,
}

fn default_work_item_type() -> String {
    "issue".to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeegleField {
    pub field_key: String,
    #[serde(default)]
    pub field_name: String,
    pub value: String,
    /// 界面上显示的选项名
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeegleAiField {
    pub field_key: String,
    #[serde(default)]
    pub field_name: String,
    #[serde(default)]
    pub field_type: String,
    /// 可选项；空 = 文本字段，AI 直接填文字
    #[serde(default)]
    pub options: Vec<MeegleOption>,
    /// AI 没给值时用的选项 id（空间默认值）
    #[serde(default)]
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeegleOption {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeegleUser {
    pub user_key: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeegleRole {
    pub role_id: String,
    #[serde(default)]
    pub role_name: String,
}

/// 飞书任务的建任务配置：清单、负责人都可不选（不选 = 只建在主人自己名下）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeishuTaskTarget {
    #[serde(default)]
    pub tasklist_guid: String,
    #[serde(default)]
    pub tasklist_name: String,
    /// 负责人：lark-cli 应用下的 open_id
    #[serde(default)]
    pub assignee_open_id: String,
    #[serde(default)]
    pub assignee_name: String,
    /// 截止日期 = 建任务后 N 天；0 = 不设
    #[serde(default)]
    pub due_days: u32,
}

/// 反序列化用：`record` 的默认值依赖 `repo`，serde 的字段默认值做不到，先读成这个再转换。
#[derive(Deserialize)]
struct ProjectRaw {
    repo: String,
    branch_glob: String,
    rules_file: String,
    description: String,
    dbs: Vec<String>,
    #[serde(default)]
    record: Option<RecordOpts>,
    #[serde(default)]
    issue_repo: Option<String>,
    #[serde(default)]
    token_env: Option<String>,
    #[serde(default)]
    meegle: Option<MeegleTarget>,
    #[serde(default)]
    feishu_task: Option<FeishuTaskTarget>,
    #[serde(default)]
    record_rule: Option<String>,
}

impl From<ProjectRaw> for Project {
    fn from(r: ProjectRaw) -> Self {
        let blank_none = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Project {
            record: r.record.unwrap_or_else(|| RecordOpts::default_for(&r.repo)),
            repo: r.repo,
            branch_glob: r.branch_glob,
            rules_file: r.rules_file,
            description: r.description,
            dbs: r.dbs,
            issue_repo: blank_none(r.issue_repo),
            token_env: blank_none(r.token_env),
            meegle: r.meegle.filter(|m| !m.project_key.trim().is_empty()),
            feishu_task: r.feishu_task,
            record_rule: blank_none(r.record_rule),
        }
    }
}

impl Project {
    /// 新项目：记录方式按仓库地址推断，issue 仓库与 token 变量名用默认。
    pub fn new(
        repo: impl Into<String>,
        branch_glob: impl Into<String>,
        rules_file: impl Into<String>,
        description: impl Into<String>,
        dbs: Vec<String>,
    ) -> Self {
        let repo = repo.into();
        Project {
            record: RecordOpts::default_for(&repo),
            repo,
            branch_glob: branch_glob.into(),
            rules_file: rules_file.into(),
            description: description.into(),
            dbs,
            issue_repo: None,
            token_env: None,
            meegle: None,
            feishu_task: None,
            record_rule: None,
        }
    }

    /// 写进运行时配置（`beetle.runtime.json`）的项目信息：机器人工作区和技能中心独立技能共用。不含 token。
    /// `dbs` 展开成关联数据源的对象数组（见 `DataSource::runtime_value`，不含密码、主机、账号）；
    /// 已经不存在的别名跳过。
    pub fn runtime_value(&self, dbs: &BTreeMap<String, DataSource>) -> serde_json::Value {
        let dbs: Vec<serde_json::Value> = self
            .dbs
            .iter()
            .filter_map(|alias| dbs.get(alias).map(|ds| ds.runtime_value(alias)))
            .collect();
        serde_json::json!({
            "repo": self.repo,
            "branch_glob": self.branch_glob,
            "rules_file": self.rules_file,
            "description": self.description,
            "dbs": dbs,
            "record": self.record,
            "issue_repo": self.issue_repo,
            "token_env": self.token_env,
            "meegle": self.meegle,
            "feishu_task": self.feishu_task,
            "record_rule": self.record_rule,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceMode {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    Green,
    Blue,
    Indigo,
    Purple,
    Orange,
    Teal,
    #[default]
    Vermilion,
}

/// 界面外观：模式 + 主题色。默认深色 + 朱红。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub mode: AppearanceMode,
    pub accent: Accent,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub bots: Vec<Bot>,
    pub projects: BTreeMap<String, Project>,
    /// 数据源：键是别名。密码不在这里，在 keyring（见 `secrets::db_password`）。
    pub dbs: BTreeMap<String, DataSource>,
    pub skill_settings: BTreeMap<String, serde_json::Value>,
    pub default_projects: Vec<String>,
    /// 机器人主人在 lark-cli 应用下的 open_id（脚本里用 lark-cli 读回消息核对发送人时用）。
    pub owner_open_id: Option<String>,
    /// 机器人主人的 union_id：open_id 按应用区分，机器人事件里的 open_id 与 lark-cli 的不同，判断主人只比它。
    pub owner_union_id: Option<String>,
    pub proxy: Option<String>,
    /// 开机自启：None = 还没初始化过（首次运行默认开启）；Some = 用户的选择（关掉后不再自动打开）。
    pub autostart: Option<bool>,
    pub appearance: Appearance,
    /// 代码缓存目录（机器人同步代码的位置）；None / 空 = 默认 `<data>/code`。
    pub code_cache_dir: Option<String>,
    /// 运行记录与原始日志保留天数；None = 默认 30 天。客户端启动时按它清理。
    pub log_retention_days: Option<u32>,
    /// 配置格式版本：启动时按它做一次性的配置升级（见 `rebrand::upgrade_config`）。旧配置没有这个字段 = 0。
    pub config_version: u32,
}

pub const DEFAULT_LOG_RETENTION_DAYS: u32 = 30;
/// 保留天数的上下限（设置页校验用同一组数）。
pub const LOG_RETENTION_RANGE: (u32, u32) = (1, 3650);

impl AppConfig {
    /// 实际使用的代码缓存目录：配置了就用配置的，否则 `<data>/code`。
    pub fn code_cache_dir(&self) -> PathBuf {
        self.code_cache_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(paths::code_cache_dir)
    }

    /// 实际使用的日志保留天数（超出范围的旧值收敛到上下限）。
    pub fn log_retention_days(&self) -> u32 {
        self.log_retention_days
            .unwrap_or(DEFAULT_LOG_RETENTION_DAYS)
            .clamp(LOG_RETENTION_RANGE.0, LOG_RETENTION_RANGE.1)
    }
}

fn config_path() -> PathBuf {
    paths::config_dir().join("config.json")
}

/// 文件不存在返回默认配置；存在但损坏返回错误（不静默覆盖用户数据）。
pub fn load() -> Result<AppConfig, String> {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("配置文件 {} 解析失败：{e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
        Err(e) => Err(format!("读取配置文件失败：{e}")),
    }
}

static SAVE_LOCK: Mutex<()> = Mutex::new(());
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 先写临时文件再 rename，避免写到一半崩溃留下半截配置。
/// 进程内用锁串行化，临时文件名唯一；失败时清理临时文件。
pub fn save(cfg: &AppConfig) -> Result<(), String> {
    let _guard = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = config_path();
    let dir = path.parent().expect("config path has parent");
    std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
    let text = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化配置失败：{e}"))?;
    let tmp = dir.join(format!(
        "config.json.{}.{}.tmp",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(e) = std::fs::write(&tmp, text) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入临时配置失败：{e}"));
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("替换配置文件失败：{e}"));
    }
    Ok(())
}

static UPDATE_LOCK: Mutex<()> = Mutex::new(());

/// 进程内唯一的「读 → 改 → 写」入口：在进程级锁内重读配置、交给 `f` 修改、保存，返回保存后的配置。
/// `f` 返回 Err 时不保存。所有会改配置的代码（supervisor、界面命令）都必须走这里，否则会互相覆盖。
/// `f` 在锁内执行，不要在里面做慢操作（网络、lark-cli）。
pub fn update<F: FnOnce(&mut AppConfig) -> Result<(), String>>(f: F) -> Result<AppConfig, String> {
    let _guard = UPDATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = load()?;
    f(&mut cfg)?;
    save(&cfg)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_home;

    #[test]
    fn save_then_load_roundtrips_and_has_no_secret_field() {
        let _home = test_home();
        let mut cfg = AppConfig::default();
        cfg.bots.push(Bot::new_default("b1", "BUG纠察队", "cli_a7f3"));
        save(&cfg).unwrap();
        let text = std::fs::read_to_string(paths::config_dir().join("config.json")).unwrap();
        assert!(!text.to_lowercase().contains("secret"));
        assert_eq!(load().unwrap().bots[0].timeout_minutes, 30);
        assert_eq!(load().unwrap().bots[0].concurrency, 2);
        assert_eq!(load().unwrap(), cfg);
    }

    #[test]
    fn concurrent_saves_leave_valid_config_and_no_tmp_files() {
        let _home = test_home();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    for j in 0..10 {
                        let mut cfg = AppConfig::default();
                        cfg.bots.push(Bot::new_default(&format!("b{i}-{j}"), "n", "cli_x"));
                        save(&cfg).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(load().unwrap().bots.len(), 1);
        let leftovers: Vec<_> = std::fs::read_dir(paths::config_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件：{leftovers:?}");
    }

    #[test]
    fn concurrent_updates_do_not_lose_fields() {
        let _home = test_home();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    for j in 0..10 {
                        update(|c| {
                            c.bots.push(Bot::new_default(&format!("b{i}-{j}"), "n", "cli_x"));
                            Ok(())
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        update(|c| {
            c.owner_open_id = Some("ou_owner".into());
            Ok(())
        })
        .unwrap();
        for h in handles {
            h.join().unwrap();
        }
        let cfg = load().unwrap();
        assert_eq!(cfg.bots.len(), 80, "每次 update 都基于最新配置，不丢写入");
        assert_eq!(cfg.owner_open_id.as_deref(), Some("ou_owner"));
    }

    #[test]
    fn update_error_does_not_save() {
        let _home = test_home();
        update(|c| {
            c.proxy = Some("http://p".into());
            Ok(())
        })
        .unwrap();
        let err = update(|c| {
            c.proxy = None;
            Err("不改了".into())
        })
        .unwrap_err();
        assert_eq!(err, "不改了");
        assert_eq!(load().unwrap().proxy.as_deref(), Some("http://p"));
    }

    #[test]
    fn load_missing_file_returns_default() {
        let _home = test_home();
        assert_eq!(load().unwrap(), AppConfig::default());
    }

    #[test]
    fn load_corrupt_file_returns_error() {
        let _home = test_home();
        std::fs::create_dir_all(paths::config_dir()).unwrap();
        std::fs::write(paths::config_dir().join("config.json"), "{not json").unwrap();
        assert!(load().is_err());
    }

    #[test]
    fn old_config_without_appearance_defaults_to_dark_vermilion() {
        let _home = test_home();
        std::fs::create_dir_all(paths::config_dir()).unwrap();
        std::fs::write(paths::config_dir().join("config.json"), r#"{"bots": []}"#).unwrap();
        let a = load().unwrap().appearance;
        assert_eq!((a.mode, a.accent), (AppearanceMode::Dark, Accent::Vermilion));
        assert_eq!(serde_json::to_value(a).unwrap(), serde_json::json!({"mode": "dark", "accent": "vermilion"}));
    }

    #[test]
    fn code_cache_dir_and_retention_default_and_override() {
        let _home = test_home();
        let mut cfg = AppConfig::default();
        assert_eq!(cfg.code_cache_dir(), paths::code_cache_dir());
        assert_eq!(cfg.log_retention_days(), 30);
        cfg.code_cache_dir = Some("  ".into());
        assert_eq!(cfg.code_cache_dir(), paths::code_cache_dir(), "空白按默认");
        cfg.code_cache_dir = Some("D:/cache/code".into());
        assert_eq!(cfg.code_cache_dir(), PathBuf::from("D:/cache/code"));
        cfg.log_retention_days = Some(7);
        assert_eq!(cfg.log_retention_days(), 7);
        cfg.log_retention_days = Some(0);
        assert_eq!(cfg.log_retention_days(), 1, "收敛到下限");
        // 旧配置没有这两个字段也能读
        std::fs::create_dir_all(paths::config_dir()).unwrap();
        std::fs::write(paths::config_dir().join("config.json"), r#"{"proxy": "http://p"}"#).unwrap();
        let old = load().unwrap();
        assert_eq!((old.code_cache_dir, old.log_retention_days), (None, None));
    }

    #[test]
    fn engine_and_trigger_serialize_lowercase() {
        assert_eq!(serde_json::to_string(&Engine::Codex).unwrap(), "\"codex\"");
        assert_eq!(serde_json::to_string(&Engine::Claude).unwrap(), "\"claude\"");
        assert_eq!(serde_json::to_string(&Trigger::Owner).unwrap(), "\"owner\"");
        assert_eq!(serde_json::to_string(&Trigger::All).unwrap(), "\"all\"");
    }

    #[test]
    fn new_default_bot_has_expected_defaults() {
        let b = Bot::new_default("b1", "n", "cli_x");
        assert_eq!(b.engine, Engine::Claude);
        assert_eq!(b.template, "bug-hunt");
        assert!(b.auto_onboard && !b.start_with_client);
        assert_eq!(b.skills, ["chat-context", "code-sync", "db-query", "issue-record", "feishu-doc-record"]);
    }

    /// 旧配置的机器人没有 trigger_mode：按 @ 触发；三种取值序列化成 snake_case 并能读回。
    #[test]
    fn trigger_mode_defaults_to_mention_and_roundtrips() {
        let mut v = serde_json::to_value(Bot::new_default("b1", "n", "cli_x")).unwrap();
        assert_eq!(v["trigger_mode"], "mention");
        v.as_object_mut().unwrap().remove("trigger_mode");
        let old: Bot = serde_json::from_value(v).unwrap();
        assert_eq!(old.trigger_mode, TriggerMode::Mention);
        for (m, s) in [(TriggerMode::Mention, "mention"), (TriggerMode::PrLink, "pr_link"), (TriggerMode::Both, "both")] {
            assert_eq!(serde_json::to_value(m).unwrap(), s);
            assert_eq!(serde_json::from_value::<TriggerMode>(serde_json::json!(s)).unwrap(), m);
        }
        assert!(TriggerMode::Mention.accepts_mention() && !TriggerMode::Mention.accepts_pr_link());
        assert!(!TriggerMode::PrLink.accepts_mention() && TriggerMode::PrLink.accepts_pr_link());
        assert!(TriggerMode::Both.accepts_mention() && TriggerMode::Both.accepts_pr_link());
        let r = Bot::new_review("b2", "审查", "cli_y");
        assert_eq!((r.template.as_str(), r.trigger_mode), ("code-review", TriggerMode::PrLink));
        assert_eq!(r.skills, ["pr-review", "code-sync"]);
        assert!(r.is_code_review() && !old.is_code_review());
    }

    /// 旧配置的项目没有 record：仓库在 gitee / github 就默认建 issue，写飞书文档默认开；新字段回写后能原样读回。
    #[test]
    fn project_record_defaults_from_repo_and_roundtrips() {
        let old = |repo: &str| -> Project {
            serde_json::from_value(serde_json::json!({
                "repo": repo, "branch_glob": "release/*", "rules_file": "AGENTS.md", "description": "", "dbs": []
            }))
            .unwrap()
        };
        let gitee = old("git@gitee.com:a/demo.git");
        assert_eq!(gitee.record, RecordOpts { issue: true, feishu_doc: true, meegle: false, feishu_task: false });
        assert_eq!((gitee.issue_repo.clone(), gitee.token_env.clone()), (None, None));
        assert_eq!(old("https://github.com/a/b").record, RecordOpts { issue: true, feishu_doc: true, meegle: false, feishu_task: false });
        assert_eq!(old("git@x:a/demo.git").record, RecordOpts { issue: false, feishu_doc: true, meegle: false, feishu_task: false });

        // 显式配置优先于推断；空白的 issue_repo / token_env 当没填
        let p: Project = serde_json::from_value(serde_json::json!({
            "repo": "git@gitee.com:a/demo.git", "branch_glob": "release/*", "rules_file": "AGENTS.md", "description": "", "dbs": [],
            "record": {"issue": false, "feishu_doc": false}, "issue_repo": " ", "token_env": "MY_TOKEN"
        }))
        .unwrap();
        assert_eq!(p.record, RecordOpts { issue: false, feishu_doc: false, meegle: false, feishu_task: false });
        assert_eq!((p.issue_repo.as_deref(), p.token_env.as_deref()), (None, Some("MY_TOKEN")));
        // 只给了 issue：feishu_doc 默认开
        let partial: RecordOpts = serde_json::from_value(serde_json::json!({"issue": true})).unwrap();
        assert_eq!(partial, RecordOpts { issue: true, feishu_doc: true, meegle: false, feishu_task: false });

        let _home = test_home();
        let mut cfg = AppConfig::default();
        cfg.projects.insert("demo".into(), p.clone());
        save(&cfg).unwrap();
        assert_eq!(load().unwrap().projects["demo"], p);
        assert!(Project::new("git@gitee.com:a/b.git", "release/*", "AGENTS.md", "x", vec![]).record.issue);
    }
}
