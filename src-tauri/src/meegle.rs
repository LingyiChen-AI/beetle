//! 飞书项目（meegle CLI）：项目页「建飞书项目工单」的配置来源。
//!
//! 用户在环境检测 / 项目页登录 meegle 后，这里用 CLI 拉空间、工单类型、建单必填字段（含下拉选项）、角色和人员，
//! 让用户选好存进项目配置（`Project.meegle`）。真正建单在机器人技能 `issue-record/scripts/create_work_item.py`。
//!
//! Windows 上 `meegle` 是 npm 的 `.cmd` 壳，带中文 / JSON 的参数经它转发不可靠，所以和 lark-cli 一样
//! 直接 `node <包>/bin/meegle.js` 调用；其它平台直接运行 `meegle`。

use crate::platform::{locate, proc};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
/// 默认处理人放进哪个角色：按角色名猜（缺陷处理 / 解决 / 经办 / 开发），用户可改。
const ASSIGNEE_ROLE_HINTS: [&str; 5] = ["缺陷处理", "解决", "经办", "开发", "处理"];

/// `meegle` 可执行文件（或 `.cmd` 壳）所在目录推出 `meegle.js`：`<npm 全局目录>/node_modules/@lark-project/meegle/bin/meegle.js`。
pub fn script_path(bin: &Path) -> PathBuf {
    bin.parent()
        .unwrap_or(Path::new("."))
        .join("node_modules")
        .join("@lark-project")
        .join("meegle")
        .join("bin")
        .join("meegle.js")
}

fn command() -> Result<Command, String> {
    let bin = locate::find("meegle").ok_or("没有找到飞书项目 CLI（meegle），请先在「环境检测」里安装")?;
    let script = script_path(&bin);
    if cfg!(windows) && script.is_file() {
        let node = locate::find("node").ok_or("没有找到 Node.js")?;
        let mut c = Command::new(node);
        c.arg(script);
        Ok(c)
    } else {
        Ok(Command::new(bin))
    }
}

/// 从 CLI 输出里取 JSON（前面可能有更新提示之类的文字）。
pub fn parse_output(out: &str) -> Option<Value> {
    let i = out.find(['{', '['])?;
    serde_json::from_str(out[i..].trim()).ok()
}

/// 错误输出 → 给用户看的一句话；看起来是没登录就直接提示去登录。
pub fn error_text(code: i32, stdout: &str, stderr: &str) -> String {
    let raw = if stderr.trim().is_empty() { stdout } else { stderr };
    let low = raw.to_lowercase();
    if low.contains("not logged") || low.contains("unauthenticated") || low.contains("auth login") || low.contains("token expired") {
        return "飞书项目 CLI 未登录或登录已过期，请先登录".to_string();
    }
    let msg: String = raw.trim().chars().take(300).collect();
    if msg.is_empty() { format!("飞书项目 CLI 执行失败（退出码 {code}）") } else { format!("飞书项目 CLI 执行失败：{msg}") }
}

fn run_json(args: &[&str]) -> Result<Value, String> {
    let mut cmd = command()?;
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = proc::spawn_hidden(&mut cmd).map_err(|e| format!("无法运行飞书项目 CLI：{e}"))?;
    let out = proc::wait_output_timeout(child, TIMEOUT)?;
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let code = out.status.code().unwrap_or(-1);
    if code != 0 {
        return Err(error_text(code, &stdout, &stderr));
    }
    parse_output(&stdout).ok_or_else(|| error_text(code, &stdout, &stderr))
}

