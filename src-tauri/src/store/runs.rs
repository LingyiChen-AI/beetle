//! 运行记录存储：`runs.db`（SQLite，WAL）。两张表 `runs`、`steps`。
//! 时间戳一律为 Unix 毫秒（i64）。

use crate::store::config::Engine;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Replied,
    Failed,
    Timeout,
    Cancelled,
}

impl RunStatus {
    fn as_str(self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Replied => "replied",
            RunStatus::Failed => "failed",
            RunStatus::Timeout => "timeout",
            RunStatus::Cancelled => "cancelled",
        }
    }

    fn parse(s: &str) -> Result<Self, String> {
        Ok(match s {
            "queued" => RunStatus::Queued,
            "running" => RunStatus::Running,
            "replied" => RunStatus::Replied,
            "failed" => RunStatus::Failed,
            "timeout" => RunStatus::Timeout,
            "cancelled" => RunStatus::Cancelled,
            other => return Err(format!("未知的运行状态：{other}")),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    Received,
    Prepare,
    Launch,
    ReadChat,
    SyncCode,
    Sql,
    WriteDoc,
    Tool,
    Thinking,
    Reply,
    Error,
}

impl StepKind {
    fn as_str(self) -> &'static str {
        match self {
            StepKind::Received => "received",
            StepKind::Prepare => "prepare",
            StepKind::Launch => "launch",
            StepKind::ReadChat => "readchat",
            StepKind::SyncCode => "synccode",
            StepKind::Sql => "sql",
            StepKind::WriteDoc => "writedoc",
            StepKind::Tool => "tool",
            StepKind::Thinking => "thinking",
            StepKind::Reply => "reply",
            StepKind::Error => "error",
        }
    }

    fn parse(s: &str) -> Result<Self, String> {
        Ok(match s {
            "received" => StepKind::Received,
            "prepare" => StepKind::Prepare,
            "launch" => StepKind::Launch,
            "readchat" => StepKind::ReadChat,
            "synccode" => StepKind::SyncCode,
            "sql" => StepKind::Sql,
            "writedoc" => StepKind::WriteDoc,
            "tool" => StepKind::Tool,
            "thinking" => StepKind::Thinking,
            "reply" => StepKind::Reply,
            "error" => StepKind::Error,
            other => return Err(format!("未知的步骤类型：{other}")),
        })
    }
}

fn engine_str(e: Engine) -> &'static str {
    match e {
        Engine::Claude => "claude",
        Engine::Codex => "codex",
    }
}

fn engine_parse(s: &str) -> Result<Engine, String> {
    match s {
        "claude" => Ok(Engine::Claude),
        "codex" => Ok(Engine::Codex),
        other => Err(format!("未知的引擎：{other}")),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub bot_id: String,
    pub chat_id: String,
    pub group_name: String,
    pub trigger_message_id: String,
    pub sender_name: String,
    pub text: String,
    pub engine: Engine,
    pub engine_version: Option<String>,
    pub session_id: Option<String>,
    pub status: RunStatus,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub final_reply: Option<String>,
    pub error: Option<String>,
    pub sql_count: u32,
    pub reply_message_id: Option<String>,
    /// 触发人的 union_id（跨应用一致）。续聊只在本次发送人与它相同时才续上同一会话。
    /// 旧库没有这一列，读出来是 None（这类会话一律不续聊）。
    #[serde(default)]
    pub sender_union_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub run_id: String,
    pub seq: u32,
    pub at: i64,
    pub kind: StepKind,
    pub title: String,
    pub detail: String,
    pub duration_ms: Option<u64>,
}

pub struct RunStore {
    conn: Mutex<Connection>,
}

/// `session_for_reply` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ReplySession {
    pub session_id: String,
    pub engine: Engine,
    pub sender_union_id: Option<String>,
}

const RUN_COLUMNS: &str = "id, bot_id, chat_id, group_name, trigger_message_id, sender_name, \
     text, engine, engine_version, session_id, status, started_at, finished_at, final_reply, \
     error, sql_count, reply_message_id, sender_union_id";

