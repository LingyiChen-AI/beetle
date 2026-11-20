import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";
import App from "../../App";
import { baseHandlers, envItem } from "../../test/fixtures";
import { callsOf, emitEvent, resetTauri, setHandlers } from "../../test/tauri";
import type { CatalogEntry } from "../../api";
import { AppearanceProvider } from "../../theme/appearance";
import { ToastProvider } from "../common";
import EnvPage from "./EnvPage";
import { clearMeegleCache } from "./MeegleFields";
import ProjectsPage from "./ProjectsPage";
import SettingsPage from "./SettingsPage";
import SkillsPage from "./SkillsPage";

vi.mock("@tauri-apps/api/core", async () => (await import("../../test/tauri")).coreMock);
vi.mock("@tauri-apps/api/event", async () => (await import("../../test/tauri")).eventMock);
vi.mock("@tauri-apps/api/window", async () => (await import("../../test/tauri")).windowMock);
vi.mock("@tauri-apps/api/app", async () => (await import("../../test/tauri")).appMock);
vi.mock("@tauri-apps/plugin-opener", async () => (await import("../../test/tauri")).openerMock);

beforeEach(() => resetTauri());

const projectsView = () => ({
  projects: { demo: { repo: "gitee.com/a/demo.git", branch_glob: "release/*", rules_file: "AGENTS.md", description: "示例后台", dbs: ["demo_config"] } },
  dbs: { demo_config: { kind: "mysql", description: "", access: "skill", skill: "my-sql", database: "demo_config", note: "" } },
  default_projects: [] as string[],
});

const skill = (name: string, display_name: string, builtin = true) => ({ name, display_name, description: `${display_name}说明`, requires: [], builtin });

