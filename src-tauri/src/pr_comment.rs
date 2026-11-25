//! 代码审查结果同步到 PR 评论（Gitee / GitHub）。
//!
//! - 评论由客户端发，Agent 仍然只读（模板里禁止它在 PR 上评论）。
//! - token 和建 issue 共用：PR 所在仓库是某个已登记项目的仓库时用该项目的 `token_env`，否则 Gitee 用 `GITEE_TOKEN`、GitHub 用 `GITHUB_TOKEN`。
//! - 每轮审查发一条新评论（增量复审也是新评论，方便看每轮的变化）。
//! - token 不进日志、不进错误信息。

use crate::dispatcher::pr_link::{PrPlatform, PrRef};
use crate::issue::{self, Platform};
use crate::review_card::{FindingStatus, ReviewResult};
use crate::store::config::AppConfig;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use std::time::Duration;

/// 评论正文里单条发现最多多少字（评论不像卡片那样挤，给足）。
const FINDING_MAX: usize = 2000;

/// 发评论：`(PR, token 环境变量名, 正文)` → 评论链接。可注入，测试里换成假的。
pub type PrCommentFn = Arc<dyn Fn(PrRef, String, String) -> BoxFuture<'static, Result<String, String>> + Send + Sync>;

/// 评论的结果，卡片底部显示。
#[derive(Debug, Clone, PartialEq)]
pub enum CommentOutcome {
    /// 已发出，带评论链接。
    Posted(String),
    /// 没发出去，带原因（不含 token）。
    Failed(String),
    /// 没有可识别的 PR，不评论。
    Skipped,
}

fn platform(p: PrPlatform) -> Platform {
    match p {
        PrPlatform::Gitee => Platform::Gitee,
        PrPlatform::Github => Platform::Github,
    }
}

/// 读 token 的环境变量名：PR 仓库是已登记项目的仓库（平台、owner、repo 都对上，不分大小写）就用项目配置的，否则用平台默认。
pub fn token_env(pr: &PrRef, cfg: &AppConfig) -> String {
    let plat = platform(pr.platform);
    cfg.projects
        .values()
        .find(|p| {
            issue::parse_repo(&p.repo).is_some_and(|(host, owner, repo)| {
                Platform::from_host(&host) == Some(plat)
                    && owner.eq_ignore_ascii_case(&pr.owner)
                    && repo.eq_ignore_ascii_case(&pr.repo)
            })
        })
        .and_then(|p| p.token_env.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from))
        .unwrap_or_else(|| plat.default_token_env().to_string())
}

fn status_note(s: FindingStatus, incremental: bool) -> &'static str {
    match s {
        FindingStatus::Carried => "（上一轮遗留，未处理）",
        FindingStatus::Resolved => "（上一轮发现，已处理）",
        FindingStatus::New if incremental => "（新发现）",
        FindingStatus::New => "",
    }
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 评论正文总长上限：GitHub 评论最多 65536 字符，Gitee 也有类似限制，留余量。
const BODY_MAX: usize = 60_000;
/// 精简模式下每段详情最多多少字。
const DETAIL_COMPACT_MAX: usize = 300;

