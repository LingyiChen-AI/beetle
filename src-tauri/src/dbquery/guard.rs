//! SQL 只读校验：用 sqlparser 解析，只放行单条 SELECT / WITH…SELECT，并补齐 / 收紧 LIMIT。
//!
//! 这是客户端里的第一道防线；第二道是会话级只读（见 `sql.rs`）。
//! 注意 sqlparser 0.55 的 `Query` 用 `limit: Option<Expr>` + `offset: Option<Offset>`（0.56 起才改成 `limit_clause`），
//! 升级 sqlparser 时这里要跟着改。

use crate::store::datasource::DbKind;
use sqlparser::ast::{Expr, Query, SetExpr, Statement, Value};
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::parser::Parser;

/// 没写 LIMIT 时补的行数（再受 max_rows 约束）。
pub const DEFAULT_LIMIT: u32 = 50;

/// 校验并改写 SQL，返回实际要执行的语句。错误信息为中文，可直接给 Agent 看。
pub fn check_sql(kind: DbKind, sql: &str, max_rows: u32) -> Result<String, String> {
    let sql = sql.trim();
    let sql = sql.strip_suffix(';').unwrap_or(sql).trim();
    if sql.is_empty() {
        return Err("SQL 为空".into());
    }
    let dialect: Box<dyn Dialect> = match kind {
        DbKind::Mysql | DbKind::Starrocks => Box::new(MySqlDialect {}),
        DbKind::Postgres => Box::new(PostgreSqlDialect {}),
        DbKind::Mongodb | DbKind::Other => return Err("这个数据源不是 SQL 库，不能用 SQL 查询".into()),
    };
    // PG 的 @> / <@ / @@ 是正常运算符，只在 MySQL 里拦 @变量。
    if matches!(kind, DbKind::Mysql | DbKind::Starrocks) && has_at_outside_literals(sql) {
        return Err("不允许使用 @ 变量（包括 @@ 系统变量）".into());
    }
    if let Some(f) = dangerous_func(sql) {
        return Err(format!("不允许调用 {f}()：它会读服务器文件、占住连接或改变数据库状态"));
    }
    let mut stmts = Parser::parse_sql(dialect.as_ref(), sql).map_err(|e| format!("SQL 解析失败：{e}"))?;
    if stmts.len() != 1 {
        return Err(format!("一次只允许一条语句，收到 {} 条", stmts.len()));
    }
    let Some(Statement::Query(query)) = stmts.first_mut() else {
        return Err("只允许 SELECT 查询（包括 WITH … SELECT），不允许改数据、改结构或 SET / SHOW 等语句".into());
    };
    check_query(query)?;
    if query.fetch.is_some() {
        return Err("请用 LIMIT 限制行数，不支持 FETCH FIRST".into());
    }
    let max = max_rows.max(1);
    let limit = match &query.limit {
        None => DEFAULT_LIMIT.min(max),
        Some(Expr::Value(v)) => {
            let v: Value = v.clone().into();
            match v {
                Value::Number(n, _) => n.parse::<u64>().map(|n| n.min(max as u64) as u32).unwrap_or(max),
                _ => max,
            }
        }
        // LIMIT ALL、表达式、参数：一律收紧到上限
        Some(_) => max,
    };
    query.limit = Some(Expr::Value(Value::Number(limit.to_string(), false).into()));
    Ok(query.to_string())
}

fn check_query(q: &Query) -> Result<(), String> {
    if !q.locks.is_empty() {
        return Err("不允许 FOR UPDATE / FOR SHARE 等加锁查询".into());
    }
    if let Some(with) = &q.with {
        for cte in &with.cte_tables {
            check_query(&cte.query)?;
        }
    }
    check_set_expr(&q.body)
}

fn check_set_expr(e: &SetExpr) -> Result<(), String> {
    match e {
        SetExpr::Select(s) => {
            if s.into.is_some() {
                return Err("不允许 SELECT … INTO".into());
            }
            Ok(())
        }
        SetExpr::Query(q) => check_query(q),
        SetExpr::SetOperation { left, right, .. } => {
            check_set_expr(left)?;
            check_set_expr(right)
        }
        _ => Err("只允许 SELECT 查询（包括 WITH … SELECT）".into()),
    }
}

