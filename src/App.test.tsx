import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";
import App from "./App";
import { BASE_ENV_IDS, baseHandlers, envItem, requirements, run, view } from "./test/fixtures";
import { callsOf, emitEvent, resetTauri, setHandlers, windowCalls } from "./test/tauri";

vi.mock("@tauri-apps/api/core", async () => (await import("./test/tauri")).coreMock);
vi.mock("@tauri-apps/api/event", async () => (await import("./test/tauri")).eventMock);
vi.mock("@tauri-apps/api/window", async () => (await import("./test/tauri")).windowMock);
vi.mock("@tauri-apps/plugin-opener", async () => (await import("./test/tauri")).openerMock);
vi.mock("@tauri-apps/api/app", async () => (await import("./test/tauri")).appMock);

beforeEach(() => {
  resetTauri();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.removeAttribute("data-accent");
});

describe("外壳与启停", () => {
  test("点启动调用 start_bot，bot-state 事件把状态改成运行中、按钮变停止", async () => {
    setHandlers(baseHandlers({ start_bot: undefined }));
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "启动" }));
    expect(callsOf("start_bot")).toEqual([{ id: "b1" }]);

    emitEvent("bot-state", { bot_id: "b1", state: { kind: "running" } });
    expect(await screen.findByRole("button", { name: "停止" })).toBeInTheDocument();
    expect(screen.getAllByText("运行中").length).toBeGreaterThan(0);
    expect(screen.getByText(/长连接已建立 · 自 \d\d:\d\d 起/)).toBeInTheDocument();
  });

  test("点停止显示停止中，收到 stopped 后回到已停止", async () => {
    setHandlers(baseHandlers({ list_bots: [view({}, { kind: "running" })], stop_bot: undefined }));
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "停止" }));
    expect(callsOf("stop_bot")).toEqual([{ id: "b1" }]);
    expect(await screen.findByRole("button", { name: "停止中…" })).toBeDisabled();

    emitEvent("bot-state", { bot_id: "b1", state: { kind: "stopped" } });
    expect(await screen.findByRole("button", { name: "启动" })).toBeInTheDocument();
  });

  test("启动失败时显示后端的中文错误", async () => {
    setHandlers(baseHandlers({ start_bot: () => Promise.reject("App Secret 不正确") }));
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "启动" }));
    expect(await screen.findByRole("status")).toHaveTextContent("App Secret 不正确");
  });

  test("bot-state 异常时显示异常原因", async () => {
    setHandlers(baseHandlers());
    render(<App />);
    await screen.findByRole("button", { name: "启动" });
    emitEvent("bot-state", { bot_id: "b1", state: { kind: "error", message: "长连接断开" } });
    expect(await screen.findByText("运行异常：长连接断开")).toBeInTheDocument();
  });

  test("环境有 error 项：环境图标显示角标，启动按钮禁用，去修复跳到环境页", async () => {
    setHandlers(
      baseHandlers({
        env_check: [envItem({ status: "error", label: "Python 3.10+" }), envItem({ id: "proxy", status: "warn", label: "系统代理变量" }), envItem({ id: "git", label: "Git" })],
      }),
    );
    render(<App />);
    expect(await screen.findByTestId("env-badge")).toHaveTextContent("2");
    expect(await screen.findByRole("button", { name: "无法启动" })).toBeDisabled();
    expect(screen.getByText(/环境检查有 1 项未通过（Python 3.10\+）/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /环境异常，无法启动/ })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "去修复" }));
    expect(await screen.findByTestId("env-summary")).toHaveTextContent("1 项异常、1 项警告");
    expect(screen.getByTestId("env-row-python")).toHaveTextContent("Python 3.10+");
  });

  test("审查 B1：只装 Claude Code（codex 一项报错）时，Claude 机器人照常可以启动", async () => {
    setHandlers(
      baseHandlers({
        env_check: [envItem({ id: "claude", label: "Claude Code CLI" }), envItem({ id: "codex", label: "Codex CLI", status: "error" })],
        start_bot: undefined,
      }),
    );
    render(<App />);
    const btn = await screen.findByRole("button", { name: "启动" });
    await waitFor(() => expect(callsOf("env_requirements").length).toBeGreaterThan(0));
    expect(screen.queryByRole("button", { name: "无法启动" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /环境异常，无法启动/ })).not.toBeInTheDocument();
    await userEvent.click(btn);
    expect(callsOf("start_bot")).toEqual([{ id: "b1" }]);
  });

  test("Codex 机器人缺 codex 时才被拦", async () => {
    setHandlers(
      baseHandlers({
        list_bots: [view({ engine: "codex" })],
        env_requirements: requirements({ b1: ["codex", ...BASE_ENV_IDS] }),
        env_check: [envItem({ id: "codex", label: "Codex CLI", status: "error" })],
      }),
    );
    render(<App />);
    expect(await screen.findByRole("button", { name: "无法启动" })).toBeDisabled();
    expect(screen.getByText(/环境检查有 1 项未通过（Codex CLI）/)).toBeInTheDocument();
  });

  test("审查 B2：异常但仍在重试（alive）时显示停止，可以停掉", async () => {
    setHandlers(
      baseHandlers({
        list_bots: [view({}, { kind: "error", message: "连接失败，正在重试：超时" }, 0, true)],
        stop_bot: undefined,
      }),
    );
    render(<App />);
    const stop = await screen.findByRole("button", { name: "停止" });
    // 重试中同时提供「立即重连」；点停止后进入停止中，重连按钮随之消失
    expect(screen.getByRole("button", { name: "立即重连" })).toBeInTheDocument();
    await userEvent.click(stop);
    expect(callsOf("stop_bot")).toEqual([{ id: "b1" }]);
  });

  test("异常且连接已结束（alive=false）时显示启动", async () => {
    setHandlers(baseHandlers({ list_bots: [view({}, { kind: "error", message: "飞书应用已被删除" }, 0, false)] }));
    render(<App />);
    expect(await screen.findByRole("button", { name: "启动" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "立即重连" })).not.toBeInTheDocument();
  });

  test("启动后后端没有任何状态事件时，按后端状态校正，不会卡在连接中", async () => {
    let started = false;
    setHandlers(
      baseHandlers({
        list_bots: () => [view({}, started ? { kind: "running" } : { kind: "stopped" })],
        start_bot: () => {
          started = true;
        },
      }),
    );
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "启动" }));
    expect(await screen.findByRole("button", { name: "停止" })).toBeInTheDocument();
  });

  test("phase23-issues 第 14 条：标题栏和查 bug 机器人头像用 beetle-8 品牌图标", async () => {
    setHandlers(baseHandlers());
    render(<App />);
    await screen.findByRole("button", { name: "启动" });
    const logos = screen.getAllByTestId("brand-logo");
    // 标题栏 + 列表头像 + 工作台头像
    expect(logos.length).toBeGreaterThanOrEqual(3);
    // Vite 会把小 SVG 内联成 data URI，按文件名或 logo 底色 #F5F0E6 识别
    for (const img of logos) expect(img.getAttribute("src")).toMatch(/beetle-8|F5F0E6/);
  });

  test("随客户端启动开关调用 set_start_with_client", async () => {
    setHandlers(baseHandlers({ set_start_with_client: undefined }));
    render(<App />);
    await userEvent.click(await screen.findByRole("switch", { name: "随客户端启动" }));
    expect(callsOf("set_start_with_client")).toEqual([{ id: "b1", on: true }]);
  });

  test("标题栏按钮调用窗口 API（关闭 = 交给后端隐藏到托盘）", async () => {
    setHandlers(baseHandlers());
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "最小化" }));
    await userEvent.click(screen.getByRole("button", { name: "最大化" }));
    await userEvent.click(screen.getByRole("button", { name: "关闭" }));
    await waitFor(() => expect(windowCalls.close).toHaveBeenCalled());
    expect(windowCalls.minimize).toHaveBeenCalled();
    expect(windowCalls.toggleMaximize).toHaveBeenCalled();
  });

  test("运行记录变化时刷新列表的今日运行数", async () => {
    setHandlers(baseHandlers());
    render(<App />);
    await screen.findByRole("button", { name: "启动" });
    const before = callsOf("list_bots").length;
    emitEvent("run-updated", { run: run() });
    await waitFor(() => expect(callsOf("list_bots").length).toBeGreaterThan(before));
  });
});