/// 按位置的扩展名猜代码块语言（猜不到就不写）。
fn snippet_lang(locations: &[String]) -> &'static str {
    let Some(loc) = locations.first() else { return "" };
    let path = loc.split([':', ' ', '（', '(']).next().unwrap_or("");
    match path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("go") => "go",
        Some("py") => "python",
        Some("rs") => "rust",
        Some("ts") => "ts",
        Some("tsx") => "tsx",
        Some("js" | "mjs" | "cjs") => "js",
        Some("jsx") => "jsx",
        Some("vue") => "vue",
        Some("java") => "java",
        Some("kt") => "kotlin",
        Some("sql") => "sql",
        Some("sh") => "bash",
        Some("yaml" | "yml") => "yaml",
        Some("json") => "json",
        Some("toml") => "toml",
        _ => "",
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 一条主要发现：小标题 → 位置 → 概括 → 代码片段 → 展开说明。`compact` 时不放代码片段、详情截短（正文超长时用）。
fn finding_block(s: &mut String, i: usize, f: &crate::review_card::Finding, incremental: bool, compact: bool) {
    let title = f.title.trim();
    let text = clip(&f.text, FINDING_MAX);
    let head = if title.is_empty() { clip(&one_line(&text), 80) } else { one_line(title) };
    s.push_str(&format!("#### {i}. [{}] {head}{}\n\n", f.severity.label(), status_note(f.status, incremental)));
    if !f.locations.is_empty() {
        let locs: Vec<String> = f.locations.iter().map(|l| format!("`{}`", one_line(l).replace('`', "'"))).collect();
        s.push_str(&format!("{}\n\n", locs.join(" · ")));
    }
    if !text.is_empty() && (!title.is_empty() || head != one_line(&text)) {
        s.push_str(&format!("{text}\n\n"));
    }
    let snippet = f.snippet.trim_matches('\n');
    if !compact && !snippet.trim().is_empty() {
        let fence = if snippet.contains("```") { "````" } else { "```" };
        s.push_str(&format!("{fence}{}\n{}\n{fence}\n\n", snippet_lang(&f.locations), clip(snippet, 4000)));
    }
    if !f.details.is_empty() {
        for d in &f.details {
            let d = if compact { clip(d, DETAIL_COMPACT_MAX) } else { clip(d, FINDING_MAX) };
            s.push_str(&format!("- {}\n", d.replace('\n', "\n  ")));
        }
        s.push('\n');
    }
}

fn list(s: &mut String, heading: &str, items: &[String], compact: bool) {
    if items.is_empty() {
        return;
    }
    s.push_str(&format!("**{heading}**\n\n"));
    for it in items {
        let it = if compact { clip(it, DETAIL_COMPACT_MAX) } else { clip(it, FINDING_MAX) };
        s.push_str(&format!("- {}\n", it.replace('\n', "\n  ")));
    }
    s.push('\n');
}

/// 评论正文（markdown）。传进来之前应已 `redacted()`。
/// 结构：标题 → 结论 → 主要发现（每条带位置、代码片段、展开说明）→ 次要 → 上一轮已处理 → 核对记录 → 底注。
/// 超过平台长度上限时去掉代码片段、截短详情；还超就硬截断。
pub fn comment_body(r: &ReviewResult) -> String {
    let full = build_body(r, false);
    if full.chars().count() <= BODY_MAX {
        return full;
    }
    let compact = build_body(r, true);
    if compact.chars().count() <= BODY_MAX {
        return compact;
    }
    let mut s: String = compact.chars().take(BODY_MAX - 200).collect();
    s.push_str("\n\n…（评论太长已截断，完整结果见客户端「运行日志」）\n");
    s
}

