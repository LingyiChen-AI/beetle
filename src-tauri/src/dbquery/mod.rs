//! 直连数据源的只读查询：`beetle --db-query <别名>`。
//!
//! 客户端读 keyring 取密码、按需起 SSH 隧道、连库、执行、断开；Agent 只拿到结果，看不到密码和连接信息。
//! 只读防线全在客户端里：SQL 解析校验 + LIMIT 收紧（`guard`）、会话级只读与语句超时（`sql`）、
//! Mongo 只 find（`mongo`）、`cost` 必填。每次查询（成功或失败）都记一行审计到 runs.db。
//!
//! 命令行约定：stdin 读 JSON（SQL：`{"sql","cost"}`；Mongo：`{"collection","filter","projection","sort","limit","cost"}`），
//! 成功 stdout 输出 `{"ok":true,"columns":[],"rows":[],"truncated":false,"elapsed_ms":N}`、退出码 0；
//! 失败 stderr 输出中文原因，执行错误退出码 1，参数 / 配置错误退出码 2。

pub mod guard;
pub mod mongo;
pub mod sql;
pub mod ssh;

use crate::store::datasource::{Access, DataSource, DbKind, DirectConn};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Read;
use std::time::{Duration, Instant};

/// 单元格里字符串（或嵌套对象序列化后）超过这么多字就截断并加「…」。
pub const MAX_CELL_CHARS: usize = 2000;
/// 整体超时 = 数据源超时 + 这个余量（连库、隧道握手、关连接）。
const OVERALL_SLACK: Duration = Duration::from_secs(5);
/// 审计里记录的 run_id 来自这个环境变量（Agent 运行时由客户端注入，没有就为空）。
pub const RUN_ID_ENV: &str = "BEETLE_RUN_ID";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryOutput {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// 结果行数达到上限（可能还有更多行没返回）。
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// 截断过长的单元格。
pub fn cap_value(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(cap_str(s)),
        v @ (Value::Array(_) | Value::Object(_)) => {
            let s = v.to_string();
            if s.chars().count() > MAX_CELL_CHARS { Value::String(cap_str(s)) } else { v }
        }
        other => other,
    }
}

fn cap_str(s: String) -> String {
    if s.chars().count() <= MAX_CELL_CHARS {
        return s;
    }
    let mut t: String = s.chars().take(MAX_CELL_CHARS).collect();
    t.push('…');
    t
}

/// 兜底：错误信息里万一带出密码，替换成 ***。
fn scrub(e: String, password: Option<&str>) -> String {
    match password {
        Some(p) if !p.is_empty() => e.replace(p, "***"),
        _ => e,
    }
}

fn require_cost(req: &Value) -> Result<(), String> {
    match req.get("cost").and_then(Value::as_str).map(str::trim) {
        Some(c) if !c.is_empty() => Ok(()),
        _ => Err("cost 必填：写明走哪个索引、预计多少行".into()),
    }
}

fn direct_of(ds: &DataSource) -> Result<&DirectConn, String> {
    match &ds.access {
        Access::Direct(c) => Ok(c),
        Access::Skill(s) => Err(format!("这个数据源配置为用技能 {} 查询，请改用该技能", s.skill)),
    }
}

/// 有 SSH 配置就起隧道（阻塞操作放到 blocking 线程）；返回（守卫，实际连接的主机，端口）。
async fn endpoint(conn: &DirectConn) -> Result<(Option<ssh::Tunnel>, String, u16), String> {
    let Some(t) = conn.ssh.clone() else {
        return Ok((None, conn.host.clone(), conn.port));
    };
    let (host, port) = (conn.host.clone(), conn.port);
    let tunnel = tokio::task::spawn_blocking(move || ssh::open(&t, &host, port))
        .await
        .map_err(|e| format!("SSH 隧道线程异常：{e}"))??;
    let local = tunnel.local_port;
    Ok((Some(tunnel), "127.0.0.1".into(), local))
}

/// 统一入口：校验请求、按类型执行。错误信息为中文，且不含密码。
pub async fn run_query(ds: &DataSource, password: Option<&str>, req: &Value) -> Result<QueryOutput, String> {
    run_query_inner(ds, password, req).await.map_err(|e| scrub(e, password))
}

async fn run_query_inner(ds: &DataSource, password: Option<&str>, req: &Value) -> Result<QueryOutput, String> {
    require_cost(req)?;
    let conn = direct_of(ds)?;
    // 先校验（不连库），校验不过就不起隧道
    let checked = match ds.kind {
        DbKind::Mysql | DbKind::Postgres | DbKind::Starrocks => {
            let raw = req.get("sql").and_then(Value::as_str).ok_or("sql 必填")?;
            Some(guard::check_sql(ds.kind, raw, conn.max_rows)?)
        }
        DbKind::Mongodb => {
            mongo::parse_request(req, conn.max_rows)?;
            None
        }
        DbKind::Other => return Err("「其他」类型的数据源不支持直连查询".into()),
    };
    let (tunnel, host, port) = endpoint(conn).await?;
    let limit = Duration::from_secs(conn.timeout_secs.max(1) as u64) + OVERALL_SLACK;
    let fut = async {
        match &checked {
            Some(text) => sql::query(ds.kind, &host, port, conn, password, text).await,
            None => mongo::find(&host, port, conn, password, req).await,
        }
    };
    let res = tokio::time::timeout(limit, fut)
        .await
        .unwrap_or_else(|_| Err(format!("查询超过 {} 秒未完成，已中止", limit.as_secs())));
    drop(tunnel); // 隧道在查询结束（含失败 / 超时）后立即关闭
    res
}

