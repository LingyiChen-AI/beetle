//! MongoDB 直连：只做 find（limit 强制、`maxTimeMS` = 超时），不支持 aggregate / 写操作。
//! 请求：`{"collection","filter","projection","sort","limit","cost"}`，filter / projection / sort 用扩展 JSON（如 `{"$oid": "..."}`）。

use super::{QueryOutput, cap_value};
use crate::dbquery::guard::DEFAULT_LIMIT;
use crate::store::datasource::DirectConn;
use futures_util::TryStreamExt;
use mongodb::Client;
use mongodb::bson::{Bson, Document, doc};
use mongodb::options::{ClientOptions, Credential, ServerAddress};
use serde_json::Value;
use std::time::{Duration, Instant};

/// 表达式里不允许出现的运算符（会在服务端执行任意 JS）。
const FORBIDDEN_OPS: [&str; 3] = ["$where", "$function", "$accumulator"];

/// 校验通过的 find 请求。
#[derive(Debug, PartialEq)]
pub struct FindReq {
    pub collection: String,
    pub filter: Document,
    pub projection: Option<Document>,
    pub sort: Option<Document>,
    pub limit: i64,
}

/// 解析并校验请求（不连库，便于单测）。
pub fn parse_request(req: &Value, max_rows: u32) -> Result<FindReq, String> {
    let collection = req
        .get("collection")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("collection 必填：要查的集合名")?
        .to_string();
    if req.get("pipeline").is_some() || req.get("aggregate").is_some() {
        return Err("只支持 find 查询，不支持 aggregate".into());
    }
    for key in ["filter", "projection", "sort"] {
        if let Some(v) = req.get(key)
            && let Some(op) = find_forbidden(v)
        {
            return Err(format!("{key} 里不允许使用 {op}"));
        }
    }
    let filter = to_doc(req.get("filter"), "filter")?.unwrap_or_default();
    let projection = to_doc(req.get("projection"), "projection")?;
    let sort = to_doc(req.get("sort"), "sort")?;
    let max = max_rows.max(1) as i64;
    let limit = match req.get("limit") {
        None | Some(Value::Null) => (DEFAULT_LIMIT as i64).min(max),
        Some(v) => match v.as_i64() {
            Some(n) if n > 0 => n.min(max),
            _ => return Err("limit 必须是正整数".into()),
        },
    };
    Ok(FindReq { collection, filter, projection, sort, limit })
}

fn find_forbidden(v: &Value) -> Option<&'static str> {
    match v {
        Value::Object(m) => {
            for (k, child) in m {
                if let Some(op) = FORBIDDEN_OPS.iter().find(|op| k.eq_ignore_ascii_case(op)) {
                    return Some(*op);
                }
                if let Some(op) = find_forbidden(child) {
                    return Some(op);
                }
            }
            None
        }
        Value::Array(a) => a.iter().find_map(find_forbidden),
        _ => None,
    }
}

fn to_doc(v: Option<&Value>, name: &str) -> Result<Option<Document>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v @ Value::Object(_)) => match Bson::try_from(v.clone()) {
            Ok(Bson::Document(d)) => Ok(Some(d)),
            Ok(_) => Err(format!("{name} 必须是 JSON 对象")),
            Err(e) => Err(format!("{name} 解析失败：{e}")),
        },
        Some(_) => Err(format!("{name} 必须是 JSON 对象")),
    }
}

async fn client(host: &str, port: u16, conn: &DirectConn, password: Option<&str>) -> Result<Client, String> {
    let addr = ServerAddress::parse(format!("{host}:{port}")).map_err(|e| format!("地址不合法：{e}"))?;
    let wait = Duration::from_secs(conn.timeout_secs.max(1) as u64);
    let mut opts = ClientOptions::default();
    opts.hosts = vec![addr];
    // 经隧道或单节点地址时不去连副本集返回的成员地址（那些常是私网地址）
    opts.direct_connection = Some(true);
    opts.server_selection_timeout = Some(wait);
    opts.connect_timeout = Some(wait);
    opts.app_name = Some("beetle".into());
    if !conn.user.is_empty() {
        let mut cred = Credential::default();
        cred.username = Some(conn.user.clone());
        cred.password = password.filter(|p| !p.is_empty()).map(str::to_string);
        // source 不填：驱动默认用 admin 库认证
        opts.credential = Some(cred);
    }
    Client::with_options(opts).map_err(|e| format!("创建 MongoDB 客户端失败：{e}"))
}

