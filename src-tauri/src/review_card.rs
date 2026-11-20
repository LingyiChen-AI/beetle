//! 代码审查机器人的结果：Agent 最终回复的结构化 JSON → 飞书消息卡片（蓝色标题头）。
//!
//! 最终回复约定（模板 `code-review.md` 第 6 步）：
//! `{"repo":"demo-service","pr":29,"title":"...","conclusion":"...","findings":[{"severity":"high|medium|low","status":"new|carried|resolved","text":"..."}],
//!   "additions":11822,"deletions":109,"files":69,"author":"张三","url":"https://...",
//!   "coverage":{"total":6,"reviewed":5,"skipped":[{"path":"a.sql","reason":"..."}]}}`（coverage 可缺省）
//!
//! 解析是宽松的：允许 ```json 代码块包裹、前后有多余文字；数字写成字符串也认；严重度 / 状态认中文写法。
//! 解析不了（不是 JSON、缺仓库或 PR 编号）返回 Err，调用方退回发纯文本。
//!
//! 卡片格式（照用户给的截图）：
//! - 标题：`代码审查 demo-service #29，3 项发现`；没有未处理的发现时 `代码审查 demo-service #29，未发现问题`
//! - 正文：`【代码审查】demo-service #29` / PR 标题 / `**结论**：…`（过长截断加 …）/ `**发现 3 项**` + 每项一行
//!   `- **[中]** （上一轮遗留，未处理）问题描述` / 上一轮已处理的单独列 / `规模：+11822/-109，69 个文件，作者 张三` /
//!   有覆盖率时 `审查覆盖：5/6 个文件，跳过 1 个（a.sql:原因）` / PR 链接。
//!
//! 每轮结果按 PR 存在 `<data>/pr-reviews/<平台>/<owner>/<repo>/<编号>.json`（owner / repo 小写），
//! 下一轮审查由 `pr_review_state.py` 读出来做增量对比。

use crate::dispatcher::pr_link::PrRef;
use crate::platform::paths;
use crate::redact::redact;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// 结论最多显示多少字，超出截断加「…」。
const CONCLUSION_MAX: usize = 200;
/// 单条发现最多显示多少字。
const FINDING_MAX: usize = 300;
/// 卡片里最多列多少条发现（其余提示去运行日志看）。
const FINDINGS_SHOWN: usize = 20;
/// 覆盖行里最多列几个跳过的文件。
const COVERAGE_SKIPPED_SHOWN: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[serde(alias = "High", alias = "HIGH", alias = "高", alias = "critical", alias = "严重")]
    High,
    #[serde(alias = "Medium", alias = "MEDIUM", alias = "中", alias = "med")]
    Medium,
    #[serde(alias = "Low", alias = "LOW", alias = "低")]
    Low,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::High => "高",
            Severity::Medium => "中",
            Severity::Low => "低",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingStatus {
    /// 本轮新发现
    #[default]
    #[serde(alias = "New", alias = "新发现", alias = "新")]
    New,
    /// 上一轮就有、这一轮还在（未处理）
    #[serde(alias = "Carried", alias = "未处理", alias = "遗留")]
    Carried,
    /// 上一轮有、这一轮已经改掉
    #[serde(alias = "Resolved", alias = "已处理", alias = "fixed")]
    Resolved,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    #[serde(default)]
    pub status: FindingStatus,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewResult {
    pub repo: String,
    #[serde(deserialize_with = "lenient_u64")]
    pub pr: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub conclusion: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub additions: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub deletions: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub files: u64,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub url: String,
    /// 本轮审查时的 PR head 提交（可选；下一轮据此判断代码有没有更新）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    /// ocr 审查覆盖率（可选）：ocr 给出的可审文件数、实际审过的数、跳过的文件和原因。
    /// 写坏了（类型不对）当没有，不影响整份结果的解析。
    #[serde(default, deserialize_with = "lenient_coverage", skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
}

/// 跳过没审的文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkippedFile {
    pub path: String,
    #[serde(default)]
    pub reason: String,
}

/// 审查覆盖率：`total` 为 ocr 给出的可审文件数，`reviewed` 为实际审过的数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    #[serde(default, deserialize_with = "lenient_u64")]
    pub total: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub reviewed: u64,
    #[serde(default, alias = "skipped_files")]
    pub skipped: Vec<SkippedFile>,
}

fn lenient_coverage<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Coverage>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(serde_json::from_value(v).ok())
}

