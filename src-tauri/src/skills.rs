//! 技能清单与内置禁止清单。技能随客户端打包在 `resources/skills/<name>/`，
//! 每个技能目录有 `SKILL.md` 和 `beetle.skill.json`（改名前叫 `bug-patrol.skill.json`，仍认）；以 `_` 开头的目录（`_shared`、`_tests`）不是技能。

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static BUILTIN_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 启动时由 `lib.rs` 传入 Tauri 的 resource 目录（其下 `skills/` 即内置技能）。只有第一次调用生效。
pub fn init(resource_dir: &Path) {
    let dir = resource_dir.join("skills");
    // 开发模式下资源目录可能没有复制技能，此时保持回落到源码目录。
    if dir.is_dir() {
        let _ = BUILTIN_DIR.set(dir);
    }
}

/// 内置技能根目录；没调用过 `init`（开发 / 测试）时指向源码里的 `src-tauri/resources/skills`。
pub fn builtin_dir() -> PathBuf {
    BUILTIN_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join("skills"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub requires: Vec<String>,
    pub builtin: bool,
}

#[derive(Deserialize)]
struct Manifest {
    display_name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    requires: Vec<String>,
}

/// 列出内置技能，按名称排序。清单文件缺失或损坏的目录跳过（不影响其他技能）。
/// 技能清单文件名。
pub const MANIFEST_FILE: &str = "beetle.skill.json";
/// 改名前的技能清单文件名：用户以前导入的技能里可能还是这个名字。
pub const LEGACY_MANIFEST_FILE: &str = "bug-patrol.skill.json";

/// 读技能目录里的清单文件：新文件名优先，没有再读旧文件名。都没有返回 None。
pub fn read_manifest_text(dir: &Path) -> Option<String> {
    [MANIFEST_FILE, LEGACY_MANIFEST_FILE]
        .iter()
        .find_map(|f| std::fs::read_to_string(dir.join(f)).ok())
}

pub fn list() -> Vec<SkillInfo> {
    let Ok(rd) = std::fs::read_dir(builtin_dir()) else {
        return Vec::new();
    };
    let mut out: Vec<SkillInfo> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('_') {
                return None;
            }
            let text = read_manifest_text(&e.path())?;
            let m: Manifest = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
            Some(SkillInfo {
                display_name: m.display_name.unwrap_or_else(|| name.clone()),
                description: m.description.unwrap_or_default(),
                requires: m.requires,
                builtin: true,
                name,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 用户从本地目录导入的技能所在目录。
pub fn user_dir() -> PathBuf {
    crate::platform::paths::config_dir().join("skills")
}

/// 技能源目录：内置优先，其次用户导入。
pub fn skill_dir(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.starts_with('_') {
        return None;
    }
    [builtin_dir().join(name), user_dir().join(name)]
        .into_iter()
        .find(|d| d.join("SKILL.md").is_file())
}

pub(crate) fn frontmatter_description(md: &str) -> String {
    let mut lines = md.trim_start_matches('\u{feff}').lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    for l in lines {
        if l.trim() == "---" {
            break;
        }
        if let Some(v) = l.trim().strip_prefix("description:") {
            return v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
        }
    }
    String::new()
}

fn read_user_skill(dir: &Path, name: &str) -> SkillInfo {
    let manifest = read_manifest_text(dir)
        .and_then(|t| serde_json::from_str::<Manifest>(t.trim_start_matches('\u{feff}')).ok());
    let md_desc = std::fs::read_to_string(dir.join("SKILL.md"))
        .map(|t| frontmatter_description(&t))
        .unwrap_or_default();
    match manifest {
        Some(m) => SkillInfo {
            name: name.to_string(),
            display_name: m.display_name.unwrap_or_else(|| name.to_string()),
            description: m.description.unwrap_or(md_desc),
            requires: m.requires,
            builtin: false,
        },
        None => SkillInfo {
            name: name.to_string(),
            display_name: name.to_string(),
            description: md_desc,
            requires: Vec::new(),
            builtin: false,
        },
    }
}

/// 用户导入的技能（`<config_dir>/skills/<name>/`，要求有 SKILL.md），按名称排序。
pub fn list_user() -> Vec<SkillInfo> {
    let Ok(rd) = std::fs::read_dir(user_dir()) else { return Vec::new() };
    let mut out: Vec<SkillInfo> = rd
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.') && e.path().join("SKILL.md").is_file())
        .map(|e| read_user_skill(&e.path(), &e.file_name().to_string_lossy()))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 内置 + 用户导入的全部技能。
pub fn list_all() -> Vec<SkillInfo> {
    let mut v = list();
    v.extend(list_user());
    v
}

/// 审查 S4：准备工作目录时 SKILL.md 只替换 `{{SKILL_DIR}}` `{{PYTHON}}` `{{NODE}}` `{{BEETLE_EXE}}`，出现别的 `{{大写变量}}` 会让
/// 绑定它的机器人每次运行都「准备工作目录失败」。导入时就用这几个变量试渲染一遍，不通过直接拒绝并说明原因。
fn check_importable_skill_md(name: &str, md_path: &Path) -> Result<(), String> {
    let md = std::fs::read_to_string(md_path).map_err(|e| format!("读取 {} 失败：{e}", md_path.display()))?;
    let mut vars = crate::templates::TemplateVars::default();
    for k in ["SKILL_DIR", "PYTHON", "NODE", "BEETLE_EXE"] {
        vars.0.insert(k, "x".to_string());
    }
    crate::templates::render(&md, &vars).map(|_| ()).map_err(|e| {
        format!("技能 {name} 的 SKILL.md 不能导入：{e}。技能里只能用 {{{{SKILL_DIR}}}}、{{{{PYTHON}}}}、{{{{NODE}}}}、{{{{BEETLE_EXE}}}} 这四个变量")
    })
}

/// 从本地目录导入技能：复制进客户端技能库。目录名即技能名；没有 `beetle.skill.json`（或旧名 `bug-patrol.skill.json`）的按普通 skill 处理（无配置项）。
/// 同名的已导入技能会被覆盖（相当于更新）；不能与内置技能重名。
pub fn import_local(dir: &Path) -> Result<SkillInfo, String> {
    if !dir.join("SKILL.md").is_file() {
        return Err(format!("{} 里没有 SKILL.md，不是一个技能目录", dir.display()));
    }
    let name = std::path::absolute(dir)
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or("无法确定技能名称")?;
    if name.is_empty() || name.starts_with('_') || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("技能目录名不合法：{name}（只能用字母、数字、- 和 _，且不能以 _ 开头）"));
    }
    if builtin_dir().join(&name).is_dir() {
        return Err(format!("不能导入与内置技能同名的技能：{name}"));
    }
    check_importable_skill_md(&name, &dir.join("SKILL.md"))?;
    let root = user_dir();
    std::fs::create_dir_all(&root).map_err(|e| format!("创建技能目录失败：{e}"))?;
    let tmp = root.join(format!(".import-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    crate::workspace::copy_dir(dir, &tmp)?;
    let dst = root.join(&name);
    if dst.exists() {
        std::fs::remove_dir_all(&dst).map_err(|e| format!("替换旧版本失败：{e}"))?;
    }
    std::fs::rename(&tmp, &dst).map_err(|e| {
        let _ = std::fs::remove_dir_all(&tmp);
        format!("导入技能失败：{e}")
    })?;
    Ok(read_user_skill(&dst, &name))
}

/// 内置禁止清单（spec §18），Claude Code 权限规则写法；机器人设置里只能追加，不能删这里的项。
/// 规则语法 `Tool(前缀:*)` 已用 `claude` 2.1.327 的 `--disallowedTools` 说明核对（示例 `Bash(git *)`，`:*` 前缀写法为兼容写法）。
/// 同一份 Bash 规则同时转成 Codex 的 execpolicy 规则（见 `workspace::codex_rules`）。
/// 分类（Bash 与 PowerShell 各一份，共同列表）：
/// - git：改历史（push/commit/reset/rebase/merge/cherry-pick/revert/tag）和改工作区（pull/fetch/checkout/switch/clean/restore/stash/am/apply）
///   一律禁，另禁带全局选项的写法（`-C`/`-c`/`--git-dir`/`--work-tree`，防绕过）。拉代码、切分支只由 code-sync 脚本做；
///   `git log/show/diff/grep/blame` 等读操作不禁。
/// - 数据库 / 中间件客户端（含 `.exe`）、`lark-cli im/api`（含 `.cmd`）。
/// - 包装执行（`bash -c`、`cmd /c`、`powershell`、`pwsh`、`Invoke-Expression`、`iex`、`Start-Process`），防止套一层绕过上面的规则。
///   `python …` / `node …` 直接调用不受影响（技能脚本这样调用）。
/// - 删除类：`rm rmdir del unlink Remove-Item erase rd ri`；`find -delete` 只能匹配 `-delete` 紧跟 find 的写法，
///   `find . -delete` 这类参数在后的写法规则语法表达不了，靠沙箱 / 提示词兜底。
pub const BUILTIN_DENY: &[&str] = &[
    "Bash(git push:*)",
    "Bash(git commit:*)",
    "Bash(git reset:*)",
    "Bash(git rebase:*)",
    "Bash(git merge:*)",
    "Bash(git cherry-pick:*)",
    "Bash(git revert:*)",
    "Bash(git tag:*)",
    "Bash(git pull:*)",
    "Bash(git fetch:*)",
    "Bash(git checkout:*)",
    "Bash(git switch:*)",
    "Bash(git clean:*)",
    "Bash(git restore:*)",
    "Bash(git stash:*)",
    "Bash(git am:*)",
    "Bash(git apply:*)",
    "Bash(git -C:*)",
    "Bash(git -c:*)",
    "Bash(git --git-dir:*)",
    "Bash(git --work-tree:*)",
    "Bash(mysql:*)",
    "Bash(mysqlsh:*)",
    "Bash(mongosh:*)",
    "Bash(mongo:*)",
    "Bash(redis-cli:*)",
    "Bash(psql:*)",
    "Bash(mysql.exe:*)",
    "Bash(mysqlsh.exe:*)",
    "Bash(mongosh.exe:*)",
    "Bash(mongo.exe:*)",
    "Bash(redis-cli.exe:*)",
    "Bash(psql.exe:*)",
    "Bash(kafka-topics:*)",
    "Bash(kafka-console-consumer:*)",
    "Bash(kafka-console-producer:*)",
    "Bash(kafka-consumer-groups:*)",
    "Bash(lark-cli im:*)",
    "Bash(lark-cli api:*)",
    "Bash(lark-cli.cmd im:*)",
    "Bash(lark-cli.cmd api:*)",
    "Bash(bash -c:*)",
    "Bash(sh -c:*)",
    "Bash(cmd /c:*)",
    "Bash(cmd.exe:*)",
    "Bash(powershell:*)",
    "Bash(powershell.exe:*)",
    "Bash(pwsh:*)",
    "Bash(Invoke-Expression:*)",
    "Bash(iex:*)",
    "Bash(Start-Process:*)",
    "Bash(rm:*)",
    "Bash(rmdir:*)",
    "Bash(del:*)",
    "Bash(unlink:*)",
    "Bash(find -delete:*)",
    "Bash(Remove-Item:*)",
    "Bash(erase:*)",
    "Bash(rd:*)",
    "Bash(ri:*)",
    "PowerShell(git push:*)",
    "PowerShell(git commit:*)",
    "PowerShell(git reset:*)",
    "PowerShell(git rebase:*)",
    "PowerShell(git merge:*)",
    "PowerShell(git cherry-pick:*)",
    "PowerShell(git revert:*)",
    "PowerShell(git tag:*)",
    "PowerShell(git pull:*)",
    "PowerShell(git fetch:*)",
    "PowerShell(git checkout:*)",
    "PowerShell(git switch:*)",
    "PowerShell(git clean:*)",
    "PowerShell(git restore:*)",
    "PowerShell(git stash:*)",
    "PowerShell(git am:*)",
    "PowerShell(git apply:*)",
    "PowerShell(git -C:*)",
    "PowerShell(git -c:*)",
    "PowerShell(git --git-dir:*)",
    "PowerShell(git --work-tree:*)",
    "PowerShell(mysql:*)",
    "PowerShell(mysqlsh:*)",
    "PowerShell(mongosh:*)",
    "PowerShell(mongo:*)",
    "PowerShell(redis-cli:*)",
    "PowerShell(psql:*)",
    "PowerShell(mysql.exe:*)",
    "PowerShell(mysqlsh.exe:*)",
    "PowerShell(mongosh.exe:*)",
    "PowerShell(mongo.exe:*)",
    "PowerShell(redis-cli.exe:*)",
    "PowerShell(psql.exe:*)",
    "PowerShell(kafka-topics:*)",
    "PowerShell(kafka-console-consumer:*)",
    "PowerShell(kafka-console-producer:*)",
    "PowerShell(kafka-consumer-groups:*)",
    "PowerShell(lark-cli im:*)",
    "PowerShell(lark-cli api:*)",
    "PowerShell(lark-cli.cmd im:*)",
    "PowerShell(lark-cli.cmd api:*)",
    "PowerShell(bash -c:*)",
    "PowerShell(sh -c:*)",
    "PowerShell(cmd /c:*)",
    "PowerShell(cmd.exe:*)",
    "PowerShell(powershell:*)",
    "PowerShell(powershell.exe:*)",
    "PowerShell(pwsh:*)",
    "PowerShell(Invoke-Expression:*)",
    "PowerShell(iex:*)",
    "PowerShell(Start-Process:*)",
    "PowerShell(rm:*)",
    "PowerShell(rmdir:*)",
    "PowerShell(del:*)",
    "PowerShell(unlink:*)",
    "PowerShell(find -delete:*)",
    "PowerShell(Remove-Item:*)",
    "PowerShell(erase:*)",
    "PowerShell(rd:*)",
    "PowerShell(ri:*)",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_returns_five_builtin_skills_without_internal_dirs() {
        let skills = list();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["chat-context", "code-sync", "db-query", "feishu-doc-record", "issue-record", "pr-review"]);
        let dq = skills.iter().find(|s| s.name == "db-query").unwrap();
        assert!(dq.builtin);
        assert!(!dq.display_name.is_empty() && !dq.description.is_empty());
    }

    fn make_skill(root: &Path, name: &str, md: &str, manifest: Option<&str>) -> PathBuf {
        let d = root.join(name);
        std::fs::create_dir_all(d.join("scripts")).unwrap();
        std::fs::write(d.join("SKILL.md"), md).unwrap();
        std::fs::write(d.join("scripts").join("run.py"), "print(1)").unwrap();
        if let Some(m) = manifest {
            std::fs::write(d.join(MANIFEST_FILE), m).unwrap();
        }
        d
    }

    #[test]
    fn import_local_copies_skill_and_lists_it_without_touching_builtin_list() {
        let home = crate::test_support::test_home();
        let src = make_skill(home.path(), "my-skill", "---\nname: my-skill\ndescription: \"做点事\"\n---\n正文", None);
        let info = import_local(&src).unwrap();
        assert_eq!((info.name.as_str(), info.description.as_str(), info.builtin), ("my-skill", "做点事", false));
        assert!(info.requires.is_empty());
        assert!(user_dir().join("my-skill").join("scripts").join("run.py").is_file());
        assert_eq!(list_user().len(), 1);
        assert_eq!(list().len(), 6, "内置清单不受影响");
        assert_eq!(list_all().len(), 7);
        assert_eq!(skill_dir("my-skill"), Some(user_dir().join("my-skill")));
        assert!(skill_dir("db-query").unwrap().starts_with(builtin_dir()));
        assert!(skill_dir("_shared").is_none() && skill_dir("nope").is_none());

        // 重新导入 = 更新：旧文件被替换，且不留临时目录。
        std::fs::write(src.join("SKILL.md"), "新版").unwrap();
        std::fs::remove_file(src.join("scripts").join("run.py")).unwrap();
        import_local(&src).unwrap();
        assert!(!user_dir().join("my-skill").join("scripts").join("run.py").exists());
        let leftovers = std::fs::read_dir(user_dir()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn import_local_uses_manifest_when_present_and_rejects_bad_dirs() {
        let home = crate::test_support::test_home();
        let src = make_skill(
            home.path(),
            "with-manifest",
            "x",
            Some(r#"{"name":"with-manifest","display_name":"带清单","requires":["python"]}"#),
        );
        let info = import_local(&src).unwrap();
        assert_eq!((info.display_name.as_str(), info.requires.clone()), ("带清单", vec!["python".to_string()]));

        let empty = home.path().join("empty-dir");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(import_local(&empty).unwrap_err().contains("SKILL.md"));
        let clash = make_skill(home.path(), "db-query", "x", None);
        assert!(import_local(&clash).unwrap_err().contains("内置"));
        let bad = make_skill(home.path(), "has space", "x", None);
        assert!(import_local(&bad).unwrap_err().contains("不合法"));
    }

    /// 审查 S4：SKILL.md 里有客户端不认识的 {{大写变量}} 时导入就拒绝，而不是等机器人每次运行都失败。
    #[test]
    fn import_local_rejects_unknown_template_vars() {
        let home = crate::test_support::test_home();
        let src = make_skill(home.path(), "third-party", "用 {{API_KEY}} 调接口，脚本在 {{SKILL_DIR}}", None);
        let err = import_local(&src).unwrap_err();
        assert!(err.contains("{{API_KEY}}") && err.contains("不能导入"), "{err}");
        assert!(!user_dir().join("third-party").exists(), "拒绝时不复制");
        // 只用三个已知变量、或小写 / 带空格的花括号（不是变量）可以导入
        let ok = make_skill(home.path(), "ok-skill", "{{SKILL_DIR}}/a.py {{PYTHON}} {{NODE}} {{ not a var }} {{lower}}", None);
        assert!(import_local(&ok).is_ok());
    }

    #[test]
    fn builtin_deny_covers_spec_section_18() {
        for must in [
            "Bash(git push:*)",
            "Bash(git commit:*)",
            "Bash(git reset:*)",
            "Bash(git rebase:*)",
            "Bash(git merge:*)",
            "Bash(mysql:*)",
            "Bash(mysqlsh:*)",
            "Bash(mongosh:*)",
            "Bash(mongo:*)",
            "Bash(redis-cli:*)",
            "Bash(psql:*)",
            "Bash(lark-cli im:*)",
            "PowerShell(Remove-Item:*)",
            "Bash(git pull:*)",
            "Bash(git fetch:*)",
            "Bash(git checkout:*)",
            "Bash(git switch:*)",
            "Bash(git clean:*)",
            "Bash(git restore:*)",
            "Bash(git stash:*)",
            "Bash(git am:*)",
            "Bash(git apply:*)",
            "PowerShell(git pull:*)",
            "PowerShell(git apply:*)",
            "Bash(git -C:*)",
            "Bash(git -c:*)",
            "Bash(bash -c:*)",
            "Bash(sh -c:*)",
            "Bash(cmd /c:*)",
            "Bash(cmd.exe:*)",
            "PowerShell(powershell:*)",
            "PowerShell(pwsh:*)",
            "PowerShell(Invoke-Expression:*)",
            "PowerShell(iex:*)",
            "PowerShell(Start-Process:*)",
            "Bash(lark-cli.cmd im:*)",
            "Bash(lark-cli api:*)",
            "Bash(lark-cli.cmd api:*)",
            "Bash(mysql.exe:*)",
            "Bash(psql.exe:*)",
            "Bash(unlink:*)",
            "Bash(find -delete:*)",
            "PowerShell(erase:*)",
            "PowerShell(rd:*)",
        ] {
            assert!(BUILTIN_DENY.contains(&must), "缺少 {must}");
        }
        assert!(BUILTIN_DENY.iter().any(|r| r.starts_with("Bash(kafka-")));
        let mut sorted = BUILTIN_DENY.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), BUILTIN_DENY.len(), "禁止清单有重复项");
    }
}
