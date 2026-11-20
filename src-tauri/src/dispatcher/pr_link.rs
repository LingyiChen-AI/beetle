//! 群消息里的 PR 链接识别与「同一 PR 短时间内不重复审」。
//!
//! 认两种链接（带查询串、`#锚点`、末尾斜杠、`/files` 之类的子页面都认）：
//! - Gitee：`https://gitee.com/{owner}/{repo}/pulls/{n}`
//! - GitHub：`https://github.com/{owner}/{repo}/pull/{n}`
//!
//! 一条消息里多个链接各算一个（同一个 PR 贴两遍只算一次）；平台与路径写法不配套的（Gitee 的 `/pull/`、GitHub 的 `/pulls/`）不认。

use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrPlatform {
    Gitee,
    Github,
}

impl PrPlatform {
    pub fn as_str(self) -> &'static str {
        match self {
            PrPlatform::Gitee => "gitee",
            PrPlatform::Github => "github",
        }
    }
}

/// 一个 PR：平台 + 仓库 + 编号。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrRef {
    pub platform: PrPlatform,
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl PrRef {
    /// 规范链接（去掉查询串、锚点、子页面），传给 Agent 和脚本用。
    pub fn url(&self) -> String {
        match self.platform {
            PrPlatform::Gitee => format!("https://gitee.com/{}/{}/pulls/{}", self.owner, self.repo, self.number),
            PrPlatform::Github => format!("https://github.com/{}/{}/pull/{}", self.owner, self.repo, self.number),
        }
    }

    /// 去重键：两个平台的 owner / repo 都不区分大小写。
    pub fn key(&self) -> String {
        format!(
            "{}/{}/{}#{}",
            self.platform.as_str(),
            self.owner.to_ascii_lowercase(),
            self.repo.to_ascii_lowercase(),
            self.number
        )
    }
}

fn pr_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)https?://(?:www\.)?(gitee\.com|github\.com)/([A-Za-z0-9_.\-]+)/([A-Za-z0-9_.\-]+)/(pulls|pull)/([0-9]{1,9})")
            .expect("PR 链接正则")
    })
}

/// 找出文本里所有 PR 链接，按出现顺序、去重。
pub fn find_pr_links(text: &str) -> Vec<PrRef> {
    let mut out: Vec<PrRef> = Vec::new();
    for c in pr_regex().captures_iter(text) {
        let whole = c.get(0).expect("整段匹配");
        // 编号后面紧跟字母数字（如 `/pull/12abc`）不是 PR 链接
        if text[whole.end()..].chars().next().is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            continue;
        }
        let platform = match c[1].to_ascii_lowercase().as_str() {
            "gitee.com" => PrPlatform::Gitee,
            _ => PrPlatform::Github,
        };
        let kind = c[4].to_ascii_lowercase();
        let path_ok = match platform {
            PrPlatform::Gitee => kind == "pulls",
            PrPlatform::Github => kind == "pull",
        };
        let (owner, repo) = (c[2].to_string(), c[3].to_string());
        if !path_ok || [&owner, &repo].iter().any(|s| s.as_str() == "." || s.as_str() == "..") {
            continue;
        }
        let Ok(number) = c[5].parse::<u64>() else { continue };
        if number == 0 {
            continue;
        }
        let pr = PrRef { platform, owner, repo, number };
        if !out.iter().any(|p| p.key() == pr.key()) {
            out.push(pr);
        }
    }
    out
}

/// 同一 PR 在窗口期（默认 10 分钟）内只审一次：记下每个 PR 最近一次开审的运行 id。
pub struct PrDedupe {
    window: Duration,
    clock: Box<dyn Fn() -> Instant + Send + Sync>,
    map: HashMap<String, (Instant, String)>,
}

impl PrDedupe {
    pub fn new() -> Self {
        Self::with_clock(Duration::from_secs(600), Box::new(Instant::now))
    }

    pub fn with_clock(window: Duration, clock: Box<dyn Fn() -> Instant + Send + Sync>) -> Self {
        PrDedupe { window, clock, map: HashMap::new() }
    }

    /// 窗口期内审过 / 正在审这个 PR 时返回那次运行的 id。
    pub fn recent(&self, key: &str) -> Option<String> {
        let now = (self.clock)();
        self.map
            .get(key)
            .filter(|(t, _)| now.saturating_duration_since(*t) < self.window)
            .map(|(_, id)| id.clone())
    }

