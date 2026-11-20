import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";
import App from "../../App";
import { baseHandlers, envItem, requirements } from "../../test/fixtures";
import { callsOf, resetTauri, setHandlers } from "../../test/tauri";
import NewBotWizard from "./NewBotWizard";
import Onboarding from "./Onboarding";

vi.mock("@tauri-apps/api/core", async () => (await import("../../test/tauri")).coreMock);
vi.mock("@tauri-apps/api/event", async () => (await import("../../test/tauri")).eventMock);
vi.mock("@tauri-apps/api/window", async () => (await import("../../test/tauri")).windowMock);
vi.mock("@tauri-apps/api/app", async () => (await import("../../test/tauri")).appMock);
vi.mock("@tauri-apps/plugin-opener", async () => (await import("../../test/tauri")).openerMock);

beforeEach(() => resetTauri());

const wizardHandlers = (extra: Record<string, unknown> = {}) => ({
  list_templates: [
    { name: "bug-hunt", builtin: true, modified: false },
    { name: "qa", builtin: true, modified: false },
  ],
  list_skills: [
    { name: "db-query", display_name: "查库（只读）", description: "", requires: [], builtin: true },
    { name: "code-sync", display_name: "代码同步", description: "", requires: [], builtin: true },
  ],
  list_projects: { projects: { demo: { repo: "r", branch_glob: "release/*", rules_file: "AGENTS.md", description: "", dbs: [] } }, dbs: {}, default_projects: [] },
  save_bot: (a: Record<string, unknown>) => ({ ...(a.bot as object), id: "b9" }),
  save_group: undefined,
  start_bot: undefined,
  feishu_cancel_create_app: undefined,
  ...extra,
});

describe("新增机器人向导 · 绑定群", () => {
  async function gotoStep3(extra: Record<string, unknown> = {}, onGoProjects?: () => void) {
    setHandlers(wizardHandlers(extra));
    render(<NewBotWizard onClose={() => {}} onCreated={() => {}} onGoProjects={onGoProjects} />);
    await userEvent.click(screen.getByRole("radio", { name: "手动填写 App ID / Secret" }));
    await userEvent.type(screen.getByLabelText("App ID"), "cli_new");
    await userEvent.type(screen.getByLabelText("App Secret"), "sec");
    await userEvent.type(screen.getByLabelText("机器人名称"), "助手");
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
  }

  test("谁能触发是「仅机器人主人 / 群内所有人」并带说明；自动接入说明；chat_id 有格式提示", async () => {
    await gotoStep3();
    expect(screen.getByRole("radio", { name: "仅机器人主人" })).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByText("仅群主")).not.toBeInTheDocument();
    expect(screen.getByRole("img", { name: /机器人主人 = 安装本客户端、登录 lark-cli 的人/ })).toBeInTheDocument();
    expect(screen.getByText("机器人被拉进新群后，由你 @ 一次即自动登记")).toBeInTheDocument();
    expect(screen.getByLabelText("群 chat_id")).toHaveAccessibleDescription("chat_id 以 oc_ 开头，可在飞书群设置里复制");
    expect(screen.getByText("负责项目（可多选）")).toBeInTheDocument();
    const chip = screen.getByRole("button", { name: "demo" });
    expect(chip).toHaveAttribute("aria-pressed", "false");
    expect(chip).toHaveAttribute("title", "点击选择");
    await userEvent.click(chip);
    expect(chip).toHaveAttribute("aria-pressed", "true");
    expect(chip).toHaveAttribute("title", "已选中，点击取消");
  });

  test("添加的群显示成卡片，可移除；没有项目时给「去添加项目」", async () => {
    const onGoProjects = vi.fn();
    await gotoStep3({ list_projects: { projects: {}, dbs: {}, default_projects: [] } }, onGoProjects);
    expect(screen.getByText("还没有手动登记的群")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("群 chat_id"), "oc_abc12345");
    await userEvent.type(screen.getByLabelText("群名称"), "运营群");
    await userEvent.click(screen.getByRole("button", { name: "添加群" }));
    const list = screen.getByTestId("wizard-groups");
    expect(list).toHaveTextContent("运营群");
    expect(list).toHaveTextContent("oc_abc12345");
    expect(list).toHaveTextContent("用默认项目");
    expect(screen.getByLabelText("群 chat_id")).toHaveValue("");
    await userEvent.click(within(list).getByRole("button", { name: "移除 运营群" }));
    expect(screen.queryByTestId("wizard-groups")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "去添加项目" }));
    expect(onGoProjects).toHaveBeenCalled();
  });
});

