//! MySQL / PostgreSQL 直连执行：每次查询新建单连接（不建池），先把会话设成只读并设语句超时，再执行，用完关掉。
//!
//! 只读第二道防线：
//! - MySQL：`SET SESSION TRANSACTION READ ONLY` + `SET SESSION max_execution_time=<ms>`；
//! - PG：`SET SESSION default_transaction_read_only = on` + `SET SESSION statement_timeout = <ms>`。
//!   用 SET 而不是连接参数，是为了兼容 pgbouncer 之类不认启动参数的代理。
//!
//! 都走文本协议：MySQL `query`（COM_QUERY），PG `simple_query`。PG 的文本协议不带类型，值一律是字符串；
//! MySQL 按列类型把整数 / 浮点转成数字，其余是字符串。
//!
//! 错误信息只用驱动的错误描述，不格式化连接选项（里面有密码）。

use super::{QueryOutput, cap_value};
use crate::store::datasource::{DbKind, DirectConn};
use mysql_async::consts::ColumnType;
use mysql_async::prelude::Queryable;
use serde_json::Value;
use std::time::{Duration, Instant};

/// 连到 `host:port`（有 SSH 隧道时是本地转发地址），执行已经过 guard 改写的 SQL。
pub async fn query(
    kind: DbKind,
    host: &str,
    port: u16,
    conn: &DirectConn,
    password: Option<&str>,
    sql: &str,
) -> Result<QueryOutput, String> {
    let start = Instant::now();
    let max = conn.max_rows.max(1) as usize;
    let (columns, rows) = match kind {
        DbKind::Mysql => mysql_query(host, port, conn, password, sql, max).await?,
        DbKind::Postgres => pg_query(host, port, conn, password, sql, max).await?,
        _ => return Err("这个数据源不是 MySQL / PostgreSQL".into()),
    };
    let truncated = rows.len() >= max;
    Ok(QueryOutput { columns, rows, truncated, elapsed_ms: start.elapsed().as_millis() as u64 })
}

/// 测试连接：只读会话里跑 `SELECT 1`。
pub async fn ping(kind: DbKind, host: &str, port: u16, conn: &DirectConn, password: Option<&str>) -> Result<(), String> {
    query(kind, host, port, conn, password, "SELECT 1")
        .await
        .map(|_| ())
        .map_err(|e| e.replacen("查询失败", "执行 SELECT 1 失败", 1))
}

fn timeout_ms(conn: &DirectConn) -> u64 {
    conn.timeout_secs.max(1) as u64 * 1000
}

fn connect_timeout(host: &str, port: u16, conn: &DirectConn) -> (Duration, String) {
    (Duration::from_millis(timeout_ms(conn)), format!("连接 {host}:{port} 超时（{} 秒）", conn.timeout_secs))
}

// ---------- MySQL ----------

async fn mysql_query(
    host: &str,
    port: u16,
    conn: &DirectConn,
    password: Option<&str>,
    sql: &str,
    max: usize,
) -> Result<(Vec<String>, Vec<Vec<Value>>), String> {
    let mut opts = mysql_async::OptsBuilder::default()
        .ip_or_hostname(host.to_string())
        .tcp_port(port)
        .user(Some(conn.user.clone()))
        .pass(password.filter(|p| !p.is_empty()).map(str::to_string));
    if !conn.database.is_empty() {
        opts = opts.db_name(Some(conn.database.clone()));
    }
    let (wait, timeout_msg) = connect_timeout(host, port, conn);
    let mut c = tokio::time::timeout(wait, mysql_async::Conn::new(opts))
        .await
        .map_err(|_| timeout_msg)?
        .map_err(|e| format!("连接 {host}:{port} 失败：{e}"))?;
    let setup = ["SET SESSION TRANSACTION READ ONLY".to_string(), format!("SET SESSION max_execution_time = {}", timeout_ms(conn))];
    for s in &setup {
        if let Err(e) = c.query_drop(s.as_str()).await {
            let _ = c.disconnect().await;
            return Err(format!("设置只读会话失败：{e}"));
        }
    }
    let res: Result<Vec<mysql_async::Row>, _> = c.query(sql).await;
    let _ = c.disconnect().await;
    let rows = res.map_err(|e| format!("查询失败：{e}"))?;
    let columns = rows
        .first()
        .map(|r| r.columns_ref().iter().map(|c| c.name_str().to_string()).collect())
        .unwrap_or_default();
    let out = rows.iter().take(max).map(mysql_row).collect();
    Ok((columns, out))
}

