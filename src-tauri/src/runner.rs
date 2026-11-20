//! 一次运行：收到一条 @ → 准备工作目录 → 拉起 Agent CLI → 把事件流翻译成步骤落库 → 超时 / 取消 → 打码后的最终回复。
//!
//! 结果规则（群里只会拿到 `Outcome::Replied` 的文本，其余由调度器换成固定文案）：
//! - 有 Final 且退出码 0 → `Replied(打码后的文本)`；
//! - 出现 `AgentEvent::Error`、没有 Final、或退出码非 0 → `Failed(原因)`，原因只进客户端日志，不发群里，
//!   也不含任何助手自由文本；退出码非 0 时不信任 Codex 的 `-o` 文件；
//! - 超时 → `Timeout`；取消 → `Cancelled`（两者都结束整棵进程树）；
//! - 找不到程序 / 起不来 → `AgentUnavailable`。
//!
//! 原始日志：stdout 每行经 `redact_json_line`（JSON 结构保持合法）打码后追加到 `data_dir()/runs/<run_id>.jsonl`，stderr 末尾写 `<run_id>.stderr.txt`。

use crate::agent::{self, AgentEngine, AgentEvent, AgentRequest, PromptContext, build_prompt, steps::truncate_chars};
use crate::platform::{locate, paths, proc};
use crate::redact::{redact, redact_json_line};
use crate::store::config::{AppConfig, Bot, Engine, Group};
use crate::store::runs::{RunRecord, RunStatus, RunStore, StepKind, StepRecord};
use crate::workspace::{self, Interpreters, TriggerInfo};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

pub struct RunInput {
    /// 运行记录 id（= 调度器 `Job.run_id`，界面按它取消）。
    pub run_id: String,
    pub bot: Bot,
    pub group: Group,
    pub cfg: AppConfig,
    pub ctx: PromptContext,
    pub resume: Option<String>,
    /// 触发人的 union_id，写进运行记录（续聊时校验发送人用）。
    pub sender_union_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Replied(String),
    Failed(String),
    Timeout,
    Cancelled,
    AgentUnavailable,
}

pub struct RunnerDeps {
    pub store: Arc<RunStore>,
    pub engines: HashMap<Engine, Arc<dyn AgentEngine>>,
    pub locate: Arc<dyn Fn(Engine) -> Option<PathBuf> + Send + Sync>,
    pub interpreters: Interpreters,
}

/// 引擎对应的命令名。
pub fn program_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Claude => "claude",
        Engine::Codex => "codex",
    }
}

/// 生产环境的 `RunnerDeps.locate`：在 PATH 和常见安装目录里找 claude / codex。
pub fn find_engine(engine: Engine) -> Option<PathBuf> {
    locate::find(program_name(engine))
}

fn raw_log_dir() -> PathBuf {
    paths::data_dir().join("runs")
}

/// 界面「原始日志」读取的文件：Agent stdout 每行（已打码）。
pub fn raw_log_path(run_id: &str) -> PathBuf {
    raw_log_dir().join(format!("{run_id}.jsonl"))
}

pub fn stderr_log_path(run_id: &str) -> PathBuf {
    raw_log_dir().join(format!("{run_id}.stderr.txt"))
}

/// 删除修改时间早于 `days` 天前的原始日志文件（与 `RunStore::purge_older_than` 一起调用），返回删除的文件数。
pub fn purge_raw_logs(days: u32) -> usize {
    let Ok(entries) = std::fs::read_dir(raw_log_dir()) else { return 0 };
    let cutoff = SystemTime::now() - Duration::from_secs(days as u64 * 86_400);
    let mut n = 0;
    for e in entries.flatten() {
        let old = e
            .metadata()
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| m.modified().ok())
            .is_some_and(|t| t < cutoff);
        if old && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
thread_local! {
    static TEST_TIMEOUT_SECS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// 测试钩子：把本线程上的运行超时改成秒级（tokio 单线程测试运行时里 run_once 就在本线程上跑）。
#[cfg(test)]
pub(crate) fn set_test_timeout_secs(secs: Option<u64>) {
    TEST_TIMEOUT_SECS.with(|c| c.set(secs));
}

fn run_timeout(bot: &Bot) -> Duration {
    #[cfg(test)]
    if let Some(s) = TEST_TIMEOUT_SECS.with(|c| c.get()) {
        return Duration::from_secs(s);
    }
    Duration::from_secs(bot.timeout_minutes as u64 * 60)
}

/// kill 之后等进程回收的上限（proc 层自己最多等 stderr 2 秒）。
const WAIT_AFTER_KILL: Duration = Duration::from_secs(10);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 步骤落库并通知界面。标题与详情一律打码。落库失败不影响运行。
struct Steps<'a, F: Fn(&StepRecord)> {
    run_id: String,
    seq: u32,
    store: &'a RunStore,
    on_step: F,
}

impl<F: Fn(&StepRecord)> Steps<'_, F> {
    fn emit(&mut self, kind: StepKind, title: &str, detail: &str) {
        self.seq += 1;
        let s = StepRecord {
            run_id: self.run_id.clone(),
            seq: self.seq,
            at: now_ms(),
            kind,
            title: redact(title),
            detail: redact(detail),
            duration_ms: None,
        };
        let _ = self.store.append_step(&s);
        (self.on_step)(&s);
    }
}

