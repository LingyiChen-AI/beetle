// 前端对 Tauri 命令与事件的唯一封装。命令名与参数一一对应 src-tauri/src/commands.rs / commands_ext.rs。
// Tauri 会把 Rust 参数名 snake_case 转成这里的 camelCase（bot_id → botId）。
//
// 开发模式下直接用浏览器打开 vite（没有 Tauri 后端，window.__TAURI_INTERNALS__ 不存在）时，
// 命令和事件改走 src/mock/devMock.ts 的示例数据，方便看界面、截图。生产构建里这段分支会被整个去掉。
import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";

/** 是否走开发 mock：仅 dev 模式、非测试、且不在 Tauri 窗口里。 */
export function isDevMock(): boolean {
  return (
    import.meta.env.DEV &&
    import.meta.env.MODE !== "test" &&
    typeof window !== "undefined" &&
    !("__TAURI_INTERNALS__" in window)
  );
}

function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (import.meta.env.DEV && isDevMock()) {
    return import("./mock/devMock").then((m) => m.mockInvoke(cmd, args ?? {}) as Promise<T>);
  }
  return tauriInvoke<T>(cmd, args);
}

function listen<T>(event: string, cb: (e: { payload: T }) => void): Promise<UnlistenFn> {
  if (import.meta.env.DEV && isDevMock()) {
    return import("./mock/devMock").then((m) => m.mockListen(event, cb as (e: { payload: unknown }) => void));
  }
  return tauriListen<T>(event, cb);
}

export type Engine = "claude" | "codex";
export type Trigger = "owner" | "all";
/** 触发方式：mention = @ 机器人；pr_link = 群里发 Gitee / GitHub PR 链接自动触发（不用 @）；both = 两者都可。 */
export type TriggerMode = "mention" | "pr_link" | "both";

/** 代码审查模板名与它的默认技能（向导选这个模板时用）。 */
export const CODE_REVIEW_TEMPLATE = "code-review";
export const CODE_REVIEW_SKILLS = ["pr-review", "code-sync"];
export const DEFAULT_SKILLS = ["chat-context", "code-sync", "db-query", "issue-record", "feishu-doc-record"];

export interface Group {
  chat_id: string;
  name: string;
  slug: string;
  projects: string[];
  doc_url: string | null;
  trigger: Trigger;
}

export interface Bot {
  id: string;
  name: string;
  app_id: string;
  engine: Engine;
  start_with_client: boolean;
  timeout_minutes: number;
  concurrency: number;
  template: string;
  skills: string[];
  extra_deny: string[];
  auto_onboard: boolean;
  groups: Group[];
  /** 后端总会返回；旧配置没有这个字段时按 mention（@ 触发）。 */
  trigger_mode?: TriggerMode;
}

export type BotState =
  | { kind: "stopped" | "connecting" | "running" }
  | { kind: "error"; message: string };

/** list_bots 的一项。不含 App Secret（密钥只在系统凭据库里）。 */
export interface BotView {
  bot: Bot;
  state: BotState;
  today_runs: number;
  /** 连接任务是否还在：异常且为 true = 「连接失败，正在重试」，此时界面给「停止」而不是「启动」。 */
  alive?: boolean;
}

/** 查 bug 找到问题后的记录方式，可同时选。 */
export interface RecordOpts {
  /** 在项目仓库（Gitee / GitHub）建 issue，同标题已存在则追加评论。 */
  issue: boolean;
  /** 追加到群配置的飞书文档。 */
  feishu_doc: boolean;
}

export interface Project {
  repo: string;
  branch_glob: string;
  rules_file: string;
  description: string;
  /** 关联的数据源别名（数据源页里配置） */
  dbs: string[];
  /** 后端总会返回；不传时后端按仓库地址推断（gitee / github → 建 issue），写飞书文档默认开。 */
  record?: RecordOpts;
  /** 建 issue 的仓库 owner/repo；null = 从仓库地址解析。 */
  issue_repo?: string | null;
  /** 读 token 的环境变量名；null = Gitee 用 GITEE_TOKEN、GitHub 用 GITHUB_TOKEN。 */
  token_env?: string | null;
}

