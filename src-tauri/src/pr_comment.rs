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

/// 评论正文（markdown）。传进来之前应已 `redacted()`。结论和发现不截短（单条超长才截）。
pub fn comment_body(r: &ReviewResult) -> String {
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
        s.push_str(&format!("#### 发现 {} 项\n\n", open.len()));
        for (i, f) in open.iter().enumerate() {
            let text = clip(&f.text, FINDING_MAX).replace('\n', "\n   ");
            s.push_str(&format!("{}. **[{}]** {}{}\n", i + 1, f.severity.label(), status_note(f.status, incremental), text));
        }
        s.push('\n');
    }
    if !resolved.is_empty() {
        s.push_str(&format!("#### 上一轮已处理 {} 项\n\n", resolved.len()));
        for f in &resolved {
            s.push_str(&format!("- ~~[{}] {}~~\n", f.severity.label(), clip(&f.text, FINDING_MAX).replace('\n', " ")));
        }
        s.push('\n');
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
        assert!(b.starts_with("### 代码审查：2 项发现\n\n**结论**：有一处并发问题。\n建议先修。\n\n#### 发现 2 项\n\n"), "{b}");
        assert!(b.contains("1. **[高]** （上一轮遗留，未处理）领取接口没有加锁\n   c.go:88\n"), "{b}");
        assert!(b.contains("2. **[低]** （新发现）日志打印了完整请求体 a.go:12\n"), "{b}");
        assert!(b.contains("#### 上一轮已处理 1 项\n\n- ~~[中] 超时没有重试 d.go:7~~\n"), "{b}");
        assert!(b.contains("<sub>规模 +120 / -9，6 个文件 · 审查覆盖 6/6 个文件 · 提交 0123456789 · 由 Beetle 代码审查机器人生成</sub>"), "{b}");
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