enum Stop {
    Timeout,
    Cancelled,
}

pub async fn run_once(
    input: RunInput,
    deps: &RunnerDeps,
    cancel: CancellationToken,
    on_step: impl Fn(&StepRecord),
) -> (RunRecord, Outcome) {
    let RunInput { run_id, bot, group, cfg, ctx, resume, sender_union_id } = input;
    let mut rec = RunRecord {
        id: run_id,
        bot_id: bot.id.clone(),
        chat_id: group.chat_id.clone(),
        group_name: group.name.clone(),
        trigger_message_id: ctx.trigger_message_id.clone(),
        sender_name: ctx.sender_name.clone(),
        text: ctx.trigger_text.clone(),
        engine: bot.engine,
        engine_version: None,
        session_id: None,
        status: RunStatus::Running,
        started_at: now_ms(),
        finished_at: None,
        final_reply: None,
        error: None,
        sql_count: 0,
        reply_message_id: None,
        sender_union_id,
    };
    // 调度器在排队时已插入一条 Queued 记录（同一 id），这里接管它。
    if deps.store.insert_run(&rec).is_err() {
        let _ = deps.store.update_run(&rec);
    }
    let mut steps = Steps { run_id: rec.id.clone(), seq: 0, store: &deps.store, on_step };
    steps.emit(
        StepKind::Received,
        &format!("收到 {} 的消息", ctx.sender_name),
        &truncate_chars(&ctx.trigger_text, 500),
    );

    let outcome = execute(&bot, &group, &cfg, &ctx, resume, deps, &cancel, &mut rec, &mut steps).await;
    // 出口统一打码：返回给调度器的失败原因与 rec.error 是同一份打码后的内容。
    let outcome = match outcome {
        Outcome::Failed(reason) => Outcome::Failed(redact(&reason)),
        o => o,
    };
    rec.error = rec.error.as_deref().map(redact);

    match &outcome {
        Outcome::Replied(text) => {
            rec.status = RunStatus::Replied;
            rec.final_reply = Some(text.clone());
            steps.emit(StepKind::Reply, "生成最终回复", &truncate_chars(text, 2000));
        }
        Outcome::Failed(reason) => {
            rec.status = RunStatus::Failed;
            rec.error = Some(reason.clone());
        }
        Outcome::Timeout => {
            rec.status = RunStatus::Timeout;
            rec.error = Some(format!("超过 {} 分钟未完成，已终止", bot.timeout_minutes));
        }
        Outcome::Cancelled => {
            rec.status = RunStatus::Cancelled;
            rec.error = Some("已被机器人主人取消".into());
        }
        Outcome::AgentUnavailable => {
            rec.status = RunStatus::Failed;
            if rec.error.is_none() {
                rec.error = Some(format!("本机 {} 不可用", program_name(bot.engine)));
            }
        }
    }
    if let Some(e) = rec.error.clone()
        && !matches!(outcome, Outcome::Replied(_))
    {
        steps.emit(StepKind::Error, "运行未完成", &e);
    }
    rec.finished_at = Some(now_ms());
    let _ = deps.store.update_run(&rec);
    (rec, outcome)
}