/** check_issue_token 的结果：只有是否检测到和用户名，不含 token。 */
export interface IssueTokenStatus {
  platform: "gitee" | "github" | null;
  platform_label: string | null;
  /** owner/repo */
  repo: string | null;
  token_env: string | null;
  detected: boolean;
  user: string | null;
  error: string | null;
}

// ───────────── 数据源（src-tauri/src/store/datasource.rs）：用户自己配置的库，两种查询方式 ─────────────
/** other 只能配合「自定义技能」查询。 */
export type DbKind = "mysql" | "postgres" | "starrocks" | "mongodb" | "other";
export type DbAccess = "direct" | "skill";

export interface SshTunnel {
  host: string;
  port: number;
  user: string;
  key_path: string;
}

/** 直连：客户端连库并强制只读，密码只在系统凭据库里（不在这里）。 */
export interface DirectConn {
  access: "direct";
  host: string;
  port: number;
  user: string;
  database: string;
  ssh: SshTunnel | null;
  timeout_secs: number;
  max_rows: number;
}

/** 自定义技能：Agent 用本机已装的某个技能去查，Beetle 只告诉它用哪个技能。 */
export interface SkillRef {
  access: "skill";
  skill: string;
  database: string;
  /** 给 Agent 的补充说明，例如「库名写 demo」 */
  note: string;
}

/** 与 Rust serde 输出一致：kind / description 加上按 access 打标签的连接字段（flatten）。 */
export type DataSource = { kind: DbKind; description: string } & (DirectConn | SkillRef);

export interface LocalSkill {
  name: string;
  location: "claude" | "codex";
  description: string;
}

export interface ProjectsView {
  projects: Record<string, Project>;
  dbs: Record<string, DataSource>;
  default_projects: string[];
}

export type RunStatus = "queued" | "running" | "replied" | "failed" | "timeout" | "cancelled";

export interface RunRecord {
  id: string;
  bot_id: string;
  chat_id: string;
  group_name: string;
  trigger_message_id: string;
  sender_name: string;
  text: string;
  engine: Engine;
  engine_version: string | null;
  session_id: string | null;
  status: RunStatus;
  started_at: number;
  finished_at: number | null;
  final_reply: string | null;
  error: string | null;
  sql_count: number;
  reply_message_id: string | null;
  /** 触发人的 union_id（续聊校验用）；旧记录没有 */
  sender_union_id?: string | null;
}

export interface StepRecord {
  run_id: string;
  seq: number;
  at: number;
  kind: string;
  title: string;
  detail: string;
  duration_ms: number | null;
}

export interface RunDetail {
  run: RunRecord;
  steps: StepRecord[];
}

export interface SkillInfo {
  name: string;
  display_name: string;
  description: string;
  requires: string[];
  builtin: boolean;
}

export interface SkillField {
  key: string;
  label: string;
  type: string;
  required: boolean;
}

export interface SkillSettingsView {
  fields: SkillField[];
  value: Record<string, unknown>;
}

export interface RegisteredApp {
  appId: string;
  botName: string | null;
}

/** 后端命令的错误是一段中文字符串；其它异常取 message。 */
export function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

export const listBots = () => invoke<BotView[]>("list_bots");
/** `appSecret` 手填的密钥；`attempt` 扫码创建应用的尝试 id（密钥由后端暂存）。两者都不传 = 不修改密钥。 */
export const saveBot = (bot: Bot, appSecret?: string | null, attempt?: string | null) =>
  invoke<Bot>("save_bot", { bot, appSecret: appSecret ?? null, attempt: attempt ?? null });