/// 测试连接：MySQL / PG 跑 `SELECT 1`，Mongo `ping`。成功返回「连接成功，耗时 x ms」。
pub async fn test_connection(ds: &DataSource, password: Option<&str>) -> Result<String, String> {
    test_connection_inner(ds, password).await.map_err(|e| scrub(e, password))
}

async fn test_connection_inner(ds: &DataSource, password: Option<&str>) -> Result<String, String> {
    let conn = direct_of(ds)?;
    if ds.kind == DbKind::Other {
        return Err("「其他」类型的数据源不支持直连".into());
    }
    let start = Instant::now();
    let (tunnel, host, port) = endpoint(conn).await?;
    let limit = Duration::from_secs(conn.timeout_secs.max(1) as u64) + OVERALL_SLACK;
    let fut = async {
        match ds.kind {
            DbKind::Mongodb => mongo::ping(&host, port, conn, password).await,
            kind => sql::ping(kind, &host, port, conn, password).await,
        }
    };
    let res = tokio::time::timeout(limit, fut)
        .await
        .unwrap_or_else(|_| Err(format!("超过 {} 秒没有连上", limit.as_secs())));
    drop(tunnel);
    res.map(|()| format!("连接成功，耗时 {} ms", start.elapsed().as_millis()))
}

/// 成功时 stdout 输出的 JSON。
pub fn output_json(out: &QueryOutput) -> Value {
    json!({
        "ok": true,
        "columns": out.columns,
        "rows": out.rows,
        "truncated": out.truncated,
        "elapsed_ms": out.elapsed_ms,
    })
}

/// 审计里记的语句摘要：SQL 原文；Mongo 记集合和 filter。
fn audit_text(req: &Value) -> String {
    if let Some(s) = req.get("sql").and_then(Value::as_str) {
        return s.to_string();
    }
    json!({"collection": req.get("collection"), "filter": req.get("filter")}).to_string()
}

fn audit(alias: &str, req: &Value, res: &Result<QueryOutput, String>, elapsed_ms: u64) {
    let run_id = std::env::var(RUN_ID_ENV).unwrap_or_default();
    let (rows, err) = match res {
        Ok(o) => (Some(o.rows.len() as u64), None),
        Err(e) => (None, Some(e.as_str())),
    };
    let r = crate::store::runs::RunStore::open(crate::commands::runs_db_path())
        .and_then(|s| s.record_db_query(alias, &run_id, &audit_text(req), rows, elapsed_ms, err));
    if let Err(e) = r {
        eprintln!("beetle: 写查询审计失败：{e}");
    }
}

/// 标准输入按 UTF-8 解码，去掉开头的 BOM（PowerShell 管道常带）。
fn decode_stdin(mut raw: Vec<u8>) -> Result<String, String> {
    if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        raw.drain(..3);
    }
    String::from_utf8(raw).map_err(|_| "标准输入不是 UTF-8".to_string())
}

const USAGE: &str = "用法：beetle --db-query <数据源别名>，查询 JSON 从标准输入读入";

