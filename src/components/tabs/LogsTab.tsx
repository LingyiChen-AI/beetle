// 运行日志：左侧运行列表（今天 / 昨天 / 近 7 天 + 状态筛选），右侧运行详情（元信息、终端风格步骤流、原始日志、飞书气泡回复预览）。
import {
  AtSign,
  Bot as BotIco,
  Brain,
  Check,
  CircleStop,
  CircleX,
  Clock,
  Copy,
  Database,
  GitBranch,
  Hash,
  ListFilter,
  LoaderCircle,
  RefreshCw,
  RotateCcw,
  ScrollText,
  Send,
  Stethoscope,
  Terminal,
  Timer,
  TimerOff,
  Wrench,
  X,
} from "lucide-react";
import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  cancelRun,
  errText,
  getRawLog,
  getRun,
  onRunStep,
  retryRun,
  subscribe,
  type BotView,
  type RawLog,
  type RunRecord,
  type RunStatus,
  type StepRecord,
} from "../../api";
import { fmtDur, fmtStepDur, hm, hms, mdhm, relTime, startOfDay } from "../../format";
import { parseReviewReply } from "../../review/card";
import { addStep, isTerminal, mergeRun } from "../../runs/merge";
import BrandLogo from "../BrandLogo";
import ReviewCardPreview from "../ReviewCardPreview";
import { RUN_META, RunChip, useToast } from "../common";
import { EmptyState, Segmented, Tooltip } from "../ui";

type Range = "today" | "yesterday" | "week";
type StatusFilter = "all" | "active" | "replied" | "failed" | "timeout" | "cancelled";

const RANGES: { id: Range; label: string }[] = [
  { id: "today", label: "今天" },
  { id: "yesterday", label: "昨天" },
  { id: "week", label: "近 7 天" },
];
const FILTERS: { id: StatusFilter; label: string }[] = [
  { id: "all", label: "全部状态" },
  { id: "active", label: "运行中 / 排队中" },
  { id: "replied", label: "已回复" },
  { id: "failed", label: "失败" },
  { id: "timeout", label: "超时" },
  { id: "cancelled", label: "已取消" },
];

export function inRange(ms: number, range: Range, now = Date.now()): boolean {
  const t0 = startOfDay(now);
  if (range === "today") return ms >= t0;
  if (range === "yesterday") return ms >= t0 - 86_400_000 && ms < t0;
  return ms >= t0 - 6 * 86_400_000;
}
const matchStatus = (s: RunStatus, f: StatusFilter) =>
  f === "all" ? true : f === "active" ? s === "running" || s === "queued" : s === f;
const isActive = (s: RunStatus) => s === "running" || s === "queued";

/** 每秒刷新一次的当前时间（只在需要实时计时时开启）。 */
function useNow(active: boolean): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

const engineName = (e: RunRecord["engine"]) => (e === "claude" ? "Claude Code" : "Codex");

/** 运行耗时：已结束 = 结束 - 开始；进行中 = 现在 - 开始。 */
const durationOf = (r: RunRecord, now: number) => (r.finished_at ?? now) - r.started_at;

// ───────────── 步骤行 ─────────────
function StepIcon({ kind }: { kind: string }) {
  switch (kind) {
    case "sql":
      return <Database style={{ color: "var(--info-fg)" }} />;
    case "error":
      return <X style={{ color: "var(--danger-fg)" }} />;
    case "thinking":
      return <Brain style={{ color: "var(--muted)" }} />;
    case "tool":
      return <Wrench style={{ color: "var(--muted)" }} />;
    default:
      return <Check style={{ color: "var(--success-fg)" }} />;
  }
}

function StepLine({ s, fresh }: { s: StepRecord; fresh: boolean }) {
  const err = s.kind === "error";
  return (
    <div className={`tl ${fresh ? "new" : ""}`} data-testid="step">
      <span className="ts">{hms(s.at)}</span>
      <span className="ic">
        <StepIcon kind={s.kind} />
      </span>
      <span>
        <span data-testid="step-title" style={{ color: err ? "var(--danger-strong)" : "var(--fg)", fontWeight: 500 }}>
          {s.title}
        </span>{" "}
        {s.detail && <span className="sub">{s.detail}</span>}
      </span>
      <span className="du">{fmtStepDur(s.duration_ms)}</span>
    </div>
  );
}

