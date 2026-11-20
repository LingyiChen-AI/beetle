import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";
import App from "../../App";
import type { StatsOverview, StatsTotals } from "../../api";
import { baseHandlers, view } from "../../test/fixtures";
import { callsOf, resetTauri, setHandlers } from "../../test/tauri";
import StatsPage, { compare } from "./StatsPage";

vi.mock("@tauri-apps/api/core", async () => (await import("../../test/tauri")).coreMock);
vi.mock("@tauri-apps/api/event", async () => (await import("../../test/tauri")).eventMock);
vi.mock("@tauri-apps/api/window", async () => (await import("../../test/tauri")).windowMock);
vi.mock("@tauri-apps/api/app", async () => (await import("../../test/tauri")).appMock);
vi.mock("@tauri-apps/plugin-opener", async () => (await import("../../test/tauri")).openerMock);
// jsdom 没有 canvas：图表组件换成占位，只记录收到的 option
vi.mock("../stats/EChart", () => ({
  default: ({ testId, ariaLabel }: { testId?: string; ariaLabel: string }) => <div data-testid={testId} role="img" aria-label={ariaLabel} />,
}));

beforeEach(() => resetTauri());

const totals = (over: Partial<StatsTotals> = {}): StatsTotals => ({
  runs: 0,
  replied: 0,
  failed: 0,
  timeout: 0,
  cancelled: 0,
  active: 0,
  settled: 0,
  success_rate: 0,
  avg_duration_ms: 0,
  p90_duration_ms: 0,
  sql_count: 0,
  ...over,
});

const overview = (over: Partial<StatsOverview> = {}): StatsOverview => ({
  range: "7d",
  bot_id: null,
  start_ms: 0,
  end_ms: 1,
  totals: totals({ runs: 10, replied: 6, failed: 2, timeout: 2, settled: 10, success_rate: 0.6, avg_duration_ms: 260_000, p90_duration_ms: 600_000, sql_count: 12 }),
  previous: totals({ runs: 8, replied: 6, failed: 2, settled: 8, success_rate: 0.75, avg_duration_ms: 300_000, sql_count: 12 }),
  daily: [{ day: "11-19", start_ms: 0, runs: 10, replied: 6, failed: 2, timeout: 2 }],
  by_bot: [{ bot_id: "b1", name: "BUG纠察队", runs: 10, replied: 6, success_rate: 0.6, avg_duration_ms: 260_000, sql_count: 12 }],
  by_group: [{ key: "oc_1", name: "示例项目群", runs: 10 }],
  by_sender: [{ key: "on_1", name: "张三", runs: 10 }],
  by_engine: [{ engine: "claude", runs: 10 }],
  failure_reasons: [
    { reason: "lark-cli 未登录", count: 2 },
    { reason: "运行超时", count: 2 },
  ],
  hourly: Array.from({ length: 24 }, (_, h) => (h === 10 ? 10 : 0)),
  ...over,
});

const bots = () => [view(), view({ id: "b2", name: "需求答疑助手", engine: "codex" })];

