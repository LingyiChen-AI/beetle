//! 建 issue：按仓库地址推断平台与 owner/repo、读 token 环境变量、检测 token 是否可用。
//!
//! 规则与技能脚本 `resources/skills/_shared/bp_issue.py` 一致（两边改一处要同步另一处）：
//! - 平台按 host 选：gitee.com → Gitee，github.com → GitHub；项目的 `issue_repo` 可改成别的 `owner/repo`（或完整仓库地址）。
//! - token 环境变量默认 Gitee `GITEE_TOKEN`、GitHub `GITHUB_TOKEN`，项目的 `token_env` 可改名。
//! - Windows 上优先读用户级环境变量（`HKCU\Environment`）的最新值：客户端进程的环境是启动时继承的，用户换了 token
//!   不重启也能检测到；没有再读当前进程环境变量。
//! - token 绝不返回给前端、不写日志、不进错误信息。

use crate::store::config::Project;
use serde::Serialize;
use std::future::Future;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Gitee,
    Github,
}

impl Platform {
    pub fn from_host(host: &str) -> Option<Platform> {
        let h = host.trim().to_ascii_lowercase();
        match h.strip_prefix("www.").unwrap_or(&h) {
            "gitee.com" => Some(Platform::Gitee),
            "github.com" => Some(Platform::Github),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Platform::Gitee => "Gitee",
            Platform::Github => "GitHub",
        }
    }

    pub fn default_token_env(self) -> &'static str {
        match self {
            Platform::Gitee => "GITEE_TOKEN",
            Platform::Github => "GITHUB_TOKEN",
        }
    }
}

/// 仓库地址 → (host 小写, owner, repo)。支持 `git@host:owner/repo(.git)`、`ssh://git@host[:port]/owner/repo.git`、
/// `https://[user:pass@]host/owner/repo(.git)`、`host/owner/repo`。解析不了返回 None。
pub fn parse_repo(url: &str) -> Option<(String, String, String)> {
    let s = url.trim();
    if s.is_empty() {
        return None;
    }
    let (host, path): (String, &str) = if let Some(i) = s.find("://") {
        let rest = &s[i + 3..];
        let (auth, path) = match rest.find('/') {
            Some(j) => (&rest[..j], &rest[j..]),
            None => (rest, ""),
        };
        let hostport = auth.rsplit('@').next().unwrap_or(auth);
        (hostport.split(':').next().unwrap_or("").to_ascii_lowercase(), path)
    } else if let Some((prefix, path)) = s.split_once(':').filter(|(p, _)| !p.contains('/') && !p.contains(char::is_whitespace)) {
        // scp 形式：git@host:owner/repo
        (prefix.rsplit('@').next().unwrap_or(prefix).to_ascii_lowercase(), path)
    } else {
        let (head, path) = s.split_once('/').unwrap_or((s, ""));
        (head.to_ascii_lowercase(), path)
    };
    let parts: Vec<&str> = path.trim_matches('/').split('/').filter(|x| !x.is_empty()).collect();
    if host.is_empty() || parts.len() < 2 {
        return None;
    }
    let repo = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    if repo.is_empty() {
        return None;
    }
    Some((host, parts[0].to_string(), repo.to_string()))
}

/// 仓库地址对应的 issue 平台（不是 gitee.com / github.com 返回 None）。
pub fn platform_of(repo_url: &str) -> Option<Platform> {
    parse_repo(repo_url).and_then(|(h, _, _)| Platform::from_host(&h))
}