/// 把字符串、引号标识符、注释的内容替换成空格，只留下「代码」部分（长度不变），供 `@` 和危险函数检查用。
fn code_only(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out: Vec<char> = chars.clone();
    let blank = |out: &mut Vec<char>, from: usize, to: usize| {
        for c in out.iter_mut().take(to.min(chars.len())).skip(from) {
            *c = ' ';
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        match c {
            '\'' | '"' | '`' => {
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\\' && c != '`' {
                        i += 2;
                        continue;
                    }
                    if chars[i] == c {
                        // 连写两个引号是转义
                        if chars.get(i + 1) == Some(&c) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                blank(&mut out, start, i + 1);
            }
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                blank(&mut out, start, i);
            }
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                blank(&mut out, start, i);
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 1;
                blank(&mut out, start, i + 1);
            }
            _ => {}
        }
        i += 1;
    }
    out.into_iter().collect()
}

/// 字符串 / 引号标识符 / 注释之外是否出现 `@`（MySQL 用户变量、系统变量）。
fn has_at_outside_literals(sql: &str) -> bool {
    code_only(sql).contains('@')
}

/// SELECT 里也能调用的危险函数：读服务器文件、拖住数据库、跨库连接、加锁。只读账号也挡不住其中一部分，所以在这里拦。
const DANGEROUS_FUNCS: &[&str] = &[
    // MySQL
    "load_file", "sleep", "benchmark", "get_lock", "release_lock", "release_all_locks",
    // PostgreSQL
    "pg_sleep", "pg_sleep_for", "pg_sleep_until", "pg_read_file", "pg_read_binary_file", "pg_ls_dir", "pg_stat_file",
    "lo_import", "lo_export", "lo_get", "dblink", "dblink_exec", "pg_advisory_lock", "pg_advisory_xact_lock",
    "pg_terminate_backend", "pg_cancel_backend", "pg_reload_conf", "set_config", "nextval", "setval",
];

fn dangerous_func(sql: &str) -> Option<&'static str> {
    let code = code_only(sql).to_lowercase();
    DANGEROUS_FUNCS.iter().copied().find(|f| {
        let re = regex::Regex::new(&format!(r"(^|[^a-z0-9_$.]){}\s*\(", regex::escape(f))).expect("固定正则");
        re.is_match(&code)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(kind: DbKind, sql: &str) -> String {
        check_sql(kind, sql, 200).unwrap_or_else(|e| panic!("{sql} 应放行，实际报错：{e}"))
    }

    #[test]
    fn guard_cases_allowed() {
        let s = ok(DbKind::Mysql, "SELECT id FROM t WHERE id=1");
        assert!(s.ends_with("LIMIT 50"), "{s}");
        assert!(s.starts_with("SELECT id FROM t WHERE id = 1"), "{s}");

        let s = ok(DbKind::Mysql, "WITH a AS (SELECT 1) SELECT * FROM a LIMIT 10");
        assert!(s.starts_with("WITH a AS (SELECT 1) SELECT * FROM a"), "{s}");
        assert!(s.ends_with("LIMIT 10"), "{s}");

        let s = ok(DbKind::Mysql, "SELECT id FROM t LIMIT 1000");
        assert!(s.ends_with("LIMIT 200"), "{s}");

        let s = ok(DbKind::Postgres, "SELECT id FROM t LIMIT 5 OFFSET 2");
        assert!(s.contains("LIMIT 5"), "{s}");
        assert!(s.contains("OFFSET 2"), "{s}");

        let s = ok(DbKind::Mysql, "select id from t order by id desc;");
        assert!(s.ends_with("LIMIT 50"), "{s}");

        // MySQL 的 LIMIT offset, count
        let s = ok(DbKind::Mysql, "SELECT id FROM t LIMIT 3, 500");
        assert!(s.contains("LIMIT 200"), "{s}");
        assert!(s.contains("OFFSET 3"), "{s}");

        // 字符串里的 @、分号不算
        let s = ok(DbKind::Mysql, "SELECT id FROM t WHERE email = 'a@b.com;x'");
        assert!(s.contains("'a@b.com;x'"), "{s}");

        // PG 的 jsonb 包含运算符
        ok(DbKind::Postgres, "SELECT id FROM t WHERE tags @> '[\"x\"]'");

        // UNION 和子查询
        let s = ok(DbKind::Postgres, "SELECT 1 UNION ALL SELECT 2");
        assert!(s.ends_with("LIMIT 50"), "{s}");
        ok(DbKind::Mysql, "SELECT * FROM (SELECT id FROM t) x");
    }

    #[test]
    fn max_rows_caps_default_limit() {
        let s = check_sql(DbKind::Mysql, "SELECT 1", 20).unwrap();
        assert!(s.ends_with("LIMIT 20"), "{s}");
    }

    #[test]
    fn guard_cases_rejected() {
        let cases: [(DbKind, &str); 13] = [
            (DbKind::Mysql, "DELETE FROM t"),
            (DbKind::Mysql, "UPDATE t SET a=1"),
            (DbKind::Mysql, "INSERT INTO t VALUES (1)"),
            (DbKind::Mysql, "SELECT 1; SELECT 2"),
            (DbKind::Mysql, "SELECT id INTO @x FROM t"),
            (DbKind::Mysql, "SELECT id FROM t FOR UPDATE"),
            (DbKind::Mysql, "SET @a=1"),
            (DbKind::Mysql, "SHOW TABLES"),
            (DbKind::Mysql, ""),
            (DbKind::Mysql, "SELEC id FRM t"),
            (DbKind::Mysql, "DROP TABLE t"),
            (DbKind::Postgres, "SELECT id INTO new_t FROM t"),
            (DbKind::Postgres, "SELECT id FROM t FOR SHARE"),
        ];
        for (kind, sql) in cases {
            let e = check_sql(kind, sql, 200).expect_err(sql);
            assert!(!e.is_empty(), "{sql}");
            assert!(e.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)), "错误信息应为中文：{e}");
        }
    }

    #[test]
    fn starrocks_uses_mysql_rules() {
        let s = ok(DbKind::Starrocks, "SELECT id FROM voc.sessions WHERE shop_id = 1");
        assert!(s.ends_with("LIMIT 50"), "{s}");
        assert!(check_sql(DbKind::Starrocks, "SELECT @x", 200).is_err());
        assert!(check_sql(DbKind::Starrocks, "INSERT INTO voc.t VALUES (1)", 200).is_err());
    }

    #[test]
    fn non_sql_kinds_rejected() {
        assert!(check_sql(DbKind::Mongodb, "SELECT 1", 200).is_err());
        assert!(check_sql(DbKind::Other, "SELECT 1", 200).is_err());
    }

    #[test]
    fn dangerous_functions_rejected() {
        for (kind, sql) in [
            (DbKind::Mysql, "SELECT LOAD_FILE('/etc/passwd')"),
            (DbKind::Mysql, "SELECT id FROM t WHERE SLEEP (10) = 0"),
            (DbKind::Mysql, "SELECT BENCHMARK(1000000, MD5('x'))"),
            (DbKind::Postgres, "SELECT pg_sleep(5)"),
            (DbKind::Postgres, "SELECT pg_read_file('postgresql.conf')"),
            (DbKind::Postgres, "SELECT * FROM dblink('host=x', 'select 1') AS t(a int)"),
            (DbKind::Postgres, "SELECT nextval('seq')"),
        ] {
            let e = check_sql(kind, sql, 200).expect_err(sql);
            assert!(e.contains("不允许调用"), "{sql} → {e}");
        }
        // 字符串里、列名里出现同名单词不算
        ok(DbKind::Mysql, "SELECT id FROM t WHERE note = 'sleep(1)'");
        ok(DbKind::Mysql, "SELECT sleep_count FROM t");
        ok(DbKind::Postgres, "SELECT t.sleep FROM t");
    }

    #[test]
    fn at_scanner_skips_literals_and_comments() {
        assert!(!has_at_outside_literals("SELECT '@' AS a, \"@\" AS b, `@c` FROM t -- @x\n"));
        assert!(!has_at_outside_literals("SELECT 1 /* @x */"));
        assert!(!has_at_outside_literals("SELECT 'it''s @' FROM t"));
        assert!(has_at_outside_literals("SELECT @x"));
        assert!(has_at_outside_literals("SELECT @@version"));
        assert!(has_at_outside_literals("SELECT 'a' , @x"));
    }
}
