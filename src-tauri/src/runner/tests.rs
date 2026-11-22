use super::fake_agent::{self, Mode, fixture_path};
use super::*;
use crate::agent::{claude::Claude, codex::Codex};
use crate::store::config::{Bot, Group, Trigger};
use crate::store::runs::{RunStatus, StepKind};
use crate::test_support::{TestHome, test_home};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CLAUDE_SESSION: &str = "5acf6c3a-0918-4494-aa50-2422a8eefbdd";

struct Env {
    _home: TestHome,
    dir: tempfile::TempDir,
    store: Arc<RunStore>,
}

fn env() -> Env {
    let home = test_home();
    let store = Arc::new(RunStore::open(paths::config_dir().join("runs.db")).unwrap());
    Env { _home: home, dir: tempfile::tempdir().unwrap(), store }
}

fn deps(env: &Env, program: Option<PathBuf>) -> RunnerDeps {
    let mut engines: HashMap<Engine, Arc<dyn AgentEngine>> = HashMap::new();
    engines.insert(Engine::Claude, Arc::new(Claude));
    engines.insert(Engine::Codex, Arc::new(Codex));
    RunnerDeps {
        store: env.store.clone(),
        engines,
        locate: Arc::new(move |_| program.clone()),
        interpreters: Interpreters { python: "python".into(), node: "node".into() },
    }
}

fn input(engine: Engine) -> RunInput {
    let mut bot = Bot::new_default("b1", "机器人", "cli_x");
    bot.engine = engine;
    let group = Group {
        chat_id: "oc_abc".into(),
        name: "测试群".into(),
        slug: "test-group".into(),
        projects: vec![],
        doc_url: None,
        trigger: Trigger::All,
        all_projects: false,
    };
    let ctx = PromptContext {
        chat_id: "oc_abc".into(),
        group_name: "测试群".into(),
        trigger_message_id: "om_trigger_42".into(),
        trigger_text: "订单 123 查不到".into(),
        sender_name: "张三".into(),
        sender_open_id: "ou_sender_7".into(),
        is_owner: false,
        parent_message_id: None,
        parent_text: None,
        root_id: None,
        time: "2026-11-19 10:00".into(),
        pr_url: None,
    };
    RunInput {
        run_id: uuid::Uuid::new_v4().to_string(),
        bot,
        group,
        cfg: AppConfig::default(),
        ctx,
        resume: None,
        sender_union_id: Some("on_sender_7".into()),
    }
}

async fn run(
    input: RunInput,
    deps: &RunnerDeps,
    cancel: CancellationToken,
) -> (RunRecord, Outcome, Vec<StepRecord>) {
    let seen = Mutex::new(Vec::new());
    let (rec, out) = run_once(input, deps, cancel, |s: &StepRecord| seen.lock().unwrap().push(s.clone())).await;
    (rec, out, seen.into_inner().unwrap())
}

fn replay(env: &Env, name: &str, code: i32) -> PathBuf {
    fake_agent::make(env.dir.path(), Mode::Replay { fixture: &fixture_path(name), code })
}

fn stored(env: &Env, id: &str) -> (RunRecord, Vec<StepRecord>) {
    env.store.get_run(id).unwrap().expect("运行记录应已落库")
}

