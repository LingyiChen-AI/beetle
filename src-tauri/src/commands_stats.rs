//! 数据统计页：从运行记录（`runs.db` 的 `runs` 表）按时间范围聚合各机器人的运行数据。
//!
//! 范围：今天 / 近 7 天 / 近 30 天（按本地自然日，含今天）。同时算「上一周期」（整体往前平移同样天数、
//! 截止到同一时刻，今天就和昨天同一时刻比）用来显示涨跌。
//! 查询只按带索引的 `started_at`（`idx_runs_started`）/ `bot_id, started_at`（`idx_runs_bot`）过滤，
//! 只取聚合要用的列；30 天的记录量在本机是几千条以内，在 Rust 里聚合（P90、失败原因归类更好写也好测）。
//! 统计用单独的只读连接，不占用 RunStore 的写锁。

use crate::commands::{blocking, runs_db_path};
use crate::store::config;
use chrono::{Datelike, Duration as Span, Local, NaiveDate, TimeZone, Timelike};
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use std::collections::HashMap;

const DAY_MS: i64 = 86_400_000;
/// 群 / 触发人排行取前几名。
const TOP_N: usize = 10;
/// 失败原因取前几类。
const REASON_TOP: usize = 5;
/// 失败原因按错误文本前多少个字归类。
const REASON_CHARS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsRange {
    Today,
    Days7,
    Days30,
}

impl StatsRange {
    pub fn parse(s: &str) -> Result<StatsRange, String> {
        match s {
            "today" => Ok(StatsRange::Today),
            "7d" => Ok(StatsRange::Days7),
            "30d" => Ok(StatsRange::Days30),
            other => Err(format!("未知的统计范围：{other}（可选 today / 7d / 30d）")),
        }
    }

    pub fn days(self) -> i64 {
        match self {
            StatsRange::Today => 1,
            StatsRange::Days7 => 7,
            StatsRange::Days30 => 30,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StatsRange::Today => "today",
            StatsRange::Days7 => "7d",
            StatsRange::Days30 => "30d",
        }
    }
}

