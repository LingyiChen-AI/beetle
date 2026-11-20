// 工作台：机器人头部（名称、状态、App ID、引擎、群数、长连接、随客户端启动、启动 / 停止）+ tab。
import {
  ArrowRight,
  CircleAlert,
  LayoutDashboard,
  LoaderCircle,
  Lock,
  Play,
  Puzzle,
  ScrollText,
  SlidersHorizontal,
  Square,
  TriangleAlert,
  Users,
  type LucideIcon,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  errText,
  listProjects,
  listRuns,
  listSkills,
  onRunUpdated,
  subscribe,
  type BotView,
  type EnvItem,
  type ProjectsView,
  type RunRecord,
  type SkillInfo,
} from "../api";
import { hm, isToday } from "../format";
import { upsertRun } from "../runs/merge";
import { avatarOf, BotIcon, engineText } from "./BotList";
import { BotChip, Switch, type BotKind } from "./common";
import ConfigTab from "./tabs/ConfigTab";
import GroupsTab from "./tabs/GroupsTab";
import LogsTab from "./tabs/LogsTab";
import OverviewTab from "./tabs/OverviewTab";
import SkillsTab from "./tabs/SkillsTab";

export type Tab = "overview" | "groups" | "skills" | "logs" | "config";
export const RUNS_LIMIT = 200;

export interface WorkbenchProps {
  view: BotView;
  index: number;
  kind: BotKind;
  runningSince: number | null;
  envErrors: EnvItem[];
  tab: Tab;
  runId: string | null;
  onTab: (t: Tab) => void;
  onOpenRun: (id: string) => void;
  onStart: () => void;
  onStop: () => void;
  onToggleAutostart: (on: boolean) => void;
  onGoEnv: () => void;
  onGoProjects: () => void;
  onGoSkills: () => void;
  onChanged: () => Promise<void> | void;
  onDeleted: () => void;
}

