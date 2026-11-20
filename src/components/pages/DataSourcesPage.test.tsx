import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";
import type { DataSource } from "../../api";
import { callsOf, resetTauri, setHandlers } from "../../test/tauri";
import { ToastProvider } from "../common";
import DataSourcesPage from "./DataSourcesPage";

vi.mock("@tauri-apps/api/core", async () => (await import("../../test/tauri")).coreMock);
vi.mock("@tauri-apps/api/event", async () => (await import("../../test/tauri")).eventMock);

beforeEach(() => resetTauri());

const direct: DataSource = {
  kind: "mysql",
  description: "示例主库",
  access: "direct",
  host: "127.0.0.1",
  port: 3306,
  user: "readonly",
  database: "demo",
  ssh: null,
  timeout_secs: 5,
  max_rows: 200,
};
const viaSkill: DataSource = { kind: "mysql", description: "", access: "skill", skill: "my-sql", database: "demo_b", note: "库名写 demo_b" };

const view = () => ({
  projects: { demo: { repo: "gitee.com/a/demo.git", branch_glob: "release/*", rules_file: "AGENTS.md", description: "", dbs: ["demo_main"] } },
  dbs: { demo_main: direct, demo_skill: viaSkill },
  default_projects: [] as string[],
});

const localSkills = [
  { name: "my-sql", location: "claude", description: "只读查库" },
  { name: "my-sql", location: "codex", description: "只读查库" },
  { name: "mongo-reader", location: "claude", description: "" },
];

const renderPage = () =>
  render(
    <ToastProvider>
      <DataSourcesPage />
    </ToastProvider>,
  );

async function openNew() {
  renderPage();
  await screen.findByTestId("ds-demo_main");
  await userEvent.click(screen.getByRole("button", { name: "新建数据源" }));
  return screen.getByRole("dialog");
}

