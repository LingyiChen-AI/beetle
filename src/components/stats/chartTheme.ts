// 统计图配色：运行时从 tokens.css 的语义 token 读出实际色值（canvas 不认 var()），
// <html> 的 data-theme / data-accent 变化时重新读，图表跟着深浅色和主题色变。
// 同一含义的颜色全页固定：成功 = success、失败 = danger、超时 = warning；Claude = 主题色、Codex = info。
import { useEffect, useState } from "react";
import { fmtDur } from "../../format";
import type { StatsBotRow, StatsDay, StatsNamedCount } from "../../api";
import type { ChartOption } from "./EChart";

export interface ChartColors {
  accent: string;
  accentText: string;
  success: string;
  danger: string;
  warning: string;
  info: string;
  fg: string;
  fgSoft: string;
  muted: string;
  dim: string;
  line: string;
  border: string;
  popover: string;
  panel: string;
  subtle: string;
  /** 分类色（机器人等没有固定含义的系列按序取） */
  series: string[];
}

function readColors(): ChartColors {
  const root = document.documentElement;
  const cs = getComputedStyle(root);
  const v = (name: string, fallback: string) => cs.getPropertyValue(name).trim() || fallback;
  const light = root.dataset.theme === "light";
  const pal = (c: string, fb: string) => v(`--${c}-${light ? "l" : "d"}`, fb);
  const accent = v("--accent", "#e8441c");
  const info = v("--info", "#60a5fa");
  return {
    accent,
    accentText: v("--accent-text", accent),
    success: v("--success", "#22c55e"),
    danger: v("--danger", "#ef4444"),
    warning: v("--warning", "#f59e0b"),
    info,
    fg: v("--fg", "#f8fafc"),
    fgSoft: v("--fg-soft", "#cbd5e1"),
    muted: v("--muted", "#94a3b8"),
    dim: v("--dim", "#64748b"),
    line: v("--line", "#1e293b"),
    border: v("--border", "#334155"),
    popover: v("--popover", "#0f172a"),
    panel: v("--panel", "#0e1223"),
    subtle: v("--subtle", "#1e293b"),
    series: [accent, info, pal("teal", "#14b8a6"), pal("purple", "#a855f7"), pal("orange", "#f97316"), pal("green", "#22c55e")],
  };
}

export function useChartColors(): ChartColors {
  const [colors, setColors] = useState<ChartColors>(readColors);
  useEffect(() => {
    const root = document.documentElement;
    const mo = new MutationObserver(() => setColors(readColors()));
    mo.observe(root, { attributes: true, attributeFilter: ["data-theme", "data-accent"] });
    // 外观在父组件的 effect 里才写到 <html> 上，挂载后再读一次
    setColors(readColors());
    return () => mo.disconnect();
  }, []);
  return colors;
}

export const ENGINE_LABEL: Record<string, string> = { claude: "Claude Code", codex: "Codex" };
export const engineColor = (c: ChartColors, engine: string) => (engine === "claude" ? c.accent : engine === "codex" ? c.info : c.dim);

const FONT = '"Inter Variable", Inter, "Microsoft YaHei", "PingFang SC", system-ui, sans-serif';

/** 公共部分：字体、提示框（悬停才出现）、坐标轴样式。 */
function base(c: ChartColors) {
  return {
    backgroundColor: "transparent",
    textStyle: { fontFamily: FONT, color: c.muted, fontSize: 12 },
    animationDuration: 400,
    tooltip: {
      show: true,
      triggerOn: "mousemove",
      backgroundColor: c.popover,
      borderColor: c.border,
      borderWidth: 1,
      padding: [6, 10],
      textStyle: { color: c.fg, fontSize: 12, fontFamily: FONT },
      extraCssText: "border-radius: 8px; box-shadow: 0 10px 30px rgba(0,0,0,.25);",
    },
  };
}

const axisCommon = (c: ChartColors) => ({
  axisLine: { lineStyle: { color: c.line } },
  axisTick: { show: false },
  axisLabel: { color: c.muted, fontSize: 11 },
  splitLine: { lineStyle: { color: c.line, type: "dashed" as const } },
});

const legend = (c: ChartColors) => ({
  top: 0,
  right: 0,
  icon: "roundRect",
  itemWidth: 10,
  itemHeight: 10,
  itemGap: 14,
  textStyle: { color: c.muted, fontSize: 12 },
});

/** 每日趋势：成功 / 失败 / 超时 堆叠柱 + 运行数折线。 */
export function trendOption(c: ChartColors, days: StatsDay[]): ChartOption {
  const ax = axisCommon(c);
  const bar = (name: string, color: string, data: number[], top = false) => ({
    name,
    type: "bar",
    stack: "s",
    barMaxWidth: 22,
    itemStyle: { color, borderRadius: top ? [4, 4, 0, 0] : 0 },
    emphasis: { focus: "series" },
    data,
  });
  return {
    ...base(c),
    tooltip: { ...base(c).tooltip, trigger: "axis", axisPointer: { type: "shadow", shadowStyle: { color: c.subtle, opacity: 0.5 } } },
    legend: legend(c),
    grid: { left: 8, right: 8, top: 34, bottom: 4, containLabel: true },
    xAxis: { type: "category", data: days.map((d) => d.day), ...ax, splitLine: { show: false } },
    yAxis: { type: "value", minInterval: 1, ...ax, axisLine: { show: false } },
    series: [
      bar("成功", c.success, days.map((d) => d.replied)),
      bar("失败", c.danger, days.map((d) => d.failed)),
      bar("超时", c.warning, days.map((d) => d.timeout), true),
      {
        name: "运行数",
        type: "line",
        smooth: true,
        symbol: "circle",
        symbolSize: 6,
        showSymbol: days.length <= 14,
        lineStyle: { color: c.accentText, width: 2 },
        itemStyle: { color: c.accentText },
        data: days.map((d) => d.runs),
      },
    ],
  };
}

