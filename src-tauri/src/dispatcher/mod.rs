//! 消息分发：过滤、去重、续聊判断、Agent 不可用提示节流。排队见 `queue`，群内文案见 `copy`，PR 链接识别见 `pr_link`。

pub mod copy;
pub mod pr_link;
pub mod queue;

use pr_link::{PrDedupe, PrRef};

use crate::feishu::transport::IncomingMessage;
use crate::runner::Outcome;
use crate::store::config::{Bot, Engine, Trigger};
use crate::store::runs::RunStore;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// 运行记录 id，同时是队列取消的键（decide 生成）。
    pub run_id: String,
    pub bot_id: String,
    pub msg: IncomingMessage,
    pub resume: Option<String>,
    pub needs_onboard: bool,
    /// 群里贴 PR 链接触发时的那个 PR（一条消息多个链接各一个任务）；@ 触发为 None。
    pub pr: Option<PrRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Run(Job),
    ReplyOnly(String),
    Ignore(&'static str),
    /// 这个 PR 窗口期内已经审过 / 正在审（`run_id` 是那次运行），只回一句不重复审。
    RecentPr { url: String, run_id: String },
}

/// 最近 N 个 message_id 的去重集合（先进先出淘汰）。
pub struct Dedupe {
    cap: usize,
    set: HashSet<String>,
    order: VecDeque<String>,
}

impl Dedupe {
    pub fn new(cap: usize) -> Self {
        Dedupe { cap: cap.max(1), set: HashSet::new(), order: VecDeque::new() }
    }