#[tokio::test]
async fn replied_run_records_steps_and_final() {
    let env = env();
    let deps = deps(&env, Some(replay(&env, "claude-ok.jsonl", 0)));
    let (rec, out, seen) = run(input(Engine::Claude), &deps, CancellationToken::new()).await;
    assert_eq!(out, Outcome::Replied("OK".into()));
    assert_eq!(rec.status, RunStatus::Replied);
    assert_eq!(rec.final_reply.as_deref(), Some("OK"));
    assert!(rec.finished_at.is_some());
    assert_eq!(rec.engine_version.as_deref(), Some("fake-agent 1.0"));
    let (db, steps) = stored(&env, &rec.id);
    assert_eq!(db, rec);
    let kinds: Vec<StepKind> = steps.iter().map(|s| s.kind).collect();
    for k in [StepKind::Received, StepKind::Prepare, StepKind::Launch, StepKind::Tool, StepKind::Reply] {
        assert!(kinds.contains(&k), "缺少步骤 {k:?}：{kinds:?}");
    }
    assert_eq!(steps, seen, "on_step 收到的步骤应与落库的一致");
    assert_eq!(steps.iter().map(|s| s.seq).collect::<Vec<_>>(), (1..=steps.len() as u32).collect::<Vec<_>>());
    // 运行时配置带的是本次的触发消息
    let rt: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(paths::workspaces_dir().join("b1/test-group/beetle.runtime.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(rt["trigger_message_id"], "om_trigger_42");
    assert_eq!(rt["trigger_sender_open_id"], "ou_sender_7");
    assert_eq!(rt["trigger_is_owner"], false);
    // 额外可写目录在拉起前已创建
    assert!(paths::code_cache_dir().is_dir());
    assert!(!paths::data_dir().join("sql-requests").exists(), "不再创建 SQL 请求目录");
}

#[tokio::test]
async fn missing_result_reports_failed_without_partial_text() {
    let env = env();
    let deps = deps(&env, Some(replay(&env, "claude-no-result.jsonl", 0)));
    let (rec, out, _) = run(input(Engine::Claude), &deps, CancellationToken::new()).await;
    let Outcome::Failed(reason) = &out else { panic!("应为 Failed：{out:?}") };
    assert_eq!(rec.status, RunStatus::Failed);
    assert_eq!(rec.final_reply, None);
    // 夹具里助手说过的话（含残缺的工具调用文本和 "OK"）一个字都不能出现在返回值里
    for t in ["OK", "别ash", "invoke"] {
        assert!(!reason.contains(t), "失败原因里混进了助手文本 {t:?}：{reason}");
    }
    assert!(rec.error.is_some());
}

#[tokio::test]
async fn nonzero_exit_fails_even_with_result() {
    let env = env();
    let deps = deps(&env, Some(replay(&env, "claude-ok.jsonl", 3)));
    let (rec, out, _) = run(input(Engine::Claude), &deps, CancellationToken::new()).await;
    assert!(matches!(out, Outcome::Failed(_)), "{out:?}");
    assert_eq!(rec.final_reply, None);
}

#[tokio::test]
async fn failed_reason_from_stderr_is_redacted() {
    let env = env();
    let mode = Mode::StderrExit { text: "boom token=abcdefgh12345678 end", code: 2 };
    let deps = deps(&env, Some(fake_agent::make(env.dir.path(), mode)));
    let (rec, out, _) = run(input(Engine::Claude), &deps, CancellationToken::new()).await;
    let Outcome::Failed(reason) = &out else { panic!("应为 Failed：{out:?}") };
    assert!(reason.contains("boom") && reason.contains("[REDACTED]"), "{reason}");
    assert!(!reason.contains("abcdefgh12345678"), "{reason}");
    assert_eq!(rec.error.as_deref(), Some(reason.as_str()), "返回值与 rec.error 应是同一份打码内容");
    assert_eq!(stored(&env, &rec.id).0.error.as_deref(), Some(reason.as_str()));
    let err_log = std::fs::read_to_string(raw_log_path(&rec.id).with_extension("stderr.txt")).unwrap();
    assert!(!err_log.contains("abcdefgh12345678"), "{err_log}");
}

#[tokio::test]
async fn error_event_reports_failed() {
    let env = env();
    let deps = deps(&env, Some(replay(&env, "claude-error.jsonl", 0)));
    let (rec, out, _) = run(input(Engine::Claude), &deps, CancellationToken::new()).await;
    assert!(matches!(out, Outcome::Failed(_)), "{out:?}");
    assert_eq!(rec.status, RunStatus::Failed);
}

#[tokio::test]
async fn timeout_kills_and_reports_timeout() {
    let env = env();
    let deps = deps(&env, Some(fake_agent::make(env.dir.path(), Mode::Sleep)));
    set_test_timeout_secs(Some(1));
    let start = Instant::now();
    let (rec, out, _) = run(input(Engine::Claude), &deps, CancellationToken::new()).await;
    set_test_timeout_secs(None);
    assert_eq!(out, Outcome::Timeout);
    assert_eq!(rec.status, RunStatus::Timeout);
    assert_eq!(rec.final_reply, None);
    assert!(start.elapsed() < Duration::from_secs(20), "超时后没有及时结束：{:?}", start.elapsed());
    assert_eq!(stored(&env, &rec.id).0.status, RunStatus::Timeout);
}

#[tokio::test]
async fn cancel_kills_and_reports_cancelled() {
    let env = env();
    let deps = deps(&env, Some(fake_agent::make(env.dir.path(), Mode::Sleep)));
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        c.cancel();
    });
    let start = Instant::now();
    let (rec, out, _) = run(input(Engine::Claude), &deps, cancel).await;
    assert_eq!(out, Outcome::Cancelled);
    assert_eq!(rec.status, RunStatus::Cancelled);
    assert!(start.elapsed() < Duration::from_secs(20), "取消后没有及时结束：{:?}", start.elapsed());
    assert_eq!(rec.session_id.as_deref(), Some("s-sleep"), "被取消前收到的会话 id 也要保存");
}

