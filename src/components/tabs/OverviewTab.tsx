// 概览：今日运行 / 成功率 / 平均耗时 + 所在群 + 最近运行 + 模板技能摘要。
import { Activity, ChevronRight, CircleCheck, Gauge, LayoutTemplate, Puzzle, ScrollText, Timer, Users } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { botStats, type BotStats, type BotView, type RunRecord, type SkillInfo } from "../../api";
import { fmtDur, hm, isToday, relTime, startOfDay } from "../../format";
import { isTerminal } from "../../runs/merge";
import { useDayStart } from "../../hooks/useDayStart";
import { RunChip } from "../common";
import { EmptyState, Truncate } from "../ui";
import type { Tab } from "../Workbench";
import { TRIGGER_LABEL } from "./GroupsTab";

export const TEMPLATE_NAMES: Record<string, string> = { "bug-hunt": "查 bug", qa: "需求答疑", "code-review": "代码审查" };
export const templateName = (t: string) => TEMPLATE_NAMES[t] ?? t;

/** 按运行记录统计（bot_stats 读不到时的兜底，也用于副标题明细）。 */
export function statsFromRuns(runs: RunRecord[], now = Date.now()) {
  const today = runs.filter((r) => isToday(r.started_at, now));
  const yStart = startOfDay(now) - 86_400_000;
  const yesterday = runs.filter((r) => r.started_at >= yStart && r.started_at < startOfDay(now)).length;
  const done = today.filter((r) => isTerminal(r.status));
  const count = (s: RunRecord["status"]) => today.filter((r) => r.status === s).length;
  const replied = today.filter((r) => r.status === "replied" && r.finished_at != null);
  const avg = replied.length
    ? replied.reduce((a, r) => a + ((r.finished_at as number) - r.started_at), 0) / replied.length
    : null;
  return {
    today: today.length,
    active: today.filter((r) => r.status === "running" || r.status === "queued").length,
    yesterday,
    done: done.length,
    replied: count("replied"),
    failed: count("failed"),
    timeout: count("timeout"),
    cancelled: count("cancelled"),
    rate: done.length ? count("replied") / done.length : null,
    avg,
  };
}