describe("项目页", () => {
  test("显示项目卡片；添加项目调用 save_project", async () => {
    setHandlers({ list_projects: projectsView(), save_project: undefined });
    render(<ProjectsPage />);
    const card = await screen.findByTestId("project-demo");
    expect(card).toHaveTextContent("gitee.com/a/demo.git");
    expect(card).toHaveTextContent("demo_config");

    await userEvent.click(screen.getByRole("button", { name: "添加项目" }));
    const form = screen.getByTestId("project-form-new");
    await userEvent.type(within(form).getByLabelText("项目名"), "demo-worker");
    await userEvent.type(within(form).getByLabelText("仓库地址"), "gitee.com/a/rpa.git");
    await userEvent.click(within(form).getByRole("button", { name: /demo_config/ }));
    await userEvent.click(within(form).getByRole("button", { name: "保存项目" }));
    expect(callsOf("save_project")[0]).toEqual({
      name: "demo-worker",
      project: {
        repo: "gitee.com/a/rpa.git",
        branch_glob: "release/*",
        rules_file: "AGENTS.md",
        description: "",
        dbs: ["demo_config"],
        record: { issue: true, feishu_doc: true, meegle: false, feishu_task: false },
        issue_repo: null,
        token_env: null,
        meegle: null,
        feishu_task: null,
        record_rule: null,
      },
    });
  });

  test("记录方式多选：识别平台与仓库、检测 token（只显示账号）、保存多选结果", async () => {
    setHandlers({
      list_projects: projectsView(),
      save_project: undefined,
      check_issue_token: { platform: "gitee", platform_label: "Gitee", repo: "team/issues", token_env: "MY_TOKEN", detected: true, user: "demo-user", error: null },
    });
    render(<ProjectsPage />);
    const card = await screen.findByTestId("project-demo");
    expect(within(card).getByTestId("record-summary-demo")).toHaveTextContent("Gitee issue · a/demo");
    expect(within(card).getByTestId("record-summary-demo")).toHaveTextContent("飞书文档");

    await userEvent.click(within(card).getByRole("button", { name: "编辑 demo" }));
    const form = screen.getByTestId("project-form-demo");
    expect(within(form).getByRole("button", { name: /建 issue/ })).toHaveAttribute("aria-pressed", "true");
    expect(within(form).getByRole("button", { name: /写飞书文档/ })).toHaveAttribute("aria-pressed", "true");
    expect(within(form).getByTestId("issue-target-demo")).toHaveTextContent("Gitee · a/demo");
    expect(within(form).getByLabelText("token 环境变量")).toHaveAttribute("placeholder", "GITEE_TOKEN");

    await userEvent.click(within(form).getByRole("button", { name: /写飞书文档/ }));
    await userEvent.type(within(form).getByLabelText("issue 仓库（可改）"), "team/issues");
    expect(within(form).getByTestId("issue-target-demo")).toHaveTextContent("Gitee · team/issues");
    await userEvent.type(within(form).getByLabelText("token 环境变量"), "MY_TOKEN");
    await userEvent.click(within(form).getByRole("button", { name: /检测 token/ }));
    expect(await within(form).findByTestId("token-status-demo")).toHaveTextContent("已检测到（账号 demo-user）");
    expect(callsOf("check_issue_token")[0]).toMatchObject({ project: { issue_repo: "team/issues", token_env: "MY_TOKEN" } });

    await userEvent.click(within(form).getByRole("button", { name: "保存项目" }));
    expect(callsOf("save_project")[0]).toEqual({
      name: "demo",
      project: {
        repo: "gitee.com/a/demo.git",
        branch_glob: "release/*",
        rules_file: "AGENTS.md",
        description: "示例后台",
        dbs: ["demo_config"],
        record: { issue: true, feishu_doc: false, meegle: false, feishu_task: false },
        issue_repo: "team/issues",
        token_env: "MY_TOKEN",
        record_rule: null,
      },
    });
  });

  test("记录方式：非 gitee / github 仓库默认不建 issue；手动勾上后显示无法识别、未检测到 token", async () => {
    setHandlers({
      list_projects: projectsView(),
      save_project: undefined,
      check_issue_token: { platform: "github", platform_label: "GitHub", repo: "o/r", token_env: "GITHUB_TOKEN", detected: false, user: null, error: "未检测到环境变量 GITHUB_TOKEN" },
    });
    render(<ProjectsPage />);
    await screen.findByTestId("project-demo");
    await userEvent.click(screen.getByRole("button", { name: "添加项目" }));
    const form = screen.getByTestId("project-form-new");
    await userEvent.type(within(form).getByLabelText("项目名"), "self-hosted");
    await userEvent.type(within(form).getByLabelText("仓库地址"), "git@git.example.com:a/b.git");
    const issue = within(form).getByRole("button", { name: /建 issue/ });
    expect(issue).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(issue);
    expect(within(form).getByTestId("issue-target-new")).toHaveTextContent("仓库不是 gitee.com / github.com");
    expect(within(form).getByRole("button", { name: /检测 token/ })).toBeDisabled();

    await userEvent.type(within(form).getByLabelText("issue 仓库（可改）"), "https://github.com/o/r");
    expect(within(form).getByTestId("issue-target-new")).toHaveTextContent("GitHub · o/r");
    await userEvent.click(within(form).getByRole("button", { name: /检测 token/ }));
    expect(await within(form).findByTestId("token-status-new")).toHaveTextContent("未检测到环境变量 GITHUB_TOKEN");

    await userEvent.click(within(form).getByRole("button", { name: /写飞书文档/ }));
    await userEvent.click(within(form).getByRole("button", { name: "保存项目" }));
    expect(callsOf("save_project")[0]).toMatchObject({
      name: "self-hosted",
      project: { record: { issue: true, feishu_doc: false }, issue_repo: "https://github.com/o/r", token_env: null },
    });
  });

  test("编辑、设为默认、删除", async () => {
    setHandlers({ list_projects: projectsView(), save_project: undefined, set_default_projects: undefined, remove_project: undefined });
    vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<ProjectsPage />);
    const card = await screen.findByTestId("project-demo");
    await userEvent.click(within(card).getByRole("button", { name: /设为默认/ }));
    expect(callsOf("set_default_projects")).toEqual([{ names: ["demo"] }]);

    await userEvent.click(within(card).getByRole("button", { name: "删除 demo" }));
    expect(callsOf("remove_project")).toEqual([{ name: "demo" }]);

    await userEvent.click(within(await screen.findByTestId("project-demo")).getByRole("button", { name: "编辑 demo" }));
    const form = screen.getByTestId("project-form-demo");
    expect(within(form).getByLabelText("项目名")).toBeDisabled();
    await userEvent.clear(within(form).getByLabelText("说明"));
    await userEvent.type(within(form).getByLabelText("说明"), "新说明");
    await userEvent.click(within(form).getByRole("button", { name: "保存项目" }));
    expect(callsOf("save_project")[0]).toMatchObject({ name: "demo", project: { description: "新说明" } });
  });

  test("关联数据源是开关式标签：选中 / 未选中用 aria-pressed 区分", async () => {
    setHandlers({ list_projects: projectsView() });
    render(<ProjectsPage />);
    await userEvent.click(within(await screen.findByTestId("project-demo")).getByRole("button", { name: "编辑 demo" }));
    const form = screen.getByTestId("project-form-demo");
    const chip = within(form).getByRole("button", { name: "demo_config" });
    expect(chip).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(chip);
    expect(chip).toHaveAttribute("aria-pressed", "false");
  });

  test("后端报错就近显示", async () => {
    setHandlers({ list_projects: projectsView(), save_project: () => Promise.reject("项目名不合法") });
    render(<ProjectsPage />);
    await userEvent.click(within(await screen.findByTestId("project-demo")).getByRole("button", { name: "编辑 demo" }));
    await userEvent.click(within(screen.getByTestId("project-form-demo")).getByRole("button", { name: "保存项目" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("项目名不合法");
  });

  test("页面不再有库别名表；项目引用的数据源已删除时标出来、可取消", async () => {
    setHandlers({ list_projects: { ...projectsView(), dbs: {} } });
    const onGo = vi.fn();
    render(<ProjectsPage onGoDataSources={onGo} />);
    await screen.findByTestId("project-demo");
    expect(screen.queryByLabelText("新库别名")).toBeNull();
    expect(screen.queryByText("库别名")).toBeNull();
    // 项目引用了已不存在的数据源：卡片里标红，编辑时可以取消
    await userEvent.click(within(screen.getByTestId("project-demo")).getByRole("button", { name: "编辑 demo" }));
    const form = screen.getByTestId("project-form-demo");
    expect(within(form).getByRole("button", { name: /demo_config（已删除）/ })).toHaveAttribute("aria-pressed", "true");
    expect(within(form).getByText("关联数据源（可多选，查 bug 时 Agent 只能查这些库）")).toBeInTheDocument();
  });

  test("没有任何数据源选项时，点「去数据源页添加」跳转", async () => {
    setHandlers({ list_projects: { projects: {}, dbs: {}, default_projects: [] } });
    const onGo = vi.fn();
    render(<ProjectsPage onGoDataSources={onGo} />);
    // 没有项目时页头和空状态各有一个「添加项目」
    await userEvent.click((await screen.findAllByRole("button", { name: "添加项目" }))[0]);
    await userEvent.click(within(screen.getByTestId("project-form-new")).getByRole("button", { name: "去数据源页添加" }));
    expect(onGo).toHaveBeenCalled();
  });
});

describe("技能中心", () => {
  const entry = (over: Partial<CatalogEntry> = {}): CatalogEntry => ({
    name: "bug-hunt",
    display_name: "查 bug",
    description: "在自己的会话里查 bug",
    version: "1.0.0",
    requires: ["python", "node", "lark_cli", "git"],
    installed: { claude: null, codex: null },
    update_available: { claude: false, codex: false },
    ...over,
  });
  const env = () => [
    envItem({ id: "python", status: "error" }),
    envItem({ id: "node", status: "ok" }),
    envItem({ id: "claude", status: "error" }),
    envItem({ id: "codex", status: "ok" }),
  ];
  const renderPage = () =>
    render(
      <ToastProvider>
        <SkillsPage env={env()} />
      </ToastProvider>,
    );

  test("列表渲染：显示名、版本、说明、依赖（缺的标红）、安装状态", async () => {
    setHandlers({
      catalog_list: [entry({ installed: { claude: "0.9.0", codex: "1.0.0" }, update_available: { claude: true, codex: false } })],
    });
    renderPage();
    const card = await screen.findByTestId("catalog-bug-hunt");
    expect(card).toHaveTextContent("查 bug");
    expect(card).toHaveTextContent("v1.0.0");
    expect(card).toHaveTextContent("在自己的会话里查 bug");
    expect(within(card).getByTestId("req-bug-hunt-python")).toHaveClass("c-err");
    expect(within(card).getByTestId("req-bug-hunt-python")).toHaveTextContent("Python（缺少）");
    expect(within(card).getByTestId("req-bug-hunt-node")).not.toHaveClass("c-err");
    expect(within(card).getByTestId("req-bug-hunt-lark_cli")).toHaveTextContent("lark-cli");
    expect(card).toHaveTextContent("已装到 Claude Code v0.9.0");
    expect(card).toHaveTextContent("已装到 Codex v1.0.0");
    expect(within(card).getByRole("button", { name: "更新 Claude Code" })).toBeInTheDocument();
    expect(within(card).queryByRole("button", { name: "更新 Codex" })).not.toBeInTheDocument();
  });

  test("安装弹窗默认勾已检测到的 CLI，勾选后调用 catalog_install(targets) 并刷新", async () => {
    setHandlers({ catalog_list: [entry()], catalog_install: undefined });
    renderPage();
    const card = await screen.findByTestId("catalog-bug-hunt");
    expect(card).toHaveTextContent("未安装");
    await userEvent.click(within(card).getByRole("button", { name: "安装 查 bug" }));
    const dialog = screen.getByRole("dialog", { name: "安装 查 bug" });
    const claude = within(dialog).getByRole("checkbox", { name: /Claude Code/ });
    const codex = within(dialog).getByRole("checkbox", { name: /Codex/ });
    expect(claude).not.toBeChecked();
    expect(codex).toBeChecked();
    await userEvent.click(claude);
    await userEvent.click(within(dialog).getByRole("button", { name: "确认安装" }));
    expect(callsOf("catalog_install")).toEqual([{ name: "bug-hunt", targets: ["claude", "codex"] }]);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(callsOf("catalog_list").length).toBe(2));
  });

  test("安装失败的错误显示在弹窗里，弹窗不关", async () => {
    setHandlers({
      catalog_list: [entry()],
      catalog_install: () => Promise.reject("Codex：已存在同名技能（不是本客户端安装的），请先手动处理：C:/Users/x/.codex/skills/bug-hunt"),
    });
    renderPage();
    await userEvent.click(within(await screen.findByTestId("catalog-bug-hunt")).getByRole("button", { name: "安装 查 bug" }));
    const dialog = screen.getByRole("dialog", { name: "安装 查 bug" });
    await userEvent.click(within(dialog).getByRole("button", { name: "确认安装" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("不是本客户端安装的");
    expect(screen.getByRole("dialog", { name: "安装 查 bug" })).toBeInTheDocument();
  });

  test("取消全部勾选时不能确认安装", async () => {
    setHandlers({ catalog_list: [entry()], catalog_install: undefined });
    renderPage();
    await userEvent.click(within(await screen.findByTestId("catalog-bug-hunt")).getByRole("button", { name: "安装 查 bug" }));
    const dialog = screen.getByRole("dialog", { name: "安装 查 bug" });
    await userEvent.click(within(dialog).getByRole("checkbox", { name: /Codex/ }));
    expect(within(dialog).getByRole("button", { name: "确认安装" })).toBeDisabled();
  });

  test("按位置卸载调用 catalog_uninstall；更新调用 catalog_install 单个位置；卸载失败就近显示", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    let failUninstall = false;
    setHandlers({
      catalog_list: [entry({ installed: { claude: "0.9.0", codex: "1.0.0" }, update_available: { claude: true, codex: false } })],
      catalog_install: undefined,
      catalog_uninstall: () => (failUninstall ? Promise.reject("Codex：bug-hunt 不是本客户端安装的，不能卸载") : undefined),
    });
    renderPage();
    const card = await screen.findByTestId("catalog-bug-hunt");
    await userEvent.click(within(card).getByRole("button", { name: "从 Codex 卸载" }));
    expect(callsOf("catalog_uninstall")).toEqual([{ name: "bug-hunt", target: "codex" }]);

    // 操作进行中按钮会禁用，等刷新完成再点下一个
    const btn = (name: string) => within(screen.getByTestId("catalog-bug-hunt")).getByRole("button", { name });
    await waitFor(() => expect(callsOf("catalog_list").length).toBe(2));
    await waitFor(() => expect(btn("更新 Claude Code")).toBeEnabled());
    await userEvent.click(btn("更新 Claude Code"));
    await waitFor(() => expect(callsOf("catalog_install")).toEqual([{ name: "bug-hunt", targets: ["claude"] }]));

    failUninstall = true;
    await waitFor(() => expect(callsOf("catalog_list").length).toBe(3));
    await waitFor(() => expect(btn("从 Codex 卸载")).toBeEnabled());
    await userEvent.click(btn("从 Codex 卸载"));
    expect(await within(screen.getByTestId("catalog-bug-hunt")).findByRole("alert")).toHaveTextContent("不能卸载");
  });

  const FILES = [
    { path: "scripts", name: "scripts", size: 0, is_dir: true, depth: 0 },
    { path: "scripts/run.py", name: "run.py", size: 40, is_dir: false, depth: 1 },
    { path: "catalog.json", name: "catalog.json", size: 20, is_dir: false, depth: 0 },
    { path: "logo.png", name: "logo.png", size: 2048, is_dir: false, depth: 0 },
    { path: "SKILL.md", name: "SKILL.md", size: 60, is_dir: false, depth: 0 },
  ];
  const CONTENT: Record<string, { content: string | null; binary?: boolean }> = {
    "SKILL.md": { content: "---\nname: bug-hunt\ndescription: 查 bug 技能\n---\n# 查 bug\n\n用 **lark-cli** 读群聊\n\n- 第一步\n- 第二步\n" },
    "scripts/run.py": { content: "def main():\n    return 1  # 注释\n" },
    "catalog.json": { content: '{"name": "bug-hunt"}' },
    "logo.png": { content: null, binary: true },
  };
  const detailHandlers = (extra: Record<string, unknown> = {}) => ({
    catalog_list: [entry({ installed: { claude: "0.9.0", codex: null }, update_available: { claude: true, codex: false } })],
    catalog_files: FILES,
    catalog_read_file: (a: Record<string, unknown>) => {
      const c = CONTENT[a.path as string];
      return { path: a.path, size: 10, content: c.content, binary: !!c.binary, too_large: false };
    },
    catalog_install: undefined,
    ...extra,
  });

  test("点卡片打开详情弹窗：头部显示版本、依赖、安装状态；默认打开 SKILL.md 并渲染 Markdown，可切到源码", async () => {
    setHandlers(detailHandlers());
    renderPage();
    const card = await screen.findByTestId("catalog-bug-hunt");
    await userEvent.click(within(card).getByRole("button", { name: "查看 查 bug 详情" }));
    const dialog = await screen.findByRole("dialog", { name: "查 bug 详情" });
    expect(callsOf("catalog_files")).toEqual([{ name: "bug-hunt" }]);
    expect(dialog).toHaveTextContent("v1.0.0");
    expect(within(dialog).getByTestId("detail-requires")).toHaveTextContent("Python（缺少）");
    expect(within(dialog).getByTestId("detail-target-claude")).toHaveTextContent("v0.9.0");
    expect(within(dialog).getByTestId("detail-target-codex")).toHaveTextContent("未安装");

    await waitFor(() => expect(callsOf("catalog_read_file")).toEqual([{ name: "bug-hunt", path: "SKILL.md" }]));
    expect(within(dialog).getByRole("treeitem", { name: /SKILL\.md/ })).toHaveAttribute("aria-selected", "true");
    const preview = await within(dialog).findByTestId("md-preview");
    expect(within(preview).getByRole("heading", { name: "查 bug" })).toBeInTheDocument();
    expect(within(preview).getByText("lark-cli").tagName).toBe("STRONG");
    expect(within(preview).getByTestId("md-front-matter")).toHaveTextContent("查 bug 技能");
    expect(within(preview).getAllByRole("listitem")).toHaveLength(2);

    await userEvent.click(within(dialog).getByRole("radio", { name: "源码" }));
    const code = within(dialog).getByTestId("code-view");
    expect(code).toHaveTextContent("# 查 bug");
    expect(code).toHaveTextContent("1");
  });

  test("详情弹窗：点文件查看代码（行号 + 着色），目录可折叠，二进制文件给提示，Esc 关闭", async () => {
    setHandlers(detailHandlers());
    renderPage();
    await userEvent.click(within(await screen.findByTestId("catalog-bug-hunt")).getByRole("button", { name: "查看 查 bug 详情" }));
    const dialog = await screen.findByRole("dialog", { name: "查 bug 详情" });
    await userEvent.click(await within(dialog).findByRole("treeitem", { name: /run\.py/ }));
    await waitFor(() => expect(callsOf("catalog_read_file").at(-1)).toEqual({ name: "bug-hunt", path: "scripts/run.py" }));
    const code = await within(dialog).findByTestId("code-view");
    expect(code).toHaveAttribute("data-lang", "python");
    expect(code.querySelector(".tk-k")).toHaveTextContent("def");
    expect(code.querySelector(".tk-c")).toHaveTextContent("# 注释");
    expect(code.querySelectorAll(".cv-ln")).toHaveLength(2);

    // 折叠 scripts 目录后 run.py 不再显示
    const dir = within(dialog).getByRole("treeitem", { name: /scripts/ });
    expect(dir).toHaveAttribute("aria-expanded", "true");
    await userEvent.click(dir);
    expect(dir).toHaveAttribute("aria-expanded", "false");
    expect(within(dialog).queryByRole("treeitem", { name: /run\.py/ })).not.toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole("treeitem", { name: /logo\.png/ }));
    expect(await within(dialog).findByText("二进制文件，不支持预览")).toBeInTheDocument();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "查 bug 详情" })).not.toBeInTheDocument());
  });

  test("详情弹窗里可更新，安装打开安装弹窗；读文件失败就近显示", async () => {
    setHandlers(detailHandlers({ catalog_read_file: () => Promise.reject("只能读取技能目录里的文件：../x") }));
    renderPage();
    await userEvent.click(within(await screen.findByTestId("catalog-bug-hunt")).getByRole("button", { name: "查看 查 bug 详情" }));
    const dialog = await screen.findByRole("dialog", { name: "查 bug 详情" });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("只能读取技能目录里的文件");

    await userEvent.click(within(dialog).getByRole("button", { name: "更新 Claude Code" }));
    await waitFor(() => expect(callsOf("catalog_install")).toEqual([{ name: "bug-hunt", targets: ["claude"] }]));

    await waitFor(() => expect(within(dialog).getByRole("button", { name: "安装" })).toBeEnabled());
    await userEvent.click(within(dialog).getByRole("button", { name: "安装" }));
    expect(screen.getByRole("dialog", { name: "安装 查 bug" })).toBeInTheDocument();
  });

  test("同步配置调用 catalog_refresh_config", async () => {
    setHandlers({ catalog_list: [entry()], catalog_refresh_config: 2 });
    renderPage();
    await screen.findByTestId("catalog-bug-hunt");
    await userEvent.click(screen.getByRole("button", { name: "同步配置" }));
    expect(callsOf("catalog_refresh_config")).toEqual([{}]);
    expect(await screen.findByText(/已把最新配置同步到 2 处/)).toBeInTheDocument();
  });
});

