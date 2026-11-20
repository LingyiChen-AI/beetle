//! 密钥打码：复制自 MemDock `src-tauri/src/secrets.rs` 的 `redact` 及其规则（只取打码部分，扫描部分本项目用不到）。
//! 用于发到群里的最终回复、运行步骤和原始日志。

use std::sync::OnceLock;

use regex::Regex;

struct Rules {
    private_key: Regex,
    private_key_open: Regex,
    password: Regex,
    password_zh: Regex,
    tokens: Vec<Regex>,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| Rules {
        private_key: Regex::new(
            r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
        )
        .unwrap(),
        private_key_open: Regex::new(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*").unwrap(),
        password: Regex::new(concat!(
            r#"(?i)(["']?[A-Za-z0-9_\-]*(?:pass(?:word)?|passwd|pwd|secret|api[_-]?key|access[_-]?key|token)[A-Za-z0-9_\-]*["']?\s*[:=]\s*)"#,
            // 与 MemDock 不同：不加引号的值在空白、引号、中文标点、英文逗号 / 分号 / 右括号 / 右花括号处截止，
            // 不吞掉后面的正文，也不破坏 JSON 行的结构。
            r#"("[^"\n]*"|'[^'\n]*'|[^\s"'，。；：、（）【】「」,;)}]+)"#
        ))
        .unwrap(),
        password_zh: Regex::new(concat!(
            "((?:密码|口令|秘钥|密钥)\\s*(?:[:：=]|是|为)\\s*)",
            "(\"[^\"\\n]*\"|'[^'\\n]*'|“[^”\\n]*”|[^\\s\"'，。；：、（）【】「」,;)}]+)"
        ))
        .unwrap(),
        tokens: [
            r"\bsk-ant-[A-Za-z0-9_\-]{16,}",
            r"\bsk-[A-Za-z0-9_\-]{16,}",
            r"\bgh[pousr]_[A-Za-z0-9]{20,}",
            r"\bgithub_pat_[A-Za-z0-9_]{20,}",
            r"\bxox[bpas]-[A-Za-z0-9\-]{10,}",
            r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
            r"(?i)\bbearer\s+[A-Za-z0-9._\-]{20,}",
            // HTTP Basic 认证：base64 的 用户名:密码
            r"(?i)\bbasic\s+[A-Za-z0-9+/]{16,}={0,2}",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect(),
    })
}

/// 把 API key、token、私钥、`password=...` 一类的值替换成 `[REDACTED]`。token 类保留前 4 个字符。
pub fn redact(text: &str) -> String {
    let r = rules();
    let mut out = r.private_key.replace_all(text, "[REDACTED]").into_owned();
    out = r.private_key_open.replace_all(&out, "[REDACTED]").into_owned();
    // 带引号的值保留引号（`"api_key":"x"` → `"api_key":"[REDACTED]"`）。
    // JSON 行请用 `redact_json_line`，文本规则不保证 JSON 结构。
    let keep_quotes = |c: &regex::Captures| {
        let (open, close) = match c[2].chars().next() {
            Some('"') => ("\"", "\""),
            Some('\'') => ("'", "'"),
            Some('“') => ("“", "”"),
            _ => ("", ""),
        };
        format!("{}{open}[REDACTED]{close}", &c[1])
    };
    out = r.password.replace_all(&out, keep_quotes).into_owned();
    out = r.password_zh.replace_all(&out, keep_quotes).into_owned();
    for re in &r.tokens {
        out = re
            .replace_all(&out, |c: &regex::Captures| {
                let head: String = c[0].chars().take(4).collect();
                format!("{head}…[REDACTED]")
            })
            .into_owned();
    }
    out
}

/// 键名是否敏感：pass / passwd / pwd / secret / api key / access key / token，以及 HTTP 认证头、cookie、凭据、私钥、会话 id。
/// 排除只是碰巧包含这些片段的常见键名（bypass、passthrough、compass、tokenizer）。
fn sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase().replace(['_', '-'], "");
    const NOT_SECRET: [&str; 4] = ["bypass", "passthrough", "compass", "tokenizer"];
    if NOT_SECRET.iter().any(|w| k.contains(w)) {
        return false;
    }
    ["pass", "pwd", "secret", "apikey", "accesskey", "token", "authorization", "cookie", "credential", "privatekey", "sessionid"]
        .iter()
        .any(|w| k.contains(w))
}

/// 递归打码 JSON 值：只改字符串；敏感键名下的字符串整体替换。返回是否有改动。
fn redact_value(v: &mut serde_json::Value) -> bool {
    use serde_json::Value;
    match v {
        Value::String(s) => {
            let r = redact(s);
            let changed = r != *s;
            *s = r;
            changed
        }
        Value::Array(a) => a.iter_mut().fold(false, |acc, x| redact_value(x) | acc),
        Value::Object(m) => m.iter_mut().fold(false, |acc, (k, x)| {
            let changed = match x {
                Value::String(s) if sensitive_key(k) => {
                    let changed = s != "[REDACTED]";
                    *s = "[REDACTED]".into();
                    changed
                }
                _ => redact_value(x),
            };
            changed | acc
        }),
        _ => false,
    }
}

