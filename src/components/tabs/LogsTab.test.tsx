import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";
import App from "../../App";
import { baseHandlers, run, step } from "../../test/fixtures";
import { callsOf, emitEvent, resetTauri, setHandlers } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", async () => (await import("../../test/tauri")).coreMock);
vi.mock("@tauri-apps/api/event", async () => (await import("../../test/tauri")).eventMock);
vi.mock("@tauri-apps/api/window", async () => (await import("../../test/tauri")).windowMock);
vi.mock("@tauri-apps/plugin-opener", async () => (await import("../../test/tauri")).openerMock);

beforeEach(() => resetTauri());

async function openLogs(extra: Record<string, unknown>) {
  setHandlers(baseHandlers(extra));
  render(<App />);
  await userEvent.click(await screen.findByRole("tab", { name: /运行日志/ }));
  return screen.findByTestId("run-detail");
}

const titles = () => screen.queryAllByTestId("step-title").map((e) => e.textContent);

describe("运行日志", () => {
  test("显示步骤流，run-step 事件实时追加（按 seq 去重排序）", async () => {
    await openLogs({
      list_runs: [run()],
      get_run: { run: run(), steps: [step(1, "收到 @ 消息"), step(2, "读群聊")] },
    });
    await waitFor(() => expect(titles()).toEqual(["收到 @ 消息", "读群聊"]));
    expect(screen.getByTestId("live-tail")).toHaveTextContent("Claude 分析中");
    expect(screen.getByTestId("reply-skeleton")).toBeInTheDocument();

    emitEvent("run-step", { step: step(4, "查库 #1", { kind: "sql" }) });
    emitEvent("run-step", { step: step(3, "同步代码") });
    emitEvent("run-step", { step: step(3, "同步代码") });
    emitEvent("run-step", { step: step(9, "别的运行", { run_id: "other" }) });
    expect(titles()).toEqual(["收到 @ 消息", "读群聊", "同步代码", "查库 #1"]);
  });

  test("run-updated 不会把运行中回退成排队中；终态后显示回复与重试", async () => {
    await openLogs({
      list_runs: [run()],
      get_run: { run: run(), steps: [] },
      retry_run: undefined,
    });
    const item = await screen.findByTestId("run-item-r1");
    emitEvent("run-updated", { run: run({ status: "queued" }) });
    expect(within(item).getByTestId("run-chip")).toHaveTextContent("运行中");

    emitEvent("run-updated", { run: run({ status: "failed", finished_at: Date.now(), error: "Agent 进程异常退出", final_reply: null }) });
    expect(within(item).getByTestId("run-chip")).toHaveTextContent("失败");
    expect(screen.getByText("Agent 进程异常退出")).toBeInTheDocument();
    expect(screen.queryByTestId("live-tail")).not.toBeInTheDocument();

    emitEvent("run-updated", { run: run({ status: "running" }) });
    expect(within(item).getByTestId("run-chip")).toHaveTextContent("失败");

    await userEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(callsOf("retry_run")).toEqual([{ id: "r1" }]);
  });

  test("已回复的运行显示飞书气泡里的最终回复", async () => {
    const done = run({ status: "replied", finished_at: Date.now(), final_reply: "结论：从库延迟\n@张三 已写到记录文档" });
    await openLogs({ list_runs: [done], get_run: { run: done, steps: [step(1, "回复群里", { kind: "reply" })] } });
    const bubble = await screen.findByTestId("reply-bubble");
    expect(bubble).toHaveTextContent("结论：从库延迟");
    expect(within(bubble).getByText("@张三")).toHaveClass("at");
    expect(screen.getByRole("button", { name: "重新运行" })).toBeInTheDocument();
  });

  test("原始日志切换：调用 get_raw_log 显示 stdout / stderr，再切回步骤流", async () => {
    await openLogs({
      list_runs: [run({ status: "failed", finished_at: Date.now() })],
      get_run: { run: run({ status: "failed" }), steps: [step(1, "启动")] },
      get_raw_log: { stdout: '{"type":"system"}', stderr: "Error: Invalid API key" },
    });
    await userEvent.click(await screen.findByRole("button", { name: "原始日志" }));
    expect(callsOf("get_raw_log")).toEqual([{ runId: "r1" }]);
    expect(await screen.findByTestId("raw-stdout")).toHaveTextContent('{"type":"system"}');
    expect(screen.getByTestId("raw-stderr")).toHaveTextContent("Invalid API key");
    expect(titles()).toEqual([]);
    await userEvent.click(screen.getByRole("button", { name: "步骤流" }));
    expect(titles()).toEqual(["启动"]);
  });

  test("运行中可取消", async () => {
    await openLogs({ list_runs: [run()], get_run: { run: run(), steps: [] }, cancel_run: undefined });
    await userEvent.click(await screen.findByRole("button", { name: "取消运行" }));
    expect(callsOf("cancel_run")).toEqual([{ id: "r1" }]);
  });

  test("时间范围与状态筛选", async () => {
    const old = run({ id: "r2", status: "replied", started_at: Date.now() - 3 * 86_400_000, finished_at: Date.now() - 3 * 86_400_000 + 1000, text: "三天前的问题" });
    await openLogs({
      list_runs: [run(), old],
      get_run: (a: Record<string, unknown>) => ({ run: a.id === "r2" ? old : run(), steps: [] }),
    });
    expect(screen.queryByTestId("run-item-r2")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("radio", { name: "近 7 天" }));
    expect(screen.getByTestId("run-item-r2")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "按状态筛选" }));
    await userEvent.click(screen.getByRole("menuitemradio", { name: "已回复" }));
    expect(screen.queryByTestId("run-item-r1")).not.toBeInTheDocument();
    expect(screen.getByTestId("run-item-r2")).toBeInTheDocument();
  });

  test("新运行通过 run-updated 插到列表最前", async () => {
    await openLogs({ list_runs: [run()], get_run: { run: run(), steps: [] } });
    await screen.findByTestId("run-item-r1");
    emitEvent("run-updated", { run: run({ id: "r9", status: "queued", text: "新的问题", started_at: Date.now() }) });
    const items = within(screen.getByTestId("run-list")).getAllByRole("button");
    expect(items[0]).toHaveTextContent("新的问题");
    expect(items[0]).toHaveTextContent("排队中");
  });
});