#[tokio::test]
async fn missing_program_reports_agent_unavailable() {
    let env = env();
    let deps = deps(&env, None);
    let (rec, out, _) = run(input(Engine::Claude), &deps, CancellationToken::new()).await;
    assert_eq!(out, Outcome::AgentUnavailable);
    assert_eq!(rec.status, RunStatus::Failed);
    assert!(rec.error.as_deref().unwrap_or("").contains("claude"), "{:?}", rec.error);
    assert!(stored(&env, &rec.id).0.finished_at.is_some());
}

#[tokio::test]
async fn record_id_is_input_run_id() {
    let env = env();
    let deps = deps(&env, None);
    let mut inp = input(Engine::Claude);
    inp.run_id = "run-fixed-1".into();
    let (rec, _, steps) = run(inp, &deps, CancellationToken::new()).await;
    assert_eq!(rec.id, "run-fixed-1");
    assert!(steps.iter().all(|s| s.run_id == "run-fixed-1"));
    assert!(env.store.get_run("run-fixed-1").unwrap().is_some());
}

#[tokio::test]
async fn queued_record_is_taken_over_by_run() {
    let env = env();
    let deps = deps(&env, None);
    let mut inp = input(Engine::Claude);
    inp.run_id = "run-queued-1".into();
    let queued = RunRecord {
        id: "run-queued-1".into(),
        bot_id: "b1".into(),
        chat_id: "oc_abc".into(),
        group_name: "测试群".into(),
        trigger_message_id: "om_trigger_42".into(),
        sender_name: "ou_sender_7".into(),
        text: "订单 123 查不到".into(),
        engine: Engine::Claude,
        engine_version: None,
        session_id: None,
        status: RunStatus::Queued,
        started_at: 1,
        finished_at: None,
        final_reply: None,
        error: None,
        sql_count: 0,
        reply_message_id: None,
        sender_union_id: Some("on_sender_7".into()),
    };
    env.store.insert_run(&queued).unwrap();
    let (rec, out, _) = run(inp, &deps, CancellationToken::new()).await;
    assert_eq!(out, Outcome::AgentUnavailable);
    let (db, steps) = stored(&env, "run-queued-1");
    assert_eq!(db, rec);
    assert_eq!(db.status, RunStatus::Failed);
    assert_eq!(db.sender_name, "张三", "开始运行后用解析出的发送者姓名覆盖排队时的占位");
    assert_eq!(db.sender_union_id.as_deref(), Some("on_sender_7"), "接管时保留触发人 union_id（续聊校验用）");
    assert!(!steps.is_empty());
}