fn mysql_row(row: &mysql_async::Row) -> Vec<Value> {
    let cols = row.columns_ref();
    (0..row.len())
        .map(|i| {
            let ty = cols[i].column_type();
            cap_value(match row.as_ref(i) {
                None | Some(mysql_async::Value::NULL) => Value::Null,
                Some(v) => mysql_value(v, ty),
            })
        })
        .collect()
}

fn is_int_type(t: ColumnType) -> bool {
    matches!(
        t,
        ColumnType::MYSQL_TYPE_TINY
            | ColumnType::MYSQL_TYPE_SHORT
            | ColumnType::MYSQL_TYPE_LONG
            | ColumnType::MYSQL_TYPE_INT24
            | ColumnType::MYSQL_TYPE_LONGLONG
            | ColumnType::MYSQL_TYPE_YEAR
    )
}

fn is_float_type(t: ColumnType) -> bool {
    matches!(t, ColumnType::MYSQL_TYPE_FLOAT | ColumnType::MYSQL_TYPE_DOUBLE)
}

fn is_binary_type(t: ColumnType) -> bool {
    matches!(
        t,
        ColumnType::MYSQL_TYPE_BLOB
            | ColumnType::MYSQL_TYPE_TINY_BLOB
            | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
            | ColumnType::MYSQL_TYPE_LONG_BLOB
            | ColumnType::MYSQL_TYPE_GEOMETRY
            | ColumnType::MYSQL_TYPE_BIT
    )
}

fn num_f64(f: f64) -> Value {
    serde_json::Number::from_f64(f).map(Value::Number).unwrap_or_else(|| Value::String(f.to_string()))
}

/// 文本协议下值基本都是 Bytes：按列类型转数字；不是合法 UTF-8 的（二进制列）显示成长度。
fn mysql_value(v: &mysql_async::Value, ty: ColumnType) -> Value {
    use mysql_async::Value as V;
    match v {
        V::NULL => Value::Null,
        V::Int(i) => Value::from(*i),
        V::UInt(u) => Value::from(*u),
        V::Float(f) => num_f64(*f as f64),
        V::Double(f) => num_f64(*f),
        V::Bytes(b) => match std::str::from_utf8(b) {
            Ok(s) if is_int_type(ty) => s
                .parse::<i64>()
                .map(Value::from)
                .or_else(|_| s.parse::<u64>().map(Value::from))
                .unwrap_or_else(|_| Value::String(s.to_string())),
            Ok(s) if is_float_type(ty) => s.parse::<f64>().map(num_f64).unwrap_or_else(|_| Value::String(s.to_string())),
            Ok(s) if !(is_binary_type(ty) && s.chars().any(|c| c.is_control() && !c.is_whitespace())) => Value::String(s.to_string()),
            _ => Value::String(format!("<binary {} bytes>", b.len())),
        },
        other => Value::String(other.as_sql(true).trim_matches('\'').to_string()),
    }
}

// ---------- PostgreSQL ----------

async fn pg_query(
    host: &str,
    port: u16,
    conn: &DirectConn,
    password: Option<&str>,
    sql: &str,
    max: usize,
) -> Result<(Vec<String>, Vec<Vec<Value>>), String> {
    let mut cfg = tokio_postgres::Config::new();
    cfg.host(host).port(port).user(&conn.user).application_name("beetle");
    cfg.ssl_mode(tokio_postgres::config::SslMode::Prefer);
    if let Some(pw) = password.filter(|p| !p.is_empty()) {
        cfg.password(pw);
    }
    if !conn.database.is_empty() {
        cfg.dbname(&conn.database);
    }
    // TLS 用 prefer：服务器支持就加密，不支持就明文；不校验证书（只读查询、内网自签证书常见）。
    let tls = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|e| format!("初始化 TLS 失败：{e}"))?;
    let tls = postgres_native_tls::MakeTlsConnector::new(tls);
    let (wait, timeout_msg) = connect_timeout(host, port, conn);
    let (client, connection) = tokio::time::timeout(wait, cfg.connect(tls))
        .await
        .map_err(|_| timeout_msg)?
        .map_err(|e| format!("连接 {host}:{port} 失败：{e}"))?;
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    let result = async {
        client
            .batch_execute(&format!(
                "SET SESSION default_transaction_read_only = on; SET SESSION statement_timeout = {}",
                timeout_ms(conn)
            ))
            .await
            .map_err(|e| format!("设置只读会话失败：{e}"))?;
        let msgs = client.simple_query(sql).await.map_err(|e| format!("查询失败：{}", pg_err(&e)))?;
        let mut columns: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Value>> = Vec::new();
        for m in msgs {
            match m {
                tokio_postgres::SimpleQueryMessage::RowDescription(cols) if columns.is_empty() => {
                    columns = cols.iter().map(|c| c.name().to_string()).collect();
                }
                tokio_postgres::SimpleQueryMessage::Row(r) => {
                    if columns.is_empty() {
                        columns = r.columns().iter().map(|c| c.name().to_string()).collect();
                    }
                    if rows.len() < max {
                        rows.push(
                            (0..r.len())
                                .map(|i| cap_value(r.get(i).map(|s| Value::String(s.to_string())).unwrap_or(Value::Null)))
                                .collect(),
                        );
                    }
                }
                _ => {}
            }
        }
        Ok((columns, rows))
    }
    .await;
    drop(client);
    driver.abort();
    result
}

