//! 流程模板：Agent 的工作说明书，渲染后写进群工作目录的 CLAUDE.md / AGENTS.md。
//! 先找用户自定义版本 `<config_dir>/templates/<name>.md`，没有再用内置版本。

use crate::platform::paths;
use crate::store::config::{Group, Project};
use std::collections::BTreeMap;

/// 模板变量。键：GROUP_NAME, CHAT_ID, PROJECTS_BLOCK, DOC_URL, SKILLS_DIR, PYTHON, NODE, BEETLE_EXE。
#[derive(Debug, Clone, Default)]
pub struct TemplateVars(pub BTreeMap<&'static str, String>);

/// 替换 `{{KEY}}`。单遍扫描模板，替换进去的值不会被再次扫描；
/// 模板里出现 vars 没有的 `{{KEY}}`（KEY 为大写字母和下划线）返回错误。
pub fn render(template: &str, vars: &TemplateVars) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end)
                if end > 0
                    && after[..end].bytes().all(|b| b.is_ascii_uppercase() || b == b'_') =>
            {
                let key = &after[..end];
                match vars.0.get(key) {
                    Some(v) => out.push_str(v),
                    None => return Err(format!("模板里有未替换的变量：{{{{{key}}}}}")),
                }
                rest = &after[end + 2..];
            }
            _ => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

pub fn builtin(name: &str) -> Option<&'static str> {
    match name {
        "bug-hunt" => Some(include_str!("../resources/templates/bug-hunt.md")),
        "qa" => Some(include_str!("../resources/templates/qa.md")),
        "code-review" => Some(include_str!("../resources/templates/code-review.md")),
        _ => None,
    }
}

/// 内置模板名。
pub const BUILTIN_NAMES: [&str; 3] = ["bug-hunt", "qa", "code-review"];

/// 渲染工作说明书时提供的全部变量名（保存模板前用它做校验）。
pub const VAR_KEYS: [&str; 8] =
    ["GROUP_NAME", "CHAT_ID", "PROJECTS_BLOCK", "DOC_URL", "SKILLS_DIR", "PYTHON", "NODE", "BEETLE_EXE"];

/// 用户自定义 / 修改过的模板所在目录。
pub fn user_dir() -> std::path::PathBuf {
    paths::config_dir().join("templates")
}

/// 模板名只允许字母、数字、`-`、`_`（同时防路径穿越）。
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("模板名不合法：{name}（只能用字母、数字、- 和 _）"));
    }
    Ok(())
}

/// 先找用户版本 `<config_dir>/templates/<name>.md`，没有再用内置。
pub fn load(name: &str) -> Result<String, String> {
    check_name(name)?;
    let path = paths::config_dir().join("templates").join(format!("{name}.md"));
    match std::fs::read_to_string(&path) {
        Ok(text) => return Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("读取模板 {} 失败：{e}", path.display())),
    }
    builtin(name)
        .map(String::from)
        .ok_or_else(|| format!("找不到模板：{name}"))
}