describe("外观", () => {
  test("启动时应用后端保存的外观", async () => {
    setHandlers(baseHandlers({ get_appearance: { mode: "light", accent: "purple" } }));
    render(<App />);
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
    expect(document.documentElement.dataset.accent).toBe("purple");
  });

  test("标题栏浮层切换深浅色与主题色，并经 set_appearance 持久化", async () => {
    setHandlers(baseHandlers());
    render(<App />);
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("dark"));
    await userEvent.click(screen.getByRole("button", { name: /外观：切换深浅色/ }));
    const pop = screen.getByRole("dialog", { name: "外观" });
    await userEvent.click(within(pop).getByRole("radio", { name: /浅色/ }));
    expect(document.documentElement.dataset.theme).toBe("light");
    await userEvent.click(within(pop).getByRole("radio", { name: "主题色：蓝" }));
    expect(document.documentElement.dataset.accent).toBe("blue");
    expect(callsOf("set_appearance")).toEqual([
      { appearance: { mode: "light", accent: "vermilion" } },
      { appearance: { mode: "light", accent: "blue" } },
    ]);
  });

  test("设置页外观分组：跟随系统", async () => {
    setHandlers(baseHandlers());
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "设置" }));
    const seg = await screen.findByRole("radiogroup", { name: "外观模式" });
    await userEvent.click(within(seg).getByRole("radio", { name: /跟随系统/ }));
    expect(callsOf("set_appearance").slice(-1)[0]).toEqual({ appearance: { mode: "system", accent: "vermilion" } });
    expect(within(seg).getByRole("radio", { name: /跟随系统/ })).toHaveAttribute("aria-checked", "true");
  });
});