describe("技能与模板 tab", () => {
  const handlers = (extra: Record<string, unknown> = {}) =>
    baseHandlers({
      list_skills: [skill("db-query", "查库（只读）"), skill("code-sync", "代码同步")],
      list_templates: [
        { name: "bug-hunt", builtin: true, modified: true },
        { name: "qa", builtin: true, modified: false },
      ],
      get_template: (a: Record<string, unknown>) => `模板 ${a.name} {{GROUP_NAME}}`,
      save_bot: (a: Record<string, unknown>) => a.bot,
      ...extra,
    });

  async function openTab() {
    render(<App />);
    await userEvent.click(await screen.findByRole("tab", { name: /技能与模板/ }));
    return screen.findByLabelText("模板内容");
  }

  test("勾选技能经 save_bot 保存", async () => {
    setHandlers(handlers());
    await openTab();
    await userEvent.click(await screen.findByRole("checkbox", { name: "代码同步" }));
    expect(callsOf("save_bot")[0]).toMatchObject({ bot: { skills: ["db-query", "code-sync"] } });
    // 审查 S6：保存到刷新完成之前勾选被禁用，下一次一定基于刷新后的机器人配置
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "查库（只读）" })).toBeEnabled());
    await userEvent.click(screen.getByRole("checkbox", { name: "查库（只读）" }));
    expect(callsOf("save_bot")[1]).toMatchObject({ bot: { skills: [] } });
  });

  test("审查 S6：保存还没完成时再点别的技能不会发出第二次（旧快照）保存", async () => {
    let finish: () => void = () => {};
    setHandlers(handlers({ save_bot: (a: Record<string, unknown>) => new Promise((r) => (finish = () => r(a.bot))) }));
    await openTab();
    await userEvent.click(await screen.findByRole("checkbox", { name: "代码同步" }));
    expect(screen.getByRole("checkbox", { name: "查库（只读）" })).toBeDisabled();
    await userEvent.click(screen.getByRole("checkbox", { name: "查库（只读）" }));
    expect(callsOf("save_bot")).toHaveLength(1);
    finish();
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "查库（只读）" })).toBeEnabled());
  });

  test("编辑模板并保存；保存失败的错误显示在编辑器下方", async () => {
    let fail = true;
    setHandlers(handlers({ save_template: () => (fail ? Promise.reject("模板里有未替换的变量：{{NOPE}}") : undefined) }));
    const editor = await openTab();
    await waitFor(() => expect(editor).toHaveValue("模板 bug-hunt {{GROUP_NAME}}"));
    await userEvent.type(editor, " 追加");
    await userEvent.click(screen.getByRole("button", { name: "保存模板" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("{{NOPE}}");
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "保存模板" }));
    expect(callsOf("save_template")[1]).toEqual({ name: "bug-hunt", content: "模板 bug-hunt {{GROUP_NAME}} 追加" });
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
  });

  test("内置已修改的模板可恢复内置；切到别的模板可设为本机器人使用", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    setHandlers(handlers({ reset_template: undefined }));
    const editor = await openTab();
    await waitFor(() => expect(editor).toHaveValue("模板 bug-hunt {{GROUP_NAME}}"));
    await userEvent.click(screen.getByRole("button", { name: "恢复内置" }));
    expect(callsOf("reset_template")).toEqual([{ name: "bug-hunt" }]);

    await userEvent.click(screen.getByRole("radio", { name: "需求答疑" }));
    await waitFor(() => expect(editor).toHaveValue("模板 qa {{GROUP_NAME}}"));
    expect(screen.queryByRole("button", { name: "恢复内置" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "用于本机器人" }));
    expect(callsOf("save_bot")[0]).toMatchObject({ bot: { template: "qa" } });
  });
});