    /// 记下这个 PR 刚开审（顺带清掉过期的记录）。
    pub fn record(&mut self, key: &str, run_id: &str) {
        let now = (self.clock)();
        let window = self.window;
        self.map.retain(|_, (t, _)| now.saturating_duration_since(*t) < window);
        self.map.insert(key.to_string(), (now, run_id.to_string()));
    }

    /// 那次运行没成功（失败、超时、取消）：放开这个 PR，允许马上重贴重审。只放开同一次运行记下的。
    pub fn release(&mut self, key: &str, run_id: &str) {
        if self.map.get(key).is_some_and(|(_, id)| id == run_id) {
            self.map.remove(key);
        }
    }
}

impl Default for PrDedupe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn urls(text: &str) -> Vec<String> {
        find_pr_links(text).iter().map(PrRef::url).collect()
    }

    #[test]
    fn recognizes_gitee_and_github_pr_link_forms() {
        for (text, want) in [
            ("https://gitee.com/example/demo-service/pulls/29", "https://gitee.com/example/demo-service/pulls/29"),
            ("看下 https://gitee.com/example/demo-service/pulls/29/ 谢谢", "https://gitee.com/example/demo-service/pulls/29"),
            ("https://gitee.com/example/demo-service/pulls/29?tab=files", "https://gitee.com/example/demo-service/pulls/29"),
            ("https://gitee.com/example/demo-service/pulls/29#note_1", "https://gitee.com/example/demo-service/pulls/29"),
            ("http://www.github.com/example/demo.service/pull/7/files", "https://github.com/example/demo.service/pull/7"),
            ("review（https://github.com/Example/Demo-Service/pull/123）", "https://github.com/Example/Demo-Service/pull/123"),
            ("HTTPS://GITHUB.COM/example/demo/pull/5?w=1#diff", "https://github.com/example/demo/pull/5"),
        ] {
            assert_eq!(urls(text), vec![want.to_string()], "{text}");
        }
        let p = &find_pr_links("https://gitee.com/example/demo-service/pulls/29")[0];
        assert_eq!((p.platform, p.owner.as_str(), p.repo.as_str(), p.number), (PrPlatform::Gitee, "example", "demo-service", 29));
        assert_eq!(p.key(), "gitee/example/demo-service#29");
    }

    #[test]
    fn multiple_links_each_counted_once_in_order() {
        let text = "两个 PR：https://github.com/example/demo/pull/2 和 https://gitee.com/example/demo-service/pulls/29，\
                    还有一遍 https://github.com/EXAMPLE/demo/pull/2#top";
        assert_eq!(
            urls(text),
            vec!["https://github.com/example/demo/pull/2".to_string(), "https://gitee.com/example/demo-service/pulls/29".to_string()]
        );
    }

    #[test]
    fn non_pr_links_do_not_match() {
        for text in [
            "https://gitee.com/example/demo-service",
            "https://gitee.com/example/demo-service/issues/IKWMBW",
            "https://github.com/example/demo/issues/3",
            "https://github.com/example/demo/pulls",
            "https://github.com/example/demo/pulls/3",
            "https://gitee.com/example/demo-service/pull/29",
            "https://github.com/example/demo/pull/12abc",
            "https://github.com/example/demo/pull/0",
            "https://gitlab.com/example/demo/merge_requests/3",
            "https://example.com/github.com/a/b/pull/1x",
            "github.com/example/demo/pull/3",
            "没有链接",
        ] {
            assert!(find_pr_links(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn pr_dedupe_window_record_and_release() {
        let base = Instant::now();
        let offset = Arc::new(Mutex::new(Duration::ZERO));
        let o = offset.clone();
        let mut d = PrDedupe::with_clock(Duration::from_secs(600), Box::new(move || base + *o.lock().unwrap()));
        assert_eq!(d.recent("k"), None);
        d.record("k", "r1");
        assert_eq!(d.recent("k").as_deref(), Some("r1"));
        *offset.lock().unwrap() = Duration::from_secs(599);
        assert_eq!(d.recent("k").as_deref(), Some("r1"));
        assert_eq!(d.recent("other"), None);
        *offset.lock().unwrap() = Duration::from_secs(601);
        assert_eq!(d.recent("k"), None, "超过 10 分钟可以再审");
        d.record("k", "r2");
        d.release("k", "r1");
        assert_eq!(d.recent("k").as_deref(), Some("r2"), "只放开同一次运行记下的");
        d.release("k", "r2");
        assert_eq!(d.recent("k"), None);
    }
}