    /// 见过返回 true；没见过则记下并返回 false。
    pub fn seen(&mut self, id: &str) -> bool {
        if self.set.contains(id) {
            return true;
        }
        if self.order.len() >= self.cap {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        self.set.insert(id.to_string());
        self.order.push_back(id.to_string());
        false
    }
}

/// 发送人是不是机器人主人：只比 union_id（open_id 按应用区分，机器人事件与 lark-cli 的永远不等）。
/// 任一方 union_id 未知都按「不是主人」处理。
pub fn is_owner(msg: &IncomingMessage, owner_union_id: Option<&str>) -> bool {
    matches!((owner_union_id, msg.sender_union_id.as_deref()), (Some(o), Some(s)) if !o.is_empty() && o == s)
}

/// 只有一个结果的旧入口（@ 触发的测试和调用沿用）：取 `decide_all` 的第一个结果，PR 去重用一次性的集合。
pub fn decide(
    msg: &IncomingMessage,
    bot: &Bot,
    owner_union_id: Option<&str>,
    seen: &mut Dedupe,
) -> Decision {
    decide_all(msg, bot, owner_union_id, seen, &mut PrDedupe::new())
        .into_iter()
        .next()
        .unwrap_or(Decision::Ignore("no-mention"))
}

/// 纯函数：只看消息、机器人配置和去重集合，不做 IO。`resume` 由 `attach_resume` 另行补上。
///
/// 触发方式（`bot.trigger_mode`）：
/// - 认 @ 时，群里 @ 机器人照旧触发一次；
/// - 认 PR 链接时，消息里每个 Gitee / GitHub PR 链接各触发一次（不用 @），同一 PR 窗口期内只审一次（回 `RecentPr`）；
/// - 只认 PR 链接却被 @ 了、又没带链接：回一句用法提示。
///
/// 群必须已登记（只有机器人主人 @ 机器人才会自动接入新群）；「谁能触发」照群配置。没 @ 的 PR 链接出现在没登记的群里一律不理，
/// 免得机器人在任何群里看到链接就回「本群还没有接入」。
pub fn decide_all(
    msg: &IncomingMessage,
    bot: &Bot,
    owner_union_id: Option<&str>,
    seen: &mut Dedupe,
    prs: &mut PrDedupe,
) -> Vec<Decision> {
    if seen.seen(&msg.message_id) {
        return vec![Decision::Ignore("duplicate")];
    }
    let mode = bot.trigger_mode;
    let links = if mode.accepts_pr_link() { pr_link::find_pr_links(&msg.text) } else { Vec::new() };
    let mentioned = msg.mentions_bot && mode.accepts_mention();
    // 只认 PR 链接的机器人被 @ 了但没带链接：不运行，回一句用法提示
    let hint_only = msg.is_group && links.is_empty() && !mentioned && msg.mentions_bot;
    if msg.is_group && links.is_empty() && !mentioned && !hint_only {
        return vec![Decision::Ignore("no-mention")];
    }
    let is_owner = is_owner(msg, owner_union_id);
    if !msg.is_group && !is_owner {
        return vec![Decision::Ignore("p2p-not-owner")];
    }
    let jobs = |needs_onboard: bool, prs: &mut PrDedupe| -> Vec<Decision> {
        let job = |pr: Option<PrRef>| Job {
            run_id: uuid::Uuid::new_v4().to_string(),
            bot_id: bot.id.clone(),
            msg: msg.clone(),
            resume: None,
            needs_onboard,
            pr,
        };
        if links.is_empty() {
            return vec![Decision::Run(job(None))];
        }
        links
            .iter()
            .map(|pr| match prs.recent(&pr.key()) {
                Some(run_id) => Decision::RecentPr { url: pr.url(), run_id },
                None => {
                    let j = job(Some(pr.clone()));
                    prs.record(&pr.key(), &j.run_id);
                    Decision::Run(j)
                }
            })
            .collect()
    };
    // 私聊只服务主人自测：不看群登记和触发范围，也不登记成群
    if !msg.is_group {
        return jobs(false, prs);
    }
    match bot.groups.iter().find(|g| g.chat_id == msg.chat_id) {
        // 只有机器人主人 @ 机器人才能让它接入新群：否则任何人拉群 @ 一次就能在主人电脑上跑 Agent。
        None if hint_only => vec![Decision::ReplyOnly(copy::GROUP_NOT_ENABLED.to_string())],
        None if bot.auto_onboard && is_owner && msg.mentions_bot => jobs(true, prs),
        None if msg.mentions_bot => vec![Decision::ReplyOnly(copy::GROUP_NOT_ENABLED.to_string())],
        None => vec![Decision::Ignore("group-not-registered")],
        Some(g) if g.trigger == Trigger::Owner && !is_owner => vec![Decision::Ignore("not-allowed")],
        Some(_) if hint_only => vec![Decision::ReplyOnly(copy::PR_LINK_HINT.to_string())],
        Some(_) => jobs(false, prs),
    }
}

/// 用户回复的是这个机器人之前的某条回复时续聊，但必须同时满足：
/// - 该会话用的引擎与机器人当前引擎一致（引擎换过则开新会话）；
/// - 本次发送人的 union_id 与当时触发人的 union_id 相同（任一方未知都不续）。否则开新会话：
///   会话上下文里可能有当时触发人才能看的内容（例如主人让机器人读过的私聊），别人回复机器人消息不能把它续出来。
///
/// 查库失败按没有处理。
pub fn attach_resume(mut job: Job, bot: &Bot, store: &RunStore) -> Job {
    let Some(parent) = job.msg.parent_id.as_deref() else { return job };
    let Ok(Some(prev)) = store.session_for_reply(&bot.id, &job.msg.chat_id, parent) else { return job };
    let same_sender = matches!(
        (prev.sender_union_id.as_deref(), job.msg.sender_union_id.as_deref()),
        (Some(a), Some(b)) if !a.is_empty() && a == b
    );
    if prev.engine == bot.engine && same_sender {
        job.resume = Some(prev.session_id);
    }
    job
}

/// 「Agent 不可用」提示节流：同一机器人窗口内只提示一次，之后同类失败只回 FAILED。
pub struct AgentNoticeThrottle {
    window: Duration,
    clock: Box<dyn Fn() -> Instant + Send + Sync>,
    last: HashMap<String, Instant>,
}

impl AgentNoticeThrottle {
    pub fn new() -> Self {
        Self::with_clock(Duration::from_secs(600), Box::new(Instant::now))
    }

    pub fn with_clock(window: Duration, clock: Box<dyn Fn() -> Instant + Send + Sync>) -> Self {
        AgentNoticeThrottle { window, clock, last: HashMap::new() }
    }

    /// 群里该发的文本；`AgentUnavailable` 在窗口内第二次起降级为 FAILED。
    pub fn reply(
        &mut self,
        bot_id: &str,
        outcome: &Outcome,
        engine: Engine,
        timeout_minutes: u32,
    ) -> String {
        if matches!(outcome, Outcome::AgentUnavailable) {
            let now = (self.clock)();
            let recent = self
                .last
                .get(bot_id)
                .is_some_and(|t| now.saturating_duration_since(*t) < self.window);
            if recent {
                return copy::FAILED.to_string();
            }
            self.last.insert(bot_id.to_string(), now);
        }
        copy::outcome_reply(outcome, engine, timeout_minutes)
    }
}

impl Default for AgentNoticeThrottle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::config::Group;
    use std::sync::{Arc, Mutex};