export default function Workbench(p: WorkbenchProps) {
  const { bot } = p.view;
  const [runs, setRuns] = useState<RunRecord[]>([]);
  const [runsError, setRunsError] = useState<string | null>(null);
  const [projects, setProjects] = useState<ProjectsView | null>(null);
  const [skills, setSkills] = useState<SkillInfo[]>([]);

  const reloadRuns = useCallback(async () => {
    try {
      setRuns(await listRuns(bot.id, RUNS_LIMIT));
      setRunsError(null);
    } catch (e) {
      setRunsError(errText(e));
    }
  }, [bot.id]);

  useEffect(() => {
    setRuns([]);
    void reloadRuns();
    return subscribe(
      onRunUpdated((run) => {
        if (run.bot_id === bot.id) setRuns((prev) => upsertRun(prev, run, RUNS_LIMIT));
      }),
    );
  }, [bot.id, reloadRuns]);

  useEffect(() => {
    listProjects().then(setProjects).catch(() => {});
    listSkills().then(setSkills).catch(() => {});
  }, []);

  const todayRuns = useMemo(() => runs.filter((r) => isToday(r.started_at)), [runs]);
  const av = avatarOf(p.index, bot);
  const blocked = p.envErrors.length > 0;
  const running = p.kind === "running";
  // 「异常：连接失败，正在重试」时连接任务还在：给「停止」（再点启动由后端立即重连）
  const retrying = p.kind === "error" && p.view.alive === true;
  const busy = p.kind === "connecting" || p.kind === "stopping";

  const bigBtn = running || retrying ? (
    <button type="button" className="btn btn-danger big-btn tr" onClick={p.onStop}>
      <Square />
      停止
    </button>
  ) : busy ? (
    <button type="button" className="btn big-btn" disabled>
      <LoaderCircle className="spin" />
      {p.kind === "connecting" ? "连接中…" : "停止中…"}
    </button>
  ) : blocked ? (
    <button type="button" className="btn big-btn btn-locked" disabled title="环境检查未通过，先去修复">
      <Lock />
      无法启动
    </button>
  ) : (
    <button type="button" className="btn btn-pri big-btn tr" onClick={p.onStart}>
      <Play />
      启动
    </button>
  );

  const tabs: { id: Tab; label: string; Icon: LucideIcon; count?: number }[] = [
    { id: "overview", label: "概览", Icon: LayoutDashboard },
    { id: "groups", label: "群", Icon: Users, count: bot.groups.length },
    { id: "skills", label: "技能与模板", Icon: Puzzle },
    { id: "logs", label: "运行日志", Icon: ScrollText, count: todayRuns.length },
    { id: "config", label: "设置", Icon: SlidersHorizontal },
  ];

  return (
    <>
      <div className="flex flex-none items-center" style={{ padding: "16px 24px 14px", gap: 14 }}>
        <div
          className={`avatar ${running ? "glow-ok" : ""}`}
          style={{ width: 46, height: 46, borderRadius: 12, background: av.bg, color: av.ink }}
        >
          <BotIcon bot={bot} size={23} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center" style={{ gap: 10 }}>
            <span className="truncate" style={{ fontSize: 18, fontWeight: 700 }}>
              {bot.name}
            </span>
            <BotChip kind={p.kind} />
          </div>
          <div className="flex flex-wrap" style={{ color: "var(--muted)", fontSize: 12, marginTop: 4, gap: 12 }}>
            <span className="mono">{bot.app_id || "未填写 App ID"}</span>
            <span>引擎 {engineText(bot.engine)}</span>
            <span>{bot.groups.length} 个群</span>
            {running && (
              <span style={{ color: "var(--success-fg)" }}>
                长连接已建立{p.runningSince ? ` · 自 ${hm(p.runningSince)} 起` : ""}
              </span>
            )}
            {p.kind === "connecting" && <span style={{ color: "var(--warning-fg)" }}>正在建立长连接…</span>}
          </div>
        </div>
        <label className="flex items-center gap-2" style={{ color: "var(--muted)", fontSize: 12, cursor: "pointer" }}>
          随客户端启动
          <Switch on={bot.start_with_client} label="随客户端启动" onChange={p.onToggleAutostart} />
        </label>
        {bigBtn}
      </div>

      {p.view.state.kind === "error" && (
        <div className="alert-err" role="alert" style={{ margin: "0 24px 12px" }}>
          <CircleAlert style={{ width: 16, height: 16 }} />
          <span className="flex-1">运行异常：{p.view.state.message}</span>
          {retrying && (
            <button type="button" className="btn btn-sm tr" onClick={p.onStart}>
              立即重连
            </button>
          )}
        </div>
      )}
      {blocked && !running && !retrying && (
        <div className="alert-err" role="alert" style={{ margin: "0 24px 12px" }}>
          <TriangleAlert style={{ width: 16, height: 16 }} />
          <span className="flex-1">
            环境检查有 {p.envErrors.length} 项未通过（{p.envErrors.map((e) => e.label).join("、")}），机器人无法启动。
          </span>
          <button type="button" className="btn btn-sm tr" onClick={p.onGoEnv}>
            去修复 <ArrowRight />
          </button>
        </div>
      )}

      <div className="tabs" role="tablist">
        {tabs.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={p.tab === t.id}
            className={`tab tr ${p.tab === t.id ? "on" : ""}`}
            onClick={() => p.onTab(t.id)}
          >
            <t.Icon />
            {t.label}
            {t.count !== undefined && <span className="cnt">{t.count}</span>}
          </button>
        ))}
      </div>

      <div className="flex min-h-0 flex-1 flex-col">
        {runsError && p.tab !== "config" && (
          <div className="alert-err" role="alert" style={{ margin: "12px 24px 0" }}>
            <CircleAlert style={{ width: 16, height: 16 }} />
            读取运行记录失败：{runsError}
          </div>
        )}
        {p.tab === "overview" && (
          <OverviewTab
            view={p.view}
            runs={runs}
            skills={skills}
            onTab={p.onTab}
            onOpenRun={p.onOpenRun}
          />
        )}
        {p.tab === "groups" && (
          <GroupsTab view={p.view} runs={runs} projects={projects} onChanged={p.onChanged} onGoProjects={p.onGoProjects} />
        )}
        {p.tab === "skills" && <SkillsTab view={p.view} onChanged={p.onChanged} />}
        {p.tab === "logs" && (
          <LogsTab view={p.view} runs={runs} runId={p.runId} onOpenRun={p.onOpenRun} onGoEnv={p.onGoEnv} onReload={reloadRuns} />
        )}
        {p.tab === "config" && <ConfigTab view={p.view} onChanged={p.onChanged} onDeleted={p.onDeleted} />}
      </div>
    </>
  );
}