/// 数字也可能被写成字符串（`"29"`、`"+11822"`），都认；null 当 0。
fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let v = Value::deserialize(d)?;
    match &v {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
            .ok_or_else(|| serde::de::Error::custom("不是非负整数")),
        Value::String(s) => {
            let t = s.trim().trim_start_matches(['+', '#']).replace(',', "");
            t.parse().map_err(|_| serde::de::Error::custom(format!("不是数字：{s}")))
        }
        Value::Null => Ok(0),
        _ => Err(serde::de::Error::custom("不是数字")),
    }
}

impl ReviewResult {
    /// 还没处理的发现（新发现 + 上一轮遗留），按严重度从高到低，同级保持原顺序。
    pub fn open_findings(&self) -> Vec<&Finding> {
        let mut v: Vec<&Finding> = self.findings.iter().filter(|f| f.status != FindingStatus::Resolved).collect();
        v.sort_by_key(|f| f.severity);
        v
    }

    pub fn resolved_findings(&self) -> Vec<&Finding> {
        self.findings.iter().filter(|f| f.status == FindingStatus::Resolved).collect()
    }

    /// 发到群里之前把所有文字过一遍打码。
    pub fn redacted(&self) -> ReviewResult {
        let mut r = self.clone();
        for s in [&mut r.repo, &mut r.title, &mut r.conclusion, &mut r.author, &mut r.url] {
            *s = redact(s);
        }
        for f in &mut r.findings {
            f.text = redact(&f.text);
        }
        if let Some(c) = &mut r.coverage {
            for s in &mut c.skipped {
                s.path = redact(&s.path);
                s.reason = redact(&s.reason);
            }
        }
        r
    }
}

fn try_parse(s: &str) -> Option<ReviewResult> {
    let r: ReviewResult = serde_json::from_str(s.trim()).ok()?;
    (!r.repo.trim().is_empty() && r.pr > 0).then_some(r)
}

/// 宽松解析最终回复：原文 → ```json 代码块里的内容 → 第一个 `{` 到最后一个 `}`。
pub fn parse(text: &str) -> Result<ReviewResult, String> {
    let text = text.trim().trim_start_matches('\u{feff}');
    if let Some(r) = try_parse(text) {
        return Ok(r);
    }
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        // 跳过语言标记那一行（```json）
        let body = after.split_once('\n').map(|(_, b)| b).unwrap_or(after);
        if let Some(end) = body.find("```")
            && let Some(r) = try_parse(&body[..end])
        {
            return Ok(r);
        }
    }
    if let (Some(a), Some(b)) = (text.find('{'), text.rfind('}'))
        && a < b
        && let Some(r) = try_parse(&text[a..=b])
    {
        return Ok(r);
    }
    Err("最终回复里没有找到约定格式的审查结果 JSON（需要 repo、pr 字段）".into())
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 一行里的文字：换行压成空格，免得一条发现在卡片里断成好几行。
fn one_line(s: &str, max: usize) -> String {
    truncate(&s.split_whitespace().collect::<Vec<_>>().join(" "), max)
}

/// 卡片标题。
pub fn card_title(r: &ReviewResult) -> String {
    let n = r.open_findings().len();
    if n == 0 {
        format!("代码审查 {} #{}，未发现问题", r.repo, r.pr)
    } else {
        format!("代码审查 {} #{}，{n} 项发现", r.repo, r.pr)
    }
}

/// 这是不是一轮增量复审（结果里有上一轮的遗留或已处理项）。
fn is_incremental(r: &ReviewResult) -> bool {
    r.findings.iter().any(|f| f.status != FindingStatus::New)
}

fn status_note(f: &Finding, incremental: bool) -> &'static str {
    match f.status {
        FindingStatus::Carried => "（上一轮遗留，未处理）",
        FindingStatus::Resolved => "（上一轮发现，已处理）",
        FindingStatus::New if incremental => "（新发现）",
        FindingStatus::New => "",
    }
}

fn finding_line(f: &Finding, incremental: bool) -> String {
    format!("- **[{}]** {}{}", f.severity.label(), status_note(f, incremental), one_line(&f.text, FINDING_MAX))
}

fn scale_line(r: &ReviewResult) -> String {
    let mut s = format!("规模：+{}/-{}，{} 个文件", r.additions, r.deletions, r.files);
    if !r.author.trim().is_empty() {
        s.push_str(&format!("，作者 {}", r.author.trim()));
    }
    s
}

