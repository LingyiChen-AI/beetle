// 运行记录 / 步骤的合并规则。事件可能乱序到达（例如 queued 的 run-updated 晚于 running），
// 合并时状态只能前进：queued → running → 终态，不允许回退。
import type { RunRecord, RunStatus, StepRecord } from "../api";

const RANK: Record<RunStatus, number> = {
  queued: 0,
  running: 1,
  replied: 2,
  failed: 2,
  timeout: 2,
  cancelled: 2,
};

export const isTerminal = (s: RunStatus) => RANK[s] === 2;

/** 用新到的记录更新旧记录；新记录状态比旧的「靠前」时整条丢弃（旧记录已经是更新的状态）。 */
export function mergeRun(prev: RunRecord | undefined, next: RunRecord): RunRecord {
  if (!prev) return next;
  if (RANK[next.status] < RANK[prev.status]) return prev;
  // 已到终态后又来一条不同终态：以先到的终态为准（防重复事件）。
  // 例外：「已回复」之后群里回复失败，后端会把记录改成「失败」（审查 B10），这一种要接受。
  const replyFailed = prev.status === "replied" && next.status === "failed";
  if (isTerminal(prev.status) && isTerminal(next.status) && prev.status !== next.status && !replyFailed) return prev;
  return next;
}

/** 把一条 run-updated 合并进列表：已有则替换（遵守不回退），没有则插到最前。 */
export function upsertRun(list: RunRecord[], next: RunRecord, limit = 500): RunRecord[] {
  const i = list.findIndex((r) => r.id === next.id);
  if (i < 0) return [next, ...list].slice(0, limit);
  const merged = mergeRun(list[i], next);
  if (merged === list[i]) return list;
  const out = [...list];
  out[i] = merged;
  return out;
}

/** 追加步骤：按 seq 去重、排序。 */
export function addStep(steps: StepRecord[], step: StepRecord): StepRecord[] {
  if (steps.some((s) => s.seq === step.seq)) return steps;
  return [...steps, step].sort((a, b) => a.seq - b.seq);
}