fn fixture_with_secret_result(dir: &Path) -> PathBuf {
    let text = fake_agent_fixture_text("claude-ok.jsonl");
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let last = lines.pop().unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&last).unwrap();
    v["result"] = "结论：配置里写的是 api_key=abcdefgh123，需要轮换".into();
    lines.push(r#"{"type":"note","text":"token: sk-abcdefghijklmnopqrstuvwx"}"#.into());
    lines.push(v.to_string());
    let p = dir.join("secret.jsonl");
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    p
}

fn fake_agent_fixture_text(name: &str) -> String {
    std::fs::read_to_string(fixture_path(name)).unwrap()
}

#[tokio::test]
async fn final_reply_is_redacted() {
    let env = env();
    let fx = fixture_with_secret_result(env.dir.path());
    let deps = deps(&env, Some(fake_agent::make(env.dir.path(), Mode::Replay { fixture: &fx, code: 0 })));
    let (rec, out, _) = run(input(Engine::Claude), &deps, CancellationToken::new()).await;
    let Outcome::Replied(text) = &out else { panic!("{out:?}") };
    assert!(text.contains("[REDACTED]"), "{text}");
    assert!(!text.contains("abcdefgh123"), "{text}");
    assert_eq!(text, "结论：配置里写的是 api_key=[REDACTED]，需要轮换", "密钥后面的正文不能被吞掉");
    assert_eq!(rec.final_reply.as_deref(), Some(text.as_str()));
}

#[tokio::test]
async fn raw_log_written_and_redacted() {
    let env = env();
    let fx = fixture_with_secret_result(env.dir.path());
    let deps = deps(&env, Some(fake_agent::make(env.dir.path(), Mode::Replay { fixture: &fx, code: 0 })));
    let (rec, _, _) = run(input(Engine::Claude), &deps, CancellationToken::new()).await;
    let fixture_lines = std::fs::read_to_string(&fx).unwrap().lines().count();
    let log = std::fs::read_to_string(raw_log_path(&rec.id)).unwrap();
    assert_eq!(log.lines().count(), fixture_lines);
    let secret = log.lines().find(|l| l.contains("\"note\"")).unwrap();
    assert!(secret.contains("[REDACTED]") && !secret.contains("sk-abcdefghijklmnopqrstuvwx"), "{secret}");
    assert!(!log.contains("abcdefgh123"));
    for l in log.lines() {
        serde_json::from_str::<serde_json::Value>(l).unwrap_or_else(|e| panic!("打码后不是合法 JSON：{e}\n{l}"));
    }
    assert!(raw_log_path(&rec.id).with_extension("stderr.txt").is_file());
}