// ───────────── 回复气泡 ─────────────
/** 把回复里的 @某人 标成飞书的蓝色。 */
function withMentions(text: string): ReactNode {
  return text.split(/(@[^\s@：:，,。]+)/g).map((part, i) =>
    part.startsWith("@") ? (
      <span key={i} className="at">
        {part}
      </span>
    ) : (
      <Fragment key={i}>{part}</Fragment>
    ),
  );
}

function ReplyBubble({ run, botName }: { run: RunRecord; botName: string }) {
  const active = isActive(run.status);
  const review = active ? null : parseReviewReply(run.final_reply);
  let body: ReactNode;
  if (active) {
    body = (
      <div className="fs-bubble" style={{ width: 520, maxWidth: "100%" }} data-testid="reply-skeleton">
        <div className="q">
          回复 {run.sender_name}：@{botName} {run.text}
        </div>
        <div className="grid" style={{ gap: 6, padding: "2px 0" }}>
          <div className="fs-skel" style={{ width: "90%" }} />
          <div className="fs-skel" style={{ width: "72%" }} />
          <div className="fs-skel" style={{ width: "56%" }} />
        </div>
        <div style={{ fontSize: 12, color: "var(--fs-sub)", marginTop: 8 }}>
          {run.status === "queued" ? "排队中，轮到后开始查" : "查完后自动发送到群里"}
        </div>
      </div>
    );
  } else if (review) {
    // 代码审查运行：final_reply 存的是解析后的审查结果，按飞书卡片的样子预览
    body = <ReviewCardPreview result={review} />;
  } else if (run.final_reply) {
    body = (
      <div className="fs-bubble" data-testid="reply-bubble">
        <div className="q">
          回复 {run.sender_name}：@{botName} {run.text}
        </div>
        <div style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>{withMentions(run.final_reply)}</div>
      </div>
    );
  } else {
    body = (
      <div className="fs-bubble" data-testid="reply-bubble" style={{ color: "var(--fs-sub)" }}>
        <div className="q">
          回复 {run.sender_name}：@{botName} {run.text}
        </div>
        这次没有生成回复内容（{RUN_META[run.status].text}）。
      </div>
    );
  }
  return (
    <div className="fs-wrap">
      <div className="fs-av">
        <BrandLogo size={32} round />
      </div>
      <div className="min-w-0">
        <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 4 }}>
          {botName}{" "}
          <span className="chip c-blue" style={{ height: 16, fontSize: 10, padding: "0 5px" }}>
            机器人
          </span>
        </div>
        {body}
      </div>
    </div>
  );
}

