//! 机器人生命周期：每个机器人一条飞书长连接 + 一个运行队列，收到的消息经
//! `dispatcher::decide` → 队列 → `runner::run_once`，结果回复到触发消息下。
//!
//! 稳定性要点（旧 bridge 的核心故障）：
//! - 启动时只取 App Secret 建连接，**不做 Agent 预检**；Agent 不可用只让那一次运行失败并在群里提示，连接不受影响。
//! - 断线重连由传输层负责（1s 起翻倍、上限 60s）；这里只做状态显示：断线期间显示「连接中」，
//!   连续 5 次失败才显示「异常：连接失败，正在重试：<原因>」，但不停止重试；连上后回到「运行中」。
//! - 飞书应用被删除 → 「异常：飞书应用已被删除」，连接结束。
//! - 每个机器人一个 tokio 任务持有 `watch` 停止信号；`stop` 发信号、取消该机器人还在排队 / 运行的任务。
//!
//! 运行记录：消息入队时先插一条 `Queued` 记录（界面能看到、能按 run_id 取消），开始运行后由 runner 接管同一 id。
//! 排队中被取消的任务不会执行，直接回 `CANCELLED` 文案并把记录标为已取消。
//! 「处理中」表情在入队时加，任务从任何路径结束时统一移除。
//! 只有机器人主人能让机器人接入新群（`decide_all` 判断），自动接入的群默认仅主人可触发。
//! 代码审查机器人（模板 `code-review`）：群里贴 PR 链接也能触发（每个链接一次运行，同一 PR 10 分钟内不重复审）；
//! 最终回复按约定 JSON 解析，渲染成飞书卡片回复并保存本轮结果（下一轮增量复审用），解析不了退回发纯文本。
//! 所有配置写入都走 `config::update`（进程级锁内重读 → 修改 → 保存），不会和界面的保存互相覆盖。
//! 客户端启动时（`Supervisor::new`）把上次遗留的排队 / 运行中记录标为失败「客户端退出，运行中断」，并清理 30 天前的记录与原始日志。
//!
//! `Supervisor::new` 必须在 tokio 运行时内调用（记下当前运行时句柄，之后的方法可以在任意线程调用）。

use crate::agent::{AgentEngine, PromptContext, claude::Claude, codex::Codex};
use crate::dispatcher::queue::{Executor, Queue};
use crate::dispatcher::pr_link::{self, PrDedupe, PrRef};
use crate::dispatcher::{AgentNoticeThrottle, Decision, Dedupe, Job, attach_resume, copy, decide_all};
use crate::feishu::lark_cli::{LarkOps, SystemLark};
use crate::feishu::transport::{self, Client, Config, IncomingMessage, Status};
use crate::onboard;
use crate::review_card;
use crate::runner::{self, RunInput, RunnerDeps};
use crate::store::config::{self, AppConfig, Bot, Engine, Group, Trigger};
use crate::store::runs::{RunRecord, RunStatus, RunStore, StepRecord};
use crate::store::secrets;
use crate::workspace::Interpreters;
use futures_util::future::BoxFuture;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// 对外事件名（Task 12 用 `app.emit(名字, 载荷)` 发给前端）。
pub const EVENT_BOT_STATE: &str = "bot-state";
pub const EVENT_RUN_UPDATED: &str = "run-updated";
pub const EVENT_RUN_STEP: &str = "run-step";

/// 上次客户端退出时还没结束的运行，启动时统一标成这个失败原因。
pub const INTERRUPTED: &str = "客户端退出，运行中断";
/// 主人私聊自测时使用的虚拟群名（不登记进配置）。
pub const P2P_GROUP_NAME: &str = "私聊自测";

const DEDUPE_CAP: usize = 4096;
/// 连续失败多少次后状态显示为异常。
const FAILS_BEFORE_ERROR: u32 = 5;
const LOG_RETENTION_DAYS: u32 = config::DEFAULT_LOG_RETENTION_DAYS;
/// 可重试的运行（本次启动后收到的消息）最多记多少条。
const RECENT_CAP: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "lowercase")]
pub enum BotState {
    Stopped,
    Connecting,
    Running,
    Error(String),
}

/// `bot-state` 事件载荷。
#[derive(Debug, Clone, Serialize)]
pub struct BotStateEvent {
    pub bot_id: String,
    pub state: BotState,
}

/// `run-updated` 事件载荷。
#[derive(Debug, Clone, Serialize)]
pub struct RunUpdatedEvent {
    pub run: RunRecord,
}

/// `run-step` 事件载荷。
#[derive(Debug, Clone, Serialize)]
pub struct RunStepEvent {
    pub step: StepRecord,
}

/// 事件出口。测试里记录下来，Task 12 用 Tauri `AppHandle::emit` 实现。实现必须很快返回、不阻塞。
pub trait EventSink: Send + Sync {
    fn bot_state(&self, event: BotStateEvent);
    fn run_updated(&self, event: RunUpdatedEvent);
    fn run_step(&self, event: RunStepEvent);
}

/// 启动前的环境检查：参数为机器人 id，返回 Err（中文原因）时不启动。生产用 `envcheck::ensure_ready`。
pub type EnvGate = dyn Fn(&str) -> Result<(), String> + Send + Sync;

pub type MessageHandler = Box<dyn Fn(IncomingMessage) -> bool + Send + Sync>;
pub type StatusHandler = Box<dyn Fn(Status) + Send + Sync>;