export const deleteBot = (id: string) => invoke<void>("delete_bot", { id });
export const startBot = (id: string) => invoke<void>("start_bot", { id });
export const stopBot = (id: string) => invoke<void>("stop_bot", { id });
export const setStartWithClient = (id: string, on: boolean) => invoke<void>("set_start_with_client", { id, on });

export const saveGroup = (botId: string, group: Group) => invoke<void>("save_group", { botId, group });
export const removeGroup = (botId: string, chatId: string) => invoke<void>("remove_group", { botId, chatId });

export const listProjects = () => invoke<ProjectsView>("list_projects");
export const saveProject = (name: string, project: Project) => invoke<void>("save_project", { name, project });
export const removeProject = (name: string) => invoke<void>("remove_project", { name });
/** 检测建 issue 用的 token（传表单里当前的项目，不必先保存）。只返回是否检测到与用户名。 */
export const checkIssueToken = (project: Project) => invoke<IssueTokenStatus>("check_issue_token", { project });
/** `password` 为 null / 空串 = 保留已保存的密码（编辑时留空不修改）。 */
export const saveDatasource = (alias: string, ds: DataSource, password: string | null) =>
  invoke<void>("save_datasource", { alias, ds, password: password ? password : null });
/** 被项目引用时后端报错「数据源 X 被项目 Y 使用…」；同时删掉凭据库里的密码。 */
export const deleteDatasource = (alias: string) => invoke<void>("delete_datasource", { alias });
/** 测试连接（直连）：password 为空时用 alias 在凭据库里已保存的密码。返回「连接成功，耗时 x ms」。 */
export const testDatasource = (ds: DataSource, alias: string | null, password: string | null) =>
  invoke<string>("test_datasource", { ds, alias, password: password ? password : null });
/** 本机已装的技能（~/.claude/skills、~/.codex/skills），给「查询技能」下拉用。 */
export const listLocalSkills = () => invoke<LocalSkill[]>("list_local_skills");
export const setDefaultProjects = (names: string[]) => invoke<void>("set_default_projects", { names });

export const listRuns = (botId: string | null, limit: number) => invoke<RunRecord[]>("list_runs", { botId, limit });
export const getRun = (id: string) => invoke<RunDetail>("get_run", { id });
export const cancelRun = (id: string) => invoke<void>("cancel_run", { id });
export const retryRun = (id: string) => invoke<void>("retry_run", { id });

export const listSkills = () => invoke<SkillInfo[]>("list_skills");
export const getSkillSettings = (name: string) => invoke<SkillSettingsView>("get_skill_settings", { name });
export const saveSkillSettings = (name: string, value: Record<string, unknown>) =>
  invoke<void>("save_skill_settings", { name, value });

export const getAutostart = () => invoke<boolean>("get_autostart");
export const setAutostart = (on: boolean) => invoke<void>("set_autostart", { on });

/** 扫码创建飞书应用：`onUrl` 收到授权页地址（让用户用飞书扫码 / 打开）。 */
export function createFeishuApp(attempt: string, onUrl: (url: string) => void): Promise<RegisteredApp> {
  if (import.meta.env.DEV && isDevMock()) {
    return import("./mock/devMock").then((m) => m.mockCreateFeishuApp(onUrl));
  }
  const channel = new Channel<string>();
  channel.onmessage = onUrl;
  return invoke<RegisteredApp>("feishu_create_app", { attempt, onUrl: channel });
}
export const cancelCreateFeishuApp = (attempt: string) => invoke<void>("feishu_cancel_create_app", { attempt });

const CREATE_ERRORS: Record<string, string> = {
  create_busy: "已有一个扫码创建在进行中，请先完成或取消",
  create_cancelled: "已取消创建",
  create_timeout: "扫码超时，请重新创建",
  create_failed: "创建失败，请重试或手动填写 App ID / Secret",
};
export const createErrorText = (e: unknown) => CREATE_ERRORS[errText(e)] ?? errText(e);