// ───────────── 运行详情 ─────────────
function RunDetailPane({ run: listRun, view, onGoEnv }: { run: RunRecord; view: BotView; onGoEnv: () => void }) {
  const toast = useToast();
  const [detailRun, setDetailRun] = useState<RunRecord | null>(null);
  const [steps, setSteps] = useState<StepRecord[]>([]);
  const [fresh, setFresh] = useState<Set<number>>(new Set());
  const [loadError, setLoadError] = useState<string | null>(null);
  const [raw, setRaw] = useState(false);
  const [rawLog, setRawLog] = useState<RawLog | null>(null);
  const [rawError, setRawError] = useState<string | null>(null);
  const [rawLoading, setRawLoading] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  const runId = listRun.id;

  // 详情 + 实时步骤：先订阅再拉详情，订阅期间到达的步骤和拉回来的合并（按 seq 去重）
  useEffect(() => {
    let alive = true;
    setSteps([]);
    setFresh(new Set());
    setDetailRun(null);
    setLoadError(null);
    const unsub = subscribe(
      onRunStep((s) => {
        if (s.run_id !== runId) return;
        setSteps((prev) => addStep(prev, s));
        setFresh((f) => new Set(f).add(s.seq));
      }),
    );
    getRun(runId)
      .then((d) => {
        if (!alive) return;
        setDetailRun(d.run);
        setSteps((prev) => d.steps.reduce(addStep, prev));
      })
      .catch((e) => alive && setLoadError(errText(e)));
    return () => {
      alive = false;
      unsub();
    };
  }, [runId]);

  const run = detailRun ? mergeRun(detailRun, listRun) : listRun;
  const active = isActive(run.status);
  const now = useNow(active);

  const loadRaw = async () => {
    setRawLoading(true);
    setRawError(null);
    try {
      setRawLog(await getRawLog(runId));
    } catch (e) {
      setRawError(errText(e));
    } finally {
      setRawLoading(false);
    }
  };
  useEffect(() => {
    setRawLog(null);
    if (raw) void loadRaw();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [raw, runId]);

  // 新步骤到达时，如果本来就在底部附近，自动滚到底
  useEffect(() => {
    const el = bodyRef.current;
    if (!el || raw) return;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 120) el.scrollTop = el.scrollHeight;
  }, [steps.length, raw]);

  const act = async (fn: () => Promise<void>, ok: string) => {
    try {
      await fn();
      toast(ok);
    } catch (e) {
      toast(errText(e), "err");
    }
  };

  const group = view.bot.groups.find((g) => g.chat_id === run.chat_id);
  const lastThinking = [...steps].reverse().find((s) => s.kind === "thinking");
  const progressCls = active ? "" : run.status === "replied" ? "" : run.status === "failed" ? "p-err" : "p-warn";
  const pct = active ? Math.min(92, 8 + steps.length * 9) : 100;
  const reasonWarn = run.status === "timeout" || run.status === "cancelled";

  const metaIcon = { width: 12, height: 12 };
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="run-detail">
      <div style={{ padding: "12px 16px", borderBottom: "1px solid var(--line)" }}>
        <div className="flex items-center" style={{ gap: 10 }}>
          <RunChip status={run.status} />
          <div className="min-w-0 flex-1 truncate" style={{ fontWeight: 600, fontSize: 14 }}>
            {run.text}
          </div>
          <button
            type="button"
            className="btn btn-ghost btn-sm tr"
            onClick={() => void navigator.clipboard?.writeText(run.id).then(() => toast(`已复制运行 ID ${run.id}`))}
          >
            <Copy />
            复制 ID
          </button>
          {active ? (
            <button type="button" className="btn btn-danger btn-sm tr" onClick={() => void act(() => cancelRun(run.id), "已取消运行")}>
              <CircleStop />
              取消运行
            </button>
          ) : run.status === "replied" ? (
            <button type="button" className="btn btn-sm tr" onClick={() => void act(() => retryRun(run.id), "已重新排队")}>
              <RotateCcw />
              重新运行
            </button>
          ) : (
            <button type="button" className="btn btn-pri btn-sm tr" onClick={() => void act(() => retryRun(run.id), "已重新排队")}>
              <RotateCcw />
              重试
            </button>
          )}
        </div>
        <div className="mono flex flex-wrap" style={{ gap: 16, marginTop: 8, fontSize: 11.5, color: "var(--muted)" }}>
          <span className="flex items-center gap-1">
            <Hash style={metaIcon} /> {run.group_name}
          </span>
          <span className="flex items-center gap-1">
            <AtSign style={metaIcon} /> {run.sender_name}
          </span>
          <span className="flex items-center gap-1">
            <Clock style={metaIcon} /> {mdhm(run.started_at)}
          </span>
          <span className="flex items-center gap-1" data-testid="run-duration">
            <Timer style={metaIcon} /> {fmtDur(durationOf(run, now))}
          </span>
          {group && group.projects.length > 0 && (
            <span className="flex items-center gap-1">
              <GitBranch style={metaIcon} /> {group.projects.join(" · ")}
            </span>
          )}
          <span className="flex items-center gap-1">
            <Database style={metaIcon} /> 查库 {run.sql_count} 次
          </span>
          <span className="flex items-center gap-1">
            <BotIco style={metaIcon} /> {engineName(run.engine)}
            {run.engine_version ? ` ${run.engine_version}` : ""}
          </span>
        </div>
        {run.error && isTerminal(run.status) && (
          <div
            className="flex items-center"
            role="alert"
            style={{
              marginTop: 10,
              padding: "8px 10px",
              borderRadius: 7,
              gap: 8,
              fontSize: 12,
              background: reasonWarn ? "var(--warning-soft)" : "var(--danger-soft)",
              color: reasonWarn ? "var(--warning-strong)" : "var(--danger-strong)",
            }}
          >
            {reasonWarn ? <TimerOff style={{ width: 15, height: 15 }} /> : <CircleX style={{ width: 15, height: 15 }} />}
            <b style={{ fontWeight: 600 }}>{run.status === "timeout" ? "超时原因" : run.status === "cancelled" ? "取消原因" : "失败原因"}</b>
            <span className="min-w-0 flex-1 truncate">{run.error}</span>
            {run.status === "failed" && (
              <button type="button" className="btn btn-sm tr" onClick={onGoEnv}>
                <Stethoscope />
                去环境检查
              </button>
            )}
          </div>
        )}
      </div>

      <div className="scroll flex min-h-0 flex-1 flex-col" style={{ padding: "12px 16px", gap: 12 }}>
        <div className="term flex flex-col" style={{ flex: "1 0 auto", minHeight: active ? 240 : 300 }}>
          <div className="flex items-center" style={{ gap: 8, padding: "8px 12px", borderBottom: "1px solid var(--line)", color: "var(--muted)", fontSize: 11.5 }}>
            {raw ? <ScrollText style={{ width: 13, height: 13 }} /> : <Terminal style={{ width: 13, height: 13 }} />}
            {raw ? (
              <span>原始日志 · stdout / stderr（已打码，最多最后 2MB）</span>
            ) : (
              <>
                <span>步骤流</span>
                <span style={{ color: "var(--dim)" }}>·</span>
                <span>读群聊 → 同步代码 → 查库 → 写文档 → 回复</span>
              </>
            )}
            <span className="flex-1" />
            {raw ? (
              <button type="button" className="btn btn-ghost btn-sm" style={{ height: 22 }} aria-label="刷新原始日志" onClick={() => void loadRaw()}>
                <RefreshCw className={rawLoading ? "spin" : ""} />
              </button>
            ) : active ? (
              <span className="flex items-center gap-1.5" style={{ color: "var(--success-fg)" }}>
                <span className="dot st-running pulse" />
                实时
              </span>
            ) : (
              <span>{steps.length} 步</span>
            )}
            <button type="button" className="btn btn-ghost btn-sm" style={{ height: 22 }} aria-pressed={raw} onClick={() => setRaw((v) => !v)}>
              {raw ? "步骤流" : "原始日志"}
            </button>
          </div>
          {!raw && (
            <div className={`prog ${active ? "indet" : ""}`} style={{ borderRadius: 0, height: 2 }}>
              <i className={progressCls} style={{ width: `${pct}%` }} />
            </div>
          )}
          <div ref={bodyRef} className="scroll flex-1" style={{ padding: "8px 0", maxHeight: active ? undefined : 520 }} data-testid="term-body">
            {raw ? (
              rawError ? (
                <pre className="raw err">读取原始日志失败：{rawError}</pre>
              ) : !rawLog ? (
                <pre className="raw">读取中…</pre>
              ) : (
                <>
                  <pre className="raw" data-testid="raw-stdout">
                    {rawLog.stdout || "（stdout 为空）"}
                  </pre>
                  {rawLog.stderr && (
                    <>
                      <div style={{ padding: "4px 12px", color: "var(--dim)" }}>── stderr ──</div>
                      <pre className="raw err" data-testid="raw-stderr">
                        {rawLog.stderr}
                      </pre>
                    </>
                  )}
                </>
              )
            ) : (
              <>
                {loadError && <pre className="raw err">读取步骤失败：{loadError}</pre>}
                {steps.map((s) => (
                  <StepLine key={s.seq} s={s} fresh={fresh.has(s.seq)} />
                ))}
                {active && (
                  <div className="tl" data-testid="live-tail">
                    <span className="ts">{hms(now)}</span>
                    <span className="ic">
                      <LoaderCircle className="spin" style={{ color: "var(--success-fg)" }} />
                    </span>
                    <span>
                      <span style={{ color: "var(--success-fg)", fontWeight: 500 }}>
                        {run.status === "queued" ? "排队中" : `${engineName(run.engine).split(" ")[0]} 分析中`}
                      </span>{" "}
                      <span className="sub">{run.status === "queued" ? "等待空闲的运行槽位…" : (lastThinking?.title ?? "等待下一步…")}</span>{" "}
                      <span className="cursor-blk" />
                    </span>
                    <span className="du" />
                  </div>
                )}
              </>
            )}
          </div>
        </div>
        <div>
          <div className="flex items-center" style={{ gap: 8, marginBottom: 8, color: "var(--muted)", fontSize: 12 }}>
            <Send style={{ width: 13, height: 13 }} />
            发到群里的回复 <span style={{ color: "var(--dim)" }}>· {run.group_name}</span>
            {active && (
              <span className="chip c-mut" style={{ height: 18, fontSize: 11 }}>
                生成中
              </span>
            )}
          </div>
          <ReplyBubble run={run} botName={view.bot.name} />
        </div>
      </div>
    </div>
  );
}