/// 某个范围的 KPI 汇总。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    pub runs: u32,
    pub replied: u32,
    pub failed: u32,
    pub timeout: u32,
    pub cancelled: u32,
    /// 还在排队 / 运行中
    pub active: u32,
    /// 已出结果（回复 + 失败 + 超时），成功率的分母；取消的不算
    pub settled: u32,
    /// 0–1；settled 为 0 时是 0
    pub success_rate: f64,
    /// 仅统计已回复的运行；没有时为 0
    pub avg_duration_ms: u64,
    pub p90_duration_ms: u64,
    /// 查库总次数
    pub sql_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DayPoint {
    /// MM-DD
    pub day: String,
    /// 这一天本地 0 点的毫秒时间戳
    pub start_ms: i64,
    pub runs: u32,
    pub replied: u32,
    pub failed: u32,
    pub timeout: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BotRow {
    pub bot_id: String,
    /// 配置里的机器人名；机器人已删除时是 bot_id
    pub name: String,
    pub runs: u32,
    pub replied: u32,
    pub success_rate: f64,
    pub avg_duration_ms: u64,
    pub sql_count: u64,
}

/// 群 / 触发人排行的一项。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NamedCount {
    /// 群是 chat_id；触发人是 union_id（旧记录没有时用名字）
    pub key: String,
    pub name: String,
    pub runs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EngineRow {
    pub engine: String,
    pub runs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReasonRow {
    pub reason: String,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatsOverview {
    pub range: String,
    pub bot_id: Option<String>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub totals: Totals,
    /// 上一周期（同样天数、往前平移、截止到同一时刻）
    pub previous: Totals,
    pub daily: Vec<DayPoint>,
    pub by_bot: Vec<BotRow>,
    pub by_group: Vec<NamedCount>,
    pub by_sender: Vec<NamedCount>,
    pub by_engine: Vec<EngineRow>,
    pub failure_reasons: Vec<ReasonRow>,
    /// 0–23 点每小时的运行数（本地时间）
    pub hourly: Vec<u32>,
}

/// 聚合用到的列。
#[derive(Debug, Clone, PartialEq)]
pub struct StatRow {
    pub bot_id: String,
    pub chat_id: String,
    pub group_name: String,
    pub sender_name: String,
    pub sender_union_id: Option<String>,
    pub engine: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub sql_count: i64,
    pub error: Option<String>,
}

/// 统计窗口：当前周期 [start_ms, end_ms)，上一周期 [prev_start_ms, prev_end_ms)，以及当前周期每天的 0 点。
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub range: StatsRange,
    pub days: Vec<(NaiveDate, i64)>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub prev_start_ms: i64,
    pub prev_end_ms: i64,
}

/// 本地某天 0 点的毫秒时间戳（夏令时切换导致 0 点不存在时取最早的有效时刻）。
pub fn local_day_start(d: NaiveDate) -> i64 {
    let t = d.and_hms_opt(0, 0, 0).expect("0 点总是合法的");
    match Local.from_local_datetime(&t) {
        chrono::LocalResult::Single(x) => x.timestamp_millis(),
        chrono::LocalResult::Ambiguous(a, _) => a.timestamp_millis(),
        chrono::LocalResult::None => Local
            .from_local_datetime(&(t + Span::hours(1)))
            .earliest()
            .map(|x| x.timestamp_millis())
            .unwrap_or_else(|| t.and_utc().timestamp_millis()),
    }
}

fn local_hour(ms: i64) -> usize {
    Local.timestamp_millis_opt(ms).earliest().map(|d| d.hour() as usize).unwrap_or(0).min(23)
}

/// `today` 是本地今天的日期，`now_ms` 是当前时刻。
pub fn window(range: StatsRange, today: NaiveDate, now_ms: i64) -> Window {
    let n = range.days();
    let days: Vec<(NaiveDate, i64)> = (0..n)
        .rev()
        .map(|k| {
            let d = today - Span::days(k);
            (d, local_day_start(d))
        })
        .collect();
    let start_ms = days[0].1;
    let shift = n * DAY_MS;
    Window { range, days, start_ms, end_ms: now_ms, prev_start_ms: start_ms - shift, prev_end_ms: now_ms - shift }
}

fn is_bad(status: &str) -> bool {
    status == "failed" || status == "timeout"
}

fn rate(replied: u32, settled: u32) -> f64 {
    if settled == 0 { 0.0 } else { replied as f64 / settled as f64 }
}

fn replied_durations(rows: &[&StatRow]) -> Vec<u64> {
    rows.iter()
        .filter(|r| r.status == "replied")
        .filter_map(|r| r.finished_at.map(|f| (f - r.started_at).max(0) as u64))
        .collect()
}

fn avg(v: &[u64]) -> u64 {
    if v.is_empty() { 0 } else { v.iter().sum::<u64>() / v.len() as u64 }
}

/// 最近秩法的 P90：排序后取第 ceil(0.9·n) 个。
pub fn p90(v: &[u64]) -> u64 {
    if v.is_empty() {
        return 0;
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    let idx = ((s.len() as f64 * 0.9).ceil() as usize).clamp(1, s.len()) - 1;
    s[idx]
}

pub fn totals(rows: &[&StatRow]) -> Totals {
    let count = |st: &str| rows.iter().filter(|r| r.status == st).count() as u32;
    let replied = count("replied");
    let failed = count("failed");
    let timeout = count("timeout");
    let durations = replied_durations(rows);
    let settled = replied + failed + timeout;
    Totals {
        runs: rows.len() as u32,
        replied,
        failed,
        timeout,
        cancelled: count("cancelled"),
        active: count("queued") + count("running"),
        settled,
        success_rate: rate(replied, settled),
        avg_duration_ms: avg(&durations),
        p90_duration_ms: p90(&durations),
        sql_count: rows.iter().map(|r| r.sql_count.max(0) as u64).sum(),
    }
}

/// 失败原因归类：取错误文本第一行、合并空白，截前 60 个字（超出加「…」）；没有错误文本时按状态给默认原因。
pub fn classify_reason(error: Option<&str>, status: &str) -> String {
    let first = error.unwrap_or("").trim().lines().next().unwrap_or("").trim();
    let line = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return if status == "timeout" { "运行超时".into() } else { "未知错误".into() };
    }
    if line.chars().count() > REASON_CHARS {
        format!("{}…", line.chars().take(REASON_CHARS).collect::<String>())
    } else {
        line
    }
}

/// 按 key 计数，保留最新一条的显示名；按次数倒序、同次数按名字排，取前 `n`。
fn top_by<F>(rows: &[&StatRow], n: usize, key_name: F) -> Vec<NamedCount>
where
    F: Fn(&StatRow) -> (String, String),
{
    let mut m: HashMap<String, (String, u32, i64)> = HashMap::new();
    for r in rows {
        let (k, name) = key_name(r);
        let e = m.entry(k).or_insert_with(|| (name.clone(), 0, i64::MIN));
        e.1 += 1;
        if r.started_at >= e.2 && !name.is_empty() {
            e.0 = name;
            e.2 = r.started_at;
        }
    }
    let mut out: Vec<NamedCount> = m.into_iter().map(|(key, (name, runs, _))| NamedCount { name: if name.is_empty() { key.clone() } else { name }, key, runs }).collect();
    out.sort_by(|a, b| b.runs.cmp(&a.runs).then_with(|| a.name.cmp(&b.name)));
    out.truncate(n);
    out
}

/// 纯聚合：`rows` 是当前周期的记录，`prev` 是上一周期的；`names` 是 bot_id → 机器人名。
pub fn aggregate(rows: &[StatRow], prev: &[StatRow], w: &Window, bot_id: Option<&str>, names: &HashMap<String, String>) -> StatsOverview {
    // 当前周期含 now 这一刻（[start, end]），上一周期 [prev_start, prev_end)
    let cur: Vec<&StatRow> = rows.iter().filter(|r| r.started_at >= w.start_ms && r.started_at <= w.end_ms).collect();
    let before: Vec<&StatRow> = prev.iter().filter(|r| r.started_at >= w.prev_start_ms && r.started_at < w.prev_end_ms).collect();

    // 每天
    let starts: Vec<i64> = w.days.iter().map(|d| d.1).collect();
    let mut daily: Vec<DayPoint> = w
        .days
        .iter()
        .map(|(d, ms)| DayPoint { day: format!("{:02}-{:02}", d.month(), d.day()), start_ms: *ms, runs: 0, replied: 0, failed: 0, timeout: 0 })
        .collect();
    let mut hourly = vec![0u32; 24];
    for r in &cur {
        let i = starts.partition_point(|s| *s <= r.started_at).saturating_sub(1);
        if let Some(p) = daily.get_mut(i) {
            p.runs += 1;
            match r.status.as_str() {
                "replied" => p.replied += 1,
                "failed" => p.failed += 1,
                "timeout" => p.timeout += 1,
                _ => {}
            }
        }
        hourly[local_hour(r.started_at)] += 1;
    }

    // 按机器人
    let mut bots: HashMap<&str, Vec<&StatRow>> = HashMap::new();
    for r in &cur {
        bots.entry(r.bot_id.as_str()).or_default().push(*r);
    }
    let mut by_bot: Vec<BotRow> = bots
        .into_iter()
        .map(|(id, rs)| {
            let t = totals(&rs);
            BotRow {
                bot_id: id.to_string(),
                name: names.get(id).cloned().unwrap_or_else(|| id.to_string()),
                runs: t.runs,
                replied: t.replied,
                success_rate: t.success_rate,
                avg_duration_ms: t.avg_duration_ms,
                sql_count: t.sql_count,
            }
        })
        .collect();
    by_bot.sort_by(|a, b| b.runs.cmp(&a.runs).then_with(|| a.name.cmp(&b.name)));

    let by_group = top_by(&cur, TOP_N, |r| (r.chat_id.clone(), r.group_name.clone()));
    let by_sender = top_by(&cur, TOP_N, |r| {
        let key = r.sender_union_id.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| format!("name:{}", r.sender_name));
        (key, r.sender_name.clone())
    });

    let mut engines: HashMap<&str, u32> = HashMap::new();
    for r in &cur {
        *engines.entry(r.engine.as_str()).or_default() += 1;
    }
    let mut by_engine: Vec<EngineRow> = engines.into_iter().map(|(e, n)| EngineRow { engine: e.to_string(), runs: n }).collect();
    by_engine.sort_by(|a, b| b.runs.cmp(&a.runs).then_with(|| a.engine.cmp(&b.engine)));

    let mut reasons: HashMap<String, u32> = HashMap::new();
    for r in cur.iter().filter(|r| is_bad(&r.status)) {
        *reasons.entry(classify_reason(r.error.as_deref(), &r.status)).or_default() += 1;
    }
    let mut failure_reasons: Vec<ReasonRow> = reasons.into_iter().map(|(reason, count)| ReasonRow { reason, count }).collect();
    failure_reasons.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.reason.cmp(&b.reason)));
    failure_reasons.truncate(REASON_TOP);

    StatsOverview {
        range: w.range.as_str().into(),
        bot_id: bot_id.map(str::to_string),
        start_ms: w.start_ms,
        end_ms: w.end_ms,
        totals: totals(&cur),
        previous: totals(&before),
        daily,
        by_bot,
        by_group,
        by_sender,
        by_engine,
        failure_reasons,
        hourly,
    }
}