    fn group(chat: &str, trigger: Trigger) -> Group {
        Group {
            chat_id: chat.into(),
            name: "g".into(),
            slug: "g".into(),
            projects: vec![],
            doc_url: None,
            trigger,
        }
    }

    fn bot(groups: Vec<Group>, auto_onboard: bool) -> Bot {
        let mut b = Bot::new_default("b1", "bot", "cli_x");
        b.groups = groups;
        b.auto_onboard = auto_onboard;
        b
    }

    fn msg(mid: &str, chat: &str, sender: &str) -> IncomingMessage {
        IncomingMessage {
            message_id: mid.into(),
            chat_id: chat.into(),
            sender_open_id: sender.into(),
            // 测试里 union_id 由 open_id 推出：ou_x → on_x（真实场景两者无关）
            sender_union_id: Some(sender.replacen("ou_", "on_", 1)),
            text: "查一下".into(),
            is_group: true,
            mentions_bot: true,
            message_type: "text".into(),
            parent_id: None,
            root_id: None,
            create_time_ms: 0,
        }
    }

    #[test]
    fn dedupes_redelivered_message() {
        let b = bot(vec![group("oc_a", Trigger::All)], false);
        let mut d = Dedupe::new(4096);
        let m = msg("m1", "oc_a", "ou_u");
        assert!(matches!(decide(&m, &b, None, &mut d), Decision::Run(_)));
        assert_eq!(decide(&m, &b, None, &mut d), Decision::Ignore("duplicate"));
        // 不同 message_id 的第二次 @ 照常处理
        assert!(matches!(decide(&msg("m2", "oc_a", "ou_u"), &b, None, &mut d), Decision::Run(_)));
    }

    #[test]
    fn dedupe_evicts_oldest_beyond_capacity() {
        let mut d = Dedupe::new(2);
        assert!(!d.seen("a"));
        assert!(!d.seen("b"));
        assert!(!d.seen("c"));
        assert!(!d.seen("a"), "a 已被淘汰");
        assert!(d.seen("c"));
    }

    #[test]
    fn group_without_mention_is_ignored() {
        let b = bot(vec![group("oc_a", Trigger::All)], false);
        let mut m = msg("m1", "oc_a", "ou_u");
        m.mentions_bot = false;
        assert_eq!(decide(&m, &b, None, &mut Dedupe::new(8)), Decision::Ignore("no-mention"));
    }

    fn p2p_msg(sender: &str) -> IncomingMessage {
        let mut m = msg("m1", "oc_p2p", sender);
        m.is_group = false;
        m.mentions_bot = false;
        m
    }

    #[test]
    fn p2p_from_non_owner_is_ignored() {
        let b = bot(vec![], true);
        let m = p2p_msg("ou_u");
        assert_eq!(
            decide(&m, &b, Some("on_owner"), &mut Dedupe::new(8)),
            Decision::Ignore("p2p-not-owner")
        );
        // 没有主人信息时私聊一律忽略
        assert_eq!(decide(&m, &b, None, &mut Dedupe::new(8)), Decision::Ignore("p2p-not-owner"));
    }

    #[test]
    fn owner_p2p_runs_without_onboard() {
        for auto_onboard in [true, false] {
            let b = bot(vec![], auto_onboard);
            match decide(&p2p_msg("ou_owner"), &b, Some("on_owner"), &mut Dedupe::new(8)) {
                Decision::Run(job) => {
                    assert!(!job.needs_onboard, "auto_onboard={auto_onboard}");
                    assert_eq!(job.msg.chat_id, "oc_p2p");
                    assert!(!job.run_id.is_empty());
                }
                other => panic!("主人私聊应该运行，实际 {other:?}"),
            }
        }
    }

    #[test]
    fn each_run_decision_gets_unique_run_id() {
        let b = bot(vec![group("oc_a", Trigger::All)], false);
        let mut d = Dedupe::new(8);
        let mut id = |mid| match decide(&msg(mid, "oc_a", "ou_u"), &b, None, &mut d) {
            Decision::Run(j) => j.run_id,
            other => panic!("{other:?}"),
        };
        assert_ne!(id("m1"), id("m2"));
    }