/// 审查覆盖行：`审查覆盖：5/6 个文件，跳过 1 个（a.sql:无法取 diff）`；跳过的最多列 3 个，多的写「等」。
fn coverage_line(c: &Coverage) -> String {
    let mut s = format!("审查覆盖：{}/{} 个文件", c.reviewed, c.total);
    if !c.skipped.is_empty() {
        let shown: Vec<String> = c
            .skipped
            .iter()
            .take(COVERAGE_SKIPPED_SHOWN)
            .map(|f| {
                let path = one_line(&f.path, 120);
                let reason = one_line(&f.reason, 60);
                if reason.is_empty() { path } else { format!("{path}:{reason}") }
            })
            .collect();
        let more = if c.skipped.len() > COVERAGE_SKIPPED_SHOWN { " 等" } else { "" };
        s.push_str(&format!("，跳过 {} 个（{}{more}）", c.skipped.len(), shown.join("；")));
    }
    s
}

/// 卡片正文（飞书 markdown）。`link` 为 true 时 PR 链接写成可点的 markdown 链接（纯文本版写原样链接）。
fn body_lines(r: &ReviewResult, link: bool) -> Vec<String> {
    let mut lines = vec![format!("【代码审查】{} #{}", r.repo, r.pr)];
    if !r.title.trim().is_empty() {
        lines.push(one_line(&r.title, 120));
    }
    if !r.conclusion.trim().is_empty() {
        lines.push(format!("**结论**：{}", one_line(&r.conclusion, CONCLUSION_MAX)));
    }
    let open = r.open_findings();
    let incremental = is_incremental(r);
    if open.is_empty() {
        lines.push("**未发现问题**".to_string());
    } else {
        lines.push(format!("**发现 {} 项**", open.len()));
        for f in open.iter().take(FINDINGS_SHOWN) {
            lines.push(finding_line(f, incremental));
        }
        if open.len() > FINDINGS_SHOWN {
            lines.push(format!("- …另有 {} 项，机器人主人可在客户端「运行日志」里查看", open.len() - FINDINGS_SHOWN));
        }
    }
    let resolved = r.resolved_findings();
    if !resolved.is_empty() {
        lines.push(format!("**上一轮已处理 {} 项**", resolved.len()));
        for f in resolved.iter().take(FINDINGS_SHOWN) {
            lines.push(finding_line(f, incremental));
        }
    }
    lines.push(scale_line(r));
    if let Some(c) = &r.coverage {
        lines.push(coverage_line(c));
    }
    let url = r.url.trim();
    if !url.is_empty() {
        lines.push(if link { format!("[{url}]({url})") } else { url.to_string() });
    }
    lines
}

/// 渲染成飞书消息卡片（`msg_type: interactive` 的 content）。传进来之前应已 `redacted()`。
pub fn render(r: &ReviewResult) -> Value {
    json!({
        "config": { "wide_screen_mode": true },
        "header": {
            "template": "blue",
            "title": { "tag": "plain_text", "content": card_title(r) },
        },
        "elements": [
            { "tag": "markdown", "content": body_lines(r, true).join("\n") },
        ],
    })
}

/// 卡片发不出去时退回的纯文本（同样的内容，去掉加粗）。
pub fn render_text(r: &ReviewResult) -> String {
    let mut lines = vec![card_title(r)];
    lines.extend(body_lines(r, false).into_iter().map(|l| l.replace("**", "")));
    lines.join("\n")
}

/// 这个 PR 的审查结果存放位置。
pub fn state_path(pr: &PrRef) -> PathBuf {
    paths::data_dir()
        .join("pr-reviews")
        .join(pr.platform.as_str())
        .join(pr.owner.to_ascii_lowercase())
        .join(pr.repo.to_ascii_lowercase())
        .join(format!("{}.json", pr.number))
}

/// 存审查结果的根目录（写进运行时配置给 `pr_review_state.py` 读）。
pub fn state_root() -> PathBuf {
    paths::data_dir().join("pr-reviews")
}