fn db_err(e: rusqlite::Error) -> String {
    format!("读取运行记录失败：{e}")
}

/// 取 [from, to) 内的记录（只取聚合用的列）；`bot_id` 为 None 时取所有机器人。
pub fn load_rows(conn: &Connection, from: i64, to: i64, bot_id: Option<&str>) -> Result<Vec<StatRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT bot_id, chat_id, group_name, sender_name, sender_union_id, engine, status, started_at, finished_at, sql_count, error \
             FROM runs WHERE started_at >= ?1 AND started_at < ?2 AND (?3 IS NULL OR bot_id = ?3)",
        )
        .map_err(db_err)?;
    stmt.query_map(params![from, to, bot_id], |r| {
        Ok(StatRow {
            bot_id: r.get(0)?,
            chat_id: r.get(1)?,
            group_name: r.get(2)?,
            sender_name: r.get(3)?,
            sender_union_id: r.get(4)?,
            engine: r.get(5)?,
            status: r.get(6)?,
            started_at: r.get(7)?,
            finished_at: r.get(8)?,
            sql_count: r.get(9)?,
            error: r.get(10)?,
        })
    })
    .map_err(db_err)?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(db_err)
}

pub fn stats_impl(conn: &Connection, w: &Window, bot_id: Option<&str>, names: &HashMap<String, String>) -> Result<StatsOverview, String> {
    // 当前周期截到 now；上一周期截到 now - N 天
    let cur = load_rows(conn, w.start_ms, w.end_ms.max(w.start_ms) + 1, bot_id)?;
    let prev = load_rows(conn, w.prev_start_ms, w.prev_end_ms, bot_id)?;
    Ok(aggregate(&cur, &prev, w, bot_id, names))
}