/// `beetle --db-query <别名>`（`args` 是 `--db-query` 之后的参数）。返回进程退出码。
pub fn cli_main(args: &[String]) -> i32 {
    let Some(alias) = args.first().map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let cfg = match crate::store::config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let Some(ds) = cfg.dbs.get(alias) else {
        let names: Vec<&str> = cfg.dbs.keys().map(String::as_str).collect();
        let list = if names.is_empty() { "（还没有配置任何数据源）".to_string() } else { names.join("、") };
        eprintln!("找不到数据源 {alias}。可用的数据源：{list}");
        return 2;
    };
    if let Access::Skill(s) = &ds.access {
        eprintln!("数据源 {alias} 配置为用技能 {} 查询，请改用该技能", s.skill);
        return 2;
    }
    let password = match crate::store::secrets::db_password(alias) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut raw = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut raw) {
        eprintln!("读取标准输入失败：{e}");
        return 2;
    }
    let input = match decode_stdin(raw) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let req: Value = match serde_json::from_str(input.trim()) {
        Ok(v @ Value::Object(_)) => v,
        Ok(_) => {
            eprintln!("标准输入必须是 JSON 对象，例如 {{\"sql\": \"SELECT …\", \"cost\": \"走主键，1 行\"}}");
            return 2;
        }
        Err(e) => {
            eprintln!("标准输入不是合法的 JSON：{e}");
            return 2;
        }
    };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("启动异步运行时失败：{e}");
            return 1;
        }
    };
    let start = Instant::now();
    let res = rt.block_on(run_query(ds, password.as_deref(), &req));
    audit(alias, &req, &res, start.elapsed().as_millis() as u64);
    match res {
        Ok(out) => {
            println!("{}", output_json(&out));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::datasource::SkillRef;

    fn direct(kind: DbKind) -> DataSource {
        DataSource {
            kind,
            description: String::new(),
            access: Access::Direct(DirectConn {
                // 不可路由的地址：测试只验证连库之前的校验
                host: "192.0.2.1".into(),
                port: 3306,
                user: "u".into(),
                database: "demo_a".into(),
                ssh: None,
                timeout_secs: 1,
                max_rows: 200,
            }),
        }
    }

    #[tokio::test]
    async fn cost_is_required() {
        for req in [json!({"sql": "SELECT 1"}), json!({"sql": "SELECT 1", "cost": "  "})] {
            let e = run_query(&direct(DbKind::Mysql), None, &req).await.unwrap_err();
            assert!(e.contains("cost 必填"), "{e}");
        }
    }

    #[tokio::test]
    async fn skill_datasource_is_rejected() {
        let ds = DataSource {
            kind: DbKind::Mysql,
            description: String::new(),
            access: Access::Skill(SkillRef { skill: "my-sql-skill".into(), database: "demo_a".into(), note: String::new() }),
        };
        let e = run_query(&ds, None, &json!({"sql": "SELECT 1", "cost": "x"})).await.unwrap_err();
        assert!(e.contains("my-sql-skill"), "{e}");
        assert!(test_connection(&ds, None).await.is_err());
    }

    #[tokio::test]
    async fn write_sql_rejected_before_connecting() {
        let start = Instant::now();
        let e = run_query(&direct(DbKind::Mysql), Some("pw"), &json!({"sql": "DELETE FROM t", "cost": "x"}))
            .await
            .unwrap_err();
        assert!(e.contains("SELECT"), "{e}");
        assert!(start.elapsed() < Duration::from_millis(500), "校验失败不该去连库");
        let e = run_query(&direct(DbKind::Mysql), None, &json!({"cost": "x"})).await.unwrap_err();
        assert!(e.contains("sql 必填"), "{e}");
        let e = run_query(&direct(DbKind::Mongodb), None, &json!({"cost": "x", "collection": "c", "filter": {"$where": "1"}}))
            .await
            .unwrap_err();
        assert!(e.contains("$where"), "{e}");
        assert!(run_query(&direct(DbKind::Other), None, &json!({"cost": "x", "sql": "SELECT 1"})).await.is_err());
    }

    #[test]
    fn error_text_is_scrubbed() {
        assert_eq!(scrub("登录失败 s3cret!".into(), Some("s3cret!")), "登录失败 ***");
        assert_eq!(scrub("登录失败".into(), Some("")), "登录失败");
        assert_eq!(scrub("登录失败".into(), None), "登录失败");
    }

    #[test]
    fn long_cells_are_capped() {
        let v = cap_value(Value::String("字".repeat(MAX_CELL_CHARS + 10)));
        let s = v.as_str().unwrap();
        assert_eq!(s.chars().count(), MAX_CELL_CHARS + 1);
        assert!(s.ends_with('…'));
        assert_eq!(cap_value(json!("短")), json!("短"));
        assert_eq!(cap_value(json!({"a": 1})), json!({"a": 1}));
        let big = json!({"a": "x".repeat(MAX_CELL_CHARS)});
        assert!(cap_value(big).is_string(), "过大的嵌套对象转成截断字符串");
        assert_eq!(cap_value(json!(12)), json!(12));
    }

    #[test]
    fn output_shape() {
        let out = QueryOutput { columns: vec!["a".into()], rows: vec![vec![json!(1)]], truncated: false, elapsed_ms: 3 };
        assert_eq!(
            output_json(&out),
            json!({"ok": true, "columns": ["a"], "rows": [[1]], "truncated": false, "elapsed_ms": 3})
        );
    }

    #[test]
    fn audit_text_for_sql_and_mongo() {
        assert_eq!(audit_text(&json!({"sql": "SELECT 1", "cost": "x"})), "SELECT 1");
        let t = audit_text(&json!({"collection": "c", "filter": {"a": 1}, "cost": "x"}));
        assert!(t.contains("\"collection\":\"c\""), "{t}");
    }

    #[test]
    fn stdin_strips_bom_and_requires_utf8() {
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("{\"sql\":\"SELECT 1\",\"cost\":\"主键\"}".as_bytes());
        let s = decode_stdin(with_bom).unwrap();
        assert!(s.starts_with('{'), "{s}");
        assert!(serde_json::from_str::<Value>(&s).is_ok());
        assert_eq!(decode_stdin(b"{}".to_vec()).unwrap(), "{}");
        assert_eq!(decode_stdin(vec![0xFF, 0xFE, 0x7B, 0x00]).unwrap_err(), "标准输入不是 UTF-8");
    }

    #[test]
    fn cli_requires_alias() {
        assert_eq!(cli_main(&[]), 2);
        assert_eq!(cli_main(&["  ".to_string()]), 2);
    }
}