fn build_body(r: &ReviewResult, compact: bool) -> String {
    let open = r.open_findings();
    let resolved = r.resolved_findings();
    let incremental = r.findings.iter().any(|f| f.status != FindingStatus::New);
    let mut s = String::new();
    if open.is_empty() {
        s.push_str("### 代码审查：未发现问题\n\n");
    } else {
        s.push_str(&format!("### 代码审查：{} 项发现\n\n", open.len()));
    }
    if !r.conclusion.trim().is_empty() {
        s.push_str(&format!("**结论**：{}\n\n", r.conclusion.trim()));
    }
    if !open.is_empty() {
        s.push_str(&format!("### 主要发现（{} 项）\n\n", open.len()));
        for (i, f) in open.iter().enumerate() {
            finding_block(&mut s, i + 1, f, incremental, compact);
        }
    }
    if !r.minor.is_empty() {
        s.push_str(&format!("### 次要（{} 项）\n\n", r.minor.len()));
        for f in &r.minor {
            let title = f.title.trim();
            let body = clip(&f.text, if compact { DETAIL_COMPACT_MAX } else { FINDING_MAX }).replace('\n', " ");
            let head = if title.is_empty() { body.clone() } else { format!("**{}**：{body}", one_line(title)) };
            let locs = if f.locations.is_empty() {
                String::new()
            } else {
                format!("（{}）", f.locations.iter().map(|l| format!("`{}`", one_line(l).replace('`', "'"))).collect::<Vec<_>>().join("、"))
            };
            s.push_str(&format!("- [{}] {head}{locs}\n", f.severity.label()));
        }
        s.push('\n');
    }
    if !resolved.is_empty() {
        s.push_str(&format!("### 上一轮已处理（{} 项）\n\n", resolved.len()));
        for f in &resolved {
            s.push_str(&format!("- ~~[{}] {}~~\n", f.severity.label(), clip(&f.text, FINDING_MAX).replace('\n', " ")));
        }
        s.push('\n');
    }
    let checks = r.checks.clone().unwrap_or_default();
    if !checks.is_empty() || r.coverage.is_some() {
        s.push_str("### 核对记录\n\n");
        list(&mut s, "风险点与结论", &checks.risks, compact);
        let mut scope = Vec::new();
        if let Some(c) = &r.coverage {
            let mut line = format!("审查覆盖 {}/{} 个文件", c.reviewed, c.total);
            if !c.skipped.is_empty() {
                let skipped: Vec<String> = c
                    .skipped
                    .iter()
                    .map(|f| if f.reason.trim().is_empty() { format!("`{}`", f.path) } else { format!("`{}`（{}）", f.path, one_line(&f.reason)) })
                    .collect();
                line.push_str(&format!("，跳过 {} 个：{}", c.skipped.len(), skipped.join("、")));
            }
            scope.push(line);
        }
        scope.extend(checks.scope.iter().cloned());
        list(&mut s, "覆盖范围", &scope, compact);
        list(&mut s, "实际执行", &checks.executed, compact);
    }
    let mut meta = vec![format!("规模 +{} / -{}，{} 个文件", r.additions, r.deletions, r.files)];
    if let Some(c) = &r.coverage {
        meta.push(format!("审查覆盖 {}/{} 个文件", c.reviewed, c.total));
    }
    if let Some(sha) = r.head_sha.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        meta.push(format!("提交 {}", sha.chars().take(10).collect::<String>()));
    }
    s.push_str(&format!("---\n<sub>{} · 由 Beetle 代码审查机器人生成</sub>\n", meta.join(" · ")));
    s
}

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// 调平台接口发评论，返回评论链接（接口没给就用 PR 链接）。错误信息不带 token。
pub async fn post(pr: &PrRef, token: &str, body: &str) -> Result<String, String> {
    let scrub = |s: String| if token.is_empty() { s } else { s.replace(token, "***") };
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("beetle-code-review")
        .build()
        .map_err(|_| "无法初始化 HTTPS 客户端".to_string())?;
    let req = match pr.platform {
        PrPlatform::Gitee => http
            .post(format!(
                "https://gitee.com/api/v5/repos/{}/{}/pulls/{}/comments",
                pct(&pr.owner),
                pct(&pr.repo),
                pr.number
            ))
            .json(&serde_json::json!({ "access_token": token, "body": body })),
        PrPlatform::Github => http
            .post(format!("https://api.github.com/repos/{}/{}/issues/{}/comments", pct(&pr.owner), pct(&pr.repo), pr.number))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&serde_json::json!({ "body": body })),
    };
    let resp = req.send().await.map_err(|e| scrub(format!("网络错误：{e}")))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    match status {
        200..=299 => {}
        401 => return Err("token 无效或已过期（接口返回 401）".into()),
        403 => return Err("token 没有评论这个 PR 的权限（接口返回 403）".into()),
        404 => return Err("找不到这个 PR，或 token 看不到这个仓库（接口返回 404）".into()),
        _ => return Err(format!("接口返回 {status}")),
    }
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    Ok(v.get("html_url").and_then(|u| u.as_str()).filter(|u| u.starts_with("https://")).map(String::from).unwrap_or_else(|| pr.url()))
}

/// 生产实现：读 token（Windows 先读用户级环境变量的最新值）再发评论。
pub fn production() -> PrCommentFn {
    Arc::new(|pr: PrRef, env: String, body: String| {
        Box::pin(async move {
            let name = env.clone();
            let token = tokio::task::spawn_blocking(move || issue::read_token(&name)).await.ok().flatten();
            let Some(token) = token else {
                return Err(format!("未配置 {env}（建 issue 用的同一个 token）"));
            };
            post(&pr, &token, &body).await
        })
    })
}