// ───────────────────────── 返回给界面的结构 ─────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeegleStatus {
    pub installed: bool,
    pub logged_in: bool,
    /// 登录的域名，如 project.feishu.cn
    pub host: Option<String>,
    /// 当前登录用户（姓名）
    pub user: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Space {
    pub project_key: String,
    pub simple_name: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkItemType {
    pub type_key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Opt {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FormField {
    pub field_key: String,
    pub field_name: String,
    pub field_type: String,
    /// 下拉选项；空 = 文本类字段，用户直接填
    pub options: Vec<Opt>,
    /// 空间配置的默认选项 id
    pub default: Option<String>,
    /// 建单时自动填、不用选的说明（如「名称」取 bug 标题）；Some 时界面只展示不让选
    pub auto: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Role {
    pub role_id: String,
    pub role_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeegleForm {
    /// 建单必填字段
    pub fields: Vec<FormField>,
    pub roles: Vec<Role>,
    /// 猜的「处理人」角色 id
    pub suggested_role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeegleUserHit {
    pub user_key: String,
    pub name: String,
    pub email: String,
}

// ───────────────────────── 解析（纯函数，单测覆盖） ─────────────────────────

fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn list<'a>(v: &'a Value, keys: &[&str]) -> Vec<&'a Value> {
    if let Some(a) = v.as_array() {
        return a.iter().collect();
    }
    keys.iter().find_map(|k| v[*k].as_array()).map(|a| a.iter().collect()).unwrap_or_default()
}

pub fn parse_status(v: &Value) -> (bool, Option<String>) {
    let host = v["host"].as_str().filter(|h| !h.is_empty()).map(String::from);
    (v["authenticated"].as_bool().unwrap_or(false), host)
}

pub fn parse_spaces(v: &Value) -> Vec<Space> {
    list(v, &["projects", "list"])
        .into_iter()
        .map(|p| Space { project_key: s(p, "project_key"), simple_name: s(p, "simple_name"), name: s(p, "name") })
        .filter(|p| !p.project_key.is_empty())
        .collect()
}

/// 工单类型：去掉停用的（is_disable = 1）和图表这类不能建单的。
pub fn parse_types(v: &Value) -> Vec<WorkItemType> {
    list(v, &["list"])
        .into_iter()
        .filter(|t| t["is_disable"].as_i64() != Some(1))
        .map(|t| WorkItemType { type_key: s(t, "type_key"), name: s(t, "name") })
        .filter(|t| !t.type_key.is_empty() && t.type_key != "chart")
        .collect()
}

/// 默认值是 base64 的 `{"label","value"}`，取 value（选项 id）。
pub fn decode_default(v: &Value) -> Option<String> {
    use base64::Engine;
    let raw = v["value"].as_str().filter(|x| !x.is_empty())?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(raw).ok()?;
    let j: Value = serde_json::from_slice(&bytes).ok()?;
    match &j["value"] {
        Value::String(x) if !x.is_empty() => Some(x.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn auto_note(key: &str, ty: &str) -> Option<String> {
    match key {
        "name" => Some("自动填：bug 标题".into()),
        "description" => Some("自动填：bug 分析报告".into()),
        _ if ty == "user" || ty == "multi_user" => Some("自动填：当前登录的飞书项目用户".into()),
        _ => None,
    }
}

/// `meta-create-fields`（必填字段）+ `meta-fields`（选项）+ `meta-roles` → 表单。
pub fn build_form(required: &Value, fields: &Value, roles: &Value) -> MeegleForm {
    let all = list(fields, &["list", "fields"]);
    let form_fields = list(required, &["FieldConfList", "list"])
        .into_iter()
        .map(|rf| {
            let key = s(rf, "field_key");
            let ty = s(rf, "field_type_key");
            let meta = all.iter().find(|f| f["field_key"].as_str() == Some(key.as_str()));
            let options = meta
                .map(|f| list(f, &["option"]))
                .unwrap_or_default()
                .into_iter()
                .filter(|o| !o["disabled"].as_bool().unwrap_or(false))
                .map(|o| Opt { id: s(o, "option_id"), name: s(o, "option_name") })
                .filter(|o| !o.id.is_empty())
                .collect();
            FormField {
                auto: auto_note(&key, &ty),
                default: decode_default(&rf["default_value"]),
                field_name: s(rf, "field_name"),
                field_type: if ty.is_empty() { meta.map(|f| s(f, "field_type")).unwrap_or_default() } else { ty },
                field_key: key,
                options,
            }
        })
        .filter(|f| !f.field_key.is_empty())
        .collect();
    let roles: Vec<Role> = list(roles, &["list"])
        .into_iter()
        .map(|r| Role { role_id: s(r, "role_id"), role_name: s(r, "role_name") })
        .filter(|r| !r.role_id.is_empty())
        .collect();
    let suggested_role = ASSIGNEE_ROLE_HINTS
        .iter()
        .find_map(|h| roles.iter().find(|r| r.role_name.contains(h)))
        .map(|r| r.role_id.clone());
    MeegleForm { fields: form_fields, roles, suggested_role }
}

pub fn parse_users(v: &Value) -> Vec<MeegleUserHit> {
    list(v, &["list", "users", "data"])
        .into_iter()
        .map(|u| {
            let name = Some(s(u, "name_cn")).filter(|x| !x.is_empty()).unwrap_or_else(|| s(u, "name"));
            MeegleUserHit { user_key: s(u, "user_key"), name, email: s(u, "email") }
        })
        .filter(|u| !u.user_key.is_empty())
        .collect()
}

// ───────────────────────── 命令 ─────────────────────────

fn status_now() -> MeegleStatus {
    let mut st = MeegleStatus { installed: locate::find("meegle").is_some(), logged_in: false, host: None, user: None, error: None };
    if !st.installed {
        st.error = Some("没有找到飞书项目 CLI（meegle）".into());
        return st;
    }
    match run_json(&["auth", "status"]) {
        Ok(v) => {
            let (ok, host) = parse_status(&v);
            st.logged_in = ok;
            st.host = host;
        }
        Err(e) => st.error = Some(e),
    }
    if st.logged_in {
        match run_json(&["user", "me"]) {
            Ok(u) => st.user = Some(s(&u, "name_cn")).filter(|x| !x.is_empty()),
            Err(e) => st.error = Some(e),
        }
    } else if st.error.is_none() {
        st.error = Some("还没有登录飞书项目 CLI".into());
    }
    st
}

#[tauri::command]
pub async fn meegle_status() -> Result<MeegleStatus, String> {
    crate::commands::blocking(|| Ok(status_now())).await
}

#[tauri::command]
pub async fn meegle_spaces() -> Result<Vec<Space>, String> {
    crate::commands::blocking(|| Ok(parse_spaces(&run_json(&["project", "search"])?))).await
}

#[tauri::command]
pub async fn meegle_types(project_key: String) -> Result<Vec<WorkItemType>, String> {
    crate::commands::blocking(move || Ok(parse_types(&run_json(&["workitem", "meta-types", "--project-key", &project_key])?))).await
}

#[tauri::command]
pub async fn meegle_form(project_key: String, work_item_type: String) -> Result<MeegleForm, String> {
    crate::commands::blocking(move || {
        let base = ["--project-key", project_key.as_str(), "--work-item-type", work_item_type.as_str()];
        let with = |cmd: &str, extra: &[&str]| {
            let mut a = vec!["workitem", cmd];
            a.extend_from_slice(&base);
            a.extend_from_slice(extra);
            run_json(&a)
        };
        let required = with("meta-create-fields", &[])?;
        let fields = with("meta-fields", &["--auto-paginate"])?;
        let roles = with("meta-roles", &[])?;
        Ok(build_form(&required, &fields, &roles))
    })
    .await
}

/// 按姓名 / 邮箱找飞书项目用户（选默认处理人）。
#[tauri::command]
pub async fn meegle_users(query: String) -> Result<Vec<MeegleUserHit>, String> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Ok(vec![]);
    }
    crate::commands::blocking(move || Ok(parse_users(&run_json(&["user", "search", "--user-keys", &q])?))).await
}

// ───────────────────────── 飞书任务（lark-cli）的配置来源 ─────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tasklist {
    pub guid: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LarkUserHit {
    pub open_id: String,
    pub name: String,
    pub department: String,
}

#[tauri::command]
pub async fn lark_tasklists() -> Result<Vec<Tasklist>, String> {
    crate::commands::blocking(|| {
        let cli = crate::feishu::lark_cli::LarkCli::detect()?;
        Ok(cli.tasklists()?.into_iter().map(|(guid, name)| Tasklist { guid, name }).collect())
    })
    .await
}

#[tauri::command]
pub async fn lark_users(query: String) -> Result<Vec<LarkUserHit>, String> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Ok(vec![]);
    }
    crate::commands::blocking(move || {
        let cli = crate::feishu::lark_cli::LarkCli::detect()?;
        Ok(cli
            .search_users(&q)?
            .into_iter()
            .map(|(open_id, name, department)| LarkUserHit { open_id, name, department })
            .collect())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn script_path_next_to_npm_shim() {
        let p = script_path(Path::new("/npm/meegle.cmd"));
        assert!(p.ends_with("node_modules/@lark-project/meegle/bin/meegle.js"), "{}", p.display());
    }

    #[test]
    fn parse_output_skips_leading_text() {
        assert_eq!(parse_output("有新版本\n{\"a\":1}\n"), Some(json!({"a": 1})));
        assert_eq!(parse_output("[1]"), Some(json!([1])));
        assert_eq!(parse_output("nothing"), None);
    }

    #[test]
    fn login_errors_are_friendly() {
        assert_eq!(error_text(1, "", "Error: not logged in, run meegle auth login"), "飞书项目 CLI 未登录或登录已过期，请先登录");
        assert!(error_text(2, "", "boom").contains("boom"));
        assert!(error_text(3, "", " ").contains("退出码 3"));
    }

    #[test]
    fn parses_status_spaces_types_users() {
        assert_eq!(parse_status(&json!({"authenticated": true, "host": "project.feishu.cn"})), (true, Some("project.feishu.cn".into())));
        assert_eq!(parse_status(&json!({})), (false, None));
        let sp = parse_spaces(&json!({"projects": [{"name": "AI 客服", "project_key": "pk1", "simple_name": "bot"}, {"name": "无 key"}]}));
        assert_eq!(sp, vec![Space { project_key: "pk1".into(), simple_name: "bot".into(), name: "AI 客服".into() }]);
        let ty = parse_types(&json!({"list": [
            {"type_key": "story", "name": "需求", "is_disable": 2},
            {"type_key": "issue", "name": "缺陷", "is_disable": 2},
            {"type_key": "chart", "name": "图表", "is_disable": 2},
            {"type_key": "old", "name": "停用", "is_disable": 1}
        ]}));
        assert_eq!(ty.iter().map(|t| t.type_key.as_str()).collect::<Vec<_>>(), ["story", "issue"]);
        let us = parse_users(&json!([{"user_key": "u1", "name_cn": "李四", "email": ""}, {"name_cn": "无 key"}]));
        assert_eq!(us, vec![MeegleUserHit { user_key: "u1".into(), name: "李四".into(), email: String::new() }]);
    }

    #[test]
    fn form_merges_required_fields_options_and_roles() {
        // P3 的默认值：base64({"label":"P3","value":"drr8b74tf"})
        let required = json!({"FieldConfList": [
            {"field_key": "name", "field_name": "名称", "field_type_key": "text"},
            {"field_key": "description", "field_name": "缺陷描述", "field_type_key": "multi_text"},
            {"field_key": "priority", "field_name": "优先级", "field_type_key": "select",
             "default_value": {"value": "eyJsYWJlbCI6IlAzIiwidmFsdWUiOiJkcnI4Yjc0dGYifQ=="}},
            {"field_key": "issue_reporter", "field_name": "报告人", "field_type_key": "multi_user"},
            {"field_key": "bug_classification", "field_name": "涉及端", "field_type_key": "select"}
        ]});
        let fields = json!({"list": [
            {"field_key": "priority", "option": [{"option_id": "0", "option_name": "P0"}, {"option_id": "drr8b74tf", "option_name": "P3"}]},
            {"field_key": "bug_classification", "option": [{"option_id": "c1", "option_name": "客户端"}, {"option_id": "x", "option_name": "停用", "disabled": true}]}
        ]});
        let roles = json!({"list": [{"role_id": "reporter", "role_name": "报告人"}, {"role_id": "r5", "role_name": "缺陷开发解决人员"}]});
        let f = build_form(&required, &fields, &roles);
        let by = |k: &str| f.fields.iter().find(|x| x.field_key == k).unwrap();
        assert_eq!(by("name").auto.as_deref(), Some("自动填：bug 标题"));
        assert!(by("issue_reporter").auto.is_some());
        assert_eq!(by("priority").default.as_deref(), Some("drr8b74tf"));
        assert_eq!(by("priority").options.len(), 2);
        assert!(by("priority").auto.is_none());
        assert_eq!(by("bug_classification").options, vec![Opt { id: "c1".into(), name: "客户端".into() }]);
        assert_eq!(f.suggested_role.as_deref(), Some("r5"));
        assert_eq!(f.roles.len(), 2);
    }
}