export default function OverviewTab({
  view,
  runs,
  skills,
  onTab,
  onOpenRun,
}: {
  view: BotView;
  runs: RunRecord[];
  skills: SkillInfo[];
  onTab: (t: Tab) => void;
  onOpenRun: (id: string) => void;
}) {
  const { bot } = view;
  const dayStart = useDayStart();
  const local = useMemo(() => statsFromRuns(runs, dayStart), [runs, dayStart]);
  const [remote, setRemote] = useState<BotStats | null>(null);
  const statusKey = runs.map((r) => r.status).join();

  useEffect(() => {
    let alive = true;
    botStats(bot.id)
      .then((s) => alive && setRemote(s))
      .catch(() => alive && setRemote(null));
    return () => {
      alive = false;
    };
  }, [bot.id, statusKey, dayStart]);

  const todayN = remote?.today_runs ?? local.today;
  const rate = remote ? (remote.today_runs ? remote.success_rate : null) : local.rate;
  const avg = remote ? (remote.today_replied ? remote.avg_duration_ms : null) : local.avg;
  const recent = runs.slice(0, 4);

  const perGroup = (chatId: string) => {
    const mine = runs.filter((r) => r.chat_id === chatId);
    const lastReply = mine.find((r) => r.status === "replied");
    return { today: mine.filter((r) => isToday(r.started_at)).length, last: lastReply ? relTime(lastReply.finished_at ?? lastReply.started_at) : "—" };
  };
  const skillNames = bot.skills.map((s) => skills.find((x) => x.name === s)?.display_name ?? s);

  const strong = { color: "var(--fg)", fontWeight: 500 } as const;
  return (
    <div className="scroll flex-1" style={{ padding: "18px 24px" }}>
      <div className="grid grid-cols-3" style={{ gap: 12 }}>
        <div className="card kpi">
          <div className="k">
            <Activity />
            今日运行
          </div>
          <div className="v mono" data-testid="kpi-today">
            {todayN}
          </div>
          <div style={{ fontSize: 12, color: "var(--muted)" }}>
            {todayN ? `${local.active} 条运行中 · 昨日 ${local.yesterday}` : "今日暂无运行"}
          </div>
        </div>
        <div className="card kpi">
          <div className="k">
            <CircleCheck />
            成功率
          </div>
          <div className="v mono" data-testid="kpi-rate" style={{ color: rate != null ? "var(--success-fg)" : "var(--dim)" }}>
            {rate != null ? `${Math.round(rate * 100)}%` : "—"}
          </div>
          <div style={{ fontSize: 12, color: "var(--muted)" }}>
            {local.done
              ? `已结束 ${local.done} 条：回复 ${local.replied} · 失败 ${local.failed} · 超时 ${local.timeout}${local.cancelled ? ` · 取消 ${local.cancelled}` : ""}`
              : "—"}
          </div>
        </div>
        <div className="card kpi">
          <div className="k">
            <Timer />
            平均耗时
          </div>
          <div className="v mono" data-testid="kpi-avg">
            {avg != null ? fmtDur(avg) : "—"}
          </div>
          <div style={{ fontSize: 12, color: "var(--muted)" }}>
            {avg != null ? `仅统计已回复 · 超时上限 ${bot.timeout_minutes} 分钟` : "—"}
          </div>
        </div>
      </div>

      <div className="grid" style={{ gridTemplateColumns: "1.35fr 1fr", gap: 12, marginTop: 12 }}>
        <div className="card">
          <div className="card-h">
            <h3>
              <Users />
              所在群
            </h3>
            <button type="button" className="btn btn-ghost btn-sm tr" onClick={() => onTab("groups")}>
              管理 <ChevronRight />
            </button>
          </div>
          {bot.groups.length === 0 ? (
            <EmptyState compact icon={Users} title="还没有群" desc="把机器人拉进飞书群并 @ 它即可自动接入" />
          ) : (
            <div className="t-scroll">
            <table className="t">
              <thead>
                <tr>
                  <th>群名</th>
                  <th>负责项目</th>
                  <th>谁能触发</th>
                  <th style={{ textAlign: "right" }}>今日</th>
                  <th>最后回复</th>
                </tr>
              </thead>
              <tbody>
                {bot.groups.map((g) => {
                  const s = perGroup(g.chat_id);
                  return (
                    <tr key={g.chat_id}>
                      <td style={{ fontWeight: 500, minWidth: 120 }}>{g.name}</td>
                      <td>
                        <div className="t-chips">
                          {g.projects.map((pr) => (
                            <span key={pr} className="chip c-mut mono">
                              {pr}
                            </span>
                          ))}
                        </div>
                      </td>
                      <td className="nw" style={{ color: "var(--muted)" }}>{TRIGGER_LABEL[g.trigger]}</td>
                      <td className="mono nw" style={{ textAlign: "right", color: s.today ? undefined : "var(--dim)" }}>
                        {s.today}
                      </td>
                      <td className="mono nw" style={{ color: "var(--muted)" }}>
                        {s.last}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            </div>
          )}
        </div>
        <div className="card">
          <div className="card-h">
            <h3>
              <ScrollText />
              最近运行
            </h3>
            <button type="button" className="btn btn-ghost btn-sm tr" onClick={() => onTab("logs")}>
              全部日志 <ChevronRight />
            </button>
          </div>
          {recent.length ? (
            recent.map((r) => (
              <button key={r.id} type="button" className="run-item tr" onClick={() => onOpenRun(r.id)}>
                <div className="flex items-center justify-between gap-2">
                  <span className="mono" style={{ color: "var(--muted)" }}>
                    {isToday(r.started_at) ? hm(r.started_at) : relTime(r.started_at)}
                  </span>
                  <RunChip status={r.status} />
                </div>
                <Truncate text={r.text} style={{ marginTop: 4 }} />
              </button>
            ))
          ) : (
            <EmptyState compact icon={ScrollText} title="暂无运行" desc="群里 @ 机器人后，这里会显示最近的运行" />
          )}
        </div>
      </div>

      <div className="card flex flex-wrap" style={{ marginTop: 12, padding: "12px 14px", gap: 28, color: "var(--muted)", fontSize: 12 }}>
        <span className="flex items-center gap-1">
          <LayoutTemplate style={{ width: 14, height: 14 }} /> 流程模板：<b style={strong}>{templateName(bot.template)}</b>
        </span>
        <span className="flex items-center gap-1">
          <Puzzle style={{ width: 14, height: 14 }} /> 技能：<b style={strong}>{skillNames.join("、") || "无"}</b>
        </span>
        <span className="flex items-center gap-1">
          <Gauge style={{ width: 14, height: 14 }} /> 同时运行 <b style={strong}>{bot.concurrency}</b> · 超时{" "}
          <b style={strong}>{bot.timeout_minutes} 分钟</b>
        </span>
      </div>
    </div>
  );
}