/// `issue_repo`：`owner/repo`，或完整仓库地址（此时平台也按它的 host 选）。
fn parse_override(s: &str) -> Option<(Option<Platform>, String, String)> {
    let s = s.trim();
    let parts: Vec<&str> = s.trim_matches('/').split('/').filter(|x| !x.is_empty()).collect();
    if parts.len() == 2 && !s.contains(':') && !s.contains('@') {
        let repo = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
        return Some((None, parts[0].to_string(), repo.to_string()));
    }
    let (h, o, r) = parse_repo(s)?;
    Some((Platform::from_host(&h), o, r))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssueTarget {
    pub platform: Platform,
    pub owner: String,
    pub repo: String,
    pub token_env: String,
}

impl IssueTarget {
    pub fn full(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// 项目的建 issue 目标；仓库不是 gitee.com / github.com（且 `issue_repo` 也没给出平台）时返回 None。
pub fn target(p: &Project) -> Option<IssueTarget> {
    let base = parse_repo(&p.repo);
    let base_platform = base.as_ref().and_then(|(h, _, _)| Platform::from_host(h));
    let (platform, owner, repo) = match p.issue_repo.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(o) => {
            let (op, owner, repo) = parse_override(o)?;
            (op.or(base_platform)?, owner, repo)
        }
        None => {
            let (_, owner, repo) = base?;
            (base_platform?, owner, repo)
        }
    };
    let token_env = p
        .token_env
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(platform.default_token_env())
        .to_string();
    Some(IssueTarget { platform, owner, repo, token_env })
}

/// 环境变量名只允许字母、数字、下划线，且不以数字开头。
pub fn valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn non_empty(v: String) -> Option<String> {
    let t = v.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// 读 token：Windows 先取用户级环境变量的最新值，没有再取当前进程环境变量。
pub fn read_token(name: &str) -> Option<String> {
    if !valid_env_name(name) {
        return None;
    }
    user_env(name).and_then(non_empty).or_else(|| std::env::var(name).ok().and_then(non_empty))
}

#[cfg(windows)]
fn user_env(name: &str) -> Option<String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("reg");
    cmd.args(["query", r"HKCU\Environment", "/v", name])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = crate::platform::proc::spawn_hidden(&mut cmd).ok()?;
    let out = crate::platform::proc::wait_output_timeout(child, Duration::from_secs(5)).ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reg_query(&String::from_utf8_lossy(&out.stdout), name)
}

#[cfg(not(windows))]
fn user_env(_name: &str) -> Option<String> {
    None
}

/// 解析 `reg query HKCU\Environment /v NAME` 的输出：`    NAME    REG_SZ    value`。
fn parse_reg_query(out: &str, name: &str) -> Option<String> {
    for line in out.lines() {
        let mut it = line.split_whitespace();
        if it.next().is_some_and(|n| n.eq_ignore_ascii_case(name))
            && it.next().is_some_and(|k| k.starts_with("REG_"))
        {
            let v = it.collect::<Vec<_>>().join(" ");
            return non_empty(v);
        }
    }
    None
}

/// `check_issue_token` 的结果：只有平台、仓库、环境变量名、是否检测到、用户名，**不含 token**。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IssueTokenStatus {
    pub platform: Option<Platform>,
    pub platform_label: Option<String>,
    /// `owner/repo`
    pub repo: Option<String>,
    pub token_env: Option<String>,
    /// 环境变量里有 token。
    pub detected: bool,
    /// token 对应的账号（接口验证通过时）。
    pub user: Option<String>,
    /// 无法建 issue 的原因 / 接口验证失败的原因（已去掉 token）。
    pub error: Option<String>,
}

/// 去掉文字里的 token（原文和 URL 里的 access_token=）。
fn scrub(text: &str, token: &str) -> String {
    let s = if token.is_empty() { text.to_string() } else { text.replace(token, "***") };
    crate::redact::redact(&s)
}

/// 检测逻辑（读 token、调接口都可注入，单测不碰真实环境和网络）。
pub async fn check_with<R, W, Fut>(project: &Project, read: R, whoami: W) -> IssueTokenStatus
where
    R: FnOnce(&str) -> Option<String>,
    W: FnOnce(Platform, String) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    let Some(t) = target(project) else {
        return IssueTokenStatus {
            error: Some("仓库地址不是 gitee.com / github.com，无法建 issue（可填写 issue 仓库）".into()),
            ..IssueTokenStatus::default()
        };
    };
    let mut st = IssueTokenStatus {
        platform: Some(t.platform),
        platform_label: Some(t.platform.label().to_string()),
        repo: Some(t.full()),
        token_env: Some(t.token_env.clone()),
        ..IssueTokenStatus::default()
    };
    let Some(token) = read(&t.token_env) else {
        st.error = Some(format!("未检测到环境变量 {}", t.token_env));
        return st;
    };
    st.detected = true;
    match whoami(t.platform, token.clone()).await {
        Ok(u) => st.user = Some(scrub(&u, &token)),
        Err(e) => st.error = Some(scrub(&e, &token)),
    }
    st
}

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// 用 token 调「当前用户」接口，返回登录名。错误信息里不带 token。
pub async fn whoami(platform: Platform, token: &str) -> Result<String, String> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("beetle-issue-record")
        .build()
        .map_err(|_| "无法初始化 HTTPS 客户端".to_string())?;
    let req = match platform {
        Platform::Github => http
            .get("https://api.github.com/user")
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28"),
        Platform::Gitee => http.get(format!("https://gitee.com/api/v5/user?access_token={}", pct(token))),
    };
    let resp = req.send().await.map_err(|e| scrub(&format!("网络错误：{e}"), token))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| scrub(&format!("读取响应失败：{e}"), token))?;
    if status == 401 {
        return Err("token 无效或已过期（接口返回 401）".into());
    }
    if !(200..300).contains(&status) {
        return Err(format!("接口返回 {status}"));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| "接口返回的不是 JSON".to_string())?;
    v.get("login")
        .and_then(|x| x.as_str())
        .map(|s| scrub(s, token))
        .ok_or_else(|| "接口返回里没有用户名".to_string())
}

