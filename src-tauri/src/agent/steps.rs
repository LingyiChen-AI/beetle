//! Agent 命令行 -> 运行步骤的分类（只用于客户端「运行日志」展示，不外发群里）。

use crate::store::runs::StepKind;

/// 按字符数截断，超长时末尾加「…」。
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 简化的 shell 分词：支持单引号、双引号（双引号内 `\"` 还原为 `"`，其余反斜杠原样保留，
/// 这样 Windows 路径不会被吃掉）。引号不闭合返回 None。
fn tokenize(cmd: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = cmd.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => {
                if in_token {
                    out.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            '"' => {
                in_token = true;
                i += 1;
                loop {
                    let ch = *chars.get(i)?;
                    if ch == '"' {
                        break;
                    }
                    if ch == '\\' && chars.get(i + 1) == Some(&'"') {
                        cur.push('"');
                        i += 2;
                        continue;
                    }
                    cur.push(ch);
                    i += 1;
                }
            }
            '\'' => {
                in_token = true;
                i += 1;
                loop {
                    let ch = *chars.get(i)?;
                    if ch == '\'' {
                        break;
                    }
                    cur.push(ch);
                    i += 1;
                }
            }
            _ => {
                in_token = true;
                cur.push(c);
            }
        }
        i += 1;
    }
    if in_token {
        out.push(cur);
    }
    Some(out)
}

/// 直连查库命令的开关：`<beetle> --db-query <别名>`，查询 JSON 从 stdin 传（here-string / heredoc）。
const DB_QUERY_FLAG: &str = "--db-query";

/// 从命令里取 `--db-query <别名>`（也认 `--db-query=<别名>`）后面的别名，去掉引号。
fn db_query_alias(cmd: &str) -> Option<String> {
    let pos = cmd.find(DB_QUERY_FLAG)?;
    let rest = &cmd[pos + DB_QUERY_FLAG.len()..];
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    let word: String = rest
        .trim_start()
        .trim_start_matches(['"', '\'', '`'])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    (!word.is_empty()).then_some(word)
}

/// 从命令里带的查询 JSON 取出要显示的内容：SQL 取 `sql`，Mongo 取 `collection`。取不到返回 None。
fn db_query_text(cmd: &str) -> Option<String> {
    let (start, end) = (cmd.find('{')?, cmd.rfind('}')?);
    if end <= start {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&cmd[start..=end]).ok()?;
    if let Some(sql) = v.get("sql").and_then(|s| s.as_str()) {
        return Some(sql.to_string());
    }
    v.get("collection").and_then(|c| c.as_str()).map(|c| format!("集合 {c}"))
}