/// PG 的错误：有数据库返回的错误就用它的消息（更具体），否则用驱动描述。
fn pg_err(e: &tokio_postgres::Error) -> String {
    e.as_db_error().map(|d| d.message().to_string()).unwrap_or_else(|| e.to_string())
}

#[cfg(test)]
mod tests {
    //! 集成测试：读环境变量 `BEETLE_TEST_MYSQL` / `BEETLE_TEST_PG`（格式 `host:port:user:password:db`），
    //! 没设就直接返回（不算失败）。账号密码不写进代码。
    use super::*;

    fn conn_from_env(var: &str) -> Option<(DirectConn, String)> {
        let v = std::env::var(var).ok()?;
        let mut p = v.splitn(5, ':');
        let (host, port, user, pw, db) = (p.next()?, p.next()?, p.next()?, p.next()?, p.next()?);
        Some((
            DirectConn {
                host: host.into(),
                port: port.parse().ok()?,
                user: user.into(),
                database: db.into(),
                ssh: None,
                timeout_secs: 5,
                max_rows: 200,
            },
            pw.into(),
        ))
    }

    async fn select_and_readonly(kind: DbKind, var: &str, one: Value) {
        let Some((c, pw)) = conn_from_env(var) else { return };
        let out = query(kind, &c.host, c.port, &c, Some(&pw), "SELECT 1 AS a").await.unwrap();
        assert_eq!(out.columns, ["a"]);
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0][0], one);
        assert!(!out.truncated);
        ping(kind, &c.host, c.port, &c, Some(&pw)).await.unwrap();
        // 绕过 guard 直接执行写语句，会话只读必须拒绝
        let e = query(kind, &c.host, c.port, &c, Some(&pw), "CREATE TABLE beetle_ro_probe (id INT)")
            .await
            .expect_err("只读会话里建表应失败");
        assert!(!e.contains(&pw), "错误信息不能带密码");
    }

    #[tokio::test]
    async fn mysql_select_and_readonly_session() {
        select_and_readonly(DbKind::Mysql, "BEETLE_TEST_MYSQL", Value::from(1)).await;
    }

    #[tokio::test]
    async fn pg_select_and_readonly_session() {
        // PG 走文本协议，值是字符串
        select_and_readonly(DbKind::Postgres, "BEETLE_TEST_PG", Value::from("1")).await;
    }

    #[tokio::test]
    async fn wrong_password_error_has_no_password() {
        let Some((c, _)) = conn_from_env("BEETLE_TEST_MYSQL") else { return };
        let bad = "definitely-wrong-pw-9f3k";
        let e = query(DbKind::Mysql, &c.host, c.port, &c, Some(bad), "SELECT 1").await.unwrap_err();
        assert!(!e.contains(bad), "{e}");
    }

    #[test]
    fn mysql_text_values_by_column_type() {
        use mysql_async::Value as V;
        assert_eq!(mysql_value(&V::Bytes(b"42".to_vec()), ColumnType::MYSQL_TYPE_LONGLONG), Value::from(42));
        assert_eq!(mysql_value(&V::Bytes(b"1.5".to_vec()), ColumnType::MYSQL_TYPE_DOUBLE), Value::from(1.5));
        assert_eq!(mysql_value(&V::Bytes(b"12.30".to_vec()), ColumnType::MYSQL_TYPE_NEWDECIMAL), Value::from("12.30"));
        assert_eq!(mysql_value(&V::Bytes("中文".as_bytes().to_vec()), ColumnType::MYSQL_TYPE_VAR_STRING), Value::from("中文"));
        assert_eq!(mysql_value(&V::Bytes(vec![0xff, 0x00, 0x01]), ColumnType::MYSQL_TYPE_BLOB), Value::from("<binary 3 bytes>"));
    }
}
