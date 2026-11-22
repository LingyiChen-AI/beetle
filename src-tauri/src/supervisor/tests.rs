use super::*;
use crate::agent::{AgentEngine, claude::Claude, codex::Codex};
use crate::dispatcher::copy;
use crate::runner::fake_agent::{self, Mode, fixture_path};
use crate::store::config::{self, AppConfig, Engine, Group, Trigger};
use crate::store::runs::{RunStatus, RunStore};
use crate::test_support::{FakeLark, TestHome, test_home};
use crate::workspace::Interpreters;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

// ---------- 假传输层 ----------

enum Cmd {
    Status(Status),
    Msg(IncomingMessage),
}

struct FakeTransport {
    tx: mpsc::UnboundedSender<Cmd>,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<Cmd>>,
    /// 按时间顺序记录：`reaction+ <mid>` / `reply <mid> <text>` / `reaction- <mid> <rid>`
    log: Mutex<Vec<String>>,
    replies: AtomicUsize,
    ended: AtomicBool,
    /// 回复这段文本时 panic（模拟执行器内部异常）。
    panic_on_reply: Mutex<Option<String>>,
    /// 为 true 时所有回复都失败（模拟机器人被移出群、令牌失效）。
    fail_reply: AtomicBool,
    /// 发出的卡片：(回复的消息 id, 卡片 JSON)。
    cards: Mutex<Vec<(String, serde_json::Value)>>,
    /// 为 true 时卡片回复失败（文本回复不受影响）。
    fail_card: AtomicBool,
}

impl FakeTransport {
    fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(FakeTransport {
            tx,
            rx: tokio::sync::Mutex::new(rx),
            log: Mutex::new(Vec::new()),
            replies: AtomicUsize::new(0),
            ended: AtomicBool::new(false),
            panic_on_reply: Mutex::new(None),
            fail_reply: AtomicBool::new(false),
            cards: Mutex::new(Vec::new()),
            fail_card: AtomicBool::new(false),
        })
    }

    fn status(&self, s: Status) {
        assert!(self.tx.send(Cmd::Status(s)).is_ok(), "传输层任务已结束");
    }

    fn msg(&self, m: IncomingMessage) {
        assert!(self.tx.send(Cmd::Msg(m)).is_ok(), "传输层任务已结束");
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    /// 发到某条消息下的回复文本。
    fn replies_to(&self, mid: &str) -> Vec<String> {
        let prefix = format!("reply {mid} ");
        self.log().iter().filter_map(|l| l.strip_prefix(&prefix).map(String::from)).collect()
    }
}

impl Transport for FakeTransport {
    fn run(&self, mut stop: watch::Receiver<bool>, on_message: MessageHandler, on_status: StatusHandler) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let mut rx = self.rx.lock().await;
            loop {
                tokio::select! {
                    _ = stop.wait_for(|s| *s) => break,
                    cmd = rx.recv() => match cmd {
                        Some(Cmd::Status(Status::AppDeleted)) => {
                            on_status(Status::AppDeleted);
                            break;
                        }
                        Some(Cmd::Status(s)) => on_status(s),
                        Some(Cmd::Msg(m)) => { on_message(m); }
                        None => break,
                    },
                }
            }
            self.ended.store(true, Ordering::SeqCst);
        })
    }

    fn reply<'a>(&'a self, message_id: &'a str, text: &'a str) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            let boom = self.panic_on_reply.lock().unwrap().as_deref() == Some(text);
            if boom {
                panic!("测试注入：回复时异常");
            }
            if self.fail_reply.load(Ordering::SeqCst) {
                self.log.lock().unwrap().push(format!("reply-failed {message_id}"));
                return Err("机器人不在群里".to_string());
            }
            self.log.lock().unwrap().push(format!("reply {message_id} {text}"));
            let n = self.replies.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(Some(format!("om_reply_{n}")))
        })
    }

    fn reply_card<'a>(&'a self, message_id: &'a str, card: &'a serde_json::Value) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            if self.fail_reply.load(Ordering::SeqCst) || self.fail_card.load(Ordering::SeqCst) {
                self.log.lock().unwrap().push(format!("card-failed {message_id}"));
                return Err("卡片发送失败".to_string());
            }
            let title = card["header"]["title"]["content"].as_str().unwrap_or_default().to_string();
            self.log.lock().unwrap().push(format!("card {message_id} {title}"));
            self.cards.lock().unwrap().push((message_id.to_string(), card.clone()));
            let n = self.replies.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(Some(format!("om_reply_{n}")))
        })
    }

    fn add_working_reaction<'a>(&'a self, message_id: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            self.log.lock().unwrap().push(format!("reaction+ {message_id}"));
            Some(format!("r_{message_id}"))
        })
    }

    fn remove_working_reaction<'a>(&'a self, message_id: &'a str, reaction_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.log.lock().unwrap().push(format!("reaction- {message_id} {reaction_id}"));
        })
    }
}

// ---------- 事件记录 ----------

#[derive(Default)]
struct RecSink {
    states: Mutex<Vec<(String, BotState)>>,
    runs: Mutex<Vec<RunRecord>>,
    steps: Mutex<Vec<StepRecord>>,
}

impl EventSink for RecSink {
    fn bot_state(&self, e: BotStateEvent) {
        self.states.lock().unwrap().push((e.bot_id, e.state));
    }
    fn run_updated(&self, e: RunUpdatedEvent) {
        self.runs.lock().unwrap().push(e.run);
    }
    fn run_step(&self, e: RunStepEvent) {
        self.steps.lock().unwrap().push(e.step);
    }
}

impl RecSink {
    fn states_of(&self, bot: &str) -> Vec<BotState> {
        self.states.lock().unwrap().iter().filter(|(b, _)| b == bot).map(|(_, s)| s.clone()).collect()
    }
}

// ---------- 测试夹具 ----------

struct H {
    _home: TestHome,
    dir: tempfile::TempDir,
    sup: Supervisor,
    sink: Arc<RecSink>,
    store: Arc<RunStore>,
    runner: Arc<RunnerDeps>,
    lark: Arc<FakeLark>,
    fakes: Arc<Mutex<HashMap<String, Arc<FakeTransport>>>>,
    created: Arc<Mutex<Vec<String>>>,
    comments: crate::pr_comment::CommentCalls,
    comment_fail: Arc<std::sync::atomic::AtomicBool>,
}

impl H {
    fn fake(&self, bot: &str) -> Arc<FakeTransport> {
        self.fakes.lock().unwrap().get(bot).cloned().expect("该机器人还没有创建传输层")
    }