/// 测试用：记下每次调用 `(PR 链接, token 变量名, 正文)`；`fail` 为 true 时返回失败。
#[cfg(test)]
pub type CommentCalls = Arc<std::sync::Mutex<Vec<(String, String, String)>>>;

#[cfg(test)]
pub fn fake(calls: CommentCalls, fail: Arc<std::sync::atomic::AtomicBool>) -> PrCommentFn {
    Arc::new(move |pr: PrRef, env: String, body: String| {
        calls.lock().unwrap().push((pr.url(), env.clone(), body));
        let failed = fail.load(std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move { if failed { Err(format!("未配置 {env}（建 issue 用的同一个 token）")) } else { Ok(format!("{}#note_1", pr.url())) } })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::pr_link::find_pr_links;
    use crate::review_card::parse;
    use crate::store::config::Project;

    fn result() -> ReviewResult {
        parse(
            r#"{"repo":"demo-service","pr":29,"title":"feat: x","conclusion":"有一处并发问题。\n建议先修。",
            "findings":[
              {"severity":"low","status":"new","text":"日志打印了完整请求体 a.go:12"},
              {"severity":"high","status":"carried","text":"领取接口没有加锁\nc.go:88"},
              {"severity":"medium","status":"resolved","text":"超时没有重试 d.go:7"}],
            "additions":120,"deletions":9,"files":6,"head_sha":"0123456789abcdef",
            "coverage":{"total":6,"reviewed":6,"skipped":[]},
            "url":"https://gitee.com/example/demo-service/pulls/29"}"#,
        )
        .unwrap()
    }

    #[test]
    fn body_lists_findings_full_text_with_meta_footer() {
        let b = comment_body(&result());
        assert!(b.starts_with("### 代码审查：2 项发现\n\n**结论**：有一处并发问题。\n建议先修。\n\n### 主要发现（2 项）\n\n"), "{b}");
        // 旧格式（只有 text）：小标题用 text 压成一行
        assert!(b.contains("#### 1. [高] 领取接口没有加锁 c.go:88（上一轮遗留，未处理）\n\n"), "{b}");
        assert!(b.contains("#### 2. [低] 日志打印了完整请求体 a.go:12（新发现）\n\n"), "{b}");
        assert!(b.contains("### 上一轮已处理（1 项）\n\n- ~~[中] 超时没有重试 d.go:7~~\n"), "{b}");
        assert!(b.contains("### 核对记录\n\n**覆盖范围**\n\n- 审查覆盖 6/6 个文件\n"), "{b}");
        assert!(b.contains("<sub>规模 +120 / -9，6 个文件 · 审查覆盖 6/6 个文件 · 提交 0123456789 · 由 Beetle 代码审查机器人生成</sub>"), "{b}");
    }

    fn detailed() -> ReviewResult {
        parse(
            r#"{"repo":"demo","pr":3,"conclusion":"不建议合并。",
            "findings":[{"severity":"high","status":"new","title":"已退款仍会发通知",
              "text":"退款成功后状态没回写，下一轮又发一次通知。",
              "locations":["flows/rules.py:248","flows/page.py:968"],
              "snippet":"def intercept(x):\n    return x.done",
              "details":["改动前：退款后直接结束。","改动后：只看 done 字段。\n第二行","建议：先查退款状态。"]}],
            "minor":[{"severity":"low","title":"日志太多","text":"每单打印两次","locations":["a.py:1"]},
                     {"severity":"low","text":"变量名可读性一般"}],
            "checks":{"risks":["幂等：同一单重跑 → 不会重复退款"],"scope":["后端 12 个文件，前端 3 个文件"],
                      "executed":["跑了 40 条样例：命中 3 条误判"]},
            "coverage":{"total":15,"reviewed":14,"skipped":[{"path":"x.bin","reason":"二进制"}]},
            "additions":10,"deletions":2,"files":15}"#,
        )
        .unwrap()
    }

    #[test]
    fn body_renders_detailed_findings_minor_and_checks() {
        let b = comment_body(&detailed());
        assert!(b.contains("#### 1. [高] 已退款仍会发通知\n\n`flows/rules.py:248` · `flows/page.py:968`\n\n退款成功后状态没回写，下一轮又发一次通知。\n\n"), "{b}");
        assert!(b.contains("```python\ndef intercept(x):\n    return x.done\n```\n\n"), "{b}");
        assert!(b.contains("- 改动前：退款后直接结束。\n- 改动后：只看 done 字段。\n  第二行\n- 建议：先查退款状态。\n"), "{b}");
        assert!(b.contains("### 次要（2 项）\n\n- [低] **日志太多**：每单打印两次（`a.py:1`）\n- [低] 变量名可读性一般\n"), "{b}");
        assert!(b.contains("**风险点与结论**\n\n- 幂等：同一单重跑 → 不会重复退款\n"), "{b}");
        assert!(b.contains("**覆盖范围**\n\n- 审查覆盖 14/15 个文件，跳过 1 个：`x.bin`（二进制）\n- 后端 12 个文件，前端 3 个文件\n"), "{b}");
        assert!(b.contains("**实际执行**\n\n- 跑了 40 条样例：命中 3 条误判\n"), "{b}");
    }

    #[test]
    fn body_too_long_drops_snippets_then_truncates() {
        let mut r = detailed();
        r.findings[0].snippet = "x".repeat(3000);
        r.findings[0].details = vec!["说明".repeat(900); 40];
        let b = comment_body(&r);
        assert!(b.chars().count() <= 60_000, "{}", b.chars().count());
        assert!(!b.contains("```"), "超长时去掉代码片段");
        assert!(b.contains("由 Beetle 代码审查机器人生成"), "精简后底注还在");
        // 精简后仍超长：硬截断并提示
        r.findings = vec![r.findings[0].clone(); 300];
        let b = comment_body(&r);
        assert!(b.chars().count() <= 60_000);
        assert!(b.ends_with("…（评论太长已截断，完整结果见客户端「运行日志」）\n"));
    }

    #[test]
    fn snippet_lang_from_location() {
        assert_eq!(snippet_lang(&["a/b.go:12".into()]), "go");
        assert_eq!(snippet_lang(&["src/x.tsx:3-9".into()]), "tsx");
        assert_eq!(snippet_lang(&["README（第 3 行）".into()]), "");
        assert_eq!(snippet_lang(&[]), "");
    }

    #[test]
    fn body_without_findings() {
        let mut r = result();
        r.findings.clear();
        r.coverage = None;
        r.head_sha = None;
        let b = comment_body(&r);
        assert!(b.starts_with("### 代码审查：未发现问题\n\n"));
        assert!(!b.contains("#### 发现"));
        assert!(b.contains("<sub>规模 +120 / -9，6 个文件 · 由 Beetle"));
    }

    #[test]
    fn token_env_prefers_matching_project_setting() {
        let mut cfg = AppConfig::default();
        let pr = find_pr_links("https://gitee.com/Example/Demo-Service/pulls/29").remove(0);
        assert_eq!(token_env(&pr, &cfg), "GITEE_TOKEN");
        let mut p = Project::new("git@gitee.com:example/demo-service.git", "release/*", "AGENTS.md", "示例", vec![]);
        p.token_env = Some("TEAM_GITEE_TOKEN".into());
        cfg.projects.insert("demo".into(), p.clone());
        assert_eq!(token_env(&pr, &cfg), "TEAM_GITEE_TOKEN");
        // 同名仓库但在 GitHub 上：不用 Gitee 项目的设置
        let gh = find_pr_links("https://github.com/example/demo-service/pull/3").remove(0);
        assert_eq!(token_env(&gh, &cfg), "GITHUB_TOKEN");
        // 项目没写 token_env：用平台默认
        p.token_env = None;
        cfg.projects.insert("demo".into(), p);
        assert_eq!(token_env(&pr, &cfg), "GITEE_TOKEN");
    }
}