// ───────────── 第二三阶段新增命令（接口约定见 docs/superpowers/plans/2026-11-19-phase23-ui-onboarding.md） ─────────────
export type EnvGroup = "runtime" | "feishu" | "network";
export type EnvStatus = "ok" | "warn" | "error" | "checking";
export interface EnvAction {
  kind: "install" | "login" | "open_settings" | "fix";
  label: string;
}
export interface EnvItem {
  id: string;
  label: string;
  group: EnvGroup;
  status: EnvStatus;
  version?: string;
  detail: string;
  action?: EnvAction;
}
export const envCheck = () => invoke<EnvItem[]>("env_check");
/** 阻断规则（见 src/envRules.ts）：基础项、引擎项、每个机器人的所需项。 */
export interface EnvRequirements {
  base: string[];
  engines: string[];
  bots: Record<string, string[]>;
}
export const envRequirements = () => invoke<EnvRequirements>("env_requirements");
export const envFix = (id: string) => invoke<void>("env_fix", { id });

export interface RawLog {
  stdout: string;
  stderr: string;
}
export const getRawLog = (runId: string) => invoke<RawLog>("get_raw_log", { runId });

export interface TemplateInfo {
  name: string;
  builtin: boolean;
  modified: boolean;
}
export const listTemplates = () => invoke<TemplateInfo[]>("list_templates");
export const getTemplate = (name: string) => invoke<string>("get_template", { name });
export const saveTemplate = (name: string, content: string) => invoke<void>("save_template", { name, content });
export const resetTemplate = (name: string) => invoke<void>("reset_template", { name });

export interface ImportPreview {
  projects: string[];
  dbs: string[];
  templates: string[];
  skills: string[];
  bots: { name: string; app_id: string }[];
}
export interface ImportSelect {
  projects: string[];
  dbs: string[];
  templates: string[];
  skills: string[];
}
export const exportConfig = (path: string) => invoke<void>("export_config", { path });
export const previewImport = (path: string) => invoke<ImportPreview>("preview_import", { path });
export const applyImport = (path: string, select: ImportSelect) => invoke<void>("apply_import", { path, select });

export interface LegacyInfo {
  found: boolean;
  app_id?: string;
  groups: { chat_id: string; name: string; doc_url: string }[];
  projects: string[];
  bridge_running: boolean;
  task_enabled: boolean;
}
export const detectLegacy = () => invoke<LegacyInfo>("detect_legacy");
export const migrateLegacy = (stopBridge: boolean) =>
  invoke<{ bot_id: string; warnings: string[] }>("migrate_legacy", { stopBridge });

export const importLocalSkill = (dir: string) => invoke<SkillInfo>("import_local_skill", { dir });

// ───────────── 技能中心（src-tauri/src/catalog.rs）：本产品自己做的技能，安装到本机 Claude Code / Codex ─────────────
export type SkillTarget = "claude" | "codex";
export const SKILL_TARGETS: { id: SkillTarget; label: string }[] = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
];
export interface CatalogEntry {
  name: string;
  display_name: string;
  description: string;
  version: string;
  /** 环境检查项 id：python / node / lark_cli / git */
  requires: string[];
  /** 本客户端安装的版本；没装为 null */
  installed: Record<SkillTarget, string | null>;
  update_available: Record<SkillTarget, boolean>;
}
export const catalogList = () => invoke<CatalogEntry[]>("catalog_list");
export const catalogInstall = (name: string, targets: SkillTarget[]) => invoke<void>("catalog_install", { name, targets });
export const catalogUninstall = (name: string, target: SkillTarget) => invoke<void>("catalog_uninstall", { name, target });
/** 把当前项目 / 数据源设置写进已安装的技能，返回刷新了几处。 */
export const catalogRefreshConfig = () => invoke<number>("catalog_refresh_config");
/** 技能目录里的一项（src-tauri/src/commands_catalog_view.rs）：先序排列，目录紧跟着它的子项。 */
export interface CatalogFile {
  /** 相对技能目录，正斜杠 */
  path: string;
  name: string;
  size: number;
  is_dir: boolean;
  /** 0 = 第一层 */
  depth: number;
}
export interface CatalogFileContent {
  path: string;
  size: number;
  /** 二进制或超过 512KB 时为 null */
  content: string | null;
  binary: boolean;
  too_large: boolean;
}
export const catalogFiles = (name: string) => invoke<CatalogFile[]>("catalog_files", { name });
export const catalogReadFile = (name: string, path: string) => invoke<CatalogFileContent>("catalog_read_file", { name, path });