    fn run_for(&self, mid: &str) -> Option<RunRecord> {
        self.store
            .list_runs(None, 100)
            .unwrap()
            .into_iter()
            .find(|r| r.trigger_message_id == mid)
    }
}

fn group(chat: &str, trigger: Trigger) -> Group {
    Group {
        chat_id: chat.into(),
        name: "测试群".into(),
        slug: "test-group".into(),
        projects: vec![],
        doc_url: None,
        trigger,
        all_projects: false,
    }
}

fn bot(id: &str, groups: Vec<Group>) -> Bot {
    let mut b = Bot::new_default(id, "机器人", &format!("cli_{id}"));
    b.groups = groups;
    b.auto_onboard = false;
    b
}

fn cfg_with(bots: Vec<Bot>) -> AppConfig {
    AppConfig {
        bots,
        owner_open_id: Some("ou_owner".into()),
        owner_union_id: Some("on_owner".into()),
        default_projects: vec!["proj-a".into()], ..AppConfig::default() }
}

/// 写好配置后建 Supervisor。`program` 为 None 时本机「找不到」Agent。
fn harness_with(cfg: AppConfig, program: impl Fn(&std::path::Path) -> Option<PathBuf>, lark: FakeLark) -> H {
    let home = test_home();
    config::save(&cfg).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let program = program(dir.path());
    let store = Arc::new(RunStore::open(crate::platform::paths::config_dir().join("runs.db")).unwrap());
    let mut engines: HashMap<Engine, Arc<dyn AgentEngine>> = HashMap::new();
    engines.insert(Engine::Claude, Arc::new(Claude));
    engines.insert(Engine::Codex, Arc::new(Codex));
    let runner = Arc::new(RunnerDeps {
        store: store.clone(),
        engines,
        locate: Arc::new(move |_| program.clone()),
        interpreters: Interpreters { python: "python".into(), node: "node".into() },
    });
    let fakes: Arc<Mutex<HashMap<String, Arc<FakeTransport>>>> = Arc::default();
    let created: Arc<Mutex<Vec<String>>> = Arc::default();
    let (f, c) = (fakes.clone(), created.clone());
    let transport: TransportFactory = Arc::new(move |b: &Bot| {
        c.lock().unwrap().push(b.id.clone());
        let t = FakeTransport::new();
        f.lock().unwrap().insert(b.id.clone(), t.clone());
        Ok(t as Arc<dyn Transport>)
    });
    let sink = Arc::new(RecSink::default());
    let lark = Arc::new(lark);
    let comments: crate::pr_comment::CommentCalls = Arc::default();
    let comment_fail: Arc<std::sync::atomic::AtomicBool> = Arc::default();
    let pr_comment = crate::pr_comment::fake(comments.clone(), comment_fail.clone());
    let sup = Supervisor::new(SupervisorDeps { runner: runner.clone(), transport, lark: lark.clone(), events: sink.clone(), pr_comment });
    H { _home: home, dir, sup, sink, store, runner, lark, fakes, created, comments, comment_fail }
}

fn harness(bots: Vec<Bot>) -> H {
    harness_with(cfg_with(bots), |_| None, FakeLark::default())
}

fn msg(mid: &str, chat: &str, sender: &str) -> IncomingMessage {
    IncomingMessage {
        message_id: mid.into(),
        chat_id: chat.into(),
        sender_open_id: sender.into(),
        // 测试里 union_id 由 open_id 推出：ou_x → on_x
        sender_union_id: Some(sender.replacen("ou_", "on_", 1)),
        text: "订单 123 查不到".into(),
        is_group: true,
        mentions_bot: true,
        message_type: "text".into(),
        parent_id: None,
        root_id: None,
        create_time_ms: 1763548800123,
    }
}

/// 轮询直到条件成立（最多 15 秒）。
async fn until(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..1500 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("等待超时：{what}");
}

fn agent_unavailable_claude() -> String {
    copy::AGENT_UNAVAILABLE.replace("{engine}", "Claude Code")
}

/// 启动机器人并让假连接报告已连上。
async fn start_connected(h: &H, bot_id: &str) -> Arc<FakeTransport> {
    h.sup.start(bot_id).unwrap();
    let t = h.fake(bot_id);
    t.status(Status::Connecting);
    t.status(Status::Connected);
    until("进入运行中", || h.sup.state(bot_id) == BotState::Running).await;
    t
}

// ---------- 生命周期 ----------

#[tokio::test]
async fn start_goes_connecting_then_running_on_connected() {
    let h = harness(vec![bot("b1", vec![])]);
    assert_eq!(h.sup.state("b1"), BotState::Stopped);
    h.sup.start("b1").unwrap();
    assert_eq!(h.sup.state("b1"), BotState::Connecting, "start 返回时就应是连接中");
    let t = h.fake("b1");
    t.status(Status::Connecting);
    t.status(Status::Connected);
    until("进入运行中", || h.sup.state("b1") == BotState::Running).await;
    assert_eq!(h.sink.states_of("b1"), vec![BotState::Connecting, BotState::Running]);
    // 重复 start 不会再建一条连接
    h.sup.start("b1").unwrap();
    assert_eq!(*h.created.lock().unwrap(), vec!["b1"]);
}

#[tokio::test]
async fn reconnects_after_transport_error() {
    let h = harness(vec![bot("b1", vec![])]);
    let t = start_connected(&h, "b1").await;
    t.status(Status::Disconnected("飞书长连接已断开".into()));
    until("断线后显示连接中", || h.sup.state("b1") == BotState::Connecting).await;
    t.status(Status::Connecting);
    t.status(Status::Disconnected("获取飞书长连接地址请求失败".into()));
    t.status(Status::Connecting);
    t.status(Status::Connected);
    until("重连后回到运行中", || h.sup.state("b1") == BotState::Running).await;
    let states = h.sink.states_of("b1");
    assert!(!states.contains(&BotState::Stopped), "重连期间不能显示已停止：{states:?}");
    assert!(!states.iter().any(|s| matches!(s, BotState::Error(_))), "2 次失败不算异常：{states:?}");
    assert_eq!(states.last(), Some(&BotState::Running));
    assert!(!t.ended.load(Ordering::SeqCst));
}