    #[test]
    fn owner_only_ignores_other_senders() {
        let b = bot(vec![group("oc_a", Trigger::Owner)], false);
        let mut d = Dedupe::new(8);
        assert_eq!(
            decide(&msg("m1", "oc_a", "ou_other"), &b, Some("on_owner"), &mut d),
            Decision::Ignore("not-allowed")
        );
        assert_eq!(
            decide(&msg("m2", "oc_a", "ou_other"), &b, None, &mut d),
            Decision::Ignore("not-allowed")
        );
        assert!(matches!(
            decide(&msg("m3", "oc_a", "ou_owner"), &b, Some("on_owner"), &mut d),
            Decision::Run(_)
        ));
    }

    #[test]
    fn all_trigger_accepts_any_sender() {
        let b = bot(vec![group("oc_a", Trigger::All)], false);
        let d = decide(&msg("m1", "oc_a", "ou_other"), &b, Some("on_owner"), &mut Dedupe::new(8));
        match d {
            Decision::Run(job) => {
                assert_eq!(job.bot_id, "b1");
                assert!(!job.needs_onboard);
                assert_eq!(job.resume, None);
            }
            other => panic!("应该运行，实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_group_without_auto_onboard_replies_not_enabled() {
        let b = bot(vec![], false);
        let d = decide(&msg("m1", "oc_new", "ou_u"), &b, None, &mut Dedupe::new(8));
        assert_eq!(d, Decision::ReplyOnly("本群还没有接入，请联系机器人主人。".into()));
    }

    #[test]
    fn unknown_group_with_auto_onboard_runs_with_onboard_flag() {
        let b = bot(vec![], true);
        match decide(&msg("m1", "oc_new", "ou_owner"), &b, Some("on_owner"), &mut Dedupe::new(8)) {
            Decision::Run(job) => assert!(job.needs_onboard),
            other => panic!("主人 @ 应该接入并运行，实际 {other:?}"),
        }
    }

    #[test]
    fn non_owner_cannot_trigger_onboard() {
        let b = bot(vec![], true);
        let not_enabled = Decision::ReplyOnly(copy::GROUP_NOT_ENABLED.to_string());
        let mut d = Dedupe::new(8);
        assert_eq!(decide(&msg("m1", "oc_new", "ou_other"), &b, Some("on_owner"), &mut d), not_enabled);
        // 不知道主人是谁时，谁 @ 都不接入
        assert_eq!(decide(&msg("m2", "oc_new", "ou_owner"), &b, None, &mut d), not_enabled);
    }

    #[test]
    fn owner_matched_by_union_id_across_apps() {
        let b = bot(vec![group("oc_a", Trigger::Owner)], true);
        // 机器人事件里的 open_id 属于机器人应用，与 lark-cli 取到的主人 open_id 不同，union_id 相同
        let mut m = msg("m1", "oc_a", "ou_bot_app_view");
        m.sender_union_id = Some("on_owner".into());
        assert!(matches!(decide(&m, &b, Some("on_owner"), &mut Dedupe::new(8)), Decision::Run(_)));
        let mut m2 = msg("m2", "oc_new", "ou_bot_app_view");
        m2.sender_union_id = Some("on_owner".into());
        match decide(&m2, &b, Some("on_owner"), &mut Dedupe::new(8)) {
            Decision::Run(job) => assert!(job.needs_onboard),
            other => panic!("主人 @ 新群应该接入，实际 {other:?}"),
        }
        // open_id 恰好相同但 union_id 不同 / 缺失：不是主人
        let mut m3 = msg("m3", "oc_a", "ou_owner");
        m3.sender_union_id = Some("on_other".into());
        assert_eq!(decide(&m3, &b, Some("on_owner"), &mut Dedupe::new(8)), Decision::Ignore("not-allowed"));
        let mut m4 = msg("m4", "oc_a", "ou_owner");
        m4.sender_union_id = None;
        assert_eq!(decide(&m4, &b, Some("on_owner"), &mut Dedupe::new(8)), Decision::Ignore("not-allowed"));
    }

    fn store_with_reply(engine: Engine) -> (tempfile::TempDir, RunStore) {
        use crate::store::runs::{RunRecord, RunStatus};
        let dir = tempfile::tempdir().unwrap();
        let store = RunStore::open(dir.path().join("runs.db")).unwrap();
        let r = RunRecord {
            id: "r1".into(),
            bot_id: "b1".into(),
            chat_id: "oc_a".into(),
            group_name: "g".into(),
            trigger_message_id: "om_t".into(),
            sender_name: "张三".into(),
            text: "查".into(),
            engine,
            engine_version: None,
            session_id: Some("s1".into()),
            status: RunStatus::Running,
            started_at: 100,
            finished_at: None,
            final_reply: None,
            error: None,
            sql_count: 0,
            reply_message_id: Some("om_reply".into()),
            // 原会话由 ou_u（union on_u）触发
            sender_union_id: Some("on_u".into()),
        };
        store.insert_run(&r).unwrap();
        (dir, store)
    }

    fn job_replying_to(parent: Option<&str>) -> Job {
        let mut m = msg("m9", "oc_a", "ou_u");
        m.parent_id = parent.map(String::from);
        Job { run_id: "r9".into(), bot_id: "b1".into(), msg: m, resume: None, needs_onboard: false, pr: None }
    }

    #[test]
    fn reply_to_bot_message_resumes_session() {
        let (_d, store) = store_with_reply(Engine::Claude);
        let b = bot(vec![], false); // 默认引擎 Claude
        let j = attach_resume(job_replying_to(Some("om_reply")), &b, &store);
        assert_eq!(j.resume.as_deref(), Some("s1"));
        // 引用的不是机器人的回复 / 没有引用：不续聊
        assert_eq!(attach_resume(job_replying_to(Some("om_other")), &b, &store).resume, None);
        assert_eq!(attach_resume(job_replying_to(None), &b, &store).resume, None);
    }

    /// 审查 S1：别人回复机器人给主人的那条消息，不能续上主人的会话（里面可能有主人的私聊内容）。
    #[test]
    fn reply_from_other_sender_starts_fresh_session() {
        let (_d, store) = store_with_reply(Engine::Claude);
        let b = bot(vec![], false);
        let mut j = job_replying_to(Some("om_reply"));
        j.msg.sender_union_id = Some("on_other".into());
        assert_eq!(attach_resume(j, &b, &store).resume, None);
        // 发送人 union_id 未知：不续
        let mut j = job_replying_to(Some("om_reply"));
        j.msg.sender_union_id = None;
        assert_eq!(attach_resume(j, &b, &store).resume, None);
        // 另一个机器人（同一个群）回复也不续
        let mut other_bot = bot(vec![], false);
        other_bot.id = "b2".into();
        assert_eq!(attach_resume(job_replying_to(Some("om_reply")), &other_bot, &store).resume, None);
    }

    #[test]
    fn old_run_without_sender_union_id_never_resumes() {
        let (_d, store) = store_with_reply(Engine::Claude);
        let (mut r, _) = store.get_run("r1").unwrap().unwrap();
        r.sender_union_id = None;
        store.update_run(&r).unwrap();
        let j = attach_resume(job_replying_to(Some("om_reply")), &bot(vec![], false), &store);
        assert_eq!(j.resume, None);
    }

    #[test]
    fn reply_after_engine_switch_starts_fresh_session() {
        let (_d, store) = store_with_reply(Engine::Claude);
        let mut b = bot(vec![], false);
        b.engine = Engine::Codex;
        let j = attach_resume(job_replying_to(Some("om_reply")), &b, &store);
        assert_eq!(j.resume, None);
    }

    fn pr_bot(mode: crate::store::config::TriggerMode, trigger: Trigger) -> Bot {
        let mut b = bot(vec![group("oc_a", trigger)], false);
        b.trigger_mode = mode;
        b
    }

    fn pr_msg(mid: &str, sender: &str, text: &str, mention: bool) -> IncomingMessage {
        let mut m = msg(mid, "oc_a", sender);
        m.text = text.into();
        m.mentions_bot = mention;
        m
    }

    fn runs(ds: &[Decision]) -> Vec<Option<String>> {
        ds.iter()
            .filter_map(|d| match d {
                Decision::Run(j) => Some(j.pr.as_ref().map(PrRef::url)),
                _ => None,
            })
            .collect()
    }

    const PR1: &str = "https://gitee.com/example/demo-service/pulls/29";
    const PR2: &str = "https://github.com/example/demo/pull/7";

    #[test]
    fn trigger_mode_mention_ignores_unmentioned_pr_links() {
        use crate::store::config::TriggerMode;
        let b = pr_bot(TriggerMode::Mention, Trigger::All);
        let mut d = Dedupe::new(8);
        let mut prs = PrDedupe::new();
        let text = format!("帮忙看下 {PR1}");
        assert_eq!(decide_all(&pr_msg("m1", "ou_u", &text, false), &b, None, &mut d, &mut prs), vec![Decision::Ignore("no-mention")]);
        // @ 了：照旧一次运行，不按链接拆
        assert_eq!(runs(&decide_all(&pr_msg("m2", "ou_u", &text, true), &b, None, &mut d, &mut prs)), vec![None]);
    }

    #[test]
    fn trigger_mode_pr_link_runs_once_per_link_without_mention() {
        use crate::store::config::TriggerMode;
        let b = pr_bot(TriggerMode::PrLink, Trigger::All);
        let mut d = Dedupe::new(8);
        let mut prs = PrDedupe::new();
        let text = format!("两个 PR {PR1} 和 {PR2}?x=1，再贴一次 {PR1}/");
        let ds = decide_all(&pr_msg("m1", "ou_u", &text, false), &b, None, &mut d, &mut prs);
        assert_eq!(runs(&ds), vec![Some(PR1.to_string()), Some(PR2.to_string())]);
        // 没有 PR 链接、也没 @：不理
        let none = decide_all(&pr_msg("m2", "ou_u", "随便聊聊 https://gitee.com/example/demo-service", false), &b, None, &mut d, &mut prs);
        assert_eq!(none, vec![Decision::Ignore("no-mention")]);
        // 只认链接的机器人被 @ 但没带链接：回用法提示，不运行
        let hint = decide_all(&pr_msg("m3", "ou_u", "review 一下", true), &b, None, &mut d, &mut prs);
        assert_eq!(hint, vec![Decision::ReplyOnly(copy::PR_LINK_HINT.to_string())]);
    }

    #[test]
    fn trigger_mode_both_accepts_mention_and_links() {
        use crate::store::config::TriggerMode;
        let b = pr_bot(TriggerMode::Both, Trigger::All);
        let mut d = Dedupe::new(8);
        let mut prs = PrDedupe::new();
        assert_eq!(runs(&decide_all(&pr_msg("m1", "ou_u", "查一下", true), &b, None, &mut d, &mut prs)), vec![None]);
        assert_eq!(runs(&decide_all(&pr_msg("m2", "ou_u", PR2, false), &b, None, &mut d, &mut prs)), vec![Some(PR2.to_string())]);
        assert_eq!(runs(&decide_all(&pr_msg("m3", "ou_u", PR1, true), &b, None, &mut d, &mut prs)), vec![Some(PR1.to_string())]);
    }

    #[test]
    fn pr_link_respects_group_registration_and_trigger_scope() {
        use crate::store::config::TriggerMode;
        let mut d = Dedupe::new(8);
        let mut prs = PrDedupe::new();
        // 仅主人可触发：别人贴链接不理，主人贴照常审
        let b = pr_bot(TriggerMode::PrLink, Trigger::Owner);
        let other = decide_all(&pr_msg("m1", "ou_other", PR1, false), &b, Some("on_owner"), &mut d, &mut prs);
        assert_eq!(other, vec![Decision::Ignore("not-allowed")]);
        let owner = decide_all(&pr_msg("m2", "ou_owner", PR1, false), &b, Some("on_owner"), &mut d, &mut prs);
        assert_eq!(runs(&owner), vec![Some(PR1.to_string())]);
        // 没登记的群：不 @ 的链接不理（也不回「未接入」），@ 了才回未接入；主人 @ 且允许自动接入才接入
        let mut b2 = Bot::new_default("b1", "bot", "cli_x");
        b2.trigger_mode = TriggerMode::PrLink;
        b2.auto_onboard = true;
        let mut m = pr_msg("m3", "ou_owner", PR2, false);
        m.chat_id = "oc_new".into();
        assert_eq!(decide_all(&m, &b2, Some("on_owner"), &mut d, &mut prs), vec![Decision::Ignore("group-not-registered")]);
        let mut m = pr_msg("m4", "ou_other", PR2, true);
        m.chat_id = "oc_new".into();
        assert_eq!(decide_all(&m, &b2, Some("on_owner"), &mut d, &mut prs), vec![Decision::ReplyOnly(copy::GROUP_NOT_ENABLED.to_string())]);
        let mut m = pr_msg("m5", "ou_owner", PR2, true);
        m.chat_id = "oc_new".into();
        match decide_all(&m, &b2, Some("on_owner"), &mut d, &mut prs).as_slice() {
            [Decision::Run(j)] => assert!(j.needs_onboard && j.pr.is_some()),
            other => panic!("主人 @ 带链接应接入并审查，实际 {other:?}"),
        }
    }

    #[test]
    fn same_pr_within_window_is_not_reviewed_twice() {
        use crate::store::config::TriggerMode;
        let b = pr_bot(TriggerMode::PrLink, Trigger::All);
        let mut d = Dedupe::new(8);
        let mut prs = PrDedupe::new();
        let first = decide_all(&pr_msg("m1", "ou_u", PR1, false), &b, None, &mut d, &mut prs);
        let Decision::Run(job) = &first[0] else { panic!("{first:?}") };
        // 换个写法再贴（带锚点、大小写不同）：不重复审，指向第一次的运行
        let again = decide_all(&pr_msg("m2", "ou_u", "https://gitee.com/Example/demo-service/pulls/29#x", false), &b, None, &mut d, &mut prs);
        let want = Decision::RecentPr { url: "https://gitee.com/Example/demo-service/pulls/29".into(), run_id: job.run_id.clone() };
        assert_eq!(again, vec![want]);
        // 同一条消息里一新一旧：新的照常审
        let mixed = decide_all(&pr_msg("m3", "ou_u", &format!("{PR1} {PR2}"), false), &b, None, &mut d, &mut prs);
        assert!(matches!(mixed[0], Decision::RecentPr { .. }));
        assert_eq!(runs(&mixed), vec![Some(PR2.to_string())]);
        // 那次运行失败放开后可以马上重审
        prs.release(&job.pr.as_ref().unwrap().key(), &job.run_id);
        assert_eq!(runs(&decide_all(&pr_msg("m4", "ou_u", PR1, false), &b, None, &mut d, &mut prs)), vec![Some(PR1.to_string())]);
    }

    #[test]
    fn owner_p2p_pr_links_split_into_jobs() {
        use crate::store::config::TriggerMode;
        let mut b = bot(vec![], false);
        b.trigger_mode = TriggerMode::PrLink;
        let mut m = p2p_msg("ou_owner");
        m.text = format!("{PR1} {PR2}");
        let ds = decide_all(&m, &b, Some("on_owner"), &mut Dedupe::new(8), &mut PrDedupe::new());
        assert_eq!(runs(&ds), vec![Some(PR1.to_string()), Some(PR2.to_string())]);
        // 私聊没带链接：照常运行一次（模板里会请对方发链接）
        let ds = decide_all(&p2p_msg("ou_owner"), &b, Some("on_owner"), &mut Dedupe::new(8), &mut PrDedupe::new());
        assert_eq!(runs(&ds), vec![None]);
    }

    #[test]
    fn agent_unavailable_notice_throttled_10_minutes() {
        let base = Instant::now();
        let offset = Arc::new(Mutex::new(Duration::ZERO));
        let o = offset.clone();
        let mut t = AgentNoticeThrottle::with_clock(
            Duration::from_secs(600),
            Box::new(move || base + *o.lock().unwrap()),
        );
        let un = Outcome::AgentUnavailable;
        let first = "本机 Codex 不可用，请机器人主人在客户端里检查环境。";
        assert_eq!(t.reply("b1", &un, Engine::Codex, 30), first);
        *offset.lock().unwrap() = Duration::from_secs(599);
        assert_eq!(t.reply("b1", &un, Engine::Codex, 30), copy::FAILED);
        // 别的机器人不受影响；其它结果不受节流
        assert_eq!(t.reply("b2", &un, Engine::Codex, 30), first);
        assert_eq!(t.reply("b1", &Outcome::Cancelled, Engine::Codex, 30), copy::CANCELLED);
        // 超过 10 分钟后再提示一次
        *offset.lock().unwrap() = Duration::from_secs(601);
        assert_eq!(t.reply("b1", &un, Engine::Codex, 30), first);
    }
}