describe("数据源页", () => {
  test("卡片列出别名、类型、查询方式和关联项目", async () => {
    setHandlers({ list_projects: view() });
    renderPage();
    const card = await screen.findByTestId("ds-demo_main");
    expect(card).toHaveTextContent("MySQL");
    expect(card).toHaveTextContent("直连 · 只读");
    expect(card).toHaveTextContent("127.0.0.1:3306");
    expect(card).toHaveTextContent("demo");
    const skillCard = screen.getByTestId("ds-demo_skill");
    expect(skillCard).toHaveTextContent("技能查询");
    expect(skillCard).toHaveTextContent("my-sql");
    expect(skillCard).toHaveTextContent("没有项目关联");
    // 技能方式不能在客户端里测试连接
    expect(within(skillCard).queryByRole("button", { name: /测试连接/ })).toBeNull();
  });

  test("新建直连数据源：save_datasource 参数正确（密码单独传）", async () => {
    setHandlers({ list_projects: view(), list_local_skills: localSkills, save_datasource: undefined });
    const dialog = await openNew();
    expect(document.querySelector("select")).toBeNull();
    await userEvent.type(within(dialog).getByLabelText("别名"), "demo_new");
    await userEvent.type(within(dialog).getByLabelText("用途说明（会写进运行时给 Agent 看）"), "新库");
    await userEvent.clear(within(dialog).getByLabelText("主机"));
    await userEvent.type(within(dialog).getByLabelText("主机"), "db.local");
    await userEvent.type(within(dialog).getByLabelText("账号"), "ro");
    await userEvent.type(within(dialog).getByLabelText("密码"), "pw1");
    await userEvent.type(within(dialog).getByLabelText("库名"), "demo");
    await userEvent.click(within(dialog).getByRole("button", { name: "保存" }));
    expect(callsOf("save_datasource")).toEqual([
      {
        alias: "demo_new",
        ds: {
          kind: "mysql",
          description: "新库",
          access: "direct",
          host: "db.local",
          port: 3306,
          user: "ro",
          database: "demo",
          ssh: null,
          timeout_secs: 5,
          max_rows: 200,
        },
        password: "pw1",
      },
    ]);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  test("别名不合法或重名时不能保存，并提示原因", async () => {
    setHandlers({ list_projects: view(), list_local_skills: localSkills, save_datasource: undefined });
    const dialog = await openNew();
    await userEvent.type(within(dialog).getByLabelText("别名"), "demo_main");
    expect(within(dialog).getByTestId("ds-problem")).toHaveTextContent("别名 demo_main 已存在");
    expect(within(dialog).getByRole("button", { name: "保存" })).toBeDisabled();
    await userEvent.clear(within(dialog).getByLabelText("别名"));
    await userEvent.type(within(dialog).getByLabelText("别名"), "坏别名");
    expect(within(dialog).getByTestId("ds-problem")).toHaveTextContent("别名只能用字母、数字、下划线、短横线");
  });

  test("SSH 跳板：勾选后填写，保存时带上 ssh", async () => {
    setHandlers({ list_projects: view(), list_local_skills: localSkills, save_datasource: undefined });
    const dialog = await openNew();
    await userEvent.type(within(dialog).getByLabelText("别名"), "demo_pg");
    await userEvent.click(within(dialog).getByRole("combobox", { name: "类型" }));
    await userEvent.click(screen.getByRole("option", { name: /PostgreSQL/ }));
    // 端口跟着类型的默认值换
    expect(within(dialog).getByLabelText("端口")).toHaveValue(5432);
    await userEvent.type(within(dialog).getByLabelText("账号"), "ro");
    await userEvent.type(within(dialog).getByLabelText("库名"), "demo");
    expect(within(dialog).queryByLabelText("SSH 主机")).toBeNull();
    await userEvent.click(within(dialog).getByRole("checkbox", { name: /通过 SSH 跳板连接/ }));
    await userEvent.type(within(dialog).getByLabelText("SSH 主机"), "jump.local");
    await userEvent.type(within(dialog).getByLabelText("私钥路径"), "C:/k/id");
    await userEvent.click(within(dialog).getByRole("button", { name: "保存" }));
    expect(callsOf("save_datasource")[0]).toMatchObject({
      alias: "demo_pg",
      ds: { kind: "postgres", port: 5432, ssh: { host: "jump.local", port: 22, user: "root", key_path: "C:/k/id" } },
      password: null,
    });
  });

  test("切到自定义技能：隐藏主机等直连字段，技能从本机列表选，保存不传密码", async () => {
    setHandlers({ list_projects: view(), list_local_skills: localSkills, save_datasource: undefined });
    const dialog = await openNew();
    await userEvent.type(within(dialog).getByLabelText("别名"), "demo_s");
    await userEvent.click(within(dialog).getByRole("radio", { name: "自定义技能" }));
    expect(within(dialog).queryByLabelText("主机")).toBeNull();
    expect(within(dialog).queryByLabelText("密码")).toBeNull();
    expect(within(dialog).queryByRole("button", { name: "测试连接" })).toBeNull();
    await waitFor(() => expect(callsOf("list_local_skills")).toHaveLength(1));
    await userEvent.click(within(dialog).getByRole("combobox", { name: "查询技能" }));
    // Claude Code / Codex 都装了的同名技能只列一次
    expect(screen.getAllByRole("option", { name: /my-sql/ })).toHaveLength(1);
    await userEvent.click(screen.getByRole("option", { name: /my-sql/ }));
    await userEvent.type(within(dialog).getByLabelText("库名（传给技能的名字）"), "demo");
    await userEvent.type(within(dialog).getByLabelText("补充说明（给 Agent）"), "库名写 demo");
    await userEvent.click(within(dialog).getByRole("button", { name: "保存" }));
    expect(callsOf("save_datasource")).toEqual([
      { alias: "demo_s", ds: { kind: "mysql", description: "", access: "skill", skill: "my-sql", database: "demo", note: "库名写 demo" }, password: null },
    ]);
  });

  test("技能不在本机列表里可以手动填写", async () => {
    setHandlers({ list_projects: view(), list_local_skills: localSkills, save_datasource: undefined });
    const dialog = await openNew();
    await userEvent.type(within(dialog).getByLabelText("别名"), "demo_s");
    await userEvent.click(within(dialog).getByRole("radio", { name: "自定义技能" }));
    await waitFor(() => expect(callsOf("list_local_skills")).toHaveLength(1));
    await userEvent.click(within(dialog).getByRole("combobox", { name: "查询技能" }));
    await userEvent.click(screen.getByRole("option", { name: /手动填写/ }));
    await userEvent.type(within(dialog).getByLabelText("技能名"), "team-sql");
    await userEvent.click(within(dialog).getByRole("button", { name: "保存" }));
    expect(callsOf("save_datasource")[0]).toMatchObject({ ds: { access: "skill", skill: "team-sql" } });
  });

  test("类型选「其他」时只能用技能方式", async () => {
    setHandlers({ list_projects: view(), list_local_skills: localSkills });
    const dialog = await openNew();
    await userEvent.click(within(dialog).getByRole("combobox", { name: "类型" }));
    await userEvent.click(screen.getByRole("option", { name: /其他/ }));
    const radios = within(within(dialog).getByTestId("ds-access")).getAllByRole("radio");
    expect(radios).toHaveLength(1);
    expect(radios[0]).toHaveTextContent("自定义技能");
    expect(radios[0]).toHaveAttribute("aria-checked", "true");
    expect(within(dialog).queryByLabelText("主机")).toBeNull();
  });

  test("编辑：别名只读，密码留空传 null（保留已保存的密码）", async () => {
    setHandlers({ list_projects: view(), list_local_skills: localSkills, save_datasource: undefined });
    renderPage();
    await userEvent.click(within(await screen.findByTestId("ds-demo_main")).getByRole("button", { name: "编辑 demo_main" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByLabelText("别名")).toBeDisabled();
    expect(within(dialog).getByLabelText("别名")).toHaveValue("demo_main");
    expect(within(dialog).getByLabelText("密码")).toHaveAttribute("placeholder", "已保存，留空不修改");
    await userEvent.click(within(dialog).getByRole("button", { name: "保存" }));
    expect(callsOf("save_datasource")).toEqual([{ alias: "demo_main", ds: direct, password: null }]);
  });

  test("编辑技能方式的数据源：回填技能名、库名和说明", async () => {
    setHandlers({ list_projects: view(), list_local_skills: localSkills, save_datasource: undefined });
    renderPage();
    await userEvent.click(within(await screen.findByTestId("ds-demo_skill")).getByRole("button", { name: "编辑 demo_skill" }));
    const dialog = screen.getByRole("dialog");
    await waitFor(() => expect(within(dialog).getByRole("combobox", { name: "查询技能" })).toHaveTextContent("my-sql"));
    expect(within(dialog).getByLabelText("库名（传给技能的名字）")).toHaveValue("demo_b");
    await userEvent.click(within(dialog).getByRole("button", { name: "保存" }));
    expect(callsOf("save_datasource")).toEqual([{ alias: "demo_skill", ds: viaSkill, password: null }]);
  });

  test("测试连接：卡片上用已保存的密码，弹窗里用当前填写的", async () => {
    setHandlers({ list_projects: view(), list_local_skills: localSkills, test_datasource: "连接成功，耗时 3 ms" });
    renderPage();
    const card = await screen.findByTestId("ds-demo_main");
    await userEvent.click(within(card).getByRole("button", { name: "测试连接 demo_main" }));
    expect(callsOf("test_datasource")).toEqual([{ ds: direct, alias: "demo_main", password: null }]);
    expect(await within(card).findByTestId("ds-test-demo_main")).toHaveTextContent("连接成功，耗时 3 ms");

    await userEvent.click(within(card).getByRole("button", { name: "编辑 demo_main" }));
    const dialog = screen.getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("密码"), "newpw");
    await userEvent.click(within(dialog).getByRole("button", { name: "测试连接" }));
    expect(callsOf("test_datasource")[1]).toEqual({ ds: direct, alias: "demo_main", password: "newpw" });
  });

  test("测试连接失败显示原因", async () => {
    setHandlers({ list_projects: view(), test_datasource: () => Promise.reject("连接 127.0.0.1:3306 被拒绝") });
    renderPage();
    const card = await screen.findByTestId("ds-demo_main");
    await userEvent.click(within(card).getByRole("button", { name: "测试连接 demo_main" }));
    expect(await within(card).findByTestId("ds-test-demo_main")).toHaveTextContent("连接 127.0.0.1:3306 被拒绝");
  });

  test("删除被项目引用的数据源：提示先取消关联，不能删", async () => {
    setHandlers({ list_projects: view(), delete_datasource: undefined });
    renderPage();
    await userEvent.click(within(await screen.findByTestId("ds-demo_main")).getByRole("button", { name: "删除 demo_main" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByRole("alert")).toHaveTextContent("数据源 demo_main 被项目 demo 使用，先在项目里取消关联");
    expect(within(dialog).getByRole("button", { name: "确认删除" })).toBeDisabled();
    expect(callsOf("delete_datasource")).toEqual([]);
  });

  test("删除未引用的数据源调用 delete_datasource；后端报错就近显示", async () => {
    setHandlers({ list_projects: view(), delete_datasource: () => Promise.reject("数据源 demo_skill 被项目 other 使用，先在项目里取消关联") });
    renderPage();
    await userEvent.click(within(await screen.findByTestId("ds-demo_skill")).getByRole("button", { name: "删除 demo_skill" }));
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "确认删除" }));
    expect(callsOf("delete_datasource")).toEqual([{ alias: "demo_skill" }]);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("被项目 other 使用");
  });

  test("没有数据源时显示空状态", async () => {
    setHandlers({ list_projects: { projects: {}, dbs: {}, default_projects: [] } });
    renderPage();
    expect(await screen.findByText("还没有数据源")).toBeInTheDocument();
  });
});