/// 本群负责的项目清单，每个项目三行（说明与仓库、关联数据源、记录方式）；群里登记了但配置里没有的项目跳过。
pub fn projects_block(group: &Group, projects: &BTreeMap<String, Project>) -> String {
    group
        .projects
        .iter()
        .filter_map(|name| projects.get(name).map(|p| (name, p)))
        .map(|(name, p)| {
            let dbs = if p.dbs.is_empty() {
                "无".to_string()
            } else {
                p.dbs
                    .iter()
                    .map(|d| format!("`{d}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!(
                "- `{name}`：{}（仓库 {}，分支 `{}` 取最新）
  数据源：{dbs}
  记录方式：{}",
                p.description,
                p.repo,
                p.branch_glob,
                record_desc(p)
            )
        })
        .collect::<Vec<_>>()
        .join("
")
}

fn one_line(s: &str) -> String {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
}

/// 优先级交给 AI 判断时，才把优先级规则写给 Agent。
fn judge_priority(m: &crate::store::config::MeegleTarget) -> bool {
    m.ai_fields.iter().any(|f| f.field_key == "priority" || f.field_name.contains("优先级"))
}

/// 项目的记录方式（建 issue / 写飞书文档，可同时选），写进项目清单给 Agent 看。
pub fn record_desc(p: &Project) -> String {
    let mut v: Vec<String> = Vec::new();
    if p.record.issue {
        v.push(match crate::issue::target(p) {
            Some(t) => format!("建 issue（{} · {}）", t.platform.label(), t.full()),
            None => "建 issue（但仓库不是 gitee.com / github.com，无法建）".to_string(),
        });
    }
    if p.record.meegle {
        v.push(match &p.meegle {
            Some(m) => {
                let judge: Vec<String> = m
                    .ai_fields
                    .iter()
                    .map(|f| {
                        let name = if f.field_name.is_empty() { &f.field_key } else { &f.field_name };
                        if f.options.is_empty() {
                            format!("{name}（填文字）")
                        } else {
                            format!("{name}（{}）", f.options.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join("/"))
                        }
                    })
                    .collect();
                let mut extra = String::new();
                if !judge.is_empty() {
                    extra += &format!("；建单时按问题判断：{}", judge.join("、"));
                }
                if judge_priority(m) {
                    extra += &format!("；优先级规则：{}", one_line(m.priority_rule_or_default()));
                }
                let cands = m.candidates();
                if !cands.is_empty() {
                    let names: Vec<String> = cands
                        .iter()
                        .enumerate()
                        .map(|(i, u)| if i == 0 && m.assignee.is_some() { format!("{}（默认）", u.name) } else { u.name.clone() })
                        .collect();
                    extra += &format!("；候选处理人：{}", names.join("、"));
                    let rule = m.assign_rule.trim();
                    extra += &if rule.is_empty() { "；都派给默认处理人".to_string() } else { format!("；分派规则：{}", one_line(rule)) };
                }
                format!(
                    "建飞书项目工单（{} · {}{}）",
                    if m.space_name.is_empty() { &m.project_key } else { &m.space_name },
                    if m.work_item_type_name.is_empty() { &m.work_item_type } else { &m.work_item_type_name },
                    extra
                )
            }
            None => "建飞书项目工单（但还没选空间，无法建）".to_string(),
        });
    }
    if p.record.feishu_task {
        let t = p.feishu_task.clone().unwrap_or_default();
        let mut extra: Vec<String> = Vec::new();
        if !t.tasklist_name.is_empty() {
            extra.push(format!("清单 {}", t.tasklist_name));
        }
        if !t.assignee_name.is_empty() {
            extra.push(format!("负责人 {}", t.assignee_name));
        }
        v.push(if extra.is_empty() { "建飞书任务".to_string() } else { format!("建飞书任务（{}）", extra.join("，")) });
    }
    if p.record.feishu_doc {
        v.push("写飞书文档".to_string());
    }
    let base = if v.is_empty() { "不记录".to_string() } else { v.join(" + ") };
    match p.record_rule.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        Some(rule) if !v.is_empty() => format!("{base}；记录规则：{}", rule.replace('\n', " ")),
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::config::Trigger;
    use crate::test_support::test_home;

    fn full_vars() -> TemplateVars {
        let mut m = BTreeMap::new();
        for (k, v) in [
            ("GROUP_NAME", "测试群"),
            ("CHAT_ID", "oc_abc"),
            ("PROJECTS_BLOCK", "- `demo`：示例服务"),
            ("DOC_URL", "https://x.feishu.cn/docx/abc"),
            ("SKILLS_DIR", "C:/skills"),
            ("PYTHON", "python"),
            ("NODE", "node"),
            ("BEETLE_EXE", "C:/Beetle/beetle.exe"),
        ] {
            m.insert(k, v.to_string());
        }
        TemplateVars(m)
    }

    fn group(projects: &[&str]) -> Group {
        Group {
            chat_id: "oc_abc".into(),
            name: "测试群".into(),
            slug: "test".into(),
            projects: projects.iter().map(|s| s.to_string()).collect(),
            doc_url: None,
            trigger: Trigger::Owner,
        }
    }

    fn project(desc: &str, dbs: &[&str]) -> Project {
        Project::new("git@x:a/b.git", "release/*", "AGENTS.md", desc, dbs.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn render_replaces_all_vars() {
        let out = render("{{GROUP_NAME}}/{{CHAT_ID}}/{{GROUP_NAME}}", &full_vars()).unwrap();
        assert_eq!(out, "测试群/oc_abc/测试群");
    }

    #[test]
    fn render_errors_on_leftover_var() {
        let err = render("x {{NOPE}}", &full_vars()).unwrap_err();
        assert!(err.contains("{{NOPE}}"));
        assert!(err.starts_with("模板里有未替换的变量："));
    }

    #[test]
    fn render_does_not_rescan_substituted_values() {
        let mut v = full_vars();
        v.0.insert("PROJECTS_BLOCK", "示例 {{FOO}} 文本".into());
        assert_eq!(render("{{PROJECTS_BLOCK}}", &v).unwrap(), "示例 {{FOO}} 文本");
    }

    #[test]
    fn builtin_bug_hunt_has_no_silence_rule() {
        let t = builtin("bug-hunt").unwrap();
        assert!(!t.contains("整个过程中不要输出任何文字"));
        assert!(t.contains("【结论】") && t.contains("【修复建议】"));
        assert!(!t.contains("群主"));
        assert!(!t.contains("{{SCRIPTS}}"));
    }

    #[test]
    fn builtin_qa_has_answer_sections() {
        let t = builtin("qa").unwrap();
        assert!(t.contains("【回答】") && t.contains("【依据】"));
        assert!(builtin("nope").is_none());
    }

    #[test]
    fn builtin_templates_render_with_full_vars() {
        for name in BUILTIN_NAMES {
            let out = render(builtin(name).unwrap(), &full_vars()).unwrap();
            assert!(out.contains("测试群"), "{name}");
            assert!(!out.contains("{{"), "{name}");
        }
    }

    #[test]
    fn builtin_code_review_outputs_json_and_forbids_writes() {
        let t = builtin("code-review").unwrap();
        for must in ["pr_fetch.py", "pr_review_state.py", "sync_code.py", "--pr", "\"findings\"", "\"severity\"", "carried", "resolved", "不改代码", "不推送"] {
            assert!(t.contains(must), "code-review 模板缺少 {must}");
        }
        assert!(!t.contains("{{SCRIPTS}}"));
    }

    #[test]
    fn user_template_overrides_builtin() {
        let home = test_home();
        assert_eq!(load("bug-hunt").unwrap(), builtin("bug-hunt").unwrap());
        let dir = paths::config_dir().join("templates");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bug-hunt.md"), "自定义 {{GROUP_NAME}}").unwrap();
        assert_eq!(load("bug-hunt").unwrap(), "自定义 {{GROUP_NAME}}");
        drop(home);
    }

    #[test]
    fn load_unknown_template_errors() {
        let _home = test_home();
        assert!(load("nope").unwrap_err().contains("nope"));
    }

    #[test]
    fn load_rejects_path_traversal() {
        let _home = test_home();
        assert!(load("../config").is_err());
    }

    #[test]
    fn projects_block_format() {
        let mut projects = BTreeMap::new();
        projects.insert("demo".to_string(), project("示例服务", &["demo_config", "demo_test"]));
        projects.insert("rpa".to_string(), project("自动化", &["demo_test", "demo_worker"]));
        let g = group(&["demo", "rpa"]);
        let block = projects_block(&g, &projects);
        assert_eq!(
            block,
            "- `demo`：示例服务（仓库 git@x:a/b.git，分支 `release/*` 取最新）\n  数据源：`demo_config`, `demo_test`\n  记录方式：写飞书文档\n\
             - `rpa`：自动化（仓库 git@x:a/b.git，分支 `release/*` 取最新）\n  数据源：`demo_test`, `demo_worker`\n  记录方式：写飞书文档"
        );
    }

    #[test]
    fn record_desc_lists_selected_methods() {
        let mut p = Project::new("git@gitee.com:team/demo.git", "release/*", "AGENTS.md", "x", vec![]);
        assert_eq!(record_desc(&p), "建 issue（Gitee · team/demo） + 写飞书文档");
        p.record.feishu_doc = false;
        assert_eq!(record_desc(&p), "建 issue（Gitee · team/demo）");
        p.record.issue = false;
        assert_eq!(record_desc(&p), "不记录");
        p.record.meegle = true;
        assert!(record_desc(&p).contains("还没选空间"));
        p.meegle = Some(crate::store::config::MeegleTarget {
            project_key: "pk".into(),
            space_name: "AI 客服".into(),
            work_item_type: "issue".into(),
            work_item_type_name: "缺陷".into(),
            ..Default::default()
        });
        assert_eq!(record_desc(&p), "建飞书项目工单（AI 客服 · 缺陷）");
        p.meegle.as_mut().unwrap().ai_fields = vec![crate::store::config::MeegleAiField {
            field_key: "priority".into(),
            field_name: "优先级".into(),
            options: vec![
                crate::store::config::MeegleOption { id: "0".into(), name: "P0".into() },
                crate::store::config::MeegleOption { id: "1".into(), name: "P1".into() },
            ],
            ..Default::default()
        }];
        let desc = record_desc(&p);
        assert!(desc.starts_with("建飞书项目工单（AI 客服 · 缺陷；建单时按问题判断：优先级（P0/P1）；优先级规则：P0："), "{desc}");
        assert!(!desc.contains('\n'), "规则压成一行");
        {
            let m = p.meegle.as_mut().unwrap();
            m.priority_rule = "P0：全挂\nP1：部分挂".into();
            m.assignee = Some(crate::store::config::MeegleUser { user_key: "u1".into(), name: "张三".into() });
            m.assignees = vec![
                crate::store::config::MeegleUser { user_key: "u1".into(), name: "张三".into() },
                crate::store::config::MeegleUser { user_key: "u2".into(), name: "李四".into() },
            ];
        }
        assert!(record_desc(&p).ends_with("优先级规则：P0：全挂 P1：部分挂；候选处理人：张三（默认）、李四；都派给默认处理人）"), "{}", record_desc(&p));
        p.meegle.as_mut().unwrap().assign_rule = "前端给张三\nRPA 给李四".into();
        assert!(record_desc(&p).ends_with("分派规则：前端给张三 RPA 给李四）"), "{}", record_desc(&p));
        p.meegle.as_mut().unwrap().assign_rule = String::new();
        p.record.meegle = false;
        p.record.feishu_task = true;
        p.feishu_task = Some(crate::store::config::FeishuTaskTarget { assignee_name: "张三".into(), ..Default::default() });
        assert_eq!(record_desc(&p), "建飞书任务（负责人 张三）");
        p.record_rule = Some("调研类建飞书任务\nbug 建 issue".into());
        assert_eq!(record_desc(&p), "建飞书任务（负责人 张三）；记录规则：调研类建飞书任务 bug 建 issue");
        p.record.feishu_task = false;
        assert_eq!(record_desc(&p), "不记录", "没勾任何方式时规则不生效");
        p.record_rule = None;
        let mut q = Project::new("git@x:a/b.git", "release/*", "AGENTS.md", "x", vec![]);
        q.record.issue = true;
        assert!(record_desc(&q).contains("无法建"));
    }

    #[test]
    fn empty_dbs_and_unknown_project() {
        let mut projects = BTreeMap::new();
        projects.insert("demo".to_string(), project("示例服务", &[]));
        let g = group(&["demo", "ghost"]);
        assert!(projects_block(&g, &projects).contains("数据源：无"));
        assert!(!projects_block(&g, &projects).contains("ghost"));
    }
}
