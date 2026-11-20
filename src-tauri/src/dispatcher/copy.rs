//! 群内固定文案。群里只发最终结果或下面这几条之一。

use crate::runner::Outcome;
use crate::store::config::Engine;

pub const AGENT_UNAVAILABLE: &str = "本机 {engine} 不可用，请机器人主人在客户端里检查环境。";
pub const TIMEOUT: &str = "这次排查超过 {minutes} 分钟还没完成，已停止。可以稍后再 @ 我重试。";
pub const FAILED: &str = "这次排查没有完成，机器人主人可以在客户端「运行日志」里查看原因。";
pub const CANCELLED: &str = "这次排查已被机器人主人取消。";
pub const GROUP_NOT_ENABLED: &str = "本群还没有接入，请联系机器人主人。";
/// 只认 PR 链接的机器人被 @ 了但消息里没有 PR 链接。
pub const PR_LINK_HINT: &str = "把 Gitee / GitHub 的 PR 链接发到群里，我会自动审查，不用 @ 我。";
pub const PR_REVIEWING: &str = "这个 PR 正在审查，结果会回复在之前那条消息下：{url}";
pub const PR_RECENTLY_REVIEWED: &str = "这个 PR 刚审过（10 分钟内不重复审查），结果见之前的回复：{url}";

/// 同一 PR 窗口期内再贴时的回复：那次运行还没结束说「正在审」，结束了说「刚审过」。
pub fn pr_recent_reply(url: &str, running: bool) -> String {
    let t = if running { PR_REVIEWING } else { PR_RECENTLY_REVIEWED };
    t.replace("{url}", url)
}

pub fn engine_display(engine: Engine) -> &'static str {
    match engine {
        Engine::Claude => "Claude Code",
        Engine::Codex => "Codex",
    }
}

/// 把运行结果换成群里要发的文本；`Replied` 原样返回，失败原因不进群。
pub fn outcome_reply(outcome: &Outcome, engine: Engine, timeout_minutes: u32) -> String {
    match outcome {
        Outcome::Replied(text) => text.clone(),
        Outcome::Failed(_) => FAILED.to_string(),
        Outcome::Timeout => TIMEOUT.replace("{minutes}", &timeout_minutes.to_string()),
        Outcome::Cancelled => CANCELLED.to_string(),
        Outcome::AgentUnavailable => {
            AGENT_UNAVAILABLE.replace("{engine}", engine_display(engine))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_reply_copy_is_exact() {
        assert_eq!(
            outcome_reply(&Outcome::Timeout, Engine::Claude, 30),
            "这次排查超过 30 分钟还没完成，已停止。可以稍后再 @ 我重试。"
        );
        assert_eq!(
            outcome_reply(&Outcome::AgentUnavailable, Engine::Codex, 30),
            "本机 Codex 不可用，请机器人主人在客户端里检查环境。"
        );
        assert_eq!(
            outcome_reply(&Outcome::AgentUnavailable, Engine::Claude, 30),
            "本机 Claude Code 不可用，请机器人主人在客户端里检查环境。"
        );
        assert_eq!(
            outcome_reply(&Outcome::Failed("堆栈 xxx".into()), Engine::Claude, 30),
            "这次排查没有完成，机器人主人可以在客户端「运行日志」里查看原因。"
        );
        assert_eq!(
            outcome_reply(&Outcome::Cancelled, Engine::Claude, 30),
            "这次排查已被机器人主人取消。"
        );
        assert_eq!(GROUP_NOT_ENABLED, "本群还没有接入，请联系机器人主人。");
    }

    #[test]
    fn pr_recent_reply_distinguishes_running_and_done() {
        let u = "https://gitee.com/example/demo-service/pulls/29";
        assert_eq!(pr_recent_reply(u, true), format!("这个 PR 正在审查，结果会回复在之前那条消息下：{u}"));
        assert_eq!(pr_recent_reply(u, false), format!("这个 PR 刚审过（10 分钟内不重复审查），结果见之前的回复：{u}"));
    }

    #[test]
    fn replied_text_is_returned_verbatim() {
        assert_eq!(
            outcome_reply(&Outcome::Replied("结论：A".into()), Engine::Codex, 30),
            "结论：A"
        );
    }
}