describe("工作台 tab", () => {
  test("概览显示 bot_stats 的三个数和群列表", async () => {
    setHandlers(baseHandlers({ bot_stats: { today_runs: 7, today_replied: 4, success_rate: 0.67, avg_duration_ms: 292_000 } }));
    render(<App />);
    await waitFor(() => expect(screen.getByTestId("kpi-today")).toHaveTextContent("7"));
    expect(screen.getByTestId("kpi-rate")).toHaveTextContent("67%");
    expect(screen.getByTestId("kpi-avg")).toHaveTextContent("4分52秒");
    expect(screen.getByRole("cell", { name: "运营群" })).toBeInTheDocument();
    // 「仅群主」统一改叫「仅机器人主人」
    expect(screen.getByRole("cell", { name: "仅机器人主人" })).toBeInTheDocument();
    expect(screen.queryByText("仅群主")).not.toBeInTheDocument();
  });

  test("群：改谁能触发调用 save_group，移除调用 remove_group", async () => {
    setHandlers(baseHandlers({ save_group: undefined, remove_group: undefined }));
    vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /群/ }));
    const card = await screen.findByTestId("group-oc_1");
    await userEvent.click(within(card).getByRole("radio", { name: "群内所有人" }));
    expect(callsOf("save_group")[0]).toMatchObject({ botId: "b1", group: { chat_id: "oc_1", trigger: "all" } });
    await userEvent.click(within(card).getByRole("button", { name: "移除群 运营群" }));
    expect(callsOf("remove_group")).toEqual([{ botId: "b1", chatId: "oc_1" }]);
  });

  test("群：自动接入开关经 save_bot 保存", async () => {
    setHandlers(baseHandlers({ save_bot: (a: Record<string, unknown>) => a.bot }));
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /群/ }));
    await userEvent.click(await screen.findByRole("switch", { name: "自动接入新群" }));
    expect(callsOf("save_bot")[0]).toMatchObject({ bot: { id: "b1", auto_onboard: false }, appSecret: null });
  });

  test("设置：保存带上密钥与禁止清单，留空密钥表示不修改", async () => {
    setHandlers(baseHandlers({ save_bot: (a: Record<string, unknown>) => a.bot }));
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /设置/ }));
    await userEvent.clear(screen.getByLabelText("名称"));
    await userEvent.type(screen.getByLabelText("名称"), "新名字");
    await userEvent.type(screen.getByLabelText("App Secret"), "sec");
    await userEvent.type(screen.getByLabelText("禁止清单"), "Bash(rm:*)");
    await userEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(callsOf("save_bot")).toHaveLength(1));
    expect(callsOf("save_bot")[0]).toMatchObject({ bot: { name: "新名字", extra_deny: ["Bash(rm:*)"] }, appSecret: "sec" });
    expect(screen.getByLabelText("App Secret")).toHaveValue("");
  });

  test("群：「添加项目」选项目后合并进所选群的负责项目", async () => {
    const proj = (repo: string) => ({ repo, branch_glob: "release/*", rules_file: "", description: "", dbs: [] });
    setHandlers(
      baseHandlers({
        list_projects: { projects: { demo: proj("r1"), "jev-rpa": proj("r2") }, dbs: {}, default_projects: [] },
        save_group: undefined,
      }),
    );
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /群/ }));
    await userEvent.click(screen.getByRole("button", { name: "添加项目" }));
    const dialog = screen.getByRole("dialog", { name: "添加项目" });
    // 群里已有的 demo 默认选中
    expect(within(dialog).getByRole("button", { name: "demo" })).toHaveAttribute("aria-pressed", "true");
    expect(within(dialog).getByRole("button", { name: "jev-rpa" })).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(within(dialog).getByRole("button", { name: "jev-rpa" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "保存" }));
    await waitFor(() => expect(callsOf("save_group")).toHaveLength(1));
    expect(callsOf("save_group")[0]).toMatchObject({ botId: "b1", group: { chat_id: "oc_1", projects: ["demo", "jev-rpa"] } });
  });

  test("群：「添加项目」里取消已有项目会从群里移除", async () => {
    setHandlers(baseHandlers({ save_group: undefined }));
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /群/ }));
    await userEvent.click(screen.getByRole("button", { name: "添加项目" }));
    const dialog = screen.getByRole("dialog", { name: "添加项目" });
    await userEvent.click(within(dialog).getByRole("button", { name: "demo" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "保存" }));
    await waitFor(() => expect(callsOf("save_group")).toHaveLength(1));
    expect(callsOf("save_group")[0]).toMatchObject({ group: { chat_id: "oc_1", projects: [] } });
  });

  test("群：群卡片「负责项目」后面有「添加」，点了进入编辑", async () => {
    setHandlers(baseHandlers());
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /群/ }));
    await userEvent.click(screen.getByRole("button", { name: "给 运营群 添加项目" }));
    expect(screen.getByRole("group", { name: "负责项目" }) ?? screen.getByLabelText("负责项目")).toBeInTheDocument();
  });

  test("设置：运行中的机器人保存后，提示里有「立即重启」，点了先停再启", async () => {
    setHandlers(
      baseHandlers({
        list_bots: [view({}, { kind: "running", since: 1 } as never)],
        save_bot: (a: Record<string, unknown>) => a.bot,
        stop_bot: undefined,
        start_bot: undefined,
      }),
    );
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /设置/ }));
    await userEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(await screen.findByText("已保存，重启机器人后生效")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "立即重启" }));
    await waitFor(() => expect(callsOf("start_bot")).toEqual([{ id: "b1" }]));
    expect(callsOf("stop_bot")).toEqual([{ id: "b1" }]);
    expect(await screen.findByText("机器人已重启，新配置已生效")).toBeInTheDocument();
  });

  test("设置：删除机器人需确认", async () => {
    setHandlers(baseHandlers({ delete_bot: undefined }));
    vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /设置/ }));
    await userEvent.click(screen.getByRole("button", { name: "删除" }));
    expect(callsOf("delete_bot")).toEqual([{ id: "b1" }]);
  });
});