describe("环境页", () => {
  const env = () => [
    envItem({ id: "python", label: "Python 3.10+", status: "error", action: { kind: "install", label: "一键安装（winget）" } }),
    envItem({ id: "lark_login", label: "lark-cli 登录", group: "feishu", status: "error", action: { kind: "login", label: "扫码登录" } }),
    envItem({ id: "git", label: "Git", status: "ok", version: "2.47.1" }),
    envItem({ id: "net_feishu", label: "网络 · open.feishu.cn", group: "network", status: "ok", version: "38 ms" }),
  ];

  test("按分组显示；单项修复显示实时输出，结束后重新检查", async () => {
    const recheck = vi.fn();
    setHandlers({ env_fix: undefined });
    render(<EnvPage env={env()} checking={false} onRecheck={recheck} />);
    expect(screen.getByText("运行环境")).toBeInTheDocument();
    expect(screen.getByText("飞书与技能")).toBeInTheDocument();
    expect(screen.getByText("网络")).toBeInTheDocument();
    expect(screen.getByTestId("env-summary")).toHaveTextContent("2 项异常");

    await userEvent.click(screen.getByRole("button", { name: "一键安装（winget）" }));
    expect(callsOf("env_fix")).toEqual([{ id: "python" }]);
    emitEvent("env-install-log", { id: "python", line: "正在下载 Python 3.12" });
    emitEvent("env-install-log", { id: "python", line: "安装完成" });
    expect(screen.getByTestId("env-log-python")).toHaveTextContent("正在下载 Python 3.12");
    expect(screen.getByTestId("env-log-python")).toHaveTextContent("安装完成");
    expect(recheck).not.toHaveBeenCalled();

    emitEvent("env-install-done", { id: "python", ok: true, message: "已安装 3.12.7" });
    await waitFor(() => expect(recheck).toHaveBeenCalled());
    expect(screen.getByTestId("env-result-python")).toHaveTextContent("已安装 3.12.7");
  });

  test("一键修复全部按顺序：上一项 done 后才修下一项", async () => {
    const recheck = vi.fn();
    setHandlers({ env_fix: undefined });
    render(<EnvPage env={env()} checking={false} onRecheck={recheck} />);
    await userEvent.click(screen.getByRole("button", { name: "一键修复全部" }));
    await waitFor(() => expect(callsOf("env_fix")).toEqual([{ id: "python" }]));

    emitEvent("env-install-done", { id: "python", ok: true, message: "ok" });
    await waitFor(() => expect(callsOf("env_fix")).toEqual([{ id: "python" }, { id: "lark_login" }]));
    expect(recheck).not.toHaveBeenCalled();

    emitEvent("env-install-done", { id: "lark_login", ok: false, message: "用户取消登录" });
    await waitFor(() => expect(recheck).toHaveBeenCalledTimes(1));
    expect(screen.getByTestId("env-result-lark_login")).toHaveTextContent("用户取消登录");
  });

  test("全部检查调用 onRecheck", async () => {
    const recheck = vi.fn();
    render(<EnvPage env={env()} checking={false} onRecheck={recheck} />);
    await userEvent.click(screen.getByRole("button", { name: "全部检查" }));
    expect(recheck).toHaveBeenCalled();
  });
});