describe("新增机器人向导", () => {
  test("手填 App ID / Secret 走完 4 步：保存机器人、登记群、立即启动", async () => {
    setHandlers(wizardHandlers());
    const onCreated = vi.fn();
    render(<NewBotWizard onClose={() => {}} onCreated={onCreated} />);

    await userEvent.click(screen.getByRole("radio", { name: "手动填写 App ID / Secret" }));
    const next = screen.getByRole("button", { name: "下一步" });
    expect(next).toBeDisabled();
    expect(screen.getByText(/同一个飞书应用只能在一台电脑上连接/)).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("App ID"), "cli_new");
    await userEvent.type(screen.getByLabelText("App Secret"), "sec");
    await userEvent.type(screen.getByLabelText("机器人名称"), "线上问题助手");
    await userEvent.click(screen.getByRole("radio", { name: "Codex" }));
    expect(next).toBeEnabled();
    await userEvent.click(next);

    // ② 模板与技能
    await userEvent.click(await screen.findByRole("radio", { name: /需求答疑/ }));
    await userEvent.click(screen.getByRole("checkbox", { name: "代码同步" }));
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));

    // ③ 绑定群
    await userEvent.click(screen.getByRole("switch", { name: "自动接入新群" }));
    await userEvent.click(screen.getByRole("radio", { name: "群内所有人" }));
    await userEvent.type(screen.getByLabelText("群 chat_id"), "oc_12345678abcd");
    await userEvent.type(screen.getByLabelText("群名称"), "运营群");
    await userEvent.click(screen.getByRole("button", { name: "demo" }));
    await userEvent.click(screen.getByRole("button", { name: "添加群" }));
    await userEvent.click(screen.getByRole("button", { name: "完成创建" }));

    await waitFor(() => expect(callsOf("save_bot")).toHaveLength(1));
    expect(callsOf("save_bot")[0]).toMatchObject({
      bot: { name: "线上问题助手", app_id: "cli_new", engine: "codex", template: "qa", auto_onboard: false, groups: [] },
      appSecret: "sec",
      attempt: null,
    });
    expect((callsOf("save_bot")[0].bot as { skills: string[] }).skills).not.toContain("code-sync");
    await waitFor(() =>
      expect(callsOf("save_group")).toEqual([
        { botId: "b9", group: { chat_id: "oc_12345678abcd", name: "运营群", slug: "beetle-5678abcd", projects: ["demo"], doc_url: null, trigger: "all" } },
      ]),
    );

    // ④ 完成
    const summary = await screen.findByTestId("wizard-summary");
    expect(summary).toHaveTextContent("cli_new");
    expect(summary).toHaveTextContent("需求答疑");
    expect(summary).toHaveTextContent("1 个");
    await userEvent.click(screen.getByRole("button", { name: "立即启动" }));
    expect(callsOf("start_bot")).toEqual([{ id: "b9" }]);
    expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "b9" }), true);
  });

  test("扫码创建：拿到 App ID 后自动填名称，保存时带 attempt 不带密钥", async () => {
    setHandlers(
      wizardHandlers({
        feishu_create_app: (a: Record<string, unknown>) => {
          (a.onUrl as { onmessage: (u: string) => void }).onmessage("https://open.feishu.cn/x");
          return { appId: "cli_scan", botName: "扫码机器人" };
        },
      }),
    );
    render(<NewBotWizard onClose={() => {}} onCreated={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: "开始扫码创建" }));
    expect(await screen.findByTestId("scan-done")).toHaveTextContent("cli_scan");
    expect(screen.getByLabelText("机器人名称")).toHaveValue("扫码机器人");
    const attempt = callsOf("feishu_create_app")[0].attempt;

    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
    await userEvent.click(screen.getByRole("button", { name: "完成创建" }));
    await waitFor(() => expect(callsOf("save_bot")).toHaveLength(1));
    expect(callsOf("save_bot")[0]).toMatchObject({ bot: { app_id: "cli_scan" }, appSecret: null, attempt });
    expect(typeof attempt).toBe("string");
  });

  test("扫码创建：等待确认时显示授权链接的二维码，可复制链接", async () => {
    let finish: (v: unknown) => void = () => {};
    setHandlers(
      wizardHandlers({
        feishu_create_app: (a: Record<string, unknown>) => {
          (a.onUrl as { onmessage: (u: string) => void }).onmessage("https://open.feishu.cn/page/cli?user_code=ABCD-1234");
          return new Promise((r) => (finish = r));
        },
      }),
    );
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    render(<NewBotWizard onClose={() => {}} onCreated={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: "开始扫码创建" }));
    const qr = await screen.findByRole("img", { name: "飞书授权二维码" });
    expect(qr.querySelector("path")?.getAttribute("d")?.length).toBeGreaterThan(100);
    // 长链接不再直接铺在界面上
    expect(screen.queryByText(/user_code=ABCD-1234/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "复制链接" }));
    expect(writeText).toHaveBeenCalledWith("https://open.feishu.cn/page/cli?user_code=ABCD-1234");
    expect(await screen.findByRole("button", { name: "已复制" })).toBeInTheDocument();
    finish({ appId: "cli_q", botName: "" });
    expect(await screen.findByTestId("scan-done")).toHaveTextContent("cli_q");
  });

  test("扫码失败显示中文原因", async () => {
    setHandlers(wizardHandlers({ feishu_create_app: () => Promise.reject("create_timeout") }));
    render(<NewBotWizard onClose={() => {}} onCreated={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: "开始扫码创建" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("扫码超时，请重新创建");
  });

  test("保存失败停在第 3 步并显示原因", async () => {
    setHandlers(wizardHandlers({ save_bot: () => Promise.reject("App ID 已被其它机器人使用") }));
    render(<NewBotWizard onClose={() => {}} onCreated={() => {}} />);
    await userEvent.click(screen.getByRole("radio", { name: "手动填写 App ID / Secret" }));
    await userEvent.type(screen.getByLabelText("App ID"), "cli_dup");
    await userEvent.type(screen.getByLabelText("App Secret"), "s");
    await userEvent.type(screen.getByLabelText("机器人名称"), "x");
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
    await userEvent.click(screen.getByRole("button", { name: "完成创建" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("App ID 已被其它机器人使用");
    expect(screen.getByRole("button", { name: "完成创建" })).toBeInTheDocument();
  });

  test("审查 B7：保存群失败后重试，沿用已建好的机器人 id，不重复创建", async () => {
    let groupFails = 1;
    setHandlers(
      wizardHandlers({
        save_bot: (a: Record<string, unknown>) => {
          const bot = a.bot as { id: string };
          return { ...bot, id: bot.id || `b${callsOf("save_bot").length}` };
        },
        save_group: () => (groupFails-- > 0 ? Promise.reject("项目「demo」不存在") : undefined),
      }),
    );
    const onCreated = vi.fn();
    render(<NewBotWizard onClose={() => {}} onCreated={onCreated} />);
    await userEvent.click(screen.getByRole("radio", { name: "手动填写 App ID / Secret" }));
    await userEvent.type(screen.getByLabelText("App ID"), "cli_new");
    await userEvent.type(screen.getByLabelText("App Secret"), "sec");
    await userEvent.type(screen.getByLabelText("机器人名称"), "助手");
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
    await userEvent.type(screen.getByLabelText("群 chat_id"), "oc_12345678abcd");
    await userEvent.click(screen.getByRole("button", { name: "添加群" }));
    await userEvent.click(screen.getByRole("button", { name: "完成创建" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("项目「demo」不存在");

    await userEvent.click(screen.getByRole("button", { name: "完成创建" }));
    await screen.findByTestId("wizard-summary");
    const saves = callsOf("save_bot").map((c) => (c.bot as { id: string }).id);
    expect(saves).toEqual(["", "b1"]);
    expect(callsOf("save_group").map((c) => c.botId)).toEqual(["b1", "b1"]);
  });

  test("机器人已建好但群没保存完就关闭：按已创建处理（列表能刷新出来）", async () => {
    setHandlers(wizardHandlers({ save_group: () => Promise.reject("目录名已被占用") }));
    const onCreated = vi.fn();
    const onClose = vi.fn();
    render(<NewBotWizard onClose={onClose} onCreated={onCreated} />);
    await userEvent.click(screen.getByRole("radio", { name: "手动填写 App ID / Secret" }));
    await userEvent.type(screen.getByLabelText("App ID"), "cli_new");
    await userEvent.type(screen.getByLabelText("App Secret"), "sec");
    await userEvent.type(screen.getByLabelText("机器人名称"), "助手");
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
    await userEvent.click(screen.getByRole("button", { name: "下一步" }));
    await userEvent.type(screen.getByLabelText("群 chat_id"), "oc_12345678abcd");
    await userEvent.click(screen.getByRole("button", { name: "添加群" }));
    await userEvent.click(screen.getByRole("button", { name: "完成创建" }));
    await screen.findByRole("alert");
    await userEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "b9" }), false);
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("首次启动引导", () => {
  const props = (over: Partial<Parameters<typeof Onboarding>[0]> = {}) => ({
    env: [envItem({ id: "git", label: "Git", status: "ok" as const })],
    checking: false,
    onRecheck: vi.fn(),
    onClose: vi.fn(),
    onCreateBot: vi.fn(),
    onChanged: vi.fn(),
    ...over,
  });

  test("环境有必需项未通过时不能下一步；修好后可以继续", async () => {
    setHandlers({});
    const p = props({ env: [envItem({ id: "python", label: "Python 3.10+", status: "error", action: { kind: "install", label: "一键安装" } })] });
    const { rerender } = render(<Onboarding {...p} />);
    await userEvent.click(screen.getByRole("button", { name: /开始设置/ }));
    expect(p.onRecheck).toHaveBeenCalled();
    expect(screen.getByText(/还有 1 项必需环境未通过（Python 3.10\+）/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /下一步/ })).toBeDisabled();
    expect(within(screen.getByTestId("onboarding-env")).getByRole("button", { name: "一键安装" })).toBeInTheDocument();

    rerender(<Onboarding {...p} env={[envItem({ id: "python", label: "Python 3.10+", status: "ok", version: "3.12.7" })]} />);
    expect(screen.getByRole("button", { name: /下一步/ })).toBeEnabled();
  });

  test("审查 B1：只装 Claude Code（codex 报错）也能下一步；两个引擎都没有才拦", async () => {
    setHandlers({});
    const p = props({
      requirements: requirements(),
      env: [envItem({ id: "claude", label: "Claude Code CLI", status: "ok" }), envItem({ id: "codex", label: "Codex CLI", status: "error" })],
    });
    const { rerender } = render(<Onboarding {...p} />);
    await userEvent.click(screen.getByRole("button", { name: /开始设置/ }));
    expect(screen.getByRole("button", { name: /下一步/ })).toBeEnabled();
    expect(screen.queryByText(/必需环境未通过/)).not.toBeInTheDocument();

    rerender(
      <Onboarding
        {...p}
        env={[envItem({ id: "claude", label: "Claude Code CLI", status: "error" }), envItem({ id: "codex", label: "Codex CLI", status: "error" })]}
      />,
    );
    expect(screen.getByRole("button", { name: /下一步/ })).toBeDisabled();
    expect(screen.getByText(/引擎装好其中一个即可/)).toBeInTheDocument();
  });

  test("发现旧版：确认停旧 bridge 后调用 migrate_legacy 并显示提示", async () => {
    setHandlers({
      detect_legacy: { found: true, app_id: "cli_old", groups: [{ chat_id: "oc_1", name: "老群", doc_url: "" }], projects: ["demo"], bridge_running: true, task_enabled: true },
      migrate_legacy: { bot_id: "b1", warnings: ["禁用计划任务需要管理员权限"] },
    });
    const confirm = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
    const p = props();
    render(<Onboarding {...p} />);
    await userEvent.click(screen.getByRole("button", { name: /开始设置/ }));
    await userEvent.click(screen.getByRole("button", { name: /下一步/ }));
    const found = await screen.findByTestId("legacy-found");
    expect(found).toHaveTextContent("cli_old");
    expect(found).toHaveTextContent("老群");
    expect(screen.getByRole("checkbox")).toBeChecked();

    await userEvent.click(screen.getByRole("button", { name: "从旧版导入" }));
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(callsOf("migrate_legacy")).toEqual([]);

    await userEvent.click(screen.getByRole("button", { name: "从旧版导入" }));
    await waitFor(() => expect(callsOf("migrate_legacy")).toEqual([{ stopBridge: true }]));
    expect(await screen.findByTestId("legacy-done")).toHaveTextContent("禁用计划任务需要管理员权限");
    expect(p.onChanged).toHaveBeenCalled();
    confirm.mockRestore();
  });

  test("不停旧 bridge 时不弹确认", async () => {
    setHandlers({
      detect_legacy: { found: true, app_id: "cli_old", groups: [], projects: [], bridge_running: false, task_enabled: false },
      migrate_legacy: { bot_id: "b1", warnings: [] },
    });
    const confirm = vi.spyOn(window, "confirm");
    render(<Onboarding {...props()} />);
    await userEvent.click(screen.getByRole("button", { name: /开始设置/ }));
    await userEvent.click(screen.getByRole("button", { name: /下一步/ }));
    await screen.findByTestId("legacy-found");
    expect(screen.getByRole("checkbox")).not.toBeChecked();
    await userEvent.click(screen.getByRole("button", { name: "从旧版导入" }));
    await waitFor(() => expect(callsOf("migrate_legacy")).toEqual([{ stopBridge: false }]));
    expect(confirm).not.toHaveBeenCalled();
    confirm.mockRestore();
  });

  test("没有旧版时直接到创建机器人，点新建进入向导；跳过关闭引导", async () => {
    setHandlers({ detect_legacy: { found: false, groups: [], projects: [], bridge_running: false, task_enabled: false } });
    const p = props();
    render(<Onboarding {...p} />);
    await userEvent.click(screen.getByRole("button", { name: /开始设置/ }));
    await userEvent.click(screen.getByRole("button", { name: /下一步/ }));
    expect(await screen.findByTestId("legacy-none")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /下一步/ }));
    await userEvent.click(screen.getByRole("button", { name: /新建机器人/ }));
    expect(p.onCreateBot).toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "跳过" }));
    expect(p.onClose).toHaveBeenCalled();
  });
});

describe("App 接入", () => {
  test("没有机器人时自动打开首次引导", async () => {
    setHandlers(baseHandlers({ list_bots: [] }));
    render(<App />);
    expect(await screen.findByRole("dialog", { name: "首次启动引导" })).toBeInTheDocument();
  });

  test("有机器人时不打开；新增机器人打开 4 步向导", async () => {
    setHandlers(baseHandlers(wizardHandlers()));
    render(<App />);
    await screen.findByRole("button", { name: "启动" });
    expect(screen.queryByRole("dialog", { name: "首次启动引导" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "新增机器人" }));
    expect(screen.getByRole("dialog", { name: "新增机器人" })).toBeInTheDocument();
    expect(screen.getByText("模板与技能")).toBeInTheDocument();
  });
});
