//! 数据源：用户自己配置的库。两种查询方式：
//! - 直连（`Access::Direct`）：客户端连库、强制只读，Agent 只拿结果（`beetle --db-query <别名>`）；
//! - 自定义技能（`Access::Skill`）：只告诉 Agent「这个库用哪个技能查」，客户端不连库。
//!
//! 密码不在这里：只存系统凭据库（见 `secrets::set_db_password`）。
//! 反序列化兼容旧格式 `{"kind":"mysql","database":"x"}`（没有 `access`）→ 技能方式 `sql-window`。

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbKind {
    Mysql,
    Postgres,
    /// StarRocks：MySQL 协议，但不支持只读事务和 max_execution_time，超时用 query_timeout。
    Starrocks,
    Mongodb,
    Other,
}

impl DbKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DbKind::Mysql => "mysql",
            DbKind::Postgres => "postgres",
            DbKind::Starrocks => "starrocks",
            DbKind::Mongodb => "mongodb",
            DbKind::Other => "other",
        }
    }

    /// 宽松解析：大小写不敏感，常见别名也认；认不出的一律当 `Other`（旧配置里可能写了别的库类型）。
    pub fn parse_lenient(s: &str) -> DbKind {
        match s.trim().to_ascii_lowercase().as_str() {
            "mysql" | "mariadb" => DbKind::Mysql,
            "postgres" | "postgresql" | "pg" => DbKind::Postgres,
            "starrocks" | "sr" | "doris" => DbKind::Starrocks,
            "mongodb" | "mongo" => DbKind::Mongodb,
            _ => DbKind::Other,
        }
    }
}

fn d22() -> u16 {
    22
}

fn d5() -> u32 {
    5
}

fn d200() -> u32 {
    200
}

/// SSH 跳板：用系统 ssh 起 `-N -L` 本地转发，查询结束即结束进程。只支持私钥登录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SshTunnel {
    pub host: String,
    #[serde(default = "d22")]
    pub port: u16,
    pub user: String,
    pub key_path: String,
}

/// 直连信息（不含密码）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectConn {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
    #[serde(default)]
    pub ssh: Option<SshTunnel>,
    /// 单条查询超时（秒），默认 5。
    #[serde(default = "d5")]
    pub timeout_secs: u32,
    /// 单次最多返回行数，默认 200。
    #[serde(default = "d200")]
    pub max_rows: u32,
}

/// 用用户本机已装的技能查：Beetle 只引用技能名，不调用它。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillRef {
    pub skill: String,
    #[serde(default)]
    pub database: String,
    /// 给 Agent 的补充说明，例如「库名写 xxx」。
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "access", rename_all = "lowercase")]
pub enum Access {
    Direct(DirectConn),
    Skill(SkillRef),
}

/// 一个数据源（键是别名，存在 `AppConfig.dbs` 里）。
/// 序列化形如 `{"kind","description","access":"direct",host,...}` 或 `{"kind","description","access":"skill",skill,...}`。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DataSource {
    pub kind: DbKind,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub access: Access,
}

/// 旧格式升级时使用的技能名（用户本机自己装的技能，仓库里不再内置）。
pub const LEGACY_SKILL: &str = "sql-window";

impl<'de> Deserialize<'de> for DataSource {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let v = Value::deserialize(d)?;
        let Value::Object(obj) = &v else {
            return Err(D::Error::custom("数据源配置必须是 JSON 对象"));
        };
        let kind = obj.get("kind").and_then(Value::as_str).map(DbKind::parse_lenient).unwrap_or(DbKind::Other);
        let description = obj.get("description").and_then(Value::as_str).unwrap_or_default().to_string();
        let access = if obj.contains_key("access") {
            Access::deserialize(v.clone()).map_err(D::Error::custom)?
        } else {
            // 旧格式 {"kind":"mysql","database":"x"}：当时只能经 sql-window 技能查。
            Access::Skill(SkillRef {
                skill: LEGACY_SKILL.to_string(),
                database: obj.get("database").and_then(Value::as_str).unwrap_or_default().to_string(),
                note: String::new(),
            })
        };
        Ok(DataSource { kind, description, access })
    }
}

impl DataSource {
    /// 运行时配置里给 Agent 看的一项：不含密码、主机、账号、SSH 信息。
    pub fn runtime_value(&self, alias: &str) -> Value {
        match &self.access {
            Access::Direct(c) => json!({
                "alias": alias,
                "kind": self.kind.as_str(),
                "description": self.description,
                "access": "direct",
                "database": c.database,
            }),
            Access::Skill(s) => json!({
                "alias": alias,
                "kind": self.kind.as_str(),
                "description": self.description,
                "access": "skill",
                "skill": s.skill,
                "database": s.database,
                "note": s.note,
            }),
        }
    }