/// 把 shell 命令归类成步骤。外层有 `powershell -Command "..."` / `bash -c "..."` 包装时先剥掉再分类。
pub fn classify_command(cmd: &str) -> (StepKind, String) {
    let tokens = tokenize(cmd);
    if let Some(tokens) = &tokens {
        if let Some(i) = tokens.iter().position(|t| {
            let t = t.to_ascii_lowercase();
            t == "-command" || t == "-c" || t == "/c"
        }) {
            let first = tokens
                .first()
                .map(|t| t.to_ascii_lowercase())
                .unwrap_or_default();
            let wrapper = ["powershell", "pwsh", "bash", "sh", "cmd", "zsh"]
                .iter()
                .any(|w| first.contains(w));
            if wrapper && i + 1 < tokens.len() {
                return classify_command(&tokens[i + 1..].join(" "));
            }
        }
    }

    if cmd.contains("chat_context.py") || cmd.contains("p2p_context.py") {
        (StepKind::ReadChat, "读取群聊上下文".to_string())
    } else if cmd.contains("sync_code.py") {
        (StepKind::SyncCode, "同步代码".to_string())
    } else if cmd.contains(DB_QUERY_FLAG) {
        let alias = db_query_alias(cmd).unwrap_or_else(|| "未知库".to_string());
        match db_query_text(cmd) {
            Some(q) => {
                let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
                (StepKind::Sql, format!("查库 {alias}：{}", truncate_chars(&q, 120)))
            }
            None => (StepKind::Sql, format!("查库 {alias}")),
        }
    } else if cmd.contains("append_bug_doc.py") {
        (StepKind::WriteDoc, "写入缺陷文档".to_string())
    } else if cmd.contains("create_issue.py") {
        (StepKind::WriteDoc, "建 issue".to_string())
    } else {
        (StepKind::Tool, truncate_chars(cmd.trim(), 120))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_db_query_extracts_alias_and_sql() {
        let cmd = "@'\n{\"sql\": \"SELECT id FROM t WHERE id=1\", \"cost\": \"主键\"}\n'@ | & \"C:/Program Files/Beetle/beetle.exe\" --db-query demo_config";
        let (k, t) = classify_command(cmd);
        assert_eq!(k, StepKind::Sql);
        assert_eq!(t, "查库 demo_config：SELECT id FROM t WHERE id=1");
        assert!(!t.contains("主键"), "cost 不显示：{t}");
    }

    #[test]
    fn classify_db_query_bash_heredoc_and_mongo() {
        let bash = "\"C:/b/beetle.exe\" --db-query demo-mongo <<'EOF'\n{\"collection\": \"orders\", \"filter\": {\"_id\": 1}, \"cost\": \"_id\"}\nEOF";
        assert_eq!(classify_command(bash), (StepKind::Sql, "查库 demo-mongo：集合 orders".to_string()));
        // 等号写法、没有可解析的 JSON：只显示别名
        assert_eq!(classify_command("beetle --db-query=demo_a < q.json"), (StepKind::Sql, "查库 demo_a".to_string()));
        assert_eq!(classify_command("beetle --db-query").1, "查库 未知库");
    }

    #[test]
    fn classify_db_query_truncates_sql_to_120_chars() {
        let long = format!("SELECT {} FROM t", "a,".repeat(200));
        let (_, t) = classify_command(&format!(r#"beetle --db-query d <<'EOF'
{{"sql": "{long}", "cost": "x"}}
EOF"#));
        // 「查库 d：」之后最多 120 字 + 省略号
        assert!(t.chars().count() <= "查库 d：".chars().count() + 121, "{t}");
        assert!(t.ends_with('…'));
    }

    #[test]
    fn classify_unwraps_powershell_db_query() {
        let cmd = r#""C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe" -Command "'{\"sql\":\"SELECT 1\",\"cost\":\"常量\"}' | C:/b/beetle.exe --db-query demo_test""#;
        let (k, t) = classify_command(cmd);
        assert_eq!(k, StepKind::Sql, "{t}");
        assert!(t.contains("demo_test"), "{t}");
    }

    #[test]
    fn classify_script_kinds() {
        let py = "python C:/ws/.claude/skills/chat-context/scripts/chat_context.py --limit 50";
        assert_eq!(classify_command(py).0, StepKind::ReadChat);
        assert_eq!(
            classify_command("python .../chat-context/scripts/p2p_context.py").0,
            StepKind::ReadChat
        );
        assert_eq!(
            classify_command("python C:/x/code-sync/scripts/sync_code.py --repo a").0,
            StepKind::SyncCode
        );
        assert_eq!(
            classify_command("python C:/x/feishu-doc-record/scripts/append_bug_doc.py --title t").0,
            StepKind::WriteDoc
        );
        assert_eq!(
            classify_command("python C:/x/issue-record/scripts/create_issue.py --project demo --file reports/a.md"),
            (StepKind::WriteDoc, "建 issue".to_string())
        );
        let (k, t) = classify_command("git log --oneline -n 5");
        assert_eq!(k, StepKind::Tool);
        assert_eq!(t, "git log --oneline -n 5");
    }

    #[test]
    fn classify_unbalanced_quotes_still_classifies() {
        let (k, t) = classify_command(r#"beetle --db-query d "SELECT"#);
        assert_eq!((k, t.as_str()), (StepKind::Sql, "查库 d"));
    }
}