#[tokio::test]
async fn raw_log_keeps_codex_usage_object_intact() {
    let env = env();
    let mut lines: Vec<String> = fake_agent_fixture_text("codex-ok.jsonl").lines().map(String::from).collect();
    lines.push(r#"{"type":"x","total_token_usage":{"input_tokens":5},"tokens":[1,2],"apiKey":"sk-xxxxxxxxxxxx"}"#.into());
    let fx = env.dir.path().join("codex-usage.jsonl");
    std::fs::write(&fx, lines.join("\n") + "\n").unwrap();
    let deps = deps(&env, Some(fake_agent::make(env.dir.path(), Mode::Replay { fixture: &fx, code: 0 })));
    let (rec, _, _) = run(input(Engine::Codex), &deps, CancellationToken::new()).await;
    let log = std::fs::read_to_string(raw_log_path(&rec.id)).unwrap();
    let got: Vec<serde_json::Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(got.len(), lines.len());
    // 没有密钥的行与原行语义一致（用量数字保留）
    for (g, l) in got.iter().zip(&lines[..lines.len() - 1]) {
        assert_eq!(g, &serde_json::from_str::<serde_json::Value>(l).unwrap());
    }
    assert_eq!(got[5]["usage"]["input_tokens"], 64160);
    let last = got.last().unwrap();
    assert_eq!(last["total_token_usage"]["input_tokens"], 5);
    assert_eq!(last["tokens"], serde_json::json!([1, 2]));
    assert_eq!(last["apiKey"], "[REDACTED]");
}

#[tokio::test]
async fn session_id_saved_for_resume() {
    let env = env();
    let deps = deps(&env, Some(replay(&env, "claude-ok.jsonl", 0)));
    let mut inp = input(Engine::Claude);
    inp.resume = Some("prev-session-1".into());
    let (rec, _, _) = run(inp, &deps, CancellationToken::new()).await;
    assert_eq!(rec.session_id.as_deref(), Some(CLAUDE_SESSION));
    assert_eq!(stored(&env, &rec.id).0.session_id.as_deref(), Some(CLAUDE_SESSION));
    let args = std::fs::read_to_string(fake_agent::args_file(env.dir.path())).unwrap();
    assert!(args.contains("--resume prev-session-1"), "{args}");
}

fn codex_last_file() -> PathBuf {
    paths::workspaces_dir().join("b1").join("test-group").join(".beetle-last.txt")
}

#[tokio::test]
async fn codex_final_comes_from_last_message_file() {
    let env = env();
    let last = codex_last_file();
    let fx = fixture_path("codex-ok.jsonl");
    let mode = Mode::WriteThenReplay { file: &last, text: "codex answer api_key=abcdefgh123", fixture: &fx, code: 0 };
    let deps = deps(&env, Some(fake_agent::make(env.dir.path(), mode)));
    let (rec, out, _) = run(input(Engine::Codex), &deps, CancellationToken::new()).await;
    assert_eq!(out, Outcome::Replied("codex answer api_key=[REDACTED]".into()));
    assert_eq!(rec.session_id.as_deref(), Some("01a1f239-9494-7390-aab9-3564643216ff"));
}

#[tokio::test]
async fn codex_nonzero_exit_ignores_last_message_file() {
    let env = env();
    let last = codex_last_file();
    let fx = fixture_path("codex-ok.jsonl");
    let mode = Mode::WriteThenReplay { file: &last, text: "half answer", fixture: &fx, code: 1 };
    let deps = deps(&env, Some(fake_agent::make(env.dir.path(), mode)));
    let (rec, out, _) = run(input(Engine::Codex), &deps, CancellationToken::new()).await;
    assert!(matches!(&out, Outcome::Failed(r) if !r.contains("half answer")), "{out:?}");
    assert_eq!(rec.final_reply, None);
}

#[tokio::test]
async fn codex_stale_last_message_is_not_reused() {
    let env = env();
    let last = codex_last_file();
    std::fs::create_dir_all(last.parent().unwrap()).unwrap();
    std::fs::write(&last, "上一次的旧答案").unwrap();
    let deps = deps(&env, Some(replay(&env, "codex-ok.jsonl", 0)));
    let (_, out, _) = run(input(Engine::Codex), &deps, CancellationToken::new()).await;
    assert!(matches!(out, Outcome::Failed(_)), "{out:?}");
}

#[test]
fn purge_raw_logs_removes_old_files_only() {
    let _home = test_home();
    let dir = paths::data_dir().join("runs");
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("old.jsonl");
    let old_err = dir.join("old.stderr.txt");
    let new = dir.join("new.jsonl");
    for p in [&old, &old_err, &new] {
        std::fs::write(p, "x").unwrap();
    }
    let ago = std::time::SystemTime::now() - Duration::from_secs(40 * 86_400);
    for p in [&old, &old_err] {
        std::fs::File::options().write(true).open(p).unwrap().set_modified(ago).unwrap();
    }
    assert_eq!(purge_raw_logs(30), 2);
    assert!(!old.exists() && !old_err.exists());
    assert!(new.exists());
}

/// 编译期检查：run_once 的 future 能被 tokio::spawn 到多线程运行时（Task 10 调度器需要）。
#[allow(dead_code)]
fn run_once_future_is_send(input: RunInput, deps: &RunnerDeps) {
    fn is_send<T: Send>(_: &T) {}
    let f = run_once(input, deps, CancellationToken::new(), |_s: &StepRecord| {});
    is_send(&f);
}