/// 项目页「记录方式」：检测建 issue 用的 token。传入表单里当前的项目（不必先保存）。只返回布尔和用户名，不返回 token。
#[tauri::command]
pub async fn check_issue_token(project: Project) -> Result<IssueTokenStatus, String> {
    let env = target(&project).map(|t| t.token_env);
    let token = match env {
        Some(e) => crate::commands::blocking(move || Ok(read_token(&e))).await?,
        None => None,
    };
    Ok(check_with(&project, move |_| token, |p, tok| async move { whoami(p, &tok).await }).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::config::RecordOpts;

    fn project(repo: &str) -> Project {
        Project::new(repo, "release/*", "AGENTS.md", "示例", vec![])
    }

    fn parsed(url: &str) -> Option<(String, String, String)> {
        parse_repo(url)
    }

    fn t(h: &str, o: &str, r: &str) -> Option<(String, String, String)> {
        Some((h.into(), o.into(), r.into()))
    }

    #[test]
    fn parse_repo_supports_scp_https_ssh_and_git_suffix() {
        assert_eq!(parsed("git@gitee.com:example/demo-issues.git"), t("gitee.com", "example", "demo-issues"));
        assert_eq!(parsed("git@github.com:example/demo-issues"), t("github.com", "example", "demo-issues"));
        assert_eq!(parsed("https://gitee.com/example/demo-issues"), t("gitee.com", "example", "demo-issues"));
        assert_eq!(parsed("https://github.com/example/demo-issues.git/"), t("github.com", "example", "demo-issues"));
        assert_eq!(parsed("https://me:tok@GitHub.com/a/b.git"), t("github.com", "a", "b"));
        assert_eq!(parsed("ssh://git@gitee.com:22/a/b.git"), t("gitee.com", "a", "b"));
        assert_eq!(parsed("gitee.com/a/demo.git"), t("gitee.com", "a", "demo"));
        assert_eq!(parsed("https://gitee.com/onlyowner"), None);
        assert_eq!(parsed(""), None);
    }

    #[test]
    fn platform_by_host() {
        assert_eq!(platform_of("git@gitee.com:a/b.git"), Some(Platform::Gitee));
        assert_eq!(platform_of("https://www.github.com/a/b"), Some(Platform::Github));
        assert_eq!(platform_of("git@gitlab.com:a/b.git"), None);
        assert_eq!(platform_of("git@x:a/demo.git"), None);
    }

    #[test]
    fn target_defaults_and_overrides() {
        let mut p = project("git@gitee.com:a/demo.git");
        assert_eq!(
            target(&p),
            Some(IssueTarget { platform: Platform::Gitee, owner: "a".into(), repo: "demo".into(), token_env: "GITEE_TOKEN".into() })
        );
        p.issue_repo = Some(" other/issues.git ".into());
        p.token_env = Some("MY_GITEE".into());
        let got = target(&p).unwrap();
        assert_eq!((got.full(), got.token_env.as_str(), got.platform), ("other/issues".to_string(), "MY_GITEE", Platform::Gitee));
        // 仓库不在 gitee / github：issue_repo 给完整地址时按它的平台
        let mut q = project("git@gitlab.com:a/b.git");
        assert_eq!(target(&q), None);
        q.issue_repo = Some("owner/repo".into());
        assert_eq!(target(&q), None, "只给 owner/repo 推不出平台");
        q.issue_repo = Some("https://github.com/o/r".into());
        let got = target(&q).unwrap();
        assert_eq!((got.platform, got.full(), got.token_env.as_str()), (Platform::Github, "o/r".to_string(), "GITHUB_TOKEN"));
    }

    #[test]
    fn default_record_follows_repo_host() {
        assert_eq!(RecordOpts::default_for("git@gitee.com:a/b.git"), RecordOpts { issue: true, feishu_doc: true });
        assert_eq!(RecordOpts::default_for("https://github.com/a/b"), RecordOpts { issue: true, feishu_doc: true });
        assert_eq!(RecordOpts::default_for("git@x:a/b.git"), RecordOpts { issue: false, feishu_doc: true });
    }

    #[test]
    fn reg_query_output_is_parsed() {
        let out = "\r\nHKEY_CURRENT_USER\\Environment\r\n    GITEE_TOKEN    REG_SZ    abc123\r\n\r\n";
        assert_eq!(parse_reg_query(out, "GITEE_TOKEN").as_deref(), Some("abc123"));
        assert_eq!(parse_reg_query(out, "gitee_token").as_deref(), Some("abc123"));
        assert_eq!(parse_reg_query(out, "GITHUB_TOKEN"), None);
        assert!(valid_env_name("GITEE_TOKEN") && !valid_env_name("A B") && !valid_env_name("1X") && !valid_env_name(""));
        assert_eq!(read_token("bad name"), None, "不合法的变量名不去读");
    }

    #[tokio::test]
    async fn check_reports_missing_token_without_calling_api() {
        let p = project("git@gitee.com:a/demo.git");
        let st = check_with(&p, |_| None, |_, _| async { Err::<String, String>("调用了接口".into()) }).await;
        assert!(!st.detected);
        assert_eq!(st.token_env.as_deref(), Some("GITEE_TOKEN"));
        assert_eq!(st.repo.as_deref(), Some("a/demo"));
        assert_eq!(st.platform, Some(Platform::Gitee));
        let err = st.error.unwrap();
        assert!(err.contains("GITEE_TOKEN") && !err.contains("调用了接口"), "没有 token 不应调接口：{err}");
    }

    #[tokio::test]
    async fn check_never_leaks_token() {
        const TOKEN: &str = "ghp_SECRET_abcdef1234567890";
        let p = project("https://github.com/o/r");
        let ok = check_with(&p, |name| (name == "GITHUB_TOKEN").then(|| TOKEN.to_string()), |pl, tok| async move {
            assert_eq!(pl, Platform::Github);
            assert_eq!(tok, TOKEN, "接口拿到的是真实 token");
            Ok::<String, String>("octocat".to_string())
        })
        .await;
        assert!(ok.detected && ok.error.is_none());
        assert_eq!(ok.user.as_deref(), Some("octocat"));
        let ok_json = serde_json::to_string(&ok).unwrap();
        assert!(!ok_json.contains(TOKEN), "{ok_json}");

        let bad = check_with(&p, |_| Some(TOKEN.to_string()), |_, tok| async move {
            Err::<String, String>(format!("请求 https://gitee.com/api/v5/user?access_token={tok} 失败，token={tok}"))
        })
        .await;
        assert!(bad.detected && bad.user.is_none());
        let bad_json = serde_json::to_string(&bad).unwrap();
        assert!(!bad_json.contains(TOKEN) && !bad_json.contains("SECRET"), "{bad_json}");
        assert!(bad.error.unwrap().contains("失败"));
    }

    #[tokio::test]
    async fn check_unsupported_repo_returns_reason() {
        let st = check_with(&project("git@gitlab.com:a/b.git"), |_| Some("x".into()), |_, _| async { Ok::<String, String>("u".to_string()) }).await;
        assert!(!st.detected && st.platform.is_none() && st.repo.is_none());
        assert!(st.error.unwrap().contains("无法建 issue"));
    }

    #[test]
    fn pct_encodes_reserved_chars() {
        assert_eq!(pct("ab-_.~9"), "ab-_.~9");
        assert_eq!(pct("a&b=c"), "a%26b%3Dc");
    }
}