#[tokio::test]
async fn five_consecutive_failures_show_error_but_keep_retrying() {
    let h = harness(vec![bot("b1", vec![])]);
    h.sup.start("b1").unwrap();
    let t = h.fake("b1");
    for i in 1..=4 {
        t.status(Status::Connecting);
        t.status(Status::Disconnected(format!("失败{i}")));
    }
    t.status(Status::Connecting);
    t.status(Status::Disconnected("获取飞书访问令牌超时".into()));
    let expected = BotState::Error("连接失败，正在重试：获取飞书访问令牌超时".into());
    until("第 5 次失败后显示异常", || h.sup.state("b1") == expected).await;
    // 之后的重试仍在继续：再次 Connecting 不把异常盖掉，连上后恢复运行中
    t.status(Status::Connecting);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.sup.state("b1"), expected);
    t.status(Status::Connected);
    until("恢复运行中", || h.sup.state("b1") == BotState::Running).await;
    assert!(!t.ended.load(Ordering::SeqCst), "异常期间不能停止重试");
}

#[tokio::test]
async fn app_deleted_sets_error_state() {
    let h = harness(vec![bot("b1", vec![])]);
    let t = start_connected(&h, "b1").await;
    t.status(Status::AppDeleted);
    let expected = BotState::Error("飞书应用已被删除".into());
    until("应用被删后显示异常", || h.sup.state("b1") == expected).await;
    until("传输层结束", || t.ended.load(Ordering::SeqCst)).await;
    // 连接已经结束，可以重新启动（会新建一条连接）
    until("可重新启动", || {
        h.sup.start("b1").unwrap();
        h.created.lock().unwrap().len() == 2
    })
    .await;
}

#[tokio::test]
async fn stop_ends_transport_and_sets_stopped() {
    let h = harness(vec![bot("b1", vec![])]);
    let t = start_connected(&h, "b1").await;
    h.sup.stop("b1");
    assert_eq!(h.sup.state("b1"), BotState::Stopped);
    until("传输层收到停止信号并结束", || t.ended.load(Ordering::SeqCst)).await;
    assert_eq!(h.sink.states_of("b1").last(), Some(&BotState::Stopped));
    // 停止之后不再接受迟到的状态
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(h.sup.state("b1"), BotState::Stopped);
}

#[tokio::test]
async fn autostart_starts_only_flagged_bots() {
    let mut b1 = bot("b1", vec![]);
    b1.start_with_client = true;
    let b2 = bot("b2", vec![]);
    let mut b3 = bot("b3", vec![]);
    b3.start_with_client = true;
    let h = harness(vec![b1, b2, b3]);
    h.sup.start_autostart_bots(&|_: &str| Ok(()));
    let mut created = h.created.lock().unwrap().clone();
    created.sort();
    assert_eq!(created, vec!["b1", "b3"]);
    assert_eq!(h.sup.state("b1"), BotState::Connecting);
    assert_eq!(h.sup.state("b2"), BotState::Stopped);
}

/// phase23-issues 第 1 条：开机自启 / 随客户端启动也先做环境检查；不通过的不建连接，状态显示原因。
#[tokio::test]
async fn autostart_skips_bots_failing_env_check() {
    let mut b1 = bot("b1", vec![]);
    b1.start_with_client = true;
    let mut b3 = bot("b3", vec![]);
    b3.start_with_client = true;
    let h = harness(vec![b1, b3]);
    let gate = |id: &str| if id == "b3" { Err("环境未就绪：Git".to_string()) } else { Ok(()) };
    h.sup.start_autostart_bots(&gate);
    assert_eq!(*h.created.lock().unwrap(), vec!["b1"]);
    assert_eq!(h.sup.state("b3"), BotState::Error("环境未就绪：Git".into()));
    assert!(!h.sup.is_alive("b3"));
    // 托盘 / 界面同一入口：环境不通过返回 Err，不建连接
    assert_eq!(h.sup.start_checked("b3", &gate).unwrap_err(), "环境未就绪：Git");
    assert_eq!(h.created.lock().unwrap().len(), 1);
}