/// 旧版 runs 表缺的列：打开库时逐个补上（SQLite 的 ALTER TABLE ADD COLUMN，旧数据该列为 NULL）。
const MIGRATED_COLUMNS: [(&str, &str); 1] = [("sender_union_id", "TEXT")];

/// 审计里 SQL 只留前这么多个字符。
const DB_QUERY_SQL_CHARS: usize = 200;

/// 等待写锁的上限（并发写时不立刻报 database is locked）。
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    // 语句在块内结束（finalize）后再改表
    let have: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(runs)")?;
        let cols = stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<rusqlite::Result<Vec<String>>>()?;
        cols
    };
    for (col, ty) in MIGRATED_COLUMNS {
        if !have.iter().any(|c| c == col) {
            conn.execute_batch(&format!("ALTER TABLE runs ADD COLUMN {col} {ty}"))?;
        }
    }
    Ok(())
}

fn db_err(e: rusqlite::Error) -> String {
    format!("运行记录数据库错误：{e}")
}

fn conv_err(col: usize, msg: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, msg.into())
}

fn row_to_run(row: &Row) -> rusqlite::Result<RunRecord> {
    let engine: String = row.get(7)?;
    let status: String = row.get(10)?;
    Ok(RunRecord {
        id: row.get(0)?,
        bot_id: row.get(1)?,
        chat_id: row.get(2)?,
        group_name: row.get(3)?,
        trigger_message_id: row.get(4)?,
        sender_name: row.get(5)?,
        text: row.get(6)?,
        engine: engine_parse(&engine).map_err(|m| conv_err(7, m))?,
        engine_version: row.get(8)?,
        session_id: row.get(9)?,
        status: RunStatus::parse(&status).map_err(|m| conv_err(10, m))?,
        started_at: row.get(11)?,
        finished_at: row.get(12)?,
        final_reply: row.get(13)?,
        error: row.get(14)?,
        sql_count: row.get(15)?,
        reply_message_id: row.get(16)?,
        sender_union_id: row.get(17)?,
    })
}

fn row_to_step(row: &Row) -> rusqlite::Result<StepRecord> {
    let kind: String = row.get(3)?;
    let duration: Option<i64> = row.get(6)?;
    Ok(StepRecord {
        run_id: row.get(0)?,
        seq: row.get(1)?,
        at: row.get(2)?,
        kind: StepKind::parse(&kind).map_err(|m| conv_err(3, m))?,
        title: row.get(4)?,
        detail: row.get(5)?,
        duration_ms: duration.map(|d| d as u64),
    })
}

