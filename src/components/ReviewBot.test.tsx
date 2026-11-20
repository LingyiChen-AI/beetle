import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";
import App from "../App";
import { cardLines, cardTitle, parseReviewReply, type ReviewResult } from "../review/card";
import { baseHandlers, run, step, view } from "../test/fixtures";
import { callsOf, resetTauri, setHandlers } from "../test/tauri";
import NewBotWizard, { newBot, withTemplate } from "./wizard/NewBotWizard";

vi.mock("@tauri-apps/api/core", async () => (await import("../test/tauri")).coreMock);
vi.mock("@tauri-apps/api/event", async () => (await import("../test/tauri")).eventMock);
vi.mock("@tauri-apps/api/window", async () => (await import("../test/tauri")).windowMock);
vi.mock("@tauri-apps/api/app", async () => (await import("../test/tauri")).appMock);
vi.mock("@tauri-apps/plugin-opener", async () => (await import("../test/tauri")).openerMock);

beforeEach(() => resetTauri());

const REVIEW: ReviewResult = {
  repo: "demo-service",
  pr: 29,
  title: "feat: 售后工单自动领取",
  conclusion: "有一处并发问题需要先修。",
  findings: [
    { severity: "low", status: "new", text: "日志打印了完整请求体 a.go:12" },
    { severity: "medium", status: "carried", text: "判重查询走了从库 b.go:40" },
    { severity: "high", status: "new", text: "领取接口没有加锁 c.go:88" },
    { severity: "medium", status: "resolved", text: "超时没有重试 d.go:7" },
  ],
  additions: 11822,
  deletions: 109,
  files: 69,
  author: "张三",
  url: "https://gitee.com/example/demo-service/pulls/29",
};

describe("触发方式", () => {
  test("设置里切到「群里发 PR 链接自动触发」并保存", async () => {
    setHandlers(baseHandlers({ save_bot: (a: Record<string, unknown>) => a.bot }));
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /设置/ }));
    const group = screen.getByRole("radiogroup", { name: "触发方式" });
    expect(within(group).getByRole("radio", { name: "@ 机器人" })).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByText(/获取群组中所有消息/)).not.toBeInTheDocument();
    await userEvent.click(within(group).getByRole("radio", { name: "群里发 PR 链接自动触发" }));
    expect(screen.getByText(/获取群组中所有消息/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(callsOf("save_bot")).toHaveLength(1));
    expect(callsOf("save_bot")[0]).toMatchObject({ bot: { id: "b1", trigger_mode: "pr_link" } });
  });

  test("旧配置没有 trigger_mode 时按 @ 机器人显示，保存补上 mention", async () => {
    setHandlers(baseHandlers({ list_bots: [view({ trigger_mode: undefined })], save_bot: (a: Record<string, unknown>) => a.bot }));
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /设置/ }));
    expect(screen.getByRole("radio", { name: "@ 机器人" })).toHaveAttribute("aria-checked", "true");
    await userEvent.click(screen.getByRole("radio", { name: "两者都可" }));
    await userEvent.click(screen.getByRole("radio", { name: "@ 机器人" }));
    await userEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(callsOf("save_bot")).toHaveLength(1));
    expect(callsOf("save_bot")[0]).toMatchObject({ bot: { trigger_mode: "mention" } });
  });
});

