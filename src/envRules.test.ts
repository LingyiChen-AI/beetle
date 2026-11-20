import { describe, expect, test } from "vitest";
import { botBlockers, onboardingBlockers } from "./envRules";
import { envItem, requirements } from "./test/fixtures";

describe("环境阻断规则（与后端 envcheck::requirements / blockers 一致）", () => {
  const env = [
    envItem({ id: "claude", status: "ok" }),
    envItem({ id: "codex", status: "error" }),
    envItem({ id: "git", status: "warn" }),
    envItem({ id: "system_proxy", status: "error" }),
  ];

  test("机器人只被自己所需项里的 error 拦：未选用的引擎、无关项、警告都不拦", () => {
    expect(botBlockers(env, requirements(), "b1")).toEqual([]);
    const codexBot = requirements({ b1: ["codex", "git"] });
    expect(botBlockers(env, codexBot, "b1").map((e) => e.id)).toEqual(["codex"]);
  });

  test("规则未知时不在前端拦（交给后端 start_bot）", () => {
    expect(botBlockers(env, null, "b1")).toEqual([]);
    expect(botBlockers(env, requirements(), "不存在的机器人")).toEqual([]);
  });

  test("首次引导：只拦基础项，引擎至少一个可用即可", () => {
    expect(onboardingBlockers(env, requirements())).toEqual([]);
    const noEngine = [envItem({ id: "claude", status: "error" }), envItem({ id: "codex", status: "error" }), envItem({ id: "node", status: "error" })];
    expect(onboardingBlockers(noEngine, requirements()).map((e) => e.id)).toEqual(["node", "claude", "codex"]);
    // 没有后端规则时用同样的兜底
    expect(onboardingBlockers(noEngine, null).map((e) => e.id)).toEqual(["node", "claude", "codex"]);
    expect(onboardingBlockers([envItem({ id: "codex", status: "error" }), envItem({ id: "claude", status: "warn" })], null)).toEqual([]);
  });
});