/// 打码原始日志的一行：能按 JSON 解析就逐个字符串值打码后重新序列化（键名、数字、布尔、null 不动，结构保持合法）；
/// 没有改动时原样返回；解析失败退回文本 `redact`。
pub fn redact_json_line(line: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line) else {
        return redact(line);
    };
    if !redact_value(&mut v) {
        return line.to_string();
    }
    serde_json::to_string(&v).unwrap_or_else(|_| redact(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_still_masks() {
        assert_eq!(redact("password: hunter2"), "password: [REDACTED]");
        assert!(redact("sk-ant-api03-abcdefghijklmnop").contains("sk-a…[REDACTED]"));
    }

    #[test]
    fn redact_masks_each_rule() {
        let cases = [
            "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----",
            "密码：abc123",
            "sk-abcdefghijklmnopqrst",
            "ghp_abcdefghijklmnopqrstuvwxyz0123",
            "xoxb-1234567890-abcdef",
            "AKIAABCDEFGHIJKLMNOP",
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz",
            "api_key=abcdefgh123",
        ];
        for text in cases {
            let out = redact(text);
            assert!(out.contains("[REDACTED]"), "{text} -> {out}");
        }
        assert_eq!(redact("api_key=abcdefgh123"), "api_key=[REDACTED]");
    }

    #[test]
    fn value_stops_at_punctuation_and_keeps_following_text() {
        assert_eq!(redact("api_key=abcdefgh123，需要轮换"), "api_key=[REDACTED]，需要轮换");
        assert_eq!(redact("密码：abc123，请改掉"), "密码：[REDACTED]，请改掉");
        assert_eq!(redact("f(token=abc12345678);"), "f(token=[REDACTED]);");
    }

    #[test]
    fn json_line_stays_valid_after_redact() {
        let out = redact(r#"{"msg":"token=abcdefgh12345678","next":1}"#);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{out}：{e}"));
        assert_eq!(v["next"], 1);
        assert!(!out.contains("abcdefgh12345678"), "{out}");
        let out = redact(r#"{"api_key":"abcdefgh12345678","next":2}"#);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{out}：{e}"));
        assert_eq!(v["api_key"], "[REDACTED]");
        assert_eq!(v["next"], 2);
    }

    fn json_redacted(line: &str) -> serde_json::Value {
        let out = redact_json_line(line);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{line} → {out}：{e}"))
    }

    #[test]
    fn json_line_keeps_structure_and_numbers() {
        use serde_json::json;
        // 键名含 token、值为对象 / 数组 / 数字：结构与数字原样保留
        let l = r#"{"token_usage":{"input":1},"next":1}"#;
        assert_eq!(json_redacted(l), json!({"token_usage":{"input":1},"next":1}));
        assert_eq!(redact_json_line(l), l, "没有改动时原样返回");
        assert_eq!(json_redacted(r#"{"tokens":[1,2],"next":1}"#), json!({"tokens":[1,2],"next":1}));
        assert_eq!(json_redacted(r#"{"inputTokens":6,"next":1}"#), json!({"inputTokens":6,"next":1}));
        // 字符串值里的密钥按文本规则打码
        assert_eq!(json_redacted(r#"{"msg":"api_key=abcdefgh123"}"#), json!({"msg":"api_key=[REDACTED]"}));
        // 敏感键名下的字符串整体替换
        assert_eq!(json_redacted(r#"{"apiKey":"sk-xxxxxxxxxxxx","n":1}"#), json!({"apiKey":"[REDACTED]","n":1}));
        // 嵌套在数组 / 对象里的也处理，键名不变
        assert_eq!(
            json_redacted(r#"{"a":[{"password":"hunter2","note":"token: sk-abcdefghijklmnopqrstuvwx"}]}"#),
            json!({"a":[{"password":"[REDACTED]","note":"token: [REDACTED]"}]})
        );
    }

    /// 第一阶段遗留：敏感键名补上认证头 / cookie / 凭据，排除 bypass、tokenizer 这类误伤。
    #[test]
    fn json_sensitive_keys_cover_auth_headers_and_skip_false_positives() {
        use serde_json::json;
        assert_eq!(
            json_redacted(r#"{"headers":{"Authorization":"Basic dXNlcjpwYXNz","Cookie":"sid=abc","X-Session-Id":"s1"},"n":1}"#),
            json!({"headers":{"Authorization":"[REDACTED]","Cookie":"[REDACTED]","X-Session-Id":"[REDACTED]"},"n":1})
        );
        assert_eq!(json_redacted(r#"{"credentials":"x","private_key":"y"}"#), json!({"credentials":"[REDACTED]","private_key":"[REDACTED]"}));
        let l = r#"{"bypass":"cache","tokenizer":"cl100k","passthrough":"on"}"#;
        assert_eq!(redact_json_line(l), l, "只是包含片段的普通键名不打码");
    }

    #[test]
    fn basic_auth_header_text_is_masked() {
        let out = redact("Authorization: Basic dXNlcm5hbWU6cGFzc3dvcmQ=");
        assert!(!out.contains("dXNlcm5hbWU6cGFzc3dvcmQ"), "{out}");
        assert_eq!(redact("basic 用法"), "basic 用法", "普通单词不受影响");
    }

    #[test]
    fn non_json_line_falls_back_to_text_redact() {
        assert_eq!(redact_json_line("api_key=abcdefgh123 后面"), "api_key=[REDACTED] 后面");
    }

    #[test]
    fn short_anthropic_style_key_still_masked() {
        let text = "key sk-ant-abc123def4567";
        assert_ne!(redact(text), text);
    }

    #[test]
    fn ordinary_text_unchanged() {
        let t = "订单 123 查询失败，原因是缺少索引";
        assert_eq!(redact(t), t);
    }
}