// ───────────── tab ─────────────
export default function LogsTab({
  view,
  runs,
  runId,
  onOpenRun,
  onGoEnv,
}: {
  view: BotView;
  runs: RunRecord[];
  runId: string | null;
  onOpenRun: (id: string) => void;
  onGoEnv: () => void;
  onReload: () => Promise<void>;
}) {
  const [range, setRange] = useState<Range>(() => {
    const target = runs.find((r) => r.id === runId);
    return target && !inRange(target.started_at, "today") ? "week" : "today";
  });
  const [filter, setFilter] = useState<StatusFilter>("all");
  const [menu, setMenu] = useState(false);
  const anyActive = runs.some((r) => isActive(r.status));
  const now = useNow(anyActive);

  const list = useMemo(
    () => runs.filter((r) => inRange(r.started_at, range) && matchStatus(r.status, filter)),
    [runs, range, filter],
  );
  const selected = list.find((r) => r.id === runId) ?? runs.find((r) => r.id === runId) ?? list[0] ?? null;

  if (!runs.length) {
    return (
      <div className="grid flex-1 place-items-center">
        <EmptyState icon={ScrollText} title="暂无运行" desc={`启动后在群里 @${view.bot.name} 即可产生运行记录`} />
      </div>
    );
  }

  return (
    <div className="grid min-h-0 flex-1" style={{ gridTemplateColumns: "292px 1fr" }}>
      <div className="flex min-h-0 flex-col" style={{ borderRight: "1px solid var(--line)" }}>
        <div className="relative flex items-center" style={{ padding: "10px 12px", borderBottom: "1px solid var(--line)", gap: 8 }}>
          <Segmented ariaLabel="时间范围" value={range} onChange={setRange} options={RANGES.map((r) => ({ value: r.id, label: r.label }))} />
          <span className="flex-1" />
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            aria-label="按状态筛选"
            aria-expanded={menu}
            style={filter !== "all" ? { color: "var(--accent-text)", background: "var(--accent-soft)" } : undefined}
            onClick={() => setMenu((v) => !v)}
          >
            <ListFilter />
          </button>
          {menu && (
            <>
              <div className="fixed inset-0" style={{ zIndex: 39 }} onClick={() => setMenu(false)} />
              <div className="pop-menu" role="menu" style={{ top: 42, right: 8, width: 180 }}>
                {FILTERS.map((f) => (
                  <button
                    key={f.id}
                    type="button"
                    role="menuitemradio"
                    aria-checked={filter === f.id}
                    className="tray-it tr"
                    onClick={() => {
                      setFilter(f.id);
                      setMenu(false);
                    }}
                  >
                    <span className="flex-1">{f.label}</span>
                    {filter === f.id && <Check style={{ color: "var(--accent-text)" }} />}
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
        <div className="scroll flex-1" data-testid="run-list">
          {list.length === 0 && (
            <EmptyState compact icon={ScrollText} title="这个范围内没有运行记录" desc="换个时间范围或状态筛选试试" />
          )}
          {list.map((x) => (
            <button
              key={x.id}
              type="button"
              className={`run-item tr ${selected?.id === x.id ? "on" : ""}`}
              aria-current={selected?.id === x.id ? "true" : undefined}
              data-testid={`run-item-${x.id}`}
              onClick={() => onOpenRun(x.id)}
            >
              <div className="flex items-center gap-2">
                <span className="mono" style={{ fontWeight: 600 }}>
                  {range === "today" ? hm(x.started_at) : relTime(x.started_at)}
                </span>
                <span className="flex-1 truncate" style={{ color: "var(--muted)", fontSize: 12 }}>
                  {x.group_name}
                </span>
                <RunChip status={x.status} />
              </div>
              <Tooltip as="div" onlyWhenTruncated content={`${x.sender_name}：${x.text}`} className="truncate" style={{ marginTop: 5 }}>
                <span style={{ color: "var(--muted)" }}>{x.sender_name}：</span>
                {x.text}
              </Tooltip>
              <div className="mono flex" style={{ marginTop: 4, fontSize: 11, color: "var(--dim)", gap: 10 }}>
                <span>{isActive(x.status) ? `已运行 ${fmtDur(durationOf(x, now))}` : fmtDur(durationOf(x, now))}</span>
                <span>查库 {x.sql_count}</span>
              </div>
            </button>
          ))}
        </div>
      </div>
      <div className="flex min-h-0 flex-col">
        {selected ? (
          <RunDetailPane key={selected.id} run={selected} view={view} onGoEnv={onGoEnv} />
        ) : (
          <div className="grid flex-1 place-items-center" style={{ color: "var(--muted)" }}>
            点左边的一条运行查看详情
          </div>
        )}
      </div>
    </div>
  );
}