describe("设置页", () => {
  const renderSettings = () =>
    render(
      <AppearanceProvider>
        <ToastProvider>
          <SettingsPage />
        </ToastProvider>
      </AppearanceProvider>,
    );

  test("导出配置调用 export_config", async () => {
    setHandlers({ get_appearance: { mode: "dark", accent: "vermilion" }, get_autostart: false, export_config: undefined });
    renderSettings();
    await userEvent.type(screen.getByLabelText("导出路径"), "D:/cfg.json");
    await userEvent.click(screen.getByRole("button", { name: "导出" }));
    expect(callsOf("export_config")).toEqual([{ path: "D:/cfg.json" }]);
    expect(await screen.findByText("0.1.0")).toBeInTheDocument();
  });

  test("phase23-issues 第 15 条：运行设置读出后保存代理 / 代码缓存目录 / 日志保留天数", async () => {
    setHandlers({
      get_appearance: { mode: "dark", accent: "vermilion" },
      get_autostart: false,
      get_runtime_settings: {
        proxy: "http://127.0.0.1:7890",
        code_cache_dir: null,
        log_retention_days: 30,
        default_code_cache_dir: "C:/Users/a/AppData/Local/Beetle/code",
        min_retention_days: 1,
        max_retention_days: 3650,
      },
      set_runtime_settings: undefined,
    });
    renderSettings();
    const card = await screen.findByTestId("runtime-settings");
    await waitFor(() => expect(within(card).getByLabelText("代理地址")).toHaveValue("http://127.0.0.1:7890"));
    expect(within(card).getByLabelText("代码缓存目录")).toHaveAttribute("placeholder", "C:/Users/a/AppData/Local/Beetle/code");
    await userEvent.clear(within(card).getByLabelText("代理地址"));
    await userEvent.type(within(card).getByLabelText("代码缓存目录"), "D:/cache");
    await userEvent.clear(within(card).getByLabelText("日志保留天数"));
    await userEvent.type(within(card).getByLabelText("日志保留天数"), "7");
    await userEvent.click(within(card).getByRole("button", { name: "保存运行设置" }));
    expect(callsOf("set_runtime_settings")).toEqual([{ settings: { proxy: null, code_cache_dir: "D:/cache", log_retention_days: 7 } }]);
  });

  test("运行设置：保留天数超出范围时不提交并提示", async () => {
    setHandlers({
      get_appearance: { mode: "dark", accent: "vermilion" },
      get_autostart: false,
      get_runtime_settings: { proxy: null, code_cache_dir: null, log_retention_days: 30, default_code_cache_dir: "C:/x", min_retention_days: 1, max_retention_days: 3650 },
      set_runtime_settings: undefined,
    });
    renderSettings();
    const card = await screen.findByTestId("runtime-settings");
    await waitFor(() => expect(within(card).getByLabelText("日志保留天数")).toHaveValue(30));
    await userEvent.clear(within(card).getByLabelText("日志保留天数"));
    await userEvent.type(within(card).getByLabelText("日志保留天数"), "0");
    await userEvent.click(within(card).getByRole("button", { name: "保存运行设置" }));
    expect(within(card).getByRole("alert")).toHaveTextContent("日志保留天数要在 1～3650 之间");
    expect(callsOf("set_runtime_settings")).toEqual([]);
  });

  test("导入：预览后按勾选调用 apply_import", async () => {
    setHandlers({
      get_appearance: { mode: "dark", accent: "vermilion" },
      get_autostart: false,
      preview_import: { projects: ["demo", "rpa"], dbs: ["demo_config"], templates: ["qa"], skills: [], bots: [{ name: "同事的机器人", app_id: "cli_x" }] },
      apply_import: undefined,
    });
    renderSettings();
    await userEvent.click(screen.getByRole("button", { name: "选择文件导入" }));
    await userEvent.type(screen.getByLabelText("配置文件路径"), "D:/in.json");
    await userEvent.click(screen.getByRole("button", { name: "预览" }));
    expect(callsOf("preview_import")).toEqual([{ path: "D:/in.json" }]);
    expect(await screen.findByText(/同事的机器人/)).toBeInTheDocument();
    // 审查 B9：仓库凭据不随配置导出，导入时提示自行配置
    expect(screen.getByTestId("import-repo-hint")).toHaveTextContent("仓库地址里的账号 / 令牌没有导出");
    await userEvent.click(screen.getByRole("button", { name: "rpa" }));
    await userEvent.click(screen.getByRole("button", { name: "导入选中项" }));
    expect(callsOf("apply_import")).toEqual([
      { path: "D:/in.json", select: { projects: ["demo"], dbs: ["demo_config"], templates: ["qa"], skills: [] } },
    ]);
    expect(await screen.findByText(/已导入：1 个项目、1 个数据源、1 个流程模板/)).toBeInTheDocument();
  });
  test("设置页不再有 SQL 执行窗口一节，也不再读技能设置", async () => {
    setHandlers({ get_appearance: { mode: "dark", accent: "vermilion" }, get_autostart: false, get_runtime_settings: { proxy: null, code_cache_dir: null, log_retention_days: 30, default_code_cache_dir: "C:/x", min_retention_days: 1, max_retention_days: 3650 } });
    renderSettings();
    expect(await screen.findByTestId("runtime-settings")).toBeInTheDocument();
    expect(screen.queryByText("SQL 执行窗口")).toBeNull();
    expect(screen.queryByTestId("sql-window-settings")).toBeNull();
    expect(callsOf("get_skill_settings")).toEqual([]);
  });
});