export type AppearanceMode = "dark" | "light" | "system";
export type Accent = "vermilion" | "green" | "blue" | "indigo" | "purple" | "orange" | "teal";
export interface Appearance {
  mode: AppearanceMode;
  accent: Accent;
}
export const getAppearance = () => invoke<Appearance>("get_appearance");
export const setAppearance = (appearance: Appearance) => invoke<void>("set_appearance", { appearance });

/** 设置页「运行」：代理、代码缓存目录、日志保留天数（空 = 默认）。 */
export interface RuntimeSettings {
  proxy: string | null;
  code_cache_dir: string | null;
  log_retention_days: number;
}
export interface RuntimeSettingsView extends RuntimeSettings {
  default_code_cache_dir: string;
  min_retention_days: number;
  max_retention_days: number;
}
export const getRuntimeSettings = () => invoke<RuntimeSettingsView>("get_runtime_settings");
export const setRuntimeSettings = (settings: RuntimeSettings) => invoke<void>("set_runtime_settings", { settings });

export interface BotStats {
  today_runs: number;
  today_replied: number;
  /** 0–1 */
  success_rate: number;
  avg_duration_ms: number;
}
export const botStats = (botId: string) => invoke<BotStats>("bot_stats", { botId });

// ───────────── 数据统计（src-tauri/src/commands_stats.rs） ─────────────
export type StatsRange = "today" | "7d" | "30d";
export interface StatsTotals {
  runs: number;
  replied: number;
  failed: number;
  timeout: number;
  cancelled: number;
  /** 还在排队 / 运行中 */
  active: number;
  /** 已出结果（回复 + 失败 + 超时），成功率的分母 */
  settled: number;
  /** 0–1 */
  success_rate: number;
  /** 仅已回复的运行 */
  avg_duration_ms: number;
  p90_duration_ms: number;
  sql_count: number;
}
export interface StatsDay {
  /** MM-DD */
  day: string;
  start_ms: number;
  runs: number;
  replied: number;
  failed: number;
  timeout: number;
}
export interface StatsBotRow {
  bot_id: string;
  name: string;
  runs: number;
  replied: number;
  success_rate: number;
  avg_duration_ms: number;
  sql_count: number;
}
export interface StatsNamedCount {
  key: string;
  name: string;
  runs: number;
}
export interface StatsOverview {
  range: StatsRange;
  bot_id: string | null;
  start_ms: number;
  end_ms: number;
  totals: StatsTotals;
  /** 上一周期（同样天数、往前平移、截止到同一时刻） */
  previous: StatsTotals;
  daily: StatsDay[];
  by_bot: StatsBotRow[];
  by_group: StatsNamedCount[];
  by_sender: StatsNamedCount[];
  by_engine: { engine: Engine | string; runs: number }[];
  failure_reasons: { reason: string; count: number }[];
  /** 0–23 点每小时运行数 */
  hourly: number[];
}
/** `botId` 为 null = 全部机器人。 */
export const statsOverview = (range: StatsRange, botId: string | null) => invoke<StatsOverview>("stats_overview", { range, botId });