/// 传输层抽象：生产用 `FeishuTransport`（包 `feishu::transport::Client`），测试用假实现。
/// 方法返回 `BoxFuture` 以便作为 `dyn Transport` 使用。
pub trait Transport: Send + Sync {
    /// 一直运行到 `stop` 变为 true（或飞书应用被删除）。内部自行断线重连，并通过 `on_status` 报告。
    /// `on_message` 必须立即返回（只入队），返回值表示是否接收（true = 向飞书确认）。
    fn run(&self, stop: watch::Receiver<bool>, on_message: MessageHandler, on_status: StatusHandler) -> BoxFuture<'_, ()>;
    /// 回复一条消息，返回新消息的 message_id。
    fn reply<'a>(&'a self, message_id: &'a str, text: &'a str) -> BoxFuture<'a, Result<Option<String>, String>>;
    /// 用消息卡片回复，返回新消息的 message_id。默认不支持（调用方退回发纯文本）。
    fn reply_card<'a>(&'a self, _message_id: &'a str, _card: &'a serde_json::Value) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async { Err("传输层不支持卡片消息".to_string()) })
    }
    /// 加「处理中」表情，返回 reaction_id；失败返回 None。
    fn add_working_reaction<'a>(&'a self, message_id: &'a str) -> BoxFuture<'a, Option<String>>;
    fn remove_working_reaction<'a>(&'a self, message_id: &'a str, reaction_id: &'a str) -> BoxFuture<'a, ()>;
}

/// 按机器人配置建传输层（生产实现在这里读 App Secret）。
pub type TransportFactory = Arc<dyn Fn(&Bot) -> Result<Arc<dyn Transport>, String> + Send + Sync>;

/// 生产传输层。
pub struct FeishuTransport(pub Client);

impl Transport for FeishuTransport {
    fn run(&self, stop: watch::Receiver<bool>, on_message: MessageHandler, on_status: StatusHandler) -> BoxFuture<'_, ()> {
        Box::pin(self.0.run(stop, on_message, on_status))
    }

    fn reply<'a>(&'a self, message_id: &'a str, text: &'a str) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(self.0.reply(message_id, text))
    }

    fn reply_card<'a>(&'a self, message_id: &'a str, card: &'a serde_json::Value) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(self.0.reply_card(message_id, card))
    }

    fn add_working_reaction<'a>(&'a self, message_id: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(self.0.working_reaction(message_id))
    }

    fn remove_working_reaction<'a>(&'a self, message_id: &'a str, reaction_id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(self.0.remove_working_reaction(message_id, reaction_id))
    }
}

/// 生产用传输层工厂：从系统凭据库取 App Secret，建飞书客户端。
pub fn feishu_transport() -> TransportFactory {
    Arc::new(|bot: &Bot| {
        let app_secret = secrets::app_secret(&bot.app_id)?;
        let client = Client::new(Config { app_id: bot.app_id.clone(), app_secret });
        Ok(Arc::new(FeishuTransport(client)) as Arc<dyn Transport>)
    })
}

pub struct SupervisorDeps {
    pub runner: Arc<RunnerDeps>,
    pub transport: TransportFactory,
    pub lark: Arc<dyn LarkOps>,
    pub events: Arc<dyn EventSink>,
}

impl SupervisorDeps {
    /// 生产依赖：Claude / Codex 两个引擎、按 PATH 与常见目录找命令、飞书传输层、本机 lark-cli。
    pub fn production(store: Arc<RunStore>, interpreters: Interpreters, events: Arc<dyn EventSink>) -> Self {
        let mut engines: HashMap<Engine, Arc<dyn AgentEngine>> = HashMap::new();
        engines.insert(Engine::Claude, Arc::new(Claude));
        engines.insert(Engine::Codex, Arc::new(Codex));
        SupervisorDeps {
            runner: Arc::new(RunnerDeps { store, engines, locate: Arc::new(runner::find_engine), interpreters }),
            transport: feishu_transport(),
            lark: Arc::new(SystemLark),
            events,
        }
    }
}

#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<Inner>,
}

struct Inner {
    deps: SupervisorDeps,
    rt: Handle,
    bots: Mutex<HashMap<String, Arc<BotRuntime>>>,
    states: Mutex<HashMap<String, BotState>>,
    /// 串行化状态事件的发出（不在 states 锁内发）。
    emit: Mutex<()>,
    owner: Mutex<Option<String>>,
    owner_fetching: AtomicBool,
    throttle: Mutex<AgentNoticeThrottle>,
    recent: Mutex<Recent>,
}

/// 一个运行中的机器人。队列只在这里和连接任务里持有，停止后随之释放。
struct BotRuntime {
    shared: Arc<BotShared>,
    queue: Queue,
    stop_tx: watch::Sender<bool>,
}

/// 连接回调和执行器共用的机器人状态（不含队列，避免引用环）。
struct BotShared {
    bot_id: String,
    transport: Arc<dyn Transport>,
    stopped: AtomicBool,
    failures: AtomicU32,
    dedupe: Mutex<Dedupe>,
    /// 同一 PR 10 分钟内只审一次（贴 PR 链接触发时用）。运行没成功会放开。
    prs: Mutex<PrDedupe>,
    /// 已提交、还没结束的运行 id。`stop` 在这把锁内置 `stopped`，`submit` 在这把锁内检查 `stopped` 并入队，
    /// 保证 stop 之后不会再有新运行入队，且 stop 时取到的 id 都已在队列里。
    active: Mutex<HashSet<String>>,
    /// 「处理中」表情，键 run_id。入队时异步加，任务结束（任何路径）时移除。
    reactions: Mutex<HashMap<String, Reaction>>,
    /// 执行器已开始处理的运行 id（区分「排队中被取消」和「执行中异常」）。
    started: Mutex<HashSet<String>>,
    /// 最近一次读成功的配置，读配置失败时兜底。
    last_cfg: Mutex<AppConfig>,
}