/// 审查 B2：「异常：连接失败，正在重试」时连接任务仍在，再点启动要真正重连（不是空操作），停止也要能停掉。
#[tokio::test]
async fn start_while_retrying_after_errors_reconnects() {
    let h = harness(vec![bot("b1", vec![])]);
    h.sup.start("b1").unwrap();
    let t = h.fake("b1");
    for i in 1..=5 {
        t.status(Status::Connecting);
        t.status(Status::Disconnected(format!("失败{i}")));
    }
    until("显示异常", || matches!(h.sup.state("b1"), BotState::Error(_))).await;
    assert!(h.sup.is_alive("b1"), "异常重试中连接任务还在");
    h.sup.start("b1").unwrap();
    assert_eq!(h.created.lock().unwrap().len(), 2, "重新建了一条连接");
    assert_eq!(h.sup.state("b1"), BotState::Connecting);
    until("旧连接结束", || t.ended.load(Ordering::SeqCst)).await;
    let t2 = h.fake("b1");
    assert!(!Arc::ptr_eq(&t, &t2));
    t2.status(Status::Connected);
    until("新连接运行中", || h.sup.state("b1") == BotState::Running).await;
    // 运行中再 start 仍是空操作
    h.sup.start("b1").unwrap();
    assert_eq!(h.created.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn stop_while_retrying_after_errors_stops() {
    let h = harness(vec![bot("b1", vec![])]);
    h.sup.start("b1").unwrap();
    let t = h.fake("b1");
    for i in 1..=5 {
        t.status(Status::Connecting);
        t.status(Status::Disconnected(format!("失败{i}")));
    }
    until("显示异常", || matches!(h.sup.state("b1"), BotState::Error(_))).await;
    h.sup.stop("b1");
    assert_eq!(h.sup.state("b1"), BotState::Stopped);
    assert!(!h.sup.is_alive("b1"));
    until("连接任务结束", || t.ended.load(Ordering::SeqCst)).await;
}

#[tokio::test]
async fn start_without_secret_sets_error_and_returns_err() {
    let h = harness(vec![bot("b1", vec![])]);
    let sup = Supervisor::new(SupervisorDeps {
        runner: h.runner.clone(),
        transport: Arc::new(|_b: &Bot| Err("未找到 cli_b1 的 App Secret，请在客户端里重新填写".to_string())),
        lark: h.lark.clone(),
        pr_comment: crate::pr_comment::fake(Arc::default(), Arc::default()),
        events: h.sink.clone(),
    });
    let err = sup.start("b1").unwrap_err();
    assert!(err.contains("App Secret"), "{err}");
    assert_eq!(sup.state("b1"), BotState::Error(err));
    assert!(sup.start("nope").is_err());
}

// ---------- 消息处理 ----------

#[tokio::test]
async fn agent_unavailable_does_not_disconnect() {
    let h = harness(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_g1", "ou_user"));
    until("群里收到回复", || !t.replies_to("om_1").is_empty()).await;
    assert_eq!(t.replies_to("om_1"), vec![agent_unavailable_claude()]);
    assert_eq!(h.sup.state("b1"), BotState::Running, "Agent 不可用不能影响连接");
    assert!(!t.ended.load(Ordering::SeqCst));
    until("运行记录已更新", || h.run_for("om_1").is_some_and(|r| r.reply_message_id.is_some())).await;
    let r = h.run_for("om_1").unwrap();
    assert_eq!(r.status, RunStatus::Failed);
    assert_eq!(r.reply_message_id.as_deref(), Some("om_reply_1"));
    // 连接仍可继续收消息；10 分钟内第二次只回 FAILED
    t.msg(msg("om_2", "oc_g1", "ou_user"));
    until("第二条回复", || !t.replies_to("om_2").is_empty()).await;
    assert_eq!(t.replies_to("om_2"), vec![copy::FAILED.to_string()]);
}

#[tokio::test]
async fn working_reaction_added_then_removed() {
    let h = harness(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_g1", "ou_user"));
    until("表情被移除", || t.log().iter().any(|l| l.starts_with("reaction-"))).await;
    assert_eq!(
        t.log(),
        vec![
            "reaction+ om_1".to_string(),
            format!("reply om_1 {}", agent_unavailable_claude()),
            "reaction- om_1 r_om_1".to_string(),
        ]
    );
}

#[tokio::test]
async fn replied_run_posts_final_and_records_reply_id() {
    let cfg = cfg_with(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let h = harness_with(
        cfg,
        |dir| Some(fake_agent::make(dir, Mode::Replay { fixture: &fixture_path("claude-ok.jsonl"), code: 0 })),
        FakeLark::default(),
    );
    let t = start_connected(&h, "b1").await;
    let mut m = msg("om_1", "oc_g1", "ou_user");
    m.parent_id = Some("om_parent".into());
    t.msg(m);
    until("回复最终结果", || !t.replies_to("om_1").is_empty()).await;
    assert_eq!(t.replies_to("om_1"), vec!["OK"]);
    until("记录回复消息 id", || h.run_for("om_1").is_some_and(|r| r.reply_message_id.is_some())).await;
    let r = h.run_for("om_1").unwrap();
    assert_eq!(r.status, RunStatus::Replied);
    assert_eq!(r.reply_message_id.as_deref(), Some("om_reply_1"));
    // phase23-issues 第 13 条：按 union_id 查姓名（假 lark-cli 返回「姓名-<传入的 id>」）
    assert_eq!(r.sender_name, "姓名-on_user", "发送者姓名按 union_id 经 lark-cli 解析");
    assert_eq!(r.sender_union_id.as_deref(), Some("on_user"));
    assert_eq!(h.lark.user_lookups.lock().unwrap().clone(), vec!["on_user".to_string()]);
    let received = h.sink.steps.lock().unwrap().iter().find(|s| s.seq == 1).cloned().unwrap();
    assert_eq!(received.title, "收到 姓名-on_user 的消息");
    // 事件：步骤实时推送；运行记录至少有「排队」和最终两次推送
    let steps = h.sink.steps.lock().unwrap().clone();
    assert!(steps.iter().all(|s| s.run_id == r.id) && !steps.is_empty());
    let runs = h.sink.runs.lock().unwrap().clone();
    assert_eq!(runs.first().map(|x| x.status), Some(RunStatus::Queued));
    assert_eq!(runs.last(), Some(&r));
    assert!(fake_agent::args_file(h.dir.path()).is_file(), "假 Agent 确实被拉起");
}

/// 消息里没有 union_id 时不去 lark-cli 查（用 open_id 查一定失败），姓名直接用 open_id。
#[tokio::test]
async fn sender_without_union_id_uses_open_id_as_name() {
    let h = harness(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let t = start_connected(&h, "b1").await;
    let mut m = msg("om_1", "oc_g1", "ou_user");
    m.sender_union_id = None;
    t.msg(m);
    until("运行记录已更新", || h.run_for("om_1").is_some_and(|r| r.reply_message_id.is_some())).await;
    assert_eq!(h.run_for("om_1").unwrap().sender_name, "ou_user");
    assert!(h.lark.user_lookups.lock().unwrap().is_empty());
}

/// 审查 B10：群里回复失败时，运行记录不能还是「已回复」。
#[tokio::test]
async fn reply_failure_marks_replied_run_failed() {
    let cfg = cfg_with(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let h = harness_with(
        cfg,
        |dir| Some(fake_agent::make(dir, Mode::Replay { fixture: &fixture_path("claude-ok.jsonl"), code: 0 })),
        FakeLark::default(),
    );
    let t = start_connected(&h, "b1").await;
    t.fail_reply.store(true, Ordering::SeqCst);
    t.msg(msg("om_1", "oc_g1", "ou_user"));
    until("记录结束", || h.run_for("om_1").is_some_and(|r| r.finished_at.is_some() && r.error.is_some())).await;
    let r = h.run_for("om_1").unwrap();
    assert_eq!(r.status, RunStatus::Failed, "群里没收到回复，不能记成已回复");
    assert_eq!(r.final_reply.as_deref(), Some("OK"), "保留最终回复，方便手工转发");
    assert!(r.error.as_deref().unwrap().contains("群里回复失败"), "{:?}", r.error);
    assert_eq!(r.reply_message_id, None);
}

#[tokio::test]
async fn cancel_running_run_replies_cancelled() {
    let cfg = cfg_with(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let h = harness_with(cfg, |dir| Some(fake_agent::make(dir, Mode::Sleep)), FakeLark::default());
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_g1", "ou_user"));
    until("开始运行", || h.run_for("om_1").is_some_and(|r| r.status == RunStatus::Running)).await;
    // 等 Agent 进程真正起来（有 Launch 步骤）再取消
    until("Agent 已启动", || h.sink.steps.lock().unwrap().iter().any(|s| s.kind == crate::store::runs::StepKind::Launch)).await;
    let id = h.run_for("om_1").unwrap().id;
    assert!(h.sup.cancel_run(&id));
    until("群里收到取消文案", || !t.replies_to("om_1").is_empty()).await;
    assert_eq!(t.replies_to("om_1"), vec![copy::CANCELLED.to_string()]);
    until("记录为已取消", || h.run_for("om_1").is_some_and(|r| r.status == RunStatus::Cancelled)).await;
    assert!(!h.sup.cancel_run("no-such-run"));
}

#[tokio::test]
async fn cancel_queued_run_replies_cancelled_without_running() {
    let cfg = cfg_with(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let h = harness_with(cfg, |dir| Some(fake_agent::make(dir, Mode::Sleep)), FakeLark::default());
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_g1", "ou_user"));
    t.msg(msg("om_2", "oc_g1", "ou_user")); // 同群串行，排在后面
    until("第二条在排队", || h.run_for("om_2").is_some_and(|r| r.status == RunStatus::Queued)).await;
    until("第一条在运行", || h.run_for("om_1").is_some_and(|r| r.status == RunStatus::Running)).await;
    until("排队中就已加处理中表情", || t.log().contains(&"reaction+ om_2".to_string())).await;
    assert_eq!(h.run_for("om_2").unwrap().status, RunStatus::Queued);
    let queued = h.run_for("om_2").unwrap().id;
    assert!(h.sup.cancel_run(&queued));
    until("排队的被取消并回复", || !t.replies_to("om_2").is_empty()).await;
    assert_eq!(t.replies_to("om_2"), vec![copy::CANCELLED.to_string()]);
    until("记录为已取消", || h.run_for("om_2").is_some_and(|r| r.status == RunStatus::Cancelled)).await;
    until("取消后表情被移除", || t.log().contains(&"reaction- om_2 r_om_2".to_string())).await;
    // 收尾：取消第一条
    let first = h.run_for("om_1").unwrap().id;
    assert!(h.sup.cancel_run(&first));
    until("第一条结束", || !t.replies_to("om_1").is_empty()).await;
}

#[tokio::test]
async fn unknown_group_without_auto_onboard_replies_not_enabled() {
    let h = harness(vec![bot("b1", vec![])]);
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_new", "ou_user"));
    until("回复未接入", || !t.replies_to("om_1").is_empty()).await;
    assert_eq!(t.replies_to("om_1"), vec![copy::GROUP_NOT_ENABLED.to_string()]);
    assert!(h.run_for("om_1").is_none(), "未接入的群不产生运行记录");
}

#[tokio::test]
async fn auto_onboard_registers_group_then_runs() {
    let mut b = bot("b1", vec![]);
    b.auto_onboard = true;
    let h = harness(vec![b]);
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_a1b2c3d4e5f6a7b8", "ou_owner"));
    until("接入后运行并回复", || !t.replies_to("om_1").is_empty()).await;
    assert_eq!(t.replies_to("om_1"), vec![agent_unavailable_claude()]);
    let g = config::load().unwrap().bots[0].groups.clone();
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].slug, "beetle-e5f6a7b8");
    assert_eq!(g[0].name, "新接入群");
    assert_eq!(g[0].projects, vec!["proj-a"]);
    assert_eq!(g[0].doc_url.as_deref(), Some("https://x.feishu.cn/docx/new_doc"));
    assert_eq!(g[0].trigger, Trigger::Owner);
    // 同一个群再 @ 一次：不再新建文档
    t.msg(msg("om_2", "oc_a1b2c3d4e5f6a7b8", "ou_owner"));
    until("第二次回复", || !t.replies_to("om_2").is_empty()).await;
    assert_eq!(h.lark.docs.lock().unwrap().len(), 1);
    assert_eq!(config::load().unwrap().bots[0].groups.len(), 1);
    assert_eq!(h.run_for("om_2").unwrap().group_name, "新接入群");
}

#[tokio::test]
async fn auto_onboard_failure_replies_failed_and_records_error() {
    let mut b = bot("b1", vec![]);
    b.auto_onboard = true;
    let lark = FakeLark { chat_name: Err("lark-cli 调用失败：未登录".into()), ..FakeLark::default() };
    let h = harness_with(cfg_with(vec![b]), |_| None, lark);
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_new12345678", "ou_owner"));
    until("回复失败", || !t.replies_to("om_1").is_empty()).await;
    assert_eq!(t.replies_to("om_1"), vec![copy::FAILED.to_string()]);
    until("记录失败原因", || h.run_for("om_1").is_some_and(|r| r.status == RunStatus::Failed)).await;
    let r = h.run_for("om_1").unwrap();
    assert!(r.error.as_deref().unwrap_or("").contains("自动接入失败"), "{:?}", r.error);
    assert!(config::load().unwrap().bots[0].groups.is_empty());
    assert_eq!(h.sup.state("b1"), BotState::Running);
}

#[tokio::test]
async fn non_owner_cannot_trigger_onboard() {
    let mut b = bot("b1", vec![]);
    b.auto_onboard = true;
    let h = harness(vec![b]);
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_new12345678", "ou_stranger"));
    until("回复未接入", || !t.replies_to("om_1").is_empty()).await;
    assert_eq!(t.replies_to("om_1"), vec![copy::GROUP_NOT_ENABLED.to_string()]);
    assert!(config::load().unwrap().bots[0].groups.is_empty(), "非主人不能让机器人接入新群");
    assert!(h.lark.docs.lock().unwrap().is_empty());
    assert!(h.run_for("om_1").is_none());
}

#[tokio::test]
async fn executor_panic_marks_failed_and_removes_reaction() {
    let h = harness(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let t = start_connected(&h, "b1").await;
    *t.panic_on_reply.lock().unwrap() = Some(agent_unavailable_claude());
    t.msg(msg("om_1", "oc_g1", "ou_user"));
    until("表情被移除", || t.log().contains(&"reaction- om_1 r_om_1".to_string())).await;
    assert_eq!(t.replies_to("om_1"), vec![copy::FAILED.to_string()]);
    until("记录为失败", || h.run_for("om_1").is_some_and(|r| r.status == RunStatus::Failed && r.error.as_deref().is_some_and(|e| e.contains("内部错误")))).await;
    assert_eq!(h.sup.state("b1"), BotState::Running);
}

#[tokio::test]
async fn stop_while_running_cancels_then_restart_works() {
    let cfg = cfg_with(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let h = harness_with(cfg, |dir| Some(fake_agent::make(dir, Mode::Sleep)), FakeLark::default());
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_g1", "ou_user"));
    until("Agent 已启动", || h.sink.steps.lock().unwrap().iter().any(|s| s.kind == crate::store::runs::StepKind::Launch)).await;
    h.sup.stop("b1");
    until("群里收到取消文案", || !t.replies_to("om_1").is_empty()).await;
    assert_eq!(t.replies_to("om_1"), vec![copy::CANCELLED.to_string()]);
    until("记录为已取消", || h.run_for("om_1").is_some_and(|r| r.status == RunStatus::Cancelled)).await;
    until("表情已移除", || t.log().contains(&"reaction- om_1 r_om_1".to_string())).await;
    until("旧连接结束", || t.ended.load(Ordering::SeqCst)).await;
    // 再次启动，新连接能正常收消息
    let t2 = start_connected(&h, "b1").await;
    assert!(!Arc::ptr_eq(&t, &t2));
    t2.msg(msg("om_2", "oc_g1", "ou_user"));
    until("新消息开始运行", || h.run_for("om_2").is_some_and(|r| r.status == RunStatus::Running)).await;
    until("新消息加了表情", || t2.log().contains(&"reaction+ om_2".to_string())).await;
    let id = h.run_for("om_2").unwrap().id;
    assert!(h.sup.cancel_run(&id));
    until("收尾", || !t2.replies_to("om_2").is_empty()).await;
}

#[tokio::test]
async fn owner_p2p_uses_virtual_group_and_is_not_registered() {
    let mut b = bot("b1", vec![]);
    b.auto_onboard = true;
    let h = harness(vec![b]);
    let t = start_connected(&h, "b1").await;
    let mut m = msg("om_1", "oc_p2pchat98765432", "ou_owner");
    m.is_group = false;
    m.mentions_bot = false;
    t.msg(m);
    until("私聊回复", || !t.replies_to("om_1").is_empty()).await;
    let r = h.run_for("om_1").unwrap();
    assert_eq!(r.group_name, "私聊自测");
    assert!(config::load().unwrap().bots[0].groups.is_empty(), "私聊不登记成群");
    assert!(h.lark.docs.lock().unwrap().is_empty(), "私聊不建文档");
}

#[tokio::test]
async fn owner_only_group_and_p2p_never_trigger_without_owner() {
    let mut cfg = cfg_with(vec![bot("b1", vec![group("oc_g1", Trigger::Owner)])]);
    cfg.owner_open_id = None;
    cfg.owner_union_id = None;
    let lark = FakeLark { owner: Err("lark-cli 没有返回当前登录用户的 union_id".into()), ..FakeLark::default() };
    let h = harness_with(cfg, |_| None, lark);
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_g1", "ou_owner"));
    let mut p = msg("om_2", "oc_p2p", "ou_owner");
    p.is_group = false;
    t.msg(p);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(t.log().is_empty(), "取不到主人时不触发：{:?}", t.log());
    assert!(h.run_for("om_1").is_none() && h.run_for("om_2").is_none());
}

#[tokio::test]
async fn missing_owner_is_fetched_once_and_saved() {
    let mut cfg = cfg_with(vec![bot("b1", vec![group("oc_g1", Trigger::Owner)])]);
    cfg.owner_open_id = None;
    cfg.owner_union_id = None;
    let h = harness_with(cfg, |_| None, FakeLark::default());
    let t = start_connected(&h, "b1").await;
    until("主人身份已保存", || {
        let c = config::load().unwrap();
        c.owner_open_id.as_deref() == Some("ou_owner") && c.owner_union_id.as_deref() == Some("on_owner")
    })
    .await;
    t.msg(msg("om_1", "oc_g1", "ou_owner"));
    until("主人可以触发", || !t.replies_to("om_1").is_empty()).await;
}

/// 旧配置只存了 lark-cli 的 open_id、没有 union_id：启动时补取 union_id；补上之前主人不被认出（安全方向）。
#[tokio::test]
async fn missing_union_id_is_backfilled_on_start() {
    let mut cfg = cfg_with(vec![bot("b1", vec![group("oc_g1", Trigger::Owner)])]);
    cfg.owner_open_id = Some("ou_2256_larkcli".into());
    cfg.owner_union_id = None;
    let h = harness_with(cfg, |_| None, FakeLark::default());
    let t = start_connected(&h, "b1").await;
    until("union_id 已补上", || config::load().unwrap().owner_union_id.as_deref() == Some("on_owner")).await;
    assert_eq!(config::load().unwrap().owner_open_id.as_deref(), Some("ou_owner"));
    t.msg(msg("om_1", "oc_g1", "ou_owner"));
    until("主人可以触发", || !t.replies_to("om_1").is_empty()).await;
}

/// 机器人事件里的 open_id 与 lark-cli 取到的主人 open_id 不同，只要 union_id 相同就是主人。
#[tokio::test]
async fn owner_matched_by_union_id_across_apps() {
    let h = harness(vec![bot("b1", vec![group("oc_g1", Trigger::Owner)])]);
    let t = start_connected(&h, "b1").await;
    let mut m = msg("om_1", "oc_g1", "ou_bot_app_ns");
    m.sender_union_id = Some("on_owner".into());
    t.msg(m);
    until("主人可以触发", || !t.replies_to("om_1").is_empty()).await;
    // open_id 和主人的 lark-cli open_id 相同但 union_id 不同：不是主人
    let mut other = msg("om_2", "oc_g1", "ou_owner");
    other.sender_union_id = Some("on_someone".into());
    t.msg(other);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(t.replies_to("om_2").is_empty());
}

#[tokio::test]
async fn redelivered_message_runs_once() {
    let h = harness(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_g1", "ou_user"));
    t.msg(msg("om_1", "oc_g1", "ou_user"));
    until("回复", || !t.replies_to("om_1").is_empty()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(t.replies_to("om_1").len(), 1);
    assert_eq!(h.store.list_runs(None, 10).unwrap().len(), 1);
}

#[tokio::test]
async fn retry_run_runs_same_message_again() {
    let h = harness(vec![bot("b1", vec![group("oc_g1", Trigger::All)])]);
    let t = start_connected(&h, "b1").await;
    t.msg(msg("om_1", "oc_g1", "ou_user"));
    until("第一次回复", || !t.replies_to("om_1").is_empty()).await;
    until("第一次记录完成", || h.run_for("om_1").is_some_and(|r| r.reply_message_id.is_some())).await;
    let first = h.run_for("om_1").unwrap().id;
    h.sup.retry_run(&first).unwrap();
    until("重试后再次回复", || t.replies_to("om_1").len() == 2).await;
    let runs: Vec<_> = h.store.list_runs(None, 10).unwrap().into_iter().filter(|r| r.trigger_message_id == "om_1").collect();
    assert_eq!(runs.len(), 2, "重试生成新的运行记录");
    assert!(h.sup.retry_run("no-such-run").is_err());
    h.sup.stop("b1");
    assert!(h.sup.retry_run(&first).is_err(), "机器人停止后不能重试");
}

#[tokio::test]
async fn startup_marks_leftover_runs_failed() {
    let home = test_home();
    let store = Arc::new(RunStore::open(crate::platform::paths::config_dir().join("runs.db")).unwrap());
    let mut r = RunRecord {
        id: "left".into(),
        bot_id: "b1".into(),
        chat_id: "oc_g1".into(),
        group_name: "g".into(),
        trigger_message_id: "om_x".into(),
        sender_name: "张三".into(),
        text: "t".into(),
        engine: Engine::Claude,
        engine_version: None,
        session_id: None,
        status: RunStatus::Running,
        started_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64,
        finished_at: None,
        final_reply: None,
        error: None,
        sql_count: 0,
        reply_message_id: None,
        sender_union_id: None,
    };
    store.insert_run(&r).unwrap();
    r.id = "left-q".into();
    r.status = RunStatus::Queued;
    store.insert_run(&r).unwrap();
    let runner = Arc::new(RunnerDeps {
        store: store.clone(),
        engines: HashMap::new(),
        locate: Arc::new(|_| None),
        interpreters: Interpreters { python: "python".into(), node: "node".into() },
    });
    let _sup = Supervisor::new(SupervisorDeps {
        runner,
        transport: Arc::new(|_b: &Bot| Err("x".to_string())),
        lark: Arc::new(FakeLark::default()),
        pr_comment: crate::pr_comment::fake(Arc::default(), Arc::default()),
        events: Arc::new(RecSink::default()),
    });
    for id in ["left", "left-q"] {
        let (got, _) = store.get_run(id).unwrap().unwrap();
        assert_eq!(got.status, RunStatus::Failed);
        assert_eq!(got.error.as_deref(), Some("客户端退出，运行中断"));
    }
    drop(home);
}

#[test]
fn bot_state_serializes_with_kind_and_message() {
    assert_eq!(serde_json::to_value(BotState::Running).unwrap(), serde_json::json!({"kind": "running"}));
    assert_eq!(
        serde_json::to_value(BotState::Error("飞书应用已被删除".into())).unwrap(),
        serde_json::json!({"kind": "error", "message": "飞书应用已被删除"})
    );
}

// ---------- 事件出口不能在内部锁内被调用 ----------

/// 在 bot_state 回调里反过来读 supervisor 状态的出口（托盘刷新就是这么做的）。
struct ReentrantSink {
    sup: std::sync::OnceLock<Supervisor>,
    seen: Mutex<Vec<BotState>>,
}

impl EventSink for ReentrantSink {
    fn bot_state(&self, event: BotStateEvent) {
        let state = self.sup.get().expect("supervisor 已设置").state(&event.bot_id);
        self.seen.lock().unwrap().push(state);
    }
    fn run_updated(&self, _event: RunUpdatedEvent) {}
    fn run_step(&self, _event: RunStepEvent) {}
}

#[tokio::test]
async fn event_sink_may_read_state_without_deadlock() {
    let _home = test_home();
    config::save(&cfg_with(vec![bot("b1", vec![])])).unwrap();
    let store = Arc::new(RunStore::open(crate::platform::paths::config_dir().join("runs.db")).unwrap());
    let runner = Arc::new(RunnerDeps {
        store,
        engines: HashMap::new(),
        locate: Arc::new(|_| None),
        interpreters: Interpreters { python: "python".into(), node: "node".into() },
    });
    let transport: TransportFactory = Arc::new(|_b: &Bot| Ok(FakeTransport::new() as Arc<dyn Transport>));
    let sink = Arc::new(ReentrantSink { sup: std::sync::OnceLock::new(), seen: Mutex::new(Vec::new()) });
    let sup = Supervisor::new(SupervisorDeps {
        runner,
        transport,
        lark: Arc::new(FakeLark::default()),
        events: sink.clone(),
        pr_comment: crate::pr_comment::fake(Arc::default(), Arc::default()),
    });
    assert!(sink.sup.set(sup.clone()).is_ok());

    // 放到普通线程里跑：死锁时只会超时，不会卡住整个测试进程
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        sup.start("b1").unwrap();
        sup.stop("b1");
        let _ = done_tx.send(());
    });
    done_rx.recv_timeout(Duration::from_secs(5)).expect("start/stop 在 5 秒内没有完成：事件回调里读状态死锁了");
    assert_eq!(*sink.seen.lock().unwrap(), vec![BotState::Connecting, BotState::Stopped]);
}

// ---------- 代码审查机器人 ----------

const REVIEW_PR: &str = "https://gitee.com/example/demo-service/pulls/29";

fn review_bot(id: &str) -> Bot {
    let mut b = Bot::new_review(id, "代码审查", &format!("cli_{id}"));
    b.groups = vec![group("oc_g1", Trigger::All)];
    b.auto_onboard = false;
    b
}

fn pr_message(mid: &str, text: &str) -> IncomingMessage {
    let mut m = msg(mid, "oc_g1", "ou_user");
    m.text = text.into();
    m.mentions_bot = false;
    m
}

/// 群里贴 PR 链接（不 @）→ 审查 → 卡片回复；本轮结果按 PR 保存；10 分钟内再贴同一个 PR 只回一句。
#[tokio::test]
async fn pr_link_triggers_review_card_and_saves_state() {
    let h = harness_with(
        cfg_with(vec![review_bot("b1")]),
        |dir| Some(fake_agent::make(dir, Mode::Replay { fixture: &fixture_path("claude-review.jsonl"), code: 0 })),
        FakeLark::default(),
    );
    let t = start_connected(&h, "b1").await;
    t.msg(pr_message("om_1", &format!("帮忙看下 {REVIEW_PR}?tab=files")));
    until("发出审查卡片", || !t.cards.lock().unwrap().is_empty()).await;
    let (mid, card) = t.cards.lock().unwrap()[0].clone();
    assert_eq!(mid, "om_1");
    assert_eq!(card["header"]["template"], "blue");
    assert_eq!(card["header"]["title"]["content"], "代码审查 demo-service #29，2 项发现");
    let md = card_text(&card);
    assert!(md.contains("<font color='orange'>**[中]**</font> （上一轮遗留，未处理）判重查询走了从库 b.go:40"), "{md}");
    assert!(md.contains("作者 张三 · +120 / -9 · 6 个文件"), "{md}");
    assert!(md.contains("已同步到 PR 评论"), "{md}");
    assert!(t.replies_to("om_1").is_empty(), "卡片发成功就不再发纯文本");
    // 同步到 PR 评论：用 PR 的规范链接、默认 token 变量，正文是完整的审查结果
    let calls = h.comments.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].0.as_str(), calls[0].1.as_str()), (REVIEW_PR, "GITEE_TOKEN"));
    assert!(calls[0].2.starts_with("### 代码审查：2 项发现"), "{}", calls[0].2);
    assert!(calls[0].2.contains("判重查询走了从库 b.go:40"));

    until("记录回复消息 id", || h.run_for("om_1").is_some_and(|r| r.reply_message_id.is_some())).await;
    let r = h.run_for("om_1").unwrap();
    assert_eq!(r.status, RunStatus::Replied);
    let saved: serde_json::Value = serde_json::from_str(r.final_reply.as_deref().unwrap()).unwrap();
    assert_eq!((saved["repo"].as_str(), saved["pr"].as_u64()), (Some("demo-service"), Some(29)), "运行记录存解析后的 JSON");
    let pr = crate::dispatcher::pr_link::find_pr_links(REVIEW_PR).remove(0);
    let state = std::fs::read_to_string(review_card::state_path(&pr)).expect("本轮结果已保存");
    assert!(state.contains("判重查询走了从库"));

    // 同一 PR 再贴：不再审，回「刚审过」（等第一次运行彻底收尾：表情移除在移出活动列表之后）
    until("表情被移除", || t.log().iter().any(|l| l.starts_with("reaction- om_1"))).await;
    t.msg(pr_message("om_2", REVIEW_PR));
    until("回复刚审过", || !t.replies_to("om_2").is_empty()).await;
    assert_eq!(t.replies_to("om_2"), vec![copy::pr_recent_reply(REVIEW_PR, false)]);
    assert!(h.run_for("om_2").is_none(), "不重复审查");
    // 普通聊天（没 @、没链接）不触发
    t.msg(pr_message("om_3", "今天发版吗"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.run_for("om_3").is_none() && t.replies_to("om_3").is_empty());
}

/// 卡片里所有文字块（markdown、底注、按钮链接）拼在一起，方便断言。
fn card_text(card: &serde_json::Value) -> String {
    card["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            e["content"]
                .as_str()
                .map(String::from)
                .or_else(|| e["elements"][0]["content"].as_str().map(String::from))
                .or_else(|| e["actions"][0]["url"].as_str().map(String::from))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// PR 评论发不出去（例如没配 token）：群里照常回卡片，底注写原因，运行记录注明。
#[tokio::test]
async fn review_comment_failure_still_replies_card() {
    let h = harness_with(
        cfg_with(vec![review_bot("b1")]),
        |dir| Some(fake_agent::make(dir, Mode::Replay { fixture: &fixture_path("claude-review.jsonl"), code: 0 })),
        FakeLark::default(),
    );
    h.comment_fail.store(true, Ordering::SeqCst);
    let t = start_connected(&h, "b1").await;
    t.msg(pr_message("om_1", REVIEW_PR));
    until("发出审查卡片", || !t.cards.lock().unwrap().is_empty()).await;
    let card = t.cards.lock().unwrap()[0].1.clone();
    let md = card_text(&card);
    assert!(md.contains("PR 评论未同步：未配置 GITEE_TOKEN"), "{md}");
    assert!(md.contains(REVIEW_PR), "仍有查看 PR 按钮：{md}");
    until("记录回复消息 id", || h.run_for("om_1").is_some_and(|r| r.reply_message_id.is_some())).await;
    let r = h.run_for("om_1").unwrap();
    assert_eq!(r.status, RunStatus::Replied);
    assert!(r.error.as_deref().unwrap_or_default().contains("PR 评论未同步"), "{:?}", r.error);
}

/// 最终回复不是约定的 JSON：退回发纯文本（打码后的原文），运行记录里注明。
#[tokio::test]
async fn review_output_not_json_falls_back_to_text() {
    let h = harness_with(
        cfg_with(vec![review_bot("b1")]),
        |dir| Some(fake_agent::make(dir, Mode::Replay { fixture: &fixture_path("claude-ok.jsonl"), code: 0 })),
        FakeLark::default(),
    );
    let t = start_connected(&h, "b1").await;
    t.msg(pr_message("om_1", REVIEW_PR));
    until("回复纯文本", || !t.replies_to("om_1").is_empty()).await;
    assert_eq!(t.replies_to("om_1"), vec!["OK"]);
    assert!(t.cards.lock().unwrap().is_empty());
    assert!(h.comments.lock().unwrap().is_empty(), "解析不了的结果不评论到 PR");
    until("记录结束", || h.run_for("om_1").is_some_and(|r| r.finished_at.is_some() && r.reply_message_id.is_some())).await;
    let r = h.run_for("om_1").unwrap();
    assert_eq!(r.status, RunStatus::Replied);
    assert!(r.error.as_deref().unwrap_or_default().contains("不是约定的 JSON"), "{:?}", r.error);
}

/// 卡片发不出去：改发同内容的纯文本，运行记录注明。
#[tokio::test]
async fn review_card_failure_falls_back_to_plain_text() {
    let h = harness_with(
        cfg_with(vec![review_bot("b1")]),
        |dir| Some(fake_agent::make(dir, Mode::Replay { fixture: &fixture_path("claude-review.jsonl"), code: 0 })),
        FakeLark::default(),
    );
    let t = start_connected(&h, "b1").await;
    t.fail_card.store(true, Ordering::SeqCst);
    t.msg(pr_message("om_1", REVIEW_PR));
    until("改发纯文本", || !t.replies_to("om_1").is_empty()).await;
    let text = t.replies_to("om_1").remove(0);
    assert!(text.starts_with("代码审查 demo-service #29，2 项发现"), "{text}");
    assert!(!text.contains("**"));
    until("记录结束", || h.run_for("om_1").is_some_and(|r| r.reply_message_id.is_some())).await;
    assert!(h.run_for("om_1").unwrap().error.unwrap_or_default().contains("卡片发送失败"));
}

/// 审查没成功（本机找不到 Agent）：这个 PR 马上放开，重贴会重新审。
#[tokio::test]
async fn failed_review_releases_pr_for_immediate_retry() {
    let h = harness(vec![review_bot("b1")]);
    let t = start_connected(&h, "b1").await;
    t.msg(pr_message("om_1", REVIEW_PR));
    until("回复失败文案", || !t.replies_to("om_1").is_empty()).await;
    until("记录结束", || h.run_for("om_1").is_some_and(|r| r.finished_at.is_some())).await;
    // 等提交任务收尾（放开 PR 在移除表情之后）
    until("表情被移除", || t.log().iter().any(|l| l.starts_with("reaction- om_1"))).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    t.msg(pr_message("om_2", REVIEW_PR));
    until("重贴后重新运行", || h.run_for("om_2").is_some()).await;
}