/** 按小时分布：0–23 点运行数柱状。 */
export function hourlyOption(c: ChartColors, hourly: number[]): ChartOption {
  const ax = axisCommon(c);
  const max = Math.max(1, ...hourly);
  return {
    ...base(c),
    tooltip: { ...base(c).tooltip, trigger: "axis", axisPointer: { type: "shadow", shadowStyle: { color: c.subtle, opacity: 0.5 } } },
    grid: { left: 8, right: 8, top: 12, bottom: 4, containLabel: true },
    xAxis: { type: "category", data: hourly.map((_, h) => `${h}点`), ...ax, splitLine: { show: false }, axisLabel: { ...ax.axisLabel, interval: 2 } },
    yAxis: { type: "value", minInterval: 1, ...ax, axisLine: { show: false } },
    series: [
      {
        name: "运行数",
        type: "bar",
        barMaxWidth: 16,
        data: hourly.map((n) => ({ value: n, itemStyle: { color: n === max && n > 0 ? c.accent : c.info, opacity: n === max ? 1 : 0.75, borderRadius: [3, 3, 0, 0] } })),
      },
    ],
  };
}

/** 横向条形排行（群 / 触发人）：从大到小，条尾标数量。 */
export function rankOption(c: ChartColors, rows: StatsNamedCount[], color: string): ChartOption {
  const ax = axisCommon(c);
  const data = [...rows].reverse();
  return {
    ...base(c),
    tooltip: { ...base(c).tooltip, trigger: "item", formatter: "{b}：{c} 次" },
    grid: { left: 4, right: 36, top: 4, bottom: 4, containLabel: true },
    xAxis: { type: "value", minInterval: 1, ...ax, axisLabel: { show: false }, splitLine: { show: false } },
    yAxis: {
      type: "category",
      data: data.map((r) => r.name),
      ...ax,
      axisLine: { show: false },
      axisLabel: { color: c.fgSoft, fontSize: 12, width: 120, overflow: "truncate" },
    },
    series: [
      {
        type: "bar",
        barMaxWidth: 14,
        showBackground: true,
        backgroundStyle: { color: c.subtle, borderRadius: 4 },
        itemStyle: { color, borderRadius: 4 },
        label: { show: true, position: "right", color: c.muted, fontSize: 11 },
        data: data.map((r) => r.runs),
      },
    ],
  };
}

/** 机器人对比：运行数（成功 / 其它）横向堆叠，标签带成功率。 */
export function botsOption(c: ChartColors, rows: StatsBotRow[]): ChartOption {
  const ax = axisCommon(c);
  const data = [...rows].reverse();
  return {
    ...base(c),
    tooltip: {
      ...base(c).tooltip,
      trigger: "axis",
      axisPointer: { type: "shadow", shadowStyle: { color: c.subtle, opacity: 0.5 } },
      formatter: (ps: { dataIndex: number }[]) => {
        const r = data[ps[0]?.dataIndex ?? 0];
        if (!r) return "";
        return `${r.name}<br/>运行 ${r.runs} 次 · 已回复 ${r.replied}<br/>成功率 ${Math.round(r.success_rate * 100)}% · 平均 ${fmtDur(r.avg_duration_ms || null)}`;
      },
    },
    legend: legend(c),
    grid: { left: 4, right: 56, top: 30, bottom: 4, containLabel: true },
    xAxis: { type: "value", minInterval: 1, ...ax, axisLabel: { show: false }, splitLine: { show: false } },
    yAxis: { type: "category", data: data.map((r) => r.name), ...ax, axisLine: { show: false }, axisLabel: { color: c.fgSoft, fontSize: 12, width: 120, overflow: "truncate" } },
    series: [
      { name: "已回复", type: "bar", stack: "b", barMaxWidth: 16, itemStyle: { color: c.success }, data: data.map((r) => r.replied) },
      {
        name: "其它",
        type: "bar",
        stack: "b",
        barMaxWidth: 16,
        itemStyle: { color: c.border, borderRadius: [0, 4, 4, 0] },
        label: {
          show: true,
          position: "right",
          color: c.muted,
          fontSize: 11,
          formatter: (p: { dataIndex: number }) => {
            const r = data[p.dataIndex];
            return r ? `${r.runs} · ${Math.round(r.success_rate * 100)}%` : "";
          },
        },
        data: data.map((r) => r.runs - r.replied),
      },
    ],
  };
}

/** 引擎占比环形图。 */
export function engineOption(c: ChartColors, rows: { engine: string; runs: number }[]): ChartOption {
  const total = rows.reduce((s, r) => s + r.runs, 0);
  return {
    ...base(c),
    tooltip: { ...base(c).tooltip, trigger: "item", formatter: "{b}：{c} 次（{d}%）" },
    legend: { ...legend(c), top: "auto", bottom: 0, right: "center" },
    series: [
      {
        type: "pie",
        radius: ["58%", "80%"],
        center: ["50%", "44%"],
        avoidLabelOverlap: true,
        padAngle: 2,
        itemStyle: { borderRadius: 6, borderColor: c.panel, borderWidth: 2 },
        label: { show: true, position: "center", formatter: () => `${total}\n次运行`, color: c.fg, fontSize: 18, fontWeight: 700, lineHeight: 24 },
        labelLine: { show: false },
        emphasis: { scale: true, scaleSize: 4, label: { show: true } },
        data: rows.map((r) => ({ name: ENGINE_LABEL[r.engine] ?? r.engine, value: r.runs, itemStyle: { color: engineColor(c, r.engine) } })),
      },
    ],
  };
}