/// 保存本轮结果（先写临时文件再改名）。同步函数，异步里放进 `spawn_blocking`。
pub fn save_state(pr: &PrRef, r: &ReviewResult) -> Result<PathBuf, String> {
    let path = state_path(pr);
    let dir = path.parent().expect("state path has parent");
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let mut v = serde_json::to_value(r).map_err(|e| format!("序列化审查结果失败：{e}"))?;
    v["saved_at"] = json!(chrono::Local::now().to_rfc3339());
    v["pr_url"] = json!(pr.url());
    let text = serde_json::to_string_pretty(&v).map_err(|e| format!("序列化审查结果失败：{e}"))?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| format!("写入 {} 失败：{e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("保存 {} 失败：{e}", path.display())
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::pr_link::find_pr_links;

    fn sample() -> ReviewResult {
        parse(
            r#"{"repo":"demo-service","pr":29,"title":"feat: 售后工单自动领取","conclusion":"整体可合并，但有两处并发问题需要先修。",
            "findings":[
              {"severity":"low","status":"new","text":"日志里打印了完整请求体 a.go:12"},
              {"severity":"medium","status":"carried","text":"判重查询走了从库，写后立读可能读不到 b.go:40"},
              {"severity":"high","status":"new","text":"领取接口没有加锁，两台机器会重复领取\nc.go:88"},
              {"severity":"medium","status":"resolved","text":"超时没有重试 d.go:7"}
            ],
            "additions":11822,"deletions":109,"files":69,"author":"张三","url":"https://gitee.com/example/demo-service/pulls/29"}"#,
        )
        .unwrap()
    }

    #[test]
    fn parse_accepts_fenced_and_surrounded_json() {
        let raw = r#"{"repo":"demo-service","pr":"29","findings":[],"additions":"+3","deletions":1,"files":"2"}"#;
        for text in [
            raw.to_string(),
            format!("```json\n{raw}\n```"),
            format!("审查完成：\n```\n{raw}\n```\n以上。"),
            format!("结果如下 {raw} 完"),
        ] {
            let r = parse(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!((r.repo.as_str(), r.pr, r.additions, r.files), ("demo-service", 29, 3, 2), "{text}");
        }
        // 中文严重度 / 状态、缺省状态
        let r = parse(r#"{"repo":"a","pr":1,"findings":[{"severity":"高","status":"未处理","text":"x"},{"severity":"low","text":"y"}]}"#).unwrap();
        assert_eq!(r.findings[0].severity, Severity::High);
        assert_eq!(r.findings[0].status, FindingStatus::Carried);
        assert_eq!(r.findings[1].status, FindingStatus::New);
    }

    #[test]
    fn parse_rejects_invalid_output() {
        for text in [
            "【结论】PR 没问题",
            "",
            r#"{"pr":29}"#,
            r#"{"repo":"a","pr":0}"#,
            r#"{"repo":"a","pr":"abc"}"#,
            r#"{"repo":"a","pr":1,"findings":[{"severity":"致命","text":"x"}]}"#,
            "```json\n{坏掉的 json\n```",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn card_has_blue_header_title_and_screenshot_layout() {
        let card = render(&sample());
        assert_eq!(card["header"]["template"], "blue");
        assert_eq!(card["header"]["title"]["content"], "代码审查 demo-service #29，3 项发现");
        let md = card["elements"][0]["content"].as_str().unwrap();
        let lines: Vec<&str> = md.lines().collect();
        assert_eq!(lines[0], "【代码审查】demo-service #29");
        assert_eq!(lines[1], "feat: 售后工单自动领取");
        assert_eq!(lines[2], "**结论**：整体可合并，但有两处并发问题需要先修。");
        assert_eq!(lines[3], "**发现 3 项**");
        // 按严重度排序；增量复审时新发现也标注；换行压成一行
        assert_eq!(lines[4], "- **[高]** （新发现）领取接口没有加锁，两台机器会重复领取 c.go:88");
        assert_eq!(lines[5], "- **[中]** （上一轮遗留，未处理）判重查询走了从库，写后立读可能读不到 b.go:40");
        assert_eq!(lines[6], "- **[低]** （新发现）日志里打印了完整请求体 a.go:12");
        assert_eq!(lines[7], "**上一轮已处理 1 项**");
        assert_eq!(lines[8], "- **[中]** （上一轮发现，已处理）超时没有重试 d.go:7");
        assert_eq!(lines[9], "规模：+11822/-109，69 个文件，作者 张三");
        assert_eq!(lines[10], "[https://gitee.com/example/demo-service/pulls/29](https://gitee.com/example/demo-service/pulls/29)");
    }

    #[test]
    fn first_round_findings_have_no_status_note_and_long_conclusion_is_truncated() {
        let mut r = sample();
        r.findings.retain(|f| f.status == FindingStatus::New);
        r.conclusion = "很长".repeat(150);
        let md = render(&r)["elements"][0]["content"].as_str().unwrap().to_string();
        assert!(md.contains("- **[高]** 领取接口没有加锁"), "{md}");
        assert!(!md.contains("（新发现）"));
        let conclusion = md.lines().find(|l| l.starts_with("**结论**")).unwrap();
        assert_eq!(conclusion.chars().count(), "**结论**：".chars().count() + CONCLUSION_MAX + 1);
        assert!(conclusion.ends_with('…'));
    }

    #[test]
    fn no_open_findings_title_and_text_fallback() {
        let mut r = sample();
        r.findings.retain(|f| f.status == FindingStatus::Resolved);
        r.author.clear();
        let card = render(&r);
        assert_eq!(card["header"]["title"]["content"], "代码审查 demo-service #29，未发现问题");
        let md = card["elements"][0]["content"].as_str().unwrap();
        assert!(md.contains("**未发现问题**") && md.contains("**上一轮已处理 1 项**"));
        assert!(md.contains("规模：+11822/-109，69 个文件\n"), "没有作者时不写「作者」：{md}");
        let text = render_text(&r);
        assert!(text.starts_with("代码审查 demo-service #29，未发现问题\n【代码审查】demo-service #29"));
        assert!(!text.contains("**"));
        assert!(text.ends_with("https://gitee.com/example/demo-service/pulls/29"));
    }

    #[test]
    fn redacted_masks_secrets_in_card_text() {
        let mut r = sample();
        r.findings[0].text = "配置里写死了 token=ghp_abcdefghijklmnopqrstuvwxyz0123".into();
        r.conclusion = "password: hunter2hunter2".into();
        let card = render(&r.redacted()).to_string();
        assert!(!card.contains("ghp_abcdefghijklmnopqrstuvwxyz0123"));
        assert!(!card.contains("hunter2hunter2"));
    }

    #[test]
    fn save_state_writes_lowercase_path_and_roundtrips() {
        let _home = crate::test_support::test_home();
        let pr = find_pr_links("https://gitee.com/Example/Demo-Service/pulls/29").remove(0);
        let path = save_state(&pr, &sample()).unwrap();
        assert_eq!(path, state_root().join("gitee").join("example").join("demo-service").join("29.json"));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["pr_url"], "https://gitee.com/Example/Demo-Service/pulls/29");
        assert!(v["saved_at"].is_string());
        let back: ReviewResult = serde_json::from_value(v).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn card_shows_coverage_line() {
        let r = parse(
            r#"{"repo":"demo-service","pr":29,"findings":[],"additions":5,"deletions":1,"files":6,"author":"张三",
            "url":"https://gitee.com/example/demo-service/pulls/29",
            "coverage":{"total":"6","reviewed":2,"skipped":[
              {"path":"a.sql","reason":"无法取 diff"},{"path":"b.go","reason":"文件过大"},{"path":"c.ts"},{"path":"d.rs","reason":"x"}]}}"#,
        )
        .unwrap();
        let c = r.coverage.as_ref().expect("coverage 要解析出来");
        assert_eq!((c.total, c.reviewed, c.skipped.len()), (6, 2, 4));
        let md = render(&r)["elements"][0]["content"].as_str().unwrap().to_string();
        let lines: Vec<&str> = md.lines().collect();
        let i = lines.iter().position(|l| l.starts_with("规模：")).unwrap();
        assert_eq!(lines[i + 1], "审查覆盖：2/6 个文件，跳过 4 个（a.sql:无法取 diff；b.go:文件过大；c.ts 等）");
        assert!(lines[i + 2].starts_with("[https://"), "覆盖行在规模行和链接之间：{md}");
        // 没有跳过的只写比例
        let mut r2 = r.clone();
        r2.coverage.as_mut().unwrap().skipped.clear();
        r2.coverage.as_mut().unwrap().reviewed = 6;
        assert!(render_text(&r2).contains("\n审查覆盖：6/6 个文件\n"));
        // 保存再读回不丢 coverage
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["coverage"]["skipped"][0]["path"], "a.sql");
        assert_eq!(serde_json::from_value::<ReviewResult>(v).unwrap(), r);
    }

    #[test]
    fn card_without_coverage_unchanged() {
        let r = sample();
        assert!(r.coverage.is_none());
        assert!(serde_json::to_value(&r).unwrap().get("coverage").is_none(), "没有覆盖率时不写这个字段");
        let md = render(&r)["elements"][0]["content"].as_str().unwrap().to_string();
        assert!(!md.contains("审查覆盖"));
        // 覆盖率写坏了不影响整份结果
        let bad = parse(r#"{"repo":"a","pr":1,"findings":[],"coverage":"全部"}"#).unwrap();
        assert!(bad.coverage.is_none());
    }
}