// ───────────── 自动更新 ─────────────
/** 更新状态：后端启动后检查一次、之后每 30 分钟检查 GitHub Release。 */
export interface UpdateStatus {
  current: string;
  available: string | null;
  notes: string | null;
  checked_at: number | null;
  error: string | null;
}
export const updateStatus = () => invoke<UpdateStatus>("update_status");
export const checkUpdate = () => invoke<UpdateStatus>("check_update");
/** 下载安装新版本，成功后客户端自动重启（这个调用一般不会返回）。 */
export const installUpdate = () => invoke<void>("install_update");
export const onUpdateAvailable = (cb: (s: UpdateStatus) => void): Promise<UnlistenFn> =>
  listen<UpdateStatus>("update-available", (e) => cb(e.payload));
/** 下载进度（字节）；total 未知时为 null。 */
export const onUpdateProgress = (cb: (p: { downloaded: number; total: number | null }) => void): Promise<UnlistenFn> =>
  listen<{ downloaded: number; total: number | null }>("update-progress", (e) => cb(e.payload));

/** 客户端版本号（tauri.conf.json 的 version）。 */
export const getAppVersion = async (): Promise<string> => {
  if (isDevMock()) return "0.1.0-dev";
  const { getVersion } = await import("@tauri-apps/api/app");
  return getVersion();
};

// ───────────── 窗口（自绘标题栏） ─────────────
// 关闭 = 触发 CloseRequested，后端 tray.rs 拦截后隐藏到托盘。
async function currentWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}
export const windowMinimize = async () => {
  if (isDevMock()) return;
  await (await currentWindow()).minimize();
};
export const windowToggleMaximize = async () => {
  if (isDevMock()) return;
  await (await currentWindow()).toggleMaximize();
};
export const windowClose = async () => {
  if (isDevMock()) return;
  await (await currentWindow()).close();
};

// ───────────── 事件 ─────────────
export interface BotStateEvent {
  bot_id: string;
  state: BotState;
}

export const onBotState = (cb: (e: BotStateEvent) => void): Promise<UnlistenFn> =>
  listen<BotStateEvent>("bot-state", (e) => cb(e.payload));
export const onRunUpdated = (cb: (run: RunRecord) => void): Promise<UnlistenFn> =>
  listen<{ run: RunRecord }>("run-updated", (e) => cb(e.payload.run));
export const onRunStep = (cb: (step: StepRecord) => void): Promise<UnlistenFn> =>
  listen<{ step: StepRecord }>("run-step", (e) => cb(e.payload.step));
export const onEnvInstallLog = (cb: (e: { id: string; line: string }) => void): Promise<UnlistenFn> =>
  listen<{ id: string; line: string }>("env-install-log", (e) => cb(e.payload));
export const onEnvInstallDone = (cb: (e: { id: string; ok: boolean; message: string }) => void): Promise<UnlistenFn> =>
  listen<{ id: string; ok: boolean; message: string }>("env-install-done", (e) => cb(e.payload));

// ───────────── 文案 ─────────────
export function stateText(s: BotState): string {
  switch (s.kind) {
    case "stopped":
      return "已停止";
    case "connecting":
      return "连接中";
    case "running":
      return "运行中";
    case "error":
      return "异常";
  }
}

export const RUN_STATUS_TEXT: Record<RunStatus, string> = {
  queued: "排队中",
  running: "运行中",
  replied: "已回复",
  failed: "失败",
  timeout: "超时",
  cancelled: "已取消",
};

/** 事件订阅的清理：组件卸载时调用，订阅还没建好（Promise 未返回）也能正确取消。 */
export function subscribe(...subs: Promise<UnlistenFn>[]): () => void {
  let cancelled = false;
  const fns: UnlistenFn[] = [];
  subs.forEach((p) =>
    p.then((fn) => {
      if (cancelled) fn();
      else fns.push(fn);
    }),
  );
  return () => {
    cancelled = true;
    fns.forEach((fn) => fn());
  };
}