describe("项目页：飞书项目工单 / 飞书任务 / 记录规则", () => {
  beforeEach(() => clearMeegleCache());
  const form = {
    fields: [
      { field_key: "name", field_name: "名称", field_type: "text", options: [], default: null, auto: "自动填：bug 标题" },
      { field_key: "priority", field_name: "优先级", field_type: "select", options: [{ id: "0", name: "P0" }, { id: "2", name: "P2" }], default: "2", auto: null },
      { field_key: "end", field_name: "涉及端", field_type: "select", options: [{ id: "c1", name: "客户端" }], default: null, auto: null },
      { field_key: "template", field_name: "缺陷类型", field_type: "work_item_template", options: [{ id: "t1", name: "用户反馈缺陷" }], default: null, auto: null },
    ],
    roles: [{ role_id: "r_fix", role_name: "缺陷处理人" }],
    suggested_role: "r_fix",
  };

  test("登录后选空间、类型、必填字段、处理人，配飞书任务和规则，一起保存", async () => {
    setHandlers({
      list_projects: projectsView(),
      save_project: undefined,
      meegle_status: { installed: true, logged_in: true, host: "project.feishu.cn", user: "张三", error: null },
      meegle_spaces: [{ project_key: "pk1", simple_name: "bot", name: "AI 客服" }],
      meegle_types: [{ type_key: "issue", name: "缺陷" }],
      meegle_form: form,
      meegle_users: [{ user_key: "u_li", name: "李四", email: "" }],
      lark_tasklists: [{ guid: "tl1", name: "调研" }],
      lark_users: [],
    });
    render(<ProjectsPage />);
    const card = await screen.findByTestId("project-demo");
    await userEvent.click(within(card).getByRole("button", { name: "编辑 demo" }));
    const f = screen.getByTestId("project-form-demo");
    await userEvent.click(within(f).getByRole("button", { name: /建飞书项目工单/ }));
    expect(await within(f).findByTestId("meegle-login-demo")).toHaveTextContent("已登录 · 张三");
    // 勾了不止一种：出现记录规则
    await userEvent.type(within(f).getByLabelText(/记录规则/), "调研类建飞书任务；bug 建工单");

    await userEvent.click(await within(f).findByTestId("mg-space-demo"));
    await userEvent.click(screen.getByRole("option", { name: "AI 客服" }));
    expect(await within(f).findByTestId("mg-ai-note-demo-priority")).toBeInTheDocument();
    // 默认：优先级、涉及端交给 AI 判断；缺陷类型（模板）用固定值
    const pri = within(f).getByTestId("mg-ai-note-demo-priority");
    expect(pri).toHaveTextContent("拿不准时用 P2");
    // 限定 AI 的选择范围：排除 P0
    await userEvent.click(within(pri).getByRole("button", { name: "P0" }));
    expect(within(pri).getByRole("button", { name: "P0" })).toHaveAttribute("aria-pressed", "false");
    expect(within(f).getByTestId("mg-ai-demo-end")).toHaveAttribute("aria-pressed", "true");
    expect(within(f).getByTestId("mg-ai-demo-template")).toHaveAttribute("aria-pressed", "false");
    expect(within(f).queryByTestId("meegle-missing-demo")).toBeNull();
    // 涉及端改成固定值再选
    await userEvent.click(within(f).getByRole("group", { name: "涉及端怎么填" }).querySelector("button:last-child") as HTMLElement);
    await userEvent.click(within(f).getByTestId("mg-f-demo-end"));
    await userEvent.click(screen.getByRole("option", { name: "客户端" }));
    expect(within(f).getByTestId("mg-auto-demo")).toHaveTextContent("名称 由机器人自动填写");
    await userEvent.type(within(f).getByLabelText(/默认处理人/), "李四");
    await userEvent.click(await within(f).findByRole("option", { name: /李四/ }));
    expect(within(f).getByTestId("mg-user-demo-picked")).toHaveTextContent("李四");

    await userEvent.click(within(f).getByRole("button", { name: /建飞书任务/ }));
    await userEvent.click(await within(f).findByTestId("tk-list-demo"));
    await userEvent.click(await screen.findByRole("option", { name: "调研" }));

    await userEvent.click(within(f).getByRole("button", { name: "保存项目" }));
    const saved = callsOf("save_project")[0] as { project: Record<string, unknown> };
    expect(saved.project.record).toEqual({ issue: true, feishu_doc: true, meegle: true, feishu_task: true });
    expect(saved.project.record_rule).toBe("调研类建飞书任务；bug 建工单");
    expect(saved.project.meegle).toEqual({
      project_key: "pk1",
      simple_name: "bot",
      space_name: "AI 客服",
      work_item_type: "issue",
      work_item_type_name: "缺陷",
      fields: [
        { field_key: "template", field_name: "缺陷类型", value: "t1", label: "用户反馈缺陷" },
        { field_key: "end", field_name: "涉及端", value: "c1", label: "客户端" },
      ],
      ai_fields: [{ field_key: "priority", field_name: "优先级", field_type: "select", options: [{ id: "2", name: "P2" }], fallback: "2" }],
      assignee: { user_key: "u_li", name: "李四" },
      role: { role_id: "r_fix", role_name: "缺陷处理人" },
    });
    expect(saved.project.feishu_task).toMatchObject({ tasklist_guid: "tl1", tasklist_name: "调研" });
  });

  test("没登录飞书项目 CLI：显示去登录，点了走环境修复", async () => {
    setHandlers({
      list_projects: projectsView(),
      meegle_status: { installed: true, logged_in: false, host: null, user: null, error: "还没有登录飞书项目 CLI" },
      env_fix: undefined,
    });
    render(<ProjectsPage />);
    const card = await screen.findByTestId("project-demo");
    await userEvent.click(within(card).getByRole("button", { name: "编辑 demo" }));
    const f = screen.getByTestId("project-form-demo");
    await userEvent.click(within(f).getByRole("button", { name: /建飞书项目工单/ }));
    expect(await within(f).findByTestId("meegle-login-demo")).toHaveTextContent("还没有登录");
    await userEvent.click(within(f).getByRole("button", { name: /去登录/ }));
    await waitFor(() => expect(callsOf("env_fix")[0]).toEqual({ id: "meegle_login" }));
  });
});