pub async fn find(host: &str, port: u16, conn: &DirectConn, password: Option<&str>, req: &Value) -> Result<QueryOutput, String> {
    let r = parse_request(req, conn.max_rows)?;
    if conn.database.is_empty() {
        return Err("数据源没有配置库名".into());
    }
    let start = Instant::now();
    let c = client(host, port, conn, password).await?;
    let coll = c.database(&conn.database).collection::<Document>(&r.collection);
    let mut action = coll
        .find(r.filter)
        .limit(r.limit)
        .max_time(Duration::from_secs(conn.timeout_secs.max(1) as u64));
    if let Some(p) = r.projection {
        action = action.projection(p);
    }
    if let Some(s) = r.sort {
        action = action.sort(s);
    }
    let mut cursor = action.await.map_err(|e| format!("查询失败：{e}"))?;
    let mut docs: Vec<Document> = Vec::new();
    while let Some(d) = cursor.try_next().await.map_err(|e| format!("读取结果失败：{e}"))? {
        docs.push(d);
        if docs.len() as i64 >= r.limit {
            break;
        }
    }
    let (columns, rows) = docs_to_table(docs);
    let truncated = rows.len() as i64 >= conn.max_rows.max(1) as i64;
    Ok(QueryOutput { columns, rows, truncated, elapsed_ms: start.elapsed().as_millis() as u64 })
}

/// 测试连接：在配置的库上跑 `ping`。
pub async fn ping(host: &str, port: u16, conn: &DirectConn, password: Option<&str>) -> Result<(), String> {
    let c = client(host, port, conn, password).await?;
    let db = if conn.database.is_empty() { "admin" } else { conn.database.as_str() };
    c.database(db)
        .run_command(doc! {"ping": 1})
        .await
        .map(|_| ())
        .map_err(|e| format!("ping 失败：{e}"))
}

/// 文档列表转成表格：列按首次出现的顶层字段顺序，缺的字段填 null；值转成宽松扩展 JSON。
pub fn docs_to_table(docs: Vec<Document>) -> (Vec<String>, Vec<Vec<Value>>) {
    let mut columns: Vec<String> = Vec::new();
    for d in &docs {
        for k in d.keys() {
            if !columns.iter().any(|c| c == k) {
                columns.push(k.clone());
            }
        }
    }
    let rows = docs
        .into_iter()
        .map(|mut d| {
            columns
                .iter()
                .map(|c| d.remove(c).map(|b| cap_value(b.into_relaxed_extjson())).unwrap_or(Value::Null))
                .collect()
        })
        .collect();
    (columns, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_defaults_and_caps_limit() {
        let r = parse_request(&json!({"collection": "orders", "cost": "x"}), 200).unwrap();
        assert_eq!(r.limit, 50);
        assert!(r.filter.is_empty());
        let r = parse_request(&json!({"collection": "orders", "limit": 1000}), 200).unwrap();
        assert_eq!(r.limit, 200);
        let r = parse_request(&json!({"collection": "orders"}), 20).unwrap();
        assert_eq!(r.limit, 20);
    }

    #[test]
    fn extended_json_filter_is_parsed() {
        let r = parse_request(
            &json!({"collection": "c", "filter": {"_id": {"$oid": "5f1d7f3a9d3b2c1a0b0c0d0e"}}, "sort": {"_id": -1}}),
            200,
        )
        .unwrap();
        assert!(matches!(r.filter.get("_id"), Some(Bson::ObjectId(_))));
        assert_eq!(r.sort.unwrap().get_i32("_id").unwrap(), -1);
    }

    #[test]
    fn rejects_bad_requests() {
        let cases = [
            json!({"filter": {}}),
            json!({"collection": ""}),
            json!({"collection": "c", "filter": {"$where": "this.a > 1"}}),
            json!({"collection": "c", "filter": {"$and": [{"$expr": {"$function": {"body": "x"}}}]}}),
            json!({"collection": "c", "projection": {"x": {"$accumulator": {}}}}),
            json!({"collection": "c", "filter": [1, 2]}),
            json!({"collection": "c", "limit": -1}),
            json!({"collection": "c", "limit": "10"}),
            json!({"collection": "c", "pipeline": []}),
        ];
        for c in cases {
            assert!(parse_request(&c, 200).is_err(), "{c}");
        }
    }

    #[test]
    fn docs_become_table_with_union_columns() {
        let (cols, rows) = docs_to_table(vec![doc! {"a": 1, "b": "x"}, doc! {"b": "y", "c": true}]);
        assert_eq!(cols, ["a", "b", "c"]);
        assert_eq!(rows[0], vec![json!(1), json!("x"), Value::Null]);
        assert_eq!(rows[1], vec![Value::Null, json!("y"), json!(true)]);
    }
}
