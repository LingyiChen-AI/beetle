// 测试数据工厂。
import type { Bot, BotView, EnvItem, RunRecord, StepRecord } from "../api";

export const bot = (over: Partial<Bot> = {}): Bot => ({
  id: "b1",
  name: "BUG纠察队",
  app_id: "cli_a1",
  engine: "claude",
  start_with_client: false,
  timeout_minutes: 30,
  concurrency: 2,
  template: "bug-hunt",
  skills: ["db-query"],
  extra_deny: [],
  auto_onboard: true,
  groups: [{ chat_id: "oc_1", name: "运营群", slug: "s1", projects: ["demo"], doc_url: null, trigger: "owner" }],
  ...over,
});

export const view = (
  over: Partial<Bot> = {},
  state: BotView["state"] = { kind: "stopped" },
  today_runs = 0,
  alive = state.kind === "running" || state.kind === "connecting",
): BotView => ({
  bot: bot(over),
  state,
  today_runs,
  alive,
});

/** 与后端 envcheck::BASE_IDS 一致 */
export const BASE_ENV_IDS = ["node", "python", "git", "lark_cli", "lark_login", "net_feishu"];
/** env_requirements：b1 是 Claude 机器人（查 bug，启用 db-query，不需要额外环境项） */
export const requirements = (bots: Record<string, string[]> = { b1: ["claude", ...BASE_ENV_IDS] }) => ({
  base: BASE_ENV_IDS,
  engines: ["claude", "codex"],
  bots,
});

export const run = (over: Partial<RunRecord> = {}): RunRecord => ({
  id: "r1",
  bot_id: "b1",
  chat_id: "oc_1",
  group_name: "运营群",
  trigger_message_id: "om_1",
  sender_name: "张三",
  text: "订单 123 查不到",
  engine: "claude",
  engine_version: null,
  session_id: null,
  status: "running",
  started_at: Date.now() - 60_000,
  finished_at: null,
  final_reply: null,
  error: null,
  sql_count: 0,
  reply_message_id: null,
  ...over,
});

export const step = (seq: number, title: string, over: Partial<StepRecord> = {}): StepRecord => ({
  run_id: "r1",
  seq,
  at: Date.now() - 60_000 + seq * 1000,
  kind: "tool",
  title,
  detail: `详情-${title}`,
  duration_ms: null,
  ...over,
});

export const envItem = (over: Partial<EnvItem> = {}): EnvItem => ({
  id: "python",
  label: "Python 3.10+",
  group: "runtime",
  status: "ok",
  detail: "",
  ...over,
});

/** App 渲染需要的基础命令返回值。 */
export function baseHandlers(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    list_bots: [view()],
    list_runs: [],
    list_projects: { projects: { demo: { repo: "r", branch_glob: "release/*", rules_file: "AGENTS.md", description: "", dbs: [] } }, dbs: {}, default_projects: [] },
    list_skills: [],
    catalog_list: [],
    env_check: [],
    env_requirements: requirements(),
    get_appearance: { mode: "dark", accent: "vermilion" },
    set_appearance: undefined,
    bot_stats: { today_runs: 0, today_replied: 0, success_rate: 0, avg_duration_ms: 0 },
    get_autostart: false,
    ...extra,
  };
}