describe("数据统计页", () => {
  test("KPI：运行数、成功率、平均耗时、查库次数与上一周期对比；默认近 7 天、全部机器人", async () => {
    setHandlers({ stats_overview: overview() });
    render(<StatsPage views={bots()} />);
    expect(await screen.findByTestId("stat-runs-value")).toHaveTextContent("10");
    expect(callsOf("stats_overview")[0]).toEqual({ range: "7d", botId: null });
    expect(screen.getByTestId("stat-runs")).toHaveTextContent("已回复 6 · 失败 2 · 超时 2");
    expect(within(screen.getByTestId("stat-runs")).getByTestId("kpi-delta")).toHaveTextContent("+25%");
    expect(screen.getByTestId("stat-rate-value")).toHaveTextContent("60%");
    expect(within(screen.getByTestId("stat-rate")).getByTestId("kpi-delta")).toHaveTextContent("-15 个百分点");
    expect(screen.getByTestId("stat-avg-value")).toHaveTextContent("4分20秒");
    expect(screen.getByTestId("stat-avg")).toHaveTextContent("P90 10分00秒");
    expect(within(screen.getByTestId("stat-avg")).getByTestId("kpi-delta")).toHaveTextContent("-13%");
    expect(screen.getByTestId("stat-sql-value")).toHaveTextContent("12");
    expect(within(screen.getByTestId("stat-sql")).getByTestId("kpi-delta")).toHaveTextContent("持平");
    // 图表与列表
    for (const id of ["chart-trend", "chart-engine", "chart-bots", "chart-hourly", "chart-groups", "chart-senders"]) {
      expect(screen.getByTestId(id)).toBeInTheDocument();
    }
    expect(screen.getByTestId("failure-reasons")).toHaveTextContent("lark-cli 未登录");
    expect(screen.getByRole("cell", { name: "BUG纠察队" })).toBeInTheDocument();
  });

  test("切换范围和机器人时按参数重新请求；选中单个机器人时不显示机器人对比", async () => {
    setHandlers({ stats_overview: (a: Record<string, unknown>) => overview({ range: a.range as StatsOverview["range"] }) });
    render(<StatsPage views={bots()} />);
    await screen.findByTestId("stat-runs-value");

    await userEvent.click(screen.getByRole("radio", { name: "今天" }));
    await waitFor(() => expect(callsOf("stats_overview").at(-1)).toEqual({ range: "today", botId: null }));
    // 今天：趋势卡改为按小时，单独的按小时分布不再重复显示
    expect(await screen.findByText("今日按小时")).toBeInTheDocument();
    expect(screen.queryByTestId("chart-hourly")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("combobox", { name: "机器人" }));
    await userEvent.click(screen.getByRole("option", { name: /需求答疑助手/ }));
    await waitFor(() => expect(callsOf("stats_overview").at(-1)).toEqual({ range: "today", botId: "b2" }));
    expect(screen.getByRole("combobox", { name: "机器人" })).toHaveTextContent("需求答疑助手");
    await waitFor(() => expect(screen.queryByTestId("chart-bots")).not.toBeInTheDocument());

    await userEvent.click(screen.getByRole("radio", { name: "近 30 天" }));
    await waitFor(() => expect(callsOf("stats_overview").at(-1)).toEqual({ range: "30d", botId: "b2" }));

    await userEvent.click(screen.getByRole("button", { name: "刷新" }));
    await waitFor(() => expect(callsOf("stats_overview").length).toBe(5));
  });

  test("没有运行记录时显示空状态，不画图", async () => {
    setHandlers({ stats_overview: overview({ totals: totals(), previous: totals(), by_bot: [], by_group: [], by_sender: [], by_engine: [], failure_reasons: [] }) });
    render(<StatsPage views={bots()} />);
    expect(await screen.findByText("这个范围内还没有运行记录")).toBeInTheDocument();
    expect(screen.getByTestId("stat-rate-value")).toHaveTextContent("—");
    expect(screen.getByTestId("stat-avg-value")).toHaveTextContent("—");
    expect(screen.queryByTestId("chart-trend")).not.toBeInTheDocument();
  });

  test("读取失败就近显示错误", async () => {
    setHandlers({ stats_overview: () => Promise.reject("运行记录数据库错误：disk I/O error") });
    render(<StatsPage views={bots()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("disk I/O error");
  });

  test("左侧图标栏的「统计」进入统计页", async () => {
    setHandlers(baseHandlers({ stats_overview: overview() }));
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "统计" }));
    expect(await screen.findByText("数据统计")).toBeInTheDocument();
    expect(await screen.findByTestId("stat-runs-value")).toHaveTextContent("10");
  });
});

describe("与上一周期对比", () => {
  test("数量按相对变化，成功率按百分点，耗时下降算好", () => {
    expect(compare(12, 10, "count")).toMatchObject({ text: "+20%", tone: "up", good: true });
    expect(compare(8, 10, "count")).toMatchObject({ text: "-20%", tone: "down", good: false });
    expect(compare(5, 0, "count")).toMatchObject({ text: "上一周期无数据", tone: "new" });
    expect(compare(0, 0, "count")).toMatchObject({ text: "持平" });
    expect(compare(0.9, 0.8, "rate")).toMatchObject({ text: "+10 个百分点", good: true });
    expect(compare(200, 300, "duration")).toMatchObject({ text: "-33%", tone: "down", good: true });
  });
});