    /// 直连信息；技能方式返回 None。
    pub fn direct(&self) -> Option<&DirectConn> {
        match &self.access {
            Access::Direct(c) => Some(c),
            Access::Skill(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct_ds() -> DataSource {
        DataSource {
            kind: DbKind::Mysql,
            description: "演示库".into(),
            access: Access::Direct(DirectConn {
                host: "10.0.0.8".into(),
                port: 3306,
                user: "reader".into(),
                database: "demo_a".into(),
                ssh: Some(SshTunnel { host: "1.2.3.4".into(), port: 22, user: "root".into(), key_path: "C:/k/id".into() }),
                timeout_secs: 5,
                max_rows: 200,
            }),
        }
    }

    #[test]
    fn old_entry_becomes_skill_sql_window() {
        let ds: DataSource = serde_json::from_value(json!({"kind": "mysql", "database": "demo_a"})).unwrap();
        assert_eq!(ds.kind, DbKind::Mysql);
        assert_eq!(ds.description, "");
        assert_eq!(
            ds.access,
            Access::Skill(SkillRef { skill: "sql-window".into(), database: "demo_a".into(), note: String::new() })
        );
    }

    #[test]
    fn unknown_kind_becomes_other() {
        let ds: DataSource = serde_json::from_value(json!({"kind": "oracle", "database": "demo_b"})).unwrap();
        assert_eq!(ds.kind, DbKind::Other);
        let ds: DataSource = serde_json::from_value(json!({"kind": "PostgreSQL", "database": "demo_c"})).unwrap();
        assert_eq!(ds.kind, DbKind::Postgres);
    }

    #[test]
    fn direct_roundtrip_and_shape() {
        let ds = direct_ds();
        let v = serde_json::to_value(&ds).unwrap();
        assert_eq!(v["access"], "direct");
        assert_eq!(v["kind"], "mysql");
        assert_eq!(v["host"], "10.0.0.8");
        assert_eq!(v["ssh"]["port"], 22);
        let back: DataSource = serde_json::from_value(v).unwrap();
        assert_eq!(back, ds);
    }

    #[test]
    fn direct_defaults_fill_in() {
        let ds: DataSource = serde_json::from_value(json!({
            "kind": "postgres", "access": "direct", "host": "h", "port": 5432, "user": "u", "database": "demo_d",
            "ssh": {"host": "j", "user": "root", "key_path": "k"}
        }))
        .unwrap();
        let c = ds.direct().unwrap();
        assert_eq!((c.timeout_secs, c.max_rows), (5, 200));
        assert_eq!(c.ssh.as_ref().unwrap().port, 22);
    }

    #[test]
    fn skill_roundtrip() {
        let ds = DataSource {
            kind: DbKind::Other,
            description: "x".into(),
            access: Access::Skill(SkillRef { skill: "my-skill".into(), database: "demo_e".into(), note: "库名写 e".into() }),
        };
        let v = serde_json::to_value(&ds).unwrap();
        assert_eq!(v, json!({"kind": "other", "description": "x", "access": "skill", "skill": "my-skill", "database": "demo_e", "note": "库名写 e"}));
        assert_eq!(serde_json::from_value::<DataSource>(v).unwrap(), ds);
    }

    #[test]
    fn unknown_access_is_error() {
        assert!(serde_json::from_value::<DataSource>(json!({"kind": "mysql", "access": "magic"})).is_err());
    }

    #[test]
    fn runtime_value_hides_connection_details() {
        let v = direct_ds().runtime_value("demo");
        assert_eq!(v, json!({"alias": "demo", "kind": "mysql", "description": "演示库", "access": "direct", "database": "demo_a"}));
        let s = v.to_string();
        for bad in ["host", "user", "password", "ssh", "10.0.0.8", "reader"] {
            assert!(!s.contains(bad), "运行时配置不该出现 {bad}：{s}");
        }
        let ds: DataSource = serde_json::from_value(json!({"kind": "mysql", "database": "demo_a"})).unwrap();
        assert_eq!(
            ds.runtime_value("a"),
            json!({"alias": "a", "kind": "mysql", "description": "", "access": "skill", "skill": "sql-window", "database": "demo_a", "note": ""})
        );
    }
}
