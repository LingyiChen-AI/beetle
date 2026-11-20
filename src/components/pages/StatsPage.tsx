// 数据统计：按时间范围（今天 / 近 7 天 / 近 30 天）和机器人筛选，展示 KPI（与上一周期对比）、每日趋势、
// 机器人对比、按小时分布、群 / 触发人 Top 10、引擎占比、失败原因。数据来自后端 stats_overview。
import {
  Activity,
  AlertTriangle,
  ArrowDownRight,
  ArrowUpRight,
  BarChart3,
  Bot,
  CircleCheck,
  Clock3,
  Cpu,
  Database,
  Minus,
  RefreshCw,
  TrendingUp,
  Users,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { errText, statsOverview, type BotView, type StatsOverview, type StatsRange, type StatsTotals } from "../../api";
import { fmtDur } from "../../format";
import { PageHead } from "../common";
import EChart from "../stats/EChart";
import { botsOption, engineOption, hourlyOption, rankOption, trendOption, useChartColors } from "../stats/chartTheme";
import { EmptyState, LoadingState, Segmented, Select, Truncate, type SelectOption } from "../ui";

const RANGES: { value: StatsRange; label: string }[] = [
  { value: "today", label: "今天" },
  { value: "7d", label: "近 7 天" },
  { value: "30d", label: "近 30 天" },
];
const PREV_TEXT: Record<StatsRange, string> = { today: "较昨日同时段", "7d": "较前 7 天", "30d": "较前 30 天" };

const pct = (r: number) => `${Math.round(r * 100)}%`;

type Trend = { text: string; tone: "up" | "down" | "flat" | "new"; good: boolean | null };

/** 与上一周期比：数量类按相对变化，成功率按百分点。`higherIsBetter=false` 时（耗时）下降算好。 */
export function compare(cur: number, prev: number, kind: "count" | "rate" | "duration"): Trend {
  if (kind === "rate") {
    const d = Math.round((cur - prev) * 100);
    if (d === 0) return { text: "持平", tone: "flat", good: null };
    return { text: `${d > 0 ? "+" : ""}${d} 个百分点`, tone: d > 0 ? "up" : "down", good: d > 0 };
  }
  if (prev === 0) return cur === 0 ? { text: "持平", tone: "flat", good: null } : { text: "上一周期无数据", tone: "new", good: null };
  const ratio = (cur - prev) / prev;
  const p = Math.round(ratio * 100);
  if (p === 0) return { text: "持平", tone: "flat", good: null };
  const up = p > 0;
  return { text: `${up ? "+" : ""}${p}%`, tone: up ? "up" : "down", good: kind === "duration" ? !up : up };
}

function Delta({ trend, prevText }: { trend: Trend; prevText: string }) {
  const Icon = trend.tone === "up" ? ArrowUpRight : trend.tone === "down" ? ArrowDownRight : Minus;
  const color = trend.good == null ? "var(--muted)" : trend.good ? "var(--success-fg)" : "var(--danger-fg)";
  return (
    <span className="flex items-center gap-1" style={{ fontSize: 12, color: "var(--dim)" }}>
      {prevText}
      <span className="flex items-center gap-0.5" style={{ color, fontWeight: 600 }} data-testid="kpi-delta">
        {trend.tone !== "new" && <Icon style={{ width: 13, height: 13 }} />}
        {trend.text}
      </span>
    </span>
  );
}

function Kpi({ icon: Icon, label, value, sub, delta, testId }: { icon: LucideIcon; label: string; value: string; sub: ReactNode; delta: ReactNode; testId: string }) {
  return (
    <div className="card kpi" data-testid={testId}>
      <div className="k">
        <Icon />
        {label}
      </div>
      <div className="v mono" data-testid={`${testId}-value`}>
        {value}
      </div>
      <div style={{ fontSize: 12, color: "var(--muted)", minHeight: 18 }}>{sub}</div>
      <div style={{ marginTop: 6, paddingTop: 8, borderTop: "1px solid var(--line)" }}>{delta}</div>
    </div>
  );
}

function ChartCard({ icon: Icon, title, hint, children, span }: { icon: LucideIcon; title: string; hint?: ReactNode; children: ReactNode; span?: number }) {
  return (
    <div className="card flex min-w-0 flex-col" style={span ? { gridColumn: `span ${span}` } : undefined}>
      <div className="card-h">
        <h3>
          <Icon />
          {title}
        </h3>
        {hint != null && <span style={{ color: "var(--dim)", fontSize: 12 }}>{hint}</span>}
      </div>
      <div style={{ padding: "12px 14px" }}>{children}</div>
    </div>
  );
}

function Kpis({ s, range }: { s: StatsOverview; range: StatsRange }) {
  const t: StatsTotals = s.totals;
  const p: StatsTotals = s.previous;
  const prevText = PREV_TEXT[range];
  return (
    <div className="grid grid-cols-4" style={{ gap: 12 }}>
      <Kpi
        icon={Activity}
        label="运行数"
        testId="stat-runs"
        value={String(t.runs)}
        sub={`已回复 ${t.replied} · 失败 ${t.failed} · 超时 ${t.timeout}${t.cancelled ? ` · 取消 ${t.cancelled}` : ""}`}
        delta={<Delta trend={compare(t.runs, p.runs, "count")} prevText={prevText} />}
      />
      <Kpi
        icon={CircleCheck}
        label="成功率"
        testId="stat-rate"
        value={t.settled ? pct(t.success_rate) : "—"}
        sub={t.settled ? `已出结果 ${t.settled} 条中回复 ${t.replied} 条` : "还没有已结束的运行"}
        delta={t.settled && p.settled ? <Delta trend={compare(t.success_rate, p.success_rate, "rate")} prevText={prevText} /> : <Delta trend={{ text: "—", tone: "new", good: null }} prevText={prevText} />}
      />
      <Kpi
        icon={Clock3}
        label="平均耗时"
        testId="stat-avg"
        value={t.replied ? fmtDur(t.avg_duration_ms) : "—"}
        sub={t.replied ? `P90 ${fmtDur(t.p90_duration_ms)} · 仅统计已回复` : "仅统计已回复的运行"}
        delta={t.replied && p.replied ? <Delta trend={compare(t.avg_duration_ms, p.avg_duration_ms, "duration")} prevText={prevText} /> : <Delta trend={{ text: "—", tone: "new", good: null }} prevText={prevText} />}
      />
      <Kpi
        icon={Database}
        label="查库次数"
        testId="stat-sql"
        value={String(t.sql_count)}
        sub={t.runs ? `平均每次运行 ${(t.sql_count / t.runs).toFixed(1)} 次` : "按项目关联的数据源只读查询"}
        delta={<Delta trend={compare(t.sql_count, p.sql_count, "count")} prevText={prevText} />}
      />
    </div>
  );
}

function Charts({ s, range, showBots }: { s: StatsOverview; range: StatsRange; showBots: boolean }) {
  const c = useChartColors();
  const trend = useMemo(() => (range === "today" ? hourlyOption(c, s.hourly) : trendOption(c, s.daily)), [c, s, range]);
  const hourly = useMemo(() => hourlyOption(c, s.hourly), [c, s]);
  const bots = useMemo(() => botsOption(c, s.by_bot), [c, s]);
  const groups = useMemo(() => rankOption(c, s.by_group, c.info), [c, s]);
  const senders = useMemo(() => rankOption(c, s.by_sender, c.series[2]), [c, s]);
  const engines = useMemo(() => engineOption(c, s.by_engine), [c, s]);
  const maxReason = Math.max(1, ...s.failure_reasons.map((r) => r.count));
  const bad = s.totals.failed + s.totals.timeout;
  const rankH = (n: number) => Math.max(120, n * 28 + 16);

  return (
    <>
      <div className="grid" style={{ gridTemplateColumns: "2fr 1fr", gap: 12 }}>
        <ChartCard icon={TrendingUp} title={range === "today" ? "今日按小时" : "每日趋势"} hint={range === "today" ? "运行数" : "柱：成功 / 失败 / 超时 · 线：运行数"}>
          <EChart option={trend} height={260} ariaLabel={range === "today" ? "今日按小时运行数" : "每日运行趋势"} testId="chart-trend" />
        </ChartCard>
        <ChartCard icon={Cpu} title="引擎占比">
          <EChart option={engines} height={260} ariaLabel="引擎占比" testId="chart-engine" />
        </ChartCard>
      </div>

      <div className="grid" style={{ gridTemplateColumns: showBots && range !== "today" ? "1fr 1fr" : "1fr", gap: 12 }}>
        {showBots && (
          <ChartCard icon={Bot} title="机器人对比" hint="运行数 · 成功率">
            <EChart option={bots} height={rankH(s.by_bot.length) + 24} ariaLabel="机器人对比" testId="chart-bots" />
          </ChartCard>
        )}
        {range !== "today" && (
          <ChartCard icon={Clock3} title="按小时分布" hint="一天里什么时候最忙">
            <EChart option={hourly} height={Math.max(200, showBots ? rankH(s.by_bot.length) + 24 : 200)} ariaLabel="按小时分布" testId="chart-hourly" />
          </ChartCard>
        )}
      </div>

      <div className="grid grid-cols-2" style={{ gap: 12 }}>
        <ChartCard icon={Users} title="群 Top 10" hint={`共 ${s.by_group.length} 个群`}>
          {s.by_group.length ? (
            <EChart option={groups} height={rankH(s.by_group.length)} ariaLabel="群 Top 10" testId="chart-groups" />
          ) : (
            <EmptyState compact title="没有群数据" />
          )}
        </ChartCard>
        <ChartCard icon={Users} title="触发人 Top 10" hint={`共 ${s.by_sender.length} 人`}>
          {s.by_sender.length ? (
            <EChart option={senders} height={rankH(s.by_sender.length)} ariaLabel="触发人 Top 10" testId="chart-senders" />
          ) : (
            <EmptyState compact title="没有触发人数据" />
          )}
        </ChartCard>
      </div>

      <div className="grid" style={{ gridTemplateColumns: "1fr 1fr", gap: 12 }}>
        <div className="card min-w-0">
          <div className="card-h">
            <h3>
              <AlertTriangle />
              失败原因 Top 5
            </h3>
            <span style={{ color: "var(--dim)", fontSize: 12 }}>失败 + 超时共 {bad} 次 · 按错误前 60 字归类</span>
          </div>
          {s.failure_reasons.length === 0 ? (
            <EmptyState compact icon={CircleCheck} title="这个范围内没有失败" desc="失败和超时的运行会按错误信息归类显示在这里" />
          ) : (
            <div data-testid="failure-reasons">
              {s.failure_reasons.map((r) => (
                <div key={r.reason} className="grid items-center" style={{ gridTemplateColumns: "1fr 140px 44px", gap: 12, padding: "9px 14px", borderBottom: "1px solid var(--line)" }}>
                  <Truncate text={r.reason} mono style={{ fontSize: 12 }} />
                  <div className="prog">
                    <i className="p-err" style={{ width: `${(r.count / maxReason) * 100}%` }} />
                  </div>
                  <span className="mono" style={{ textAlign: "right", fontWeight: 600 }}>
                    {r.count}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
        <div className="card min-w-0">
          <div className="card-h">
            <h3>
              <Bot />
              机器人明细
            </h3>
          </div>
          <table className="t">
            <thead>
              <tr>
                <th>机器人</th>
                <th style={{ textAlign: "right" }}>运行</th>
                <th style={{ textAlign: "right" }}>成功率</th>
                <th style={{ textAlign: "right" }}>平均耗时</th>
                <th style={{ textAlign: "right" }}>查库</th>
              </tr>
            </thead>
            <tbody>
              {s.by_bot.map((b) => (
                <tr key={b.bot_id}>
                  <td style={{ maxWidth: 180 }}>
                    <Truncate text={b.name} style={{ fontWeight: 500 }} />
                  </td>
                  <td className="mono" style={{ textAlign: "right" }}>
                    {b.runs}
                  </td>
                  <td className="mono" style={{ textAlign: "right", color: b.success_rate >= 0.8 ? "var(--success-fg)" : b.success_rate < 0.5 ? "var(--danger-fg)" : undefined }}>
                    {pct(b.success_rate)}
                  </td>
                  <td className="mono" style={{ textAlign: "right", color: "var(--muted)" }}>
                    {b.replied ? fmtDur(b.avg_duration_ms) : "—"}
                  </td>
                  <td className="mono" style={{ textAlign: "right", color: "var(--muted)" }}>
                    {b.sql_count}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </>
  );
}

export default function StatsPage({ views }: { views: BotView[] }) {
  const [range, setRange] = useState<StatsRange>("7d");
  const [botId, setBotId] = useState<string>("");
  const [data, setData] = useState<StatsOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError(null);
    statsOverview(range, botId || null)
      .then((s) => alive && setData(s))
      .catch((e) => alive && setError(errText(e)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [range, botId, tick]);

  const botOptions: SelectOption[] = useMemo(
    () => [
      { value: "", label: "全部机器人", icon: BarChart3 },
      ...views.map((v) => ({ value: v.bot.id, label: v.bot.name, description: v.bot.engine === "claude" ? "Claude Code" : "Codex", icon: Bot })),
    ],
    [views],
  );

  return (
    <>
      <PageHead
        title="数据统计"
        sub="各机器人的运行量、成功率、耗时、查库次数与失败原因"
        right={
          <>
            <Segmented ariaLabel="统计范围" size="md" value={range} onChange={setRange} options={RANGES} />
            <Select ariaLabel="机器人" style={{ width: 180 }} value={botId} options={botOptions} onChange={setBotId} />
            <button type="button" className="btn btn-ghost tr" aria-label="刷新" disabled={loading} onClick={() => setTick((n) => n + 1)}>
              <RefreshCw className={loading ? "spin" : ""} />
            </button>
          </>
        }
      />
      <div className="scroll flex-1" style={{ padding: "16px 24px 24px" }}>
        <div className="grid" style={{ gap: 12, maxWidth: 1280 }}>
          {error && (
            <div className="alert-err" role="alert">
              读取统计失败：{error}
            </div>
          )}
          {!data && loading && <LoadingState text="正在统计…" />}
          {data && (
            <div className="grid" style={{ gap: 12, opacity: loading ? 0.6 : 1, transition: "opacity .2s ease" }}>
              <Kpis s={data} range={range} />
              {data.totals.runs === 0 ? (
                <div className="card">
                  <EmptyState
                    icon={BarChart3}
                    title="这个范围内还没有运行记录"
                    desc="在群里 @ 机器人查问题后，这里会显示运行趋势、各群和触发人的分布、失败原因等统计"
                  />
                </div>
              ) : (
                <Charts s={data} range={range} showBots={!botId} />
              )}
            </div>
          )}
        </div>
      </div>
    </>
  );
}