#[allow(clippy::too_many_arguments)]
async fn execute<F: Fn(&StepRecord)>(
    bot: &Bot,
    group: &Group,
    cfg: &AppConfig,
    ctx: &PromptContext,
    resume: Option<String>,
    deps: &RunnerDeps,
    cancel: &CancellationToken,
    rec: &mut RunRecord,
    steps: &mut Steps<'_, F>,
) -> Outcome {
    let name = program_name(bot.engine);
    let (Some(engine), Some(program)) = (deps.engines.get(&bot.engine), (deps.locate)(bot.engine)) else {
        rec.error = Some(format!("本机找不到 {name} 程序"));
        return Outcome::AgentUnavailable;
    };

    // 工作目录（运行时配置带本次触发消息）与 Agent 要写的工作区外目录。
    let trigger = TriggerInfo {
        message_id: ctx.trigger_message_id.clone(),
        sender_open_id: ctx.sender_open_id.clone(),
        is_owner: ctx.is_owner,
    };
    let ws = match workspace::prepare(cfg, bot, group, &deps.interpreters, &trigger) {
        Ok(ws) => ws,
        Err(e) => return Outcome::Failed(format!("准备工作目录失败：{e}")),
    };
    let extra_writable_dirs = vec![cfg.code_cache_dir()];
    for d in &extra_writable_dirs {
        if let Err(e) = std::fs::create_dir_all(d) {
            return Outcome::Failed(format!("创建目录 {} 失败：{e}", d.display()));
        }
    }
    steps.emit(StepKind::Prepare, "准备工作目录", &ws.dir.to_string_lossy());

    // 版本号尽力而为，失败不影响运行。
    let p = program.clone();
    rec.engine_version = tokio::task::spawn_blocking(move || locate::version(&p).ok()).await.ok().flatten();

    let req = AgentRequest {
        workspace: ws.dir.clone(),
        runtime_json: ws.runtime_json.clone(),
        prompt: build_prompt(ctx),
        resume_session: resume,
        proxy: cfg.proxy.clone(),
        extra_writable_dirs,
    };
    let mut spec = engine.launch(&program, &req);
    // 直连查库（beetle --db-query）写审计时用它关联到本次运行。
    spec.env.push(("BEETLE_RUN_ID".into(), rec.id.clone()));
    agent::clear_stale_outputs(&spec);
    let mut child = match proc::spawn(&spec) {
        Ok(c) => c,
        Err(e) => {
            rec.error = Some(e);
            return Outcome::AgentUnavailable;
        }
    };
    steps.emit(
        StepKind::Launch,
        &format!("启动 {name}"),
        &format!("{}（{}）", program.display(), rec.engine_version.as_deref().unwrap_or("版本未知")),
    );

    let _ = std::fs::create_dir_all(raw_log_dir());
    let mut raw = std::fs::OpenOptions::new().create(true).append(true).open(raw_log_path(&rec.id)).ok();

    let mut lines = child.lines();
    let deadline = tokio::time::sleep(run_timeout(bot));
    tokio::pin!(deadline);
    let mut stop: Option<Stop> = None;
    let mut final_text: Option<String> = None;
    let mut error: Option<String> = None;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => { stop = Some(Stop::Cancelled); break; }
            _ = &mut deadline => { stop = Some(Stop::Timeout); break; }
            line = lines.recv() => {
                let Some(line) = line else { break };
                if let Some(f) = raw.as_mut() {
                    let _ = writeln!(f, "{}", redact_json_line(&String::from_utf8_lossy(&line)));
                }
                for ev in engine.parse_line(&line) {
                    match ev {
                        AgentEvent::Session(id) => rec.session_id = Some(id),
                        AgentEvent::Step { kind, title, detail } => {
                            if kind == StepKind::Sql {
                                rec.sql_count += 1;
                            }
                            steps.emit(kind, &title, &detail);
                        }
                        AgentEvent::Final(t) => final_text = Some(t),
                        AgentEvent::Error(e) => {
                            steps.emit(StepKind::Error, "Agent 报告错误", &e);
                            error = Some(e);
                        }
                    }
                }
            }
        }
    }

    let exit = if stop.is_some() {
        child.kill_tree();
        tokio::time::timeout(WAIT_AFTER_KILL, child.wait()).await.ok()
    } else {
        // 行流结束但进程可能还没退出：继续受超时与取消约束。
        // 提前返回时 wait future 被丢弃，句柄随之 drop（Windows 上 Job 关闭即结束整棵进程树）。
        let wait = child.wait();
        tokio::pin!(wait);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => { stop = Some(Stop::Cancelled); None }
            _ = &mut deadline => { stop = Some(Stop::Timeout); None }
            info = &mut wait => Some(info),
        }
    };
    if let Some(info) = &exit {
        let _ = std::fs::write(stderr_log_path(&rec.id), redact(&info.stderr_tail));
    }

    match stop {
        Some(Stop::Cancelled) => return Outcome::Cancelled,
        Some(Stop::Timeout) => return Outcome::Timeout,
        None => {}
    }
    if let Some(e) = error {
        return Outcome::Failed(format!("Agent 报告错误：{e}"));
    }
    let info = exit.expect("未停止时一定等到了退出信息");
    if info.code != Some(0) {
        let tail: String = {
            let t = info.stderr_tail.trim();
            let n = t.chars().count();
            t.chars().skip(n.saturating_sub(500)).collect()
        };
        return Outcome::Failed(format!("Agent 异常退出（退出码 {:?}）：{tail}", info.code));
    }
    // 退出码 0 才读最终结果；Codex 优先取 -o 文件。
    match engine.final_from_files(&spec).or(final_text) {
        Some(t) => Outcome::Replied(redact(&t)),
        None => Outcome::Failed("Agent 正常退出但没有输出最终结果".into()),
    }
}

#[cfg(test)]
pub(crate) mod fake_agent;
#[cfg(test)]
mod tests;