describe("新增机器人向导：代码审查模板", () => {
  test("选代码审查模板：默认 PR 链接触发、技能 PR 审查 + 代码同步；换回别的模板恢复默认", () => {
    const b = withTemplate(newBot(), "code-review");
    expect(b).toMatchObject({ template: "code-review", trigger_mode: "pr_link", skills: ["pr-review", "code-sync"] });
    const back = withTemplate(b, "bug-hunt");
    expect(back).toMatchObject({ template: "bug-hunt", trigger_mode: "mention" });
    expect(back.skills).toEqual(newBot().skills);
    // 不涉及代码审查的切换不动技能
    const qa = withTemplate({ ...newBot(), skills: ["code-sync"] }, "qa");
    expect(qa).toMatchObject({ template: "qa", trigger_mode: "mention", skills: ["code-sync"] });
  });

  test("走完向导：保存的机器人带代码审查默认值，完成页显示触发方式", async () => {
    setHandlers({
      list_templates: [
        { name: "bug-hunt", builtin: true, modified: false },
        { name: "qa", builtin: true, modified: false },
        { name: "code-review", builtin: true, modified: false },
      ],
      list_skills: [
        { name: "pr-review", display_name: "PR 审查", description: "", requires: [], builtin: true },
        { name: "code-sync", display_name: "代码同步", description: "", requires: [], builtin: true },
        { name: "db-query", display_name: "数据库只读查询", description: "", requires: [], builtin: true },
      ],
      list_projects: { projects: {}, dbs: {}, default_projects: [] },
      save_bot: (a: Record<string, unknown>) => ({ ...(a.bot as object), id: "b9" }),
      save_group: undefined,
      feishu_cancel_create_app: undefined,
    });
    render(<NewBotWizard onClose={() => {}} onCreated={() => {}} />);
    await userEvent.click(screen.getByRole("radio", { name: "手动填写 App ID / Secret" }));
    await userEvent.type(screen.getByLabelText("App ID"), "cli_review");
    await userEvent.type(screen.getByLabelText("App Secret"), "sec");
    await userEvent.type(screen.getByLabelText("机器人名称"), "代码审查");
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));

    await userEvent.click(await screen.findByRole("radio", { name: /代码审查/ }));
    const modes = screen.getByRole("radiogroup", { name: "触发方式" });
    expect(within(modes).getByRole("radio", { name: "群里发 PR 链接自动触发" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("checkbox", { name: "PR 审查" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("checkbox", { name: "代码同步" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("checkbox", { name: "数据库只读查询" })).toHaveAttribute("aria-checked", "false");
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
    await userEvent.click(screen.getByRole("button", { name: "完成创建" }));

    await waitFor(() => expect(callsOf("save_bot")).toHaveLength(1));
    expect(callsOf("save_bot")[0]).toMatchObject({
      bot: { template: "code-review", trigger_mode: "pr_link", skills: ["pr-review", "code-sync"] },
    });
    const summary = await screen.findByTestId("wizard-summary");
    expect(summary).toHaveTextContent("代码审查");
    expect(summary).toHaveTextContent("群里发 PR 链接自动触发");
  });
});

describe("审查结果卡片", () => {
  test("卡片布局与后端一致：标题、概要、结论、编号发现、已处理、查看 PR", () => {
    expect(cardTitle(REVIEW)).toBe("代码审查 demo-service #29，3 项发现");
    const lines = cardLines(REVIEW).map((l) => `${l.kind}:` + (l.index ? `${l.index}. ` : "") + (l.severity ? `[${l.severity}] ` : "") + l.text);
    expect(lines).toEqual([
      "title:feat: 售后工单自动领取",
      "meta:demo-service #29 · 作者 张三 · +11822 / -109 · 69 个文件",
      "divider:",
      "heading:结论",
      "text:有一处并发问题需要先修。",
      "divider:",
      "heading:发现 3 项",
      "finding:1. [high] （新发现）领取接口没有加锁 c.go:88",
      "finding:2. [medium] （上一轮遗留，未处理）判重查询走了从库 b.go:40",
      "finding:3. [low] （新发现）日志打印了完整请求体 a.go:12",
      "heading:上一轮已处理 1 项",
      "resolved:[medium] 超时没有重试 d.go:7",
      "link:https://gitee.com/example/demo-service/pulls/29",
    ]);
    // 有覆盖率时出底注
    const cov = cardLines({ ...REVIEW, coverage: { total: 6, reviewed: 5, skipped: [{ path: "a.sql", reason: "无法取 diff" }] } });
    expect(cov.find((l) => l.kind === "note")?.text).toBe("审查覆盖：5/6 个文件，跳过 1 个（a.sql:无法取 diff）");
    const clean = { ...REVIEW, findings: [] };
    expect(cardTitle(clean)).toBe("代码审查 demo-service #29，未发现问题");
    expect(cardLines(clean).some((l) => l.text === "未发现问题")).toBe(true);
    // 不是审查结果的回复不当卡片
    expect(parseReviewReply("结论：从库延迟")).toBeNull();
    expect(parseReviewReply('{"repo":"a","pr":0,"findings":[]}')).toBeNull();
    expect(parseReviewReply(JSON.stringify(REVIEW))).toEqual(REVIEW);
  });

  test("运行日志里代码审查运行的回复按卡片样式预览", async () => {
    const done = run({ status: "replied", finished_at: Date.now(), text: REVIEW.url as string, final_reply: JSON.stringify(REVIEW) });
    setHandlers(
      baseHandlers({
        list_bots: [view({ template: "code-review", trigger_mode: "pr_link", skills: ["pr-review", "code-sync"] })],
        list_runs: [done],
        get_run: { run: done, steps: [step(1, "回复群里", { kind: "reply" })] },
      }),
    );
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /运行日志/ }));
    const card = await screen.findByTestId("review-card");
    expect(within(card).getByTestId("review-card-title")).toHaveTextContent("代码审查 demo-service #29，3 项发现");
    expect(within(card).getAllByTestId("review-finding")).toHaveLength(4);
    expect(card).toHaveTextContent("（上一轮遗留，未处理）判重查询走了从库 b.go:40");
    expect(card).toHaveTextContent("demo-service #29 · 作者 张三 · +11822 / -109 · 69 个文件");
    expect(card).toHaveTextContent("查看 PR");
    expect(screen.queryByTestId("reply-bubble")).not.toBeInTheDocument();
  });
});