impl RunStore {
    pub fn open(path: impl AsRef<Path>) -> Result<RunStore, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建运行记录目录失败：{e}"))?;
        }
        let conn = Connection::open(path).map_err(db_err)?;
        // journal_mode 会返回一行结果，必须用 query_row 读掉
        conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY,
                bot_id TEXT NOT NULL,
                chat_id TEXT NOT NULL,
                group_name TEXT NOT NULL,
                trigger_message_id TEXT NOT NULL,
                sender_name TEXT NOT NULL,
                text TEXT NOT NULL,
                engine TEXT NOT NULL,
                engine_version TEXT,
                session_id TEXT,
                status TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                finished_at INTEGER,
                final_reply TEXT,
                error TEXT,
                sql_count INTEGER NOT NULL DEFAULT 0,
                reply_message_id TEXT,
                sender_union_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_runs_started ON runs(started_at DESC);
            CREATE INDEX IF NOT EXISTS idx_runs_bot ON runs(bot_id, started_at DESC);
            CREATE INDEX IF NOT EXISTS idx_runs_reply ON runs(chat_id, reply_message_id);
            CREATE TABLE IF NOT EXISTS steps (
                run_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                at INTEGER NOT NULL,
                kind TEXT NOT NULL,
                title TEXT NOT NULL,
                detail TEXT NOT NULL,
                duration_ms INTEGER,
                PRIMARY KEY (run_id, seq)
            );
            CREATE TABLE IF NOT EXISTS db_queries (
                id INTEGER PRIMARY KEY,
                at INTEGER NOT NULL,
                alias TEXT NOT NULL,
                run_id TEXT NOT NULL,
                sql TEXT NOT NULL,
                \"rows\" INTEGER,
                elapsed_ms INTEGER NOT NULL,
                error TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_db_queries_at ON db_queries(at DESC);",
        )
        .map_err(db_err)?;
        migrate(&conn).map_err(db_err)?;
        conn.busy_timeout(BUSY_TIMEOUT).map_err(db_err)?;
        Ok(RunStore { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.conn.lock().map_err(|_| "运行记录数据库锁已损坏".to_string())
    }

    pub fn insert_run(&self, r: &RunRecord) -> Result<(), String> {
        let conn = self.lock()?;
        conn.execute(
            &format!(
                "INSERT INTO runs ({RUN_COLUMNS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)"
            ),
            params![
                r.id,
                r.bot_id,
                r.chat_id,
                r.group_name,
                r.trigger_message_id,
                r.sender_name,
                r.text,
                engine_str(r.engine),
                r.engine_version,
                r.session_id,
                r.status.as_str(),
                r.started_at,
                r.finished_at,
                r.final_reply,
                r.error,
                r.sql_count,
                r.reply_message_id,
                r.sender_union_id,
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn update_run(&self, r: &RunRecord) -> Result<(), String> {
        let conn = self.lock()?;
        let n = conn
            .execute(
                "UPDATE runs SET bot_id=?2, chat_id=?3, group_name=?4, trigger_message_id=?5, \
                 sender_name=?6, text=?7, engine=?8, engine_version=?9, session_id=?10, status=?11, \
                 started_at=?12, finished_at=?13, final_reply=?14, error=?15, sql_count=?16, \
                 reply_message_id=?17, sender_union_id=?18 WHERE id=?1",
                params![
                    r.id,
                    r.bot_id,
                    r.chat_id,
                    r.group_name,
                    r.trigger_message_id,
                    r.sender_name,
                    r.text,
                    engine_str(r.engine),
                    r.engine_version,
                    r.session_id,
                    r.status.as_str(),
                    r.started_at,
                    r.finished_at,
                    r.final_reply,
                    r.error,
                    r.sql_count,
                    r.reply_message_id,
                    r.sender_union_id,
                ],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err(format!("运行记录不存在：{}", r.id));
        }
        Ok(())
    }

    pub fn append_step(&self, s: &StepRecord) -> Result<(), String> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO steps (run_id, seq, at, kind, title, detail, duration_ms) \
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                s.run_id,
                s.seq,
                s.at,
                s.kind.as_str(),
                s.title,
                s.detail,
                s.duration_ms.map(|d| d as i64),
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// 直连查询审计（`beetle --db-query`）：成功失败都记一行。`sql` 只留前 200 个字符；
    /// `rows` 失败时为 None；`run_id` 没有时传空串。
    pub fn record_db_query(
        &self,
        alias: &str,
        run_id: &str,
        sql: &str,
        rows: Option<u64>,
        elapsed_ms: u64,
        error: Option<&str>,
    ) -> Result<(), String> {
        let sql: String = sql.chars().take(DB_QUERY_SQL_CHARS).collect();
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO db_queries (at, alias, run_id, sql, \"rows\", elapsed_ms, error) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![at, alias, run_id, sql, rows.map(|r| r as i64), elapsed_ms as i64, error],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// `since_ms` 之后的直连查询次数（统计页用）。
    pub fn count_db_queries(&self, since_ms: i64) -> Result<u64, String> {
        let conn = self.lock()?;
        conn.query_row("SELECT COUNT(*) FROM db_queries WHERE at >= ?1", params![since_ms], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
            .map_err(db_err)
    }

    pub fn get_run(&self, id: &str) -> Result<Option<(RunRecord, Vec<StepRecord>)>, String> {
        let conn = self.lock()?;
        let run = conn
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id=?1"),
                params![id],
                row_to_run,
            )
            .optional()
            .map_err(db_err)?;
        let Some(run) = run else { return Ok(None) };
        let mut stmt = conn
            .prepare(
                "SELECT run_id, seq, at, kind, title, detail, duration_ms FROM steps \
                 WHERE run_id=?1 ORDER BY seq ASC",
            )
            .map_err(db_err)?;
        let steps = stmt
            .query_map(params![id], row_to_step)
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;
        Ok(Some((run, steps)))
    }

    /// 按 started_at 倒序；`bot_id` 为 None 时返回所有机器人的记录。
    pub fn list_runs(&self, bot_id: Option<&str>, limit: u32) -> Result<Vec<RunRecord>, String> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM runs WHERE (?1 IS NULL OR bot_id=?1) \
             ORDER BY started_at DESC, rowid DESC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        stmt.query_map(params![bot_id, limit], row_to_run)
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)
    }

    /// 用户回复机器人的某条回复时，找回当时的会话以便续聊：同一机器人、同一群、回复的正是那条机器人消息。
    /// 返回（会话 id，引擎，当时触发人的 union_id）；是否续聊由调用方按引擎和触发人决定。
    pub fn session_for_reply(
        &self,
        bot_id: &str,
        chat_id: &str,
        reply_message_id: &str,
    ) -> Result<Option<ReplySession>, String> {
        let conn = self.lock()?;
        let row: Option<(String, String, Option<String>)> = conn
            .query_row(
                "SELECT session_id, engine, sender_union_id FROM runs \
                 WHERE bot_id=?1 AND chat_id=?2 AND reply_message_id=?3 AND session_id IS NOT NULL \
                 ORDER BY started_at DESC, rowid DESC LIMIT 1",
                params![bot_id, chat_id, reply_message_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(db_err)?;
        match row {
            Some((session_id, eng, sender_union_id)) => {
                Ok(Some(ReplySession { session_id, engine: engine_parse(&eng)?, sender_union_id }))
            }
            None => Ok(None),
        }
    }

    /// 客户端启动时调用：上次退出时还在排队 / 运行的记录一律标为失败，返回改动条数。
    pub fn fail_unfinished(&self, error: &str) -> Result<usize, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let conn = self.lock()?;
        conn.execute(
            "UPDATE runs SET status='failed', error=?1, finished_at=?2 \
             WHERE status IN ('running', 'queued')",
            params![error, now],
        )
        .map_err(db_err)
    }

    /// 删除 started_at 早于 `days` 天前的运行及其步骤，返回删除的运行数。
    pub fn purge_older_than(&self, days: u32) -> Result<usize, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let cutoff = now - (days as i64) * 86_400_000;
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(db_err)?;
        tx.execute(
            "DELETE FROM steps WHERE run_id IN (SELECT id FROM runs WHERE started_at < ?1)",
            params![cutoff],
        )
        .map_err(db_err)?;
        let n = tx
            .execute("DELETE FROM runs WHERE started_at < ?1", params![cutoff])
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_ms() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
    }

    fn run(id: &str, bot: &str, started_at: i64) -> RunRecord {
        RunRecord {
            id: id.into(),
            bot_id: bot.into(),
            chat_id: "oc_x".into(),
            group_name: "测试群".into(),
            trigger_message_id: format!("om_{id}"),
            sender_name: "张三".into(),
            text: "查一下订单".into(),
            engine: Engine::Claude,
            engine_version: Some("1.0.0".into()),
            session_id: None,
            status: RunStatus::Running,
            started_at,
            finished_at: None,
            final_reply: None,
            error: None,
            sql_count: 0,
            reply_message_id: None,
            sender_union_id: None,
        }
    }

    fn step(run_id: &str, seq: u32, kind: StepKind) -> StepRecord {
        StepRecord {
            run_id: run_id.into(),
            seq,
            at: 1000 + seq as i64,
            kind,
            title: format!("步骤{seq}"),
            detail: "详情".into(),
            duration_ms: if seq == 1 { Some(250) } else { None },
        }
    }

    fn open_store() -> (tempfile::TempDir, RunStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = RunStore::open(dir.path().join("runs.db")).unwrap();
        (dir, store)
    }

    #[test]
    fn insert_update_get_roundtrip() {
        let (_d, store) = open_store();
        let mut r = run("r1", "b1", 5000);
        store.insert_run(&r).unwrap();
        // 故意乱序追加，读取时必须按 seq 升序
        store.append_step(&step("r1", 2, StepKind::Sql)).unwrap();
        store.append_step(&step("r1", 1, StepKind::Received)).unwrap();
        store.append_step(&step("r1", 3, StepKind::Reply)).unwrap();

        r.status = RunStatus::Replied;
        r.final_reply = Some("原因是缺少索引".into());
        r.finished_at = Some(9000);
        r.sql_count = 2;
        r.session_id = Some("s1".into());
        r.reply_message_id = Some("om_reply".into());
        store.update_run(&r).unwrap();

        let (got, steps) = store.get_run("r1").unwrap().unwrap();
        assert_eq!(got, r);
        assert_eq!(got.status, RunStatus::Replied);
        assert_eq!(steps.iter().map(|s| s.seq).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(steps[0].kind, StepKind::Received);
        assert_eq!(steps[0].duration_ms, Some(250));
        assert_eq!(steps[1].duration_ms, None);
        assert!(store.get_run("nope").unwrap().is_none());
        assert!(store.update_run(&run("nope", "b1", 1)).is_err());
    }

    #[test]
    fn list_runs_newest_first_and_filters_by_bot() {
        let (_d, store) = open_store();
        store.insert_run(&run("a", "b1", 100)).unwrap();
        store.insert_run(&run("b", "b2", 300)).unwrap();
        store.insert_run(&run("c", "b1", 200)).unwrap();

        let all = store.list_runs(None, 10).unwrap();
        assert_eq!(all.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["b", "c", "a"]);
        let b1 = store.list_runs(Some("b1"), 10).unwrap();
        assert_eq!(b1.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["c", "a"]);
        assert_eq!(store.list_runs(None, 2).unwrap().len(), 2);
    }

    #[test]
    fn session_for_reply_finds_session_by_bot_reply_message() {
        let (_d, store) = open_store();
        let mut r = run("r1", "b1", 100);
        r.engine = Engine::Codex;
        r.session_id = Some("s1".into());
        r.reply_message_id = Some("om_reply".into());
        r.sender_union_id = Some("on_owner".into());
        store.insert_run(&r).unwrap();

        assert_eq!(
            store.session_for_reply("b1", "oc_x", "om_reply").unwrap(),
            Some(ReplySession { session_id: "s1".into(), engine: Engine::Codex, sender_union_id: Some("on_owner".into()) })
        );
        assert_eq!(store.session_for_reply("b1", "oc_other", "om_reply").unwrap(), None);
        assert_eq!(store.session_for_reply("b1", "oc_x", "om_unknown").unwrap(), None);
        // 同一个群里的另一个机器人不能续这个机器人的会话（工作目录不同）
        assert_eq!(store.session_for_reply("b2", "oc_x", "om_reply").unwrap(), None);
    }

    /// 旧版 runs.db 没有 sender_union_id 列：打开时自动补列，旧记录该列为 None，新记录能写能读。
    #[test]
    fn old_db_without_sender_union_id_is_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runs.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE runs (id TEXT PRIMARY KEY, bot_id TEXT NOT NULL, chat_id TEXT NOT NULL, group_name TEXT NOT NULL,                  trigger_message_id TEXT NOT NULL, sender_name TEXT NOT NULL, text TEXT NOT NULL, engine TEXT NOT NULL,                  engine_version TEXT, session_id TEXT, status TEXT NOT NULL, started_at INTEGER NOT NULL, finished_at INTEGER,                  final_reply TEXT, error TEXT, sql_count INTEGER NOT NULL DEFAULT 0, reply_message_id TEXT);
                 INSERT INTO runs (id, bot_id, chat_id, group_name, trigger_message_id, sender_name, text, engine, status, started_at, session_id, reply_message_id)                  VALUES ('old', 'b1', 'oc_x', 'g', 'om_t', '张三', 't', 'claude', 'replied', 1, 's_old', 'om_r');",
            )
            .unwrap();
        }
        let store = RunStore::open(&path).unwrap();
        let (old, _) = store.get_run("old").unwrap().unwrap();
        assert_eq!(old.sender_union_id, None);
        assert_eq!(store.session_for_reply("b1", "oc_x", "om_r").unwrap().unwrap().sender_union_id, None);
        let mut r = run("new", "b1", 2);
        r.sender_union_id = Some("on_u".into());
        store.insert_run(&r).unwrap();
        assert_eq!(store.get_run("new").unwrap().unwrap().0.sender_union_id.as_deref(), Some("on_u"));
        drop(store);
        // 再打开一次不会重复加列
        assert!(RunStore::open(&path).is_ok());
    }

    #[test]
    fn same_millisecond_runs_list_newest_inserted_first() {
        let (_d, store) = open_store();
        store.insert_run(&run("first", "b1", 100)).unwrap();
        store.insert_run(&run("second", "b1", 100)).unwrap();
        let ids: Vec<String> = store.list_runs(None, 10).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["second", "first"]);
    }

    #[test]
    fn purge_removes_runs_and_steps_older_than_days() {
        let (_d, store) = open_store();
        let day = 86_400_000;
        store.insert_run(&run("old", "b1", now_ms() - 40 * day)).unwrap();
        store.append_step(&step("old", 1, StepKind::Received)).unwrap();
        store.insert_run(&run("new", "b1", now_ms() - day)).unwrap();
        store.append_step(&step("new", 1, StepKind::Received)).unwrap();

        assert_eq!(store.purge_older_than(30).unwrap(), 1);
        assert!(store.get_run("old").unwrap().is_none());
        let (_, steps) = store.get_run("new").unwrap().unwrap();
        assert_eq!(steps.len(), 1);
        // 旧步骤也被清掉
        let n: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM steps WHERE run_id='old'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn fail_unfinished_marks_running_and_queued_only() {
        let (_d, store) = open_store();
        let mut queued = run("q", "b1", 100);
        queued.status = RunStatus::Queued;
        store.insert_run(&queued).unwrap();
        store.insert_run(&run("r", "b1", 200)).unwrap();
        let mut done = run("d", "b1", 300);
        done.status = RunStatus::Replied;
        done.finished_at = Some(400);
        store.insert_run(&done).unwrap();

        assert_eq!(store.fail_unfinished("客户端退出，运行中断").unwrap(), 2);
        for id in ["q", "r"] {
            let (r, _) = store.get_run(id).unwrap().unwrap();
            assert_eq!(r.status, RunStatus::Failed, "{id}");
            assert_eq!(r.error.as_deref(), Some("客户端退出，运行中断"));
            assert!(r.finished_at.is_some());
        }
        assert_eq!(store.get_run("d").unwrap().unwrap().0, done, "已结束的运行不动");
    }

    #[test]
    fn reopen_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("runs.db");
        RunStore::open(&path).unwrap().insert_run(&run("r1", "b1", 1)).unwrap();
        assert!(RunStore::open(&path).unwrap().get_run("r1").unwrap().is_some());
    }

    #[test]
    fn record_db_query_truncates_sql_and_counts() {
        let (_d, store) = open_store();
        let long = format!("SELECT {} FROM t", "列".repeat(300));
        store.record_db_query("demo_a", "run-1", &long, Some(3), 12, None).unwrap();
        store.record_db_query("demo_a", "", "DELETE FROM t", None, 1, Some("只允许 SELECT 查询")).unwrap();
        assert_eq!(store.count_db_queries(0).unwrap(), 2);
        assert_eq!(store.count_db_queries(i64::MAX).unwrap(), 0);
        let conn = store.lock().unwrap();
        let (sql, rows, err): (String, Option<i64>, Option<String>) = conn
            .query_row("SELECT sql, \"rows\", error FROM db_queries WHERE run_id='run-1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(sql.chars().count(), 200);
        assert_eq!(rows, Some(3));
        assert_eq!(err, None);
        let (rows, err): (Option<i64>, Option<String>) = conn
            .query_row("SELECT \"rows\", error FROM db_queries WHERE run_id=''", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(rows, None);
        assert_eq!(err.as_deref(), Some("只允许 SELECT 查询"));
    }
}