/// 表情状态：加表情的请求可能比任务结束还晚返回，所以要区分。
enum Reaction {
    /// 正在加
    Pending,
    /// 已加上
    Added(String),
    /// 任务已结束、表情还没加上：加上后立即移除
    Finished,
}

#[derive(Default)]
struct Recent {
    /// run_id → (机器人 id, 触发消息, 贴链接触发时的 PR)
    map: HashMap<String, (String, IncomingMessage, Option<PrRef>)>,
    order: VecDeque<String>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn log(text: impl AsRef<str>) {
    eprintln!("beetle: {}", text.as_ref());
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 消息时间（本地时区）；没有时间戳时用当前时间。
fn format_time(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    let t = (ms > 0).then(|| Local.timestamp_millis_opt(ms).single()).flatten().unwrap_or_else(Local::now);
    t.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn find_bot(cfg: &AppConfig, bot_id: &str) -> Option<Bot> {
    cfg.bots.iter().find(|b| b.id == bot_id).cloned()
}

/// 主人私聊自测用的虚拟群：不登记进配置，不建文档。
fn p2p_group(cfg: &AppConfig, chat_id: &str) -> Group {
    Group {
        chat_id: chat_id.to_string(),
        name: P2P_GROUP_NAME.to_string(),
        slug: format!("p2p-{}", onboard::chat_tail(chat_id, 8)),
        projects: cfg.default_projects.clone(),
        doc_url: None,
        trigger: Trigger::Owner,
    }
}

/// 排队记录上显示的群名。
fn group_label(bot: &Bot, msg: &IncomingMessage) -> String {
    if !msg.is_group {
        return P2P_GROUP_NAME.to_string();
    }
    bot.groups
        .iter()
        .find(|g| g.chat_id == msg.chat_id)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "未登记的群".to_string())
}

impl BotShared {
    fn config(&self) -> AppConfig {
        match config::load() {
            Ok(c) => {
                *lock(&self.last_cfg) = c.clone();
                c
            }
            Err(e) => {
                log(format!("读取配置失败，沿用上一次的配置：{e}"));
                lock(&self.last_cfg).clone()
            }
        }
    }
}

impl Supervisor {
    /// 必须在 tokio 运行时内调用。顺带清理上次遗留的运行记录和 30 天前的日志。
    pub fn new(deps: SupervisorDeps) -> Self {
        let store = &deps.runner.store;
        match store.fail_unfinished(INTERRUPTED) {
            Ok(0) => {}
            Ok(n) => log(format!("{n} 条上次未结束的运行已标为失败（{INTERRUPTED}）")),
            Err(e) => log(format!("标记遗留运行失败：{e}")),
        }
        let cfg = config::load().ok();
        // 日志保留天数在「设置 → 运行」里可改；读不到配置时按默认 30 天。
        let retention = cfg.as_ref().map_or(LOG_RETENTION_DAYS, |c| c.log_retention_days());
        if let Err(e) = store.purge_older_than(retention) {
            log(format!("清理旧运行记录失败：{e}"));
        }
        runner::purge_raw_logs(retention);
        // 内存里的主人身份是 union_id（跨应用一致）；open_id 只给脚本里的 lark-cli 比对用。
        let owner = cfg.and_then(|c| c.owner_union_id);
        Supervisor {
            inner: Arc::new(Inner {
                deps,
                rt: Handle::current(),
                bots: Mutex::new(HashMap::new()),
                states: Mutex::new(HashMap::new()),
                emit: Mutex::new(()),
                owner: Mutex::new(owner),
                owner_fetching: AtomicBool::new(false),
                throttle: Mutex::new(AgentNoticeThrottle::new()),
                recent: Mutex::new(Recent::default()),
            }),
        }
    }

    /// 机器人的连接任务是否还在（运行中、连接中，或「异常：连接失败，正在重试」时都为 true）。
    pub fn is_alive(&self, bot_id: &str) -> bool {
        lock(&self.inner.bots).contains_key(bot_id)
    }

    /// 先过环境检查再启动（界面、托盘、开机自启共用）。`gate` 返回 Err 时不启动：
    /// 机器人没在运行就把状态标为异常（原因 = 缺的环境项），让托盘 / 列表能看到为什么没起来。
    pub fn start_checked(&self, bot_id: &str, gate: &EnvGate) -> Result<(), String> {
        if let Err(e) = gate(bot_id) {
            if !self.is_alive(bot_id) {
                self.inner.set_state(bot_id, BotState::Error(e.clone()));
            }
            return Err(e);
        }
        self.start(bot_id)
    }

    /// 启动机器人：取 App Secret、建连接，立即返回（状态先是「连接中」）。已在运行（连接中 / 运行中）则什么都不做；
    /// 处于「异常：连接失败，正在重试」时先停掉旧连接再重新建立（立即重连，而不是空操作）。
    /// 取不到密钥 / 找不到机器人返回中文错误（取不到密钥时状态为异常）。
    pub fn start(&self, bot_id: &str) -> Result<(), String> {
        let inner = &self.inner;
        if lock(&inner.bots).contains_key(bot_id) {
            if !matches!(inner.state(bot_id), BotState::Error(_)) {
                return Ok(());
            }
            log(format!("机器人 {bot_id} 处于异常重试中，先停止旧连接再重新启动"));
            self.stop(bot_id);
        }
        let cfg = config::load()?;
        let bot = find_bot(&cfg, bot_id).ok_or_else(|| format!("找不到机器人 {bot_id}"))?;
        inner.ensure_owner(&cfg);
        let transport = match (inner.deps.transport)(&bot) {
            Ok(t) => t,
            Err(e) => {
                inner.set_state(bot_id, BotState::Error(e.clone()));
                return Err(e);
            }
        };
        let shared = Arc::new(BotShared {
            bot_id: bot_id.to_string(),
            transport,
            stopped: AtomicBool::new(false),
            failures: AtomicU32::new(0),
            dedupe: Mutex::new(Dedupe::new(DEDUPE_CAP)),
            prs: Mutex::new(PrDedupe::new()),
            active: Mutex::new(HashSet::new()),
            reactions: Mutex::new(HashMap::new()),
            started: Mutex::new(HashSet::new()),
            last_cfg: Mutex::new(cfg),
        });
        let weak = Arc::downgrade(inner);
        let queue = Queue::new(bot.concurrency, executor(weak.clone(), shared.clone()));
        let (stop_tx, stop_rx) = watch::channel(false);
        {
            let mut bots = lock(&inner.bots);
            if bots.contains_key(bot_id) {
                return Ok(());
            }
            bots.insert(bot_id.to_string(), Arc::new(BotRuntime { shared: shared.clone(), queue: queue.clone(), stop_tx }));
        }
        inner.set_bot_state(&shared, BotState::Connecting);

        let on_message: MessageHandler = {
            let (weak, shared) = (weak.clone(), shared.clone());
            Box::new(move |msg| {
                if let Some(inner) = weak.upgrade()
                    && !shared.stopped.load(Ordering::SeqCst)
                {
                    inner.on_message(&shared, &queue, msg);
                }
                // 不管处理结果都向飞书确认，否则会被反复重推。
                true
            })
        };
        let on_status: StatusHandler = {
            let (weak, shared) = (weak.clone(), shared.clone());
            Box::new(move |status| {
                if let Some(inner) = weak.upgrade() {
                    inner.on_status(&shared, status);
                }
            })
        };
        inner.rt.spawn(async move {
            let transport = shared.transport.clone();
            transport.run(stop_rx, on_message, on_status).await;
            let Some(inner) = weak.upgrade() else { return };
            {
                let mut bots = lock(&inner.bots);
                if bots.get(&shared.bot_id).is_some_and(|r| Arc::ptr_eq(&r.shared, &shared)) {
                    bots.remove(&shared.bot_id);
                }
            }
            // 不是 stop 引起的结束：应用被删时已是异常；其它意外结束也标异常，避免界面一直显示运行中。
            if !shared.stopped.load(Ordering::SeqCst) && !matches!(inner.state(&shared.bot_id), BotState::Error(_)) {
                inner.set_bot_state(&shared, BotState::Error("飞书连接意外结束".into()));
            }
        });
        Ok(())
    }

    /// 停止机器人：结束连接，取消它还在排队 / 运行的任务（群里回取消文案），状态变为已停止。
    pub fn stop(&self, bot_id: &str) {
        let runtime = lock(&self.inner.bots).remove(bot_id);
        if let Some(rt) = runtime {
            let ids: Vec<String> = {
                let active = lock(&rt.shared.active);
                rt.shared.stopped.store(true, Ordering::SeqCst);
                active.iter().cloned().collect()
            };
            let _ = rt.stop_tx.send(true);
            for id in ids {
                rt.queue.cancel(&id);
            }
        }
        self.inner.set_state(bot_id, BotState::Stopped);
    }

    pub fn state(&self, bot_id: &str) -> BotState {
        self.inner.state(bot_id)
    }

    /// 启动所有勾了「随客户端启动」的机器人（同样先过环境检查）；单个失败只记日志（状态里能看到原因）。
    pub fn start_autostart_bots(&self, gate: &EnvGate) {
        let cfg = match config::load() {
            Ok(c) => c,
            Err(e) => {
                log(format!("读取配置失败，未自动启动机器人：{e}"));
                return;
            }
        };
        for b in cfg.bots.iter().filter(|b| b.start_with_client) {
            if let Err(e) = self.start_checked(&b.id, gate) {
                log(format!("机器人 {} 自动启动失败：{e}", b.name));
            }
        }
    }

    /// 取消一次运行（运行中的结束进程树，排队中的直接移出队列）。找到并取消返回 true。
    pub fn cancel_run(&self, run_id: &str) -> bool {
        let bots: Vec<Arc<BotRuntime>> = lock(&self.inner.bots).values().cloned().collect();
        bots.iter().any(|rt| lock(&rt.shared.active).contains(run_id) && rt.queue.cancel(run_id))
    }

    /// 用同一条触发消息重新运行一次（生成新的运行记录）。只支持本次客户端启动后收到的消息，且机器人要在运行。
    pub fn retry_run(&self, run_id: &str) -> Result<(), String> {
        let inner = &self.inner;
        let (bot_id, msg, pr) = lock(&inner.recent)
            .map
            .get(run_id)
            .cloned()
            .ok_or_else(|| "只能重试本次客户端启动后收到的运行，请在群里重新 @ 机器人".to_string())?;
        let rt = lock(&inner.bots)
            .get(&bot_id)
            .cloned()
            .ok_or_else(|| "机器人没有在运行，请先启动机器人".to_string())?;
        let cfg = rt.shared.config();
        let bot = find_bot(&cfg, &bot_id).ok_or_else(|| format!("找不到机器人 {bot_id}"))?;
        let registered = bot.groups.iter().any(|g| g.chat_id == msg.chat_id);
        let job = Job {
            run_id: uuid::Uuid::new_v4().to_string(),
            bot_id,
            needs_onboard: msg.is_group && !registered && bot.auto_onboard,
            msg,
            resume: None,
            pr,
        };
        let job = attach_resume(job, &bot, &inner.deps.runner.store);
        inner.submit(&rt.shared, &rt.queue, &bot, job);
        Ok(())
    }
}

/// 任务结束时移除「处理中」表情；表情还在加的路上就标记一下，由加表情的任务加上后立即移除。
async fn clear_reaction(shared: &BotShared, run_id: &str, mid: &str) {
    let rid = {
        let mut map = lock(&shared.reactions);
        match map.remove(run_id) {
            Some(Reaction::Added(r)) => Some(r),
            Some(Reaction::Pending) => {
                map.insert(run_id.to_string(), Reaction::Finished);
                None
            }
            _ => None,
        }
    };
    if let Some(r) = rid {
        shared.transport.remove_working_reaction(mid, &r).await;
    }
}

fn executor(weak: Weak<Inner>, shared: Arc<BotShared>) -> Executor {
    Arc::new(move |job: Job, cancel: CancellationToken| {
        let (weak, shared) = (weak.clone(), shared.clone());
        Box::pin(async move {
            lock(&shared.started).insert(job.run_id.clone());
            if let Some(inner) = weak.upgrade() {
                inner.execute(&shared, job, cancel).await;
            }
        })
    })
}

impl Inner {
    fn state(&self, bot_id: &str) -> BotState {
        lock(&self.states).get(bot_id).cloned().unwrap_or(BotState::Stopped)
    }

    /// 状态有变化才通知。`emit` 锁保证事件顺序与状态变化一致；`states` 锁在通知前释放，
    /// 所以 EventSink 回调里可以放心调用 `state()`（托盘刷新就会）。EventSink 绝不能在任何内部锁持有期间被调用。
    fn put_state(&self, bot_id: &str, state: BotState, skip: Option<&BotShared>) {
        let _emit = lock(&self.emit);
        {
            let mut states = lock(&self.states);
            if skip.is_some_and(|s| s.stopped.load(Ordering::SeqCst)) {
                return;
            }
            if states.get(bot_id).unwrap_or(&BotState::Stopped) == &state {
                return;
            }
            states.insert(bot_id.to_string(), state.clone());
        }
        self.deps.events.bot_state(BotStateEvent { bot_id: bot_id.to_string(), state });
    }

    fn set_state(&self, bot_id: &str, state: BotState) {
        self.put_state(bot_id, state, None);
    }

    /// 连接回调用：机器人已停止后不再改状态（`stop` 先置标志再写「已停止」，同一把锁下判断，不会被迟到的状态覆盖）。
    fn set_bot_state(&self, shared: &BotShared, state: BotState) {
        self.put_state(&shared.bot_id, state, Some(shared));
    }

    fn on_status(&self, shared: &BotShared, status: Status) {
        let state = match status {
            Status::Connecting => {
                // 已显示异常时，后续重试不把异常盖回「连接中」，直到真正连上。
                if shared.failures.load(Ordering::SeqCst) >= FAILS_BEFORE_ERROR {
                    return;
                }
                BotState::Connecting
            }
            Status::Connected => {
                shared.failures.store(0, Ordering::SeqCst);
                BotState::Running
            }
            Status::Disconnected(reason) => {
                let n = shared.failures.fetch_add(1, Ordering::SeqCst) + 1;
                log(format!("机器人 {} 连接断开（连续第 {n} 次），稍后重试：{reason}", shared.bot_id));
                if n >= FAILS_BEFORE_ERROR {
                    BotState::Error(format!("连接失败，正在重试：{reason}"))
                } else {
                    BotState::Connecting
                }
            }
            Status::AppDeleted => BotState::Error(transport::APP_DELETED.to_string()),
        };
        self.set_bot_state(shared, state);
    }

    /// 启动机器人时，配置里没有主人 union_id（首次启动，或旧配置只存了 open_id）就经 lark-cli 取一次，
    /// 同时保存 open_id（lark-cli 命名空间）和 union_id（后台进行，不阻塞启动）。
    fn ensure_owner(self: &Arc<Self>, cfg: &AppConfig) {
        if let Some(o) = cfg.owner_union_id.as_ref().filter(|o| !o.is_empty()) {
            *lock(&self.owner) = Some(o.clone());
            return;
        }
        if lock(&self.owner).is_some() || self.owner_fetching.swap(true, Ordering::SeqCst) {
            return;
        }
        let inner = self.clone();
        self.rt.spawn_blocking(move || {
            match inner.deps.lark.current_user() {
                Ok(ids) => {
                    *lock(&inner.owner) = Some(ids.union_id.clone());
                    let saved = config::update(|c| {
                        c.owner_open_id = Some(ids.open_id);
                        c.owner_union_id = Some(ids.union_id);
                        Ok(())
                    });
                    if let Err(e) = saved {
                        log(format!("保存机器人主人身份失败：{e}"));
                    }
                }
                Err(e) => log(format!(
                    "取不到机器人主人的身份（{e}）：「仅机器人主人」可触发的群和私聊不会触发，登录 lark-cli 后重新启动机器人"
                )),
            }
            inner.owner_fetching.store(false, Ordering::SeqCst);
        });
    }

    /// 连接回调里调用，必须很快返回：只做判断和入队。
    fn on_message(self: &Arc<Self>, shared: &Arc<BotShared>, queue: &Queue, msg: IncomingMessage) {
        let cfg = shared.config();
        let Some(bot) = find_bot(&cfg, &shared.bot_id) else {
            log(format!("机器人 {} 已不在配置里，忽略消息 {}", shared.bot_id, msg.message_id));
            return;
        };
        let owner = lock(&self.owner).clone();
        let decisions = {
            let mut seen = lock(&shared.dedupe);
            let mut prs = lock(&shared.prs);
            decide_all(&msg, &bot, owner.as_deref(), &mut seen, &mut prs)
        };
        for decision in decisions {
            match decision {
                Decision::Ignore(reason) => {
                    if owner.is_none() && matches!(reason, "not-allowed" | "p2p-not-owner") {
                        log(format!(
                            "消息 {} 未触发：没有取到机器人主人的 union_id（lark-cli 未登录？），「仅机器人主人」的群和私聊不会触发",
                            msg.message_id
                        ));
                    } else if !matches!(reason, "no-mention" | "group-not-registered") {
                        log(format!("忽略消息 {}：{reason}", msg.message_id));
                    }
                }
                Decision::ReplyOnly(text) => self.spawn_reply(shared, &msg.message_id, text),
                Decision::RecentPr { url, run_id } => {
                    let running = lock(&shared.active).contains(&run_id);
                    log(format!("消息 {} 里的 PR {url} 10 分钟内已审过 / 正在审，不重复审查", msg.message_id));
                    self.spawn_reply(shared, &msg.message_id, copy::pr_recent_reply(&url, running));
                }
                Decision::Run(job) => {
                    let job = attach_resume(job, &bot, &self.deps.runner.store);
                    self.submit(shared, queue, &bot, job);
                }
            }
        }
    }

    /// 只回一句固定文案（不运行）。
    fn spawn_reply(&self, shared: &Arc<BotShared>, message_id: &str, text: String) {
        let (t, mid) = (shared.transport.clone(), message_id.to_string());
        self.rt.spawn(async move {
            if let Err(e) = t.reply(&mid, &text).await {
                log(format!("回复消息 {mid} 失败：{e}"));
            }
        });
    }

    /// 插入排队记录并入队；另起一个任务等它结束，处理「排队中被取消」和「执行器异常」。
    fn submit(self: &Arc<Self>, shared: &Arc<BotShared>, queue: &Queue, bot: &Bot, job: Job) {
        let rec = RunRecord {
            id: job.run_id.clone(),
            bot_id: bot.id.clone(),
            chat_id: job.msg.chat_id.clone(),
            group_name: group_label(bot, &job.msg),
            trigger_message_id: job.msg.message_id.clone(),
            // 姓名在开始运行时经 lark-cli 解析，排队时先用 open_id 占位。
            sender_name: job.msg.sender_open_id.clone(),
            text: job.msg.text.clone(),
            engine: bot.engine,
            engine_version: None,
            session_id: None,
            status: RunStatus::Queued,
            started_at: now_ms(),
            finished_at: None,
            final_reply: None,
            error: None,
            sql_count: 0,
            reply_message_id: None,
            sender_union_id: job.msg.sender_union_id.clone(),
        };
        let run_id = job.run_id.clone();
        let mid = job.msg.message_id.clone();
        let pr_key = job.pr.as_ref().map(PrRef::key);
        let queued_rec = rec.clone();
        let handle = {
            // 与 stop 同一把锁：stop 之后不再入队；入队的 id 在 stop 取 id 时一定已在队列里。
            let mut active = lock(&shared.active);
            if shared.stopped.load(Ordering::SeqCst) {
                log(format!("机器人 {} 已停止，消息 {mid} 不再处理", shared.bot_id));
                if let Some(k) = &pr_key {
                    lock(&shared.prs).release(k, &run_id);
                }
                return;
            }
            if let Err(e) = self.deps.runner.store.insert_run(&rec) {
                log(format!("写入排队记录失败：{e}"));
            }
            {
                let mut recent = lock(&self.recent);
                if recent.order.len() >= RECENT_CAP
                    && let Some(old) = recent.order.pop_front()
                {
                    recent.map.remove(&old);
                }
                recent.order.push_back(run_id.clone());
                recent.map.insert(run_id.clone(), (bot.id.clone(), job.msg.clone(), job.pr.clone()));
            }
            active.insert(run_id.clone());
            // 入队时就加「处理中」表情（spec §7：排队时先给原消息加表情）。
            lock(&shared.reactions).insert(run_id.clone(), Reaction::Pending);
            self.spawn_add_reaction(shared, &run_id, &mid);
            let _enter = self.rt.enter();
            queue.submit(job)
        };
        // 不在 active 锁内通知 EventSink
        self.deps.events.run_updated(RunUpdatedEvent { run: queued_rec });
        let (weak, shared) = (Arc::downgrade(self), shared.clone());
        self.rt.spawn(async move {
            let finished = handle.finished().await;
            lock(&shared.active).remove(&run_id);
            let started = lock(&shared.started).remove(&run_id);
            if !finished && let Some(inner) = weak.upgrade() {
                let t = shared.transport.clone();
                if started {
                    inner.finish_without_run(&t, &run_id, RunStatus::Failed, "运行异常中断（内部错误）", copy::FAILED).await;
                } else {
                    inner.finish_without_run(&t, &run_id, RunStatus::Cancelled, "排队中被机器人主人取消", copy::CANCELLED).await;
                }
            }
            // 所有结束路径（正常结束、排队中取消、接入失败、执行器 panic）都在这里统一移除表情。
            clear_reaction(&shared, &run_id, &mid).await;
            // 贴链接触发的审查没成功（失败、超时、取消）：放开这个 PR，允许马上重贴重审
            if let Some(k) = pr_key
                && let Some(inner) = weak.upgrade()
            {
                let ok = matches!(inner.deps.runner.store.get_run(&run_id), Ok(Some((r, _))) if r.status == RunStatus::Replied);
                if !ok {
                    lock(&shared.prs).release(&k, &run_id);
                }
            }
        });
    }

    fn spawn_add_reaction(&self, shared: &Arc<BotShared>, run_id: &str, mid: &str) {
        let (shared, run_id, mid) = (shared.clone(), run_id.to_string(), mid.to_string());
        self.rt.spawn(async move {
            let rid = shared.transport.add_working_reaction(&mid).await;
            let leftover = {
                let mut map = lock(&shared.reactions);
                match (map.remove(&run_id), rid) {
                    (Some(Reaction::Pending), Some(r)) => {
                        map.insert(run_id, Reaction::Added(r));
                        None
                    }
                    // 任务在表情加上之前就结束了：加上后马上去掉
                    (Some(Reaction::Finished), Some(r)) => Some(r),
                    _ => None,
                }
            };
            if let Some(r) = leftover {
                shared.transport.remove_working_reaction(&mid, &r).await;
            }
        });
    }

    /// 一次任务：（自动接入）→ 解析发送者姓名与被引用消息 → run_once → 回复 → 记回复 id。
    /// 「处理中」表情在入队时加、结束时由 `submit` 的等待任务统一移除。
    /// 开头和自动接入之后各检查一次取消：已取消就按 CANCELLED 收尾，不接入、不建文档、不拉起 Agent。
    async fn execute(self: &Arc<Self>, shared: &Arc<BotShared>, job: Job, cancel: CancellationToken) {
        let t = shared.transport.clone();
        let msg = job.msg.clone();
        if cancel.is_cancelled() {
            self.finish_cancelled(&t, &job.run_id).await;
            return;
        }
        let mut cfg = shared.config();
        let Some(mut bot) = find_bot(&cfg, &shared.bot_id) else {
            self.finish_without_run(&t, &job.run_id, RunStatus::Failed, "机器人已被删除", copy::FAILED).await;
            return;
        };
        let group = if !msg.is_group {
            p2p_group(&cfg, &msg.chat_id)
        } else if let Some(g) = bot.groups.iter().find(|g| g.chat_id == msg.chat_id).cloned() {
            g
        } else if job.needs_onboard {
            match self.onboard(&bot.id, &msg.chat_id).await {
                Ok((g, new_cfg)) => {
                    log(format!("群「{}」（{}）已自动接入", g.name, g.chat_id));
                    cfg = new_cfg;
                    bot = find_bot(&cfg, &shared.bot_id).unwrap_or(bot);
                    g
                }
                Err(e) => {
                    log(format!("群 {} 自动接入失败：{e}", msg.chat_id));
                    let error = format!("自动接入失败：{e}");
                    self.finish_without_run(&t, &job.run_id, RunStatus::Failed, &error, copy::FAILED).await;
                    return;
                }
            }
        } else {
            let error = "本群已不在机器人的群列表里";
            self.finish_without_run(&t, &job.run_id, RunStatus::Failed, error, copy::GROUP_NOT_ENABLED).await;
            return;
        };
        if cancel.is_cancelled() {
            self.finish_cancelled(&t, &job.run_id).await;
            return;
        }

        let (sender_name, parent_text) = self.resolve_people(&msg).await;
        let owner = lock(&self.owner).clone();
        let ctx = PromptContext {
            chat_id: msg.chat_id.clone(),
            group_name: group.name.clone(),
            trigger_message_id: msg.message_id.clone(),
            trigger_text: msg.text.clone(),
            sender_name,
            sender_open_id: msg.sender_open_id.clone(),
            is_owner: crate::dispatcher::is_owner(&msg, owner.as_deref()),
            parent_message_id: msg.parent_id.clone(),
            parent_text,
            root_id: msg.root_id.clone(),
            time: format_time(msg.create_time_ms),
            pr_url: job.pr.as_ref().map(PrRef::url),
        };
        let input = RunInput {
            run_id: job.run_id.clone(),
            bot: bot.clone(),
            group,
            cfg,
            ctx,
            resume: job.resume.clone(),
            sender_union_id: msg.sender_union_id.clone(),
        };
        let (events, store) = (self.deps.events.clone(), self.deps.runner.store.clone());
        let on_step = move |s: &StepRecord| {
            // 第一个步骤落库时运行记录刚变成「运行中」，推一次给界面。
            if s.seq == 1
                && let Ok(Some((run, _))) = store.get_run(&s.run_id)
            {
                events.run_updated(RunUpdatedEvent { run });
            }
            events.run_step(RunStepEvent { step: s.clone() });
        };
        let (mut rec, outcome) = runner::run_once(input, &self.deps.runner, cancel, on_step).await;
        if bot.is_code_review()
            && let runner::Outcome::Replied(text) = &outcome
        {
            self.reply_review(&t, &mut rec, text, job.pr.as_ref()).await;
            return;
        }
        let text = lock(&self.throttle).reply(&bot.id, &outcome, bot.engine, bot.timeout_minutes);
        self.reply_and_record(&t, &mut rec, &text).await;
    }

    /// 代码审查的最终回复：按约定 JSON 解析 → 保存本轮结果 → 卡片回复（卡片发不出去改发同内容的纯文本）。
    /// 解析不了就把（runner 已打码的）原文当纯文本发出去，并在运行记录里注明。卡片里的文字再过一遍打码。
    /// 运行记录的 `final_reply` 存解析后的 JSON，界面按卡片样式预览。
    async fn reply_review(&self, t: &Arc<dyn Transport>, rec: &mut RunRecord, text: &str, pr: Option<&PrRef>) {
        let result = match review_card::parse(text) {
            Ok(r) => r.redacted(),
            Err(e) => {
                log(format!("运行 {} 的审查结果解析失败，按纯文本回复：{e}", rec.id));
                rec.error = Some(format!("审查结果不是约定的 JSON（{e}），已按纯文本回复"));
                self.reply_and_record(t, rec, text).await;
                return;
            }
        };
        if let Ok(json) = serde_json::to_string(&result) {
            rec.final_reply = Some(json);
        }
        // 保存本轮结果：PR 取触发时识别的；@ 触发的从结果里的链接认
        let target = pr.cloned().or_else(|| pr_link::find_pr_links(&result.url).into_iter().next());
        match target {
            Some(p) => {
                let r = result.clone();
                let saved = tokio::task::spawn_blocking(move || review_card::save_state(&p, &r))
                    .await
                    .unwrap_or_else(|_| Err("保存任务异常中断".into()));
                if let Err(e) = saved {
                    log(format!("保存运行 {} 的审查结果失败：{e}", rec.id));
                }
            }
            None => log(format!("运行 {} 的审查结果里没有可识别的 PR 链接，本轮结果不保存", rec.id)),
        }
        let card = review_card::render(&result);
        match t.reply_card(&rec.trigger_message_id, &card).await {
            Ok(id) => {
                rec.reply_message_id = id;
                if let Err(e) = self.deps.runner.store.update_run(rec) {
                    log(format!("更新运行记录失败：{e}"));
                }
                self.deps.events.run_updated(RunUpdatedEvent { run: rec.clone() });
            }
            Err(e) => {
                log(format!("运行 {} 的审查卡片发送失败，改发纯文本：{e}", rec.id));
                rec.error = Some(format!("卡片发送失败（{e}），已改发纯文本"));
                self.reply_and_record(t, rec, &review_card::render_text(&result)).await;
            }
        }
    }

    async fn finish_cancelled(&self, t: &Arc<dyn Transport>, run_id: &str) {
        self.finish_without_run(t, run_id, RunStatus::Cancelled, "已被机器人主人取消", copy::CANCELLED).await;
    }

    /// 登记群（群已登记则直接返回它，不会重复建文档）；配置写入走 `config::update`。
    async fn onboard(self: &Arc<Self>, bot_id: &str, chat_id: &str) -> Result<(Group, AppConfig), String> {
        let lark = self.deps.lark.clone();
        let (bot_id, chat_id) = (bot_id.to_string(), chat_id.to_string());
        tokio::task::spawn_blocking(move || onboard::register_group(&bot_id, &chat_id, lark.as_ref()))
            .await
            .map_err(|_| "自动接入任务异常中断".to_string())?
    }

    /// 发送者姓名（取不到用 open_id）与被引用消息内容（取不到为 None）。
    /// 姓名按 union_id 经 lark-cli 查：机器人事件里的 open_id 属于机器人应用，拿到 lark-cli 应用里查会报 99992361。
    async fn resolve_people(&self, msg: &IncomingMessage) -> (String, Option<String>) {
        let lark = self.deps.lark.clone();
        let (open_id, union_id, parent) = (msg.sender_open_id.clone(), msg.sender_union_id.clone(), msg.parent_id.clone());
        let fallback = open_id.clone();
        tokio::task::spawn_blocking(move || {
            let name = match union_id.as_deref().filter(|u| !u.is_empty()) {
                Some(u) => lark.user_name(u).unwrap_or_else(|e| {
                    log(format!("取发送者姓名失败，用 open_id 代替：{e}"));
                    open_id.clone()
                }),
                None => {
                    log("消息里没有发送者的 union_id，姓名用 open_id 代替");
                    open_id.clone()
                }
            };
            let parent_text = parent.and_then(|p| {
                lark.message_text(&p)
                    .map_err(|e| log(format!("取被引用消息 {p} 失败：{e}")))
                    .ok()
            });
            (name, parent_text)
        })
        .await
        .unwrap_or((fallback, None))
    }

    /// 没有真正运行就结束的任务（排队中取消、自动接入失败等）：改记录、回复固定文案。
    async fn finish_without_run(&self, t: &Arc<dyn Transport>, run_id: &str, status: RunStatus, error: &str, reply: &str) {
        let mut rec = match self.deps.runner.store.get_run(run_id) {
            Ok(Some((rec, _))) => rec,
            other => {
                log(format!("找不到运行记录 {run_id}（{:?}），未回复群里", other.err()));
                return;
            }
        };
        rec.status = status;
        rec.error = Some(error.to_string());
        rec.finished_at = Some(now_ms());
        self.reply_and_record(t, &mut rec, reply).await;
    }

    /// 回复到触发消息下，记下回复的 message_id（续聊用）后落库并通知界面。
    /// 回复失败：记日志、把原因追加进记录；本来是「已回复」的改成「失败」（群里其实没收到，不能算成功），
    /// `final_reply` 保留，方便主人在客户端里复制后手工转发。
    async fn reply_and_record(&self, t: &Arc<dyn Transport>, rec: &mut RunRecord, text: &str) {
        match t.reply(&rec.trigger_message_id, text).await {
            Ok(id) => rec.reply_message_id = id,
            Err(e) => {
                log(format!("回复消息 {} 失败：{e}", rec.trigger_message_id));
                rec.error = Some(match rec.error.take() {
                    Some(prev) => format!("{prev}；群里回复失败：{e}"),
                    None => format!("群里回复失败：{e}"),
                });
                if rec.status == RunStatus::Replied {
                    rec.status = RunStatus::Failed;
                }
            }
        }
        if let Err(e) = self.deps.runner.store.update_run(rec) {
            log(format!("更新运行记录失败：{e}"));
        }
        self.deps.events.run_updated(RunUpdatedEvent { run: rec.clone() });
    }
}

#[cfg(test)]
mod tests;