/// 统计概览。`range`：today / 7d / 30d；`bot_id` 为空 = 全部机器人。
#[tauri::command]
pub async fn stats_overview(range: String, bot_id: Option<String>) -> Result<StatsOverview, String> {
    blocking(move || {
        let range = StatsRange::parse(&range)?;
        let names: HashMap<String, String> = config::load()?.bots.into_iter().map(|b| (b.id, b.name)).collect();
        let conn = Connection::open_with_flags(runs_db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .map_err(|e| format!("打开运行记录失败：{e}"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(db_err)?;
        let now = Local::now();
        let w = window(range, now.date_naive(), now.timestamp_millis());
        stats_impl(&conn, &w, bot_id.as_deref().filter(|s| !s.is_empty()), &names)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::config::Engine;
    use crate::store::runs::{RunRecord, RunStatus, RunStore};

    const H: i64 = 3_600_000;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 19).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn rec(id: &str, bot: &str, chat: &str, who: &str, engine: Engine, status: RunStatus, started: i64, dur_ms: Option<i64>, sql: u32, error: Option<&str>) -> RunRecord {
        RunRecord {
            id: id.into(),
            bot_id: bot.into(),
            chat_id: chat.into(),
            group_name: format!("{chat}群"),
            trigger_message_id: format!("om_{id}"),
            sender_name: who.into(),
            text: "查一下".into(),
            engine,
            engine_version: None,
            session_id: None,
            status,
            started_at: started,
            finished_at: dur_ms.map(|d| started + d),
            final_reply: None,
            error: error.map(str::to_string),
            sql_count: sql,
            reply_message_id: None,
            sender_union_id: Some(format!("on_{who}")),
        }
    }

    /// 造一批运行：今天 5 条（3 回复 / 1 失败 / 1 超时），前天 2 条，8 天前 1 条（只在 30 天里），上一周期（14 天前之内）1 条。
    fn seed() -> (tempfile::TempDir, Connection, Window, Window, Window) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runs.db");
        let store = RunStore::open(&path).unwrap();
        let d0 = local_day_start(today());
        let now = d0 + 15 * H;
        let d2 = local_day_start(today() - Span::days(2));
        let d8 = local_day_start(today() - Span::days(8));
        let d10 = local_day_start(today() - Span::days(10));
        let runs = [
            rec("a", "b1", "oc_1", "张三", Engine::Claude, RunStatus::Replied, d0 + 9 * H, Some(60_000), 2, None),
            rec("b", "b1", "oc_1", "张三", Engine::Claude, RunStatus::Replied, d0 + 9 * H + 60_000, Some(120_000), 1, None),
            rec("c", "b1", "oc_2", "李四", Engine::Claude, RunStatus::Replied, d0 + 10 * H, Some(600_000), 0, None),
            rec("d", "b2", "oc_2", "李四", Engine::Codex, RunStatus::Failed, d0 + 14 * H, Some(5_000), 3, Some("lark-cli 未登录：请先 lark-cli auth login\n详细堆栈……")),
            rec("e", "b2", "oc_3", "王五", Engine::Codex, RunStatus::Timeout, d0 + 14 * H + 1, Some(1_800_000), 0, None),
            rec("f", "b1", "oc_1", "张三", Engine::Claude, RunStatus::Failed, d2 + 11 * H, Some(1_000), 0, Some("lark-cli 未登录：请先 lark-cli auth login")),
            rec("g", "b1", "oc_1", "王五", Engine::Claude, RunStatus::Cancelled, d2 + 12 * H, None, 0, None),
            rec("h", "b2", "oc_3", "王五", Engine::Codex, RunStatus::Replied, d8 + 8 * H, Some(30_000), 4, None),
            rec("i", "b1", "oc_1", "张三", Engine::Claude, RunStatus::Replied, d10 + 8 * H, Some(30_000), 1, None),
            // 今天但在 now 之后（不该出现）
            rec("z", "b1", "oc_1", "张三", Engine::Claude, RunStatus::Replied, now + H, Some(1_000), 9, None),
        ];
        for r in &runs {
            store.insert_run(r).unwrap();
        }
        drop(store);
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let wt = window(StatsRange::Today, today(), now);
        let w7 = window(StatsRange::Days7, today(), now);
        let w30 = window(StatsRange::Days30, today(), now);
        (dir, conn, wt, w7, w30)
    }

    fn names() -> HashMap<String, String> {
        HashMap::from([("b1".to_string(), "查 bug 助手".to_string()), ("b2".to_string(), "答疑助手".to_string())])
    }

    #[test]
    fn range_parse_and_window_days() {
        assert_eq!(StatsRange::parse("today"), Ok(StatsRange::Today));
        assert_eq!(StatsRange::parse("7d"), Ok(StatsRange::Days7));
        assert_eq!(StatsRange::parse("30d"), Ok(StatsRange::Days30));
        assert!(StatsRange::parse("1y").unwrap_err().contains("未知的统计范围"));
        let now = local_day_start(today()) + 15 * H;
        let w = window(StatsRange::Days7, today(), now);
        assert_eq!(w.days.len(), 7);
        assert_eq!(w.days.last().unwrap().0, today());
        assert_eq!(w.days[0].0, today() - Span::days(6));
        assert_eq!(w.start_ms, local_day_start(today() - Span::days(6)));
        assert_eq!(w.prev_end_ms, now - 7 * DAY_MS);
        assert_eq!(w.prev_start_ms, w.start_ms - 7 * DAY_MS);
    }

    #[test]
    fn today_totals_rates_durations_and_breakdowns() {
        let (_d, conn, wt, _, _) = seed();
        let s = stats_impl(&conn, &wt, None, &names()).unwrap();
        let t = &s.totals;
        assert_eq!((t.runs, t.replied, t.failed, t.timeout, t.cancelled, t.active, t.settled), (5, 3, 1, 1, 0, 0, 5));
        assert!((t.success_rate - 0.6).abs() < 1e-9);
        // 只算已回复：60s / 120s / 600s
        assert_eq!(t.avg_duration_ms, 260_000);
        assert_eq!(t.p90_duration_ms, 600_000);
        assert_eq!(t.sql_count, 6, "now 之后的记录不算");
        assert_eq!(s.daily.len(), 1);
        assert_eq!((s.daily[0].runs, s.daily[0].replied, s.daily[0].failed, s.daily[0].timeout), (5, 3, 1, 1));
        assert_eq!(s.hourly.len(), 24);
        assert_eq!((s.hourly[9], s.hourly[10], s.hourly[14]), (2, 1, 2));
        assert_eq!(s.hourly.iter().sum::<u32>(), 5);

        assert_eq!(s.by_bot.len(), 2);
        assert_eq!((s.by_bot[0].bot_id.as_str(), s.by_bot[0].name.as_str(), s.by_bot[0].runs), ("b1", "查 bug 助手", 3));
        assert!((s.by_bot[0].success_rate - 1.0).abs() < 1e-9);
        assert_eq!((s.by_bot[1].name.as_str(), s.by_bot[1].runs, s.by_bot[1].success_rate), ("答疑助手", 2, 0.0));

        assert_eq!(s.by_group.iter().map(|g| (g.key.as_str(), g.runs)).collect::<Vec<_>>(), vec![("oc_1", 2), ("oc_2", 2), ("oc_3", 1)]);
        assert_eq!(s.by_group[0].name, "oc_1群");
        assert_eq!(s.by_sender.iter().map(|g| (g.name.as_str(), g.runs)).collect::<Vec<_>>(), vec![("张三", 2), ("李四", 2), ("王五", 1)]);
        assert_eq!(s.by_engine.iter().map(|e| (e.engine.as_str(), e.runs)).collect::<Vec<_>>(), vec![("claude", 3), ("codex", 2)]);
        assert_eq!(
            s.failure_reasons.iter().map(|r| (r.reason.as_str(), r.count)).collect::<Vec<_>>(),
            vec![("lark-cli 未登录：请先 lark-cli auth login", 1), ("运行超时", 1)]
        );
        // 上一周期 = 昨天同一时刻之前：没有记录
        assert_eq!(s.previous, Totals::default());
    }

    #[test]
    fn seven_and_thirty_days_with_previous_period() {
        let (_d, conn, _, w7, w30) = seed();
        let s7 = stats_impl(&conn, &w7, None, &names()).unwrap();
        assert_eq!(s7.totals.runs, 7);
        assert_eq!(s7.totals.cancelled, 1);
        assert_eq!(s7.totals.settled, 6, "取消的不算进成功率分母");
        assert_eq!(s7.daily.len(), 7);
        let d2 = &s7.daily[4];
        assert_eq!((d2.runs, d2.failed), (2, 1));
        assert_eq!(d2.day, format!("{:02}-{:02}", (today() - Span::days(2)).month(), (today() - Span::days(2)).day()));
        assert_eq!(s7.daily.iter().map(|d| d.runs).sum::<u32>(), 7);
        // 同一错误文本（第一行相同）归为一类
        assert_eq!(s7.failure_reasons[0], ReasonRow { reason: "lark-cli 未登录：请先 lark-cli auth login".into(), count: 2 });
        // 上一周期：8 天前、10 天前各一条
        assert_eq!(s7.previous.runs, 2);
        assert_eq!(s7.previous.replied, 2);

        let s30 = stats_impl(&conn, &w30, None, &names()).unwrap();
        assert_eq!(s30.totals.runs, 9);
        assert_eq!(s30.daily.len(), 30);
        assert_eq!(s30.totals.sql_count, 11);
    }

    #[test]
    fn bot_filter_and_unknown_bot_name() {
        let (_d, conn, _, w7, _) = seed();
        let s = stats_impl(&conn, &w7, Some("b2"), &HashMap::new()).unwrap();
        assert_eq!(s.bot_id.as_deref(), Some("b2"));
        assert_eq!(s.totals.runs, 2);
        assert_eq!(s.by_bot.len(), 1);
        assert_eq!(s.by_bot[0].name, "b2", "配置里没有的机器人用 id 显示");
        assert_eq!(s.by_engine, vec![EngineRow { engine: "codex".into(), runs: 2 }]);
        assert_eq!(s.previous.runs, 1);
    }

    #[test]
    fn empty_db_gives_zeroes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runs.db");
        drop(RunStore::open(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        let now = local_day_start(today()) + H;
        let s = stats_impl(&conn, &window(StatsRange::Days30, today(), now), None, &HashMap::new()).unwrap();
        assert_eq!(s.totals, Totals::default());
        assert_eq!(s.daily.len(), 30);
        assert!(s.daily.iter().all(|d| d.runs == 0));
        assert!(s.by_bot.is_empty() && s.by_group.is_empty() && s.failure_reasons.is_empty());
        assert_eq!(s.hourly, vec![0; 24]);
    }

    #[test]
    fn reason_classification_truncates_and_defaults() {
        assert_eq!(classify_reason(None, "timeout"), "运行超时");
        assert_eq!(classify_reason(Some("   "), "failed"), "未知错误");
        assert_eq!(classify_reason(Some("  a   b \n c"), "failed"), "a b");
        let long = "错".repeat(80);
        let r = classify_reason(Some(&long), "failed");
        assert_eq!(r.chars().count(), REASON_CHARS + 1);
        assert!(r.ends_with('…'));
        assert_eq!(p90(&[]), 0);
        assert_eq!(p90(&[5]), 5);
        assert_eq!(p90(&(1..=10).collect::<Vec<u64>>()), 9);
        assert_eq!(p90(&(1..=20).collect::<Vec<u64>>()), 18);
    }

    #[test]
    fn top_lists_are_capped() {
        let w = window(StatsRange::Today, today(), local_day_start(today()) + 20 * H);
        let rows: Vec<StatRow> = (0..15)
            .map(|i| StatRow {
                bot_id: "b1".into(),
                chat_id: format!("oc_{i}"),
                group_name: format!("群{i}"),
                sender_name: format!("人{i}"),
                sender_union_id: None,
                engine: "claude".into(),
                status: "replied".into(),
                started_at: w.start_ms + i as i64 * H,
                finished_at: Some(w.start_ms + i as i64 * H + 1000),
                sql_count: 0,
                error: None,
            })
            .collect();
        let s = aggregate(&rows, &[], &w, None, &HashMap::new());
        assert_eq!(s.by_group.len(), TOP_N);
        assert_eq!(s.by_sender.len(), TOP_N);
        assert_eq!(s.by_sender[0].key, "name:人0", "没有 union_id 时按名字归并");
    }
}
