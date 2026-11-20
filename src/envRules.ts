// 「哪些环境项会阻止启动」：规则由后端 `env_requirements` 给出（src-tauri/src/envcheck/mod.rs 的 requirements / blockers），
// 与后端 start_bot、托盘点击、开机自启里的 ensure_ready 同一套：
//   机器人所需项 = 基础项 + 该机器人所选引擎 + 已启用技能的 requires；所需项里状态为 error 的阻止启动（warn 不拦）。
// 首次引导没有具体机器人：只拦基础项，引擎至少有一个不是 error 即可。
import type { EnvItem, EnvRequirements } from "./api";

/** 后端规则还没取到（或取失败）时的兜底：与 envcheck::BASE_IDS / ENGINE_IDS 一致。 */
const FALLBACK_BASE = ["node", "python", "git", "lark_cli", "lark_login", "net_feishu"];
const FALLBACK_ENGINES = ["claude", "codex"];

/** 某个机器人的阻断项。规则未知（后端没返回这个机器人）时不在前端拦，交给后端 start_bot 判断。 */
export function botBlockers(env: EnvItem[], req: EnvRequirements | null, botId: string): EnvItem[] {
  const ids = req?.bots[botId];
  if (!ids) return [];
  return env.filter((e) => e.status === "error" && ids.includes(e.id));
}

/** 首次引导「环境检查」一步的阻断项：基础项里的 error，加上「所有引擎都是 error」时的引擎项。 */
export function onboardingBlockers(env: EnvItem[], req: EnvRequirements | null): EnvItem[] {
  const base = req?.base ?? FALLBACK_BASE;
  const engines = req?.engines ?? FALLBACK_ENGINES;
  const baseErrors = env.filter((e) => e.status === "error" && base.includes(e.id));
  const engineItems = env.filter((e) => engines.includes(e.id));
  const anyEngineUsable = engineItems.length === 0 || engineItems.some((e) => e.status !== "error");
  return anyEngineUsable ? baseErrors : [...baseErrors, ...engineItems];
}
