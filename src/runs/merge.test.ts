import { describe, expect, test } from "vitest";
import { run, step } from "../test/fixtures";
import { addStep, mergeRun, upsertRun } from "./merge";

describe("运行记录合并不回退", () => {
  test("running 之后到达的 queued 被丢弃", () => {
    const prev = run({ status: "running", sql_count: 2 });
    expect(mergeRun(prev, run({ status: "queued" }))).toBe(prev);
  });

  test("终态之后到达的 queued / running 被丢弃", () => {
    for (const late of ["queued", "running"] as const) {
      const prev = run({ status: "replied" });
      expect(mergeRun(prev, run({ status: late }))).toBe(prev);
    }
  });

  test("状态前进正常替换，同状态更新字段", () => {
    expect(mergeRun(run({ status: "queued" }), run({ status: "running" })).status).toBe("running");
    expect(mergeRun(run({ status: "running", sql_count: 1 }), run({ status: "running", sql_count: 3 })).sql_count).toBe(3);
    expect(mergeRun(run({ status: "running" }), run({ status: "failed" })).status).toBe("failed");
  });

  test("审查 B10：已回复后群里回复失败改记失败，界面跟着变；其它终态之间不互相覆盖", () => {
    const failed = run({ status: "failed", error: "群里回复失败：机器人不在群里", final_reply: "OK" });
    expect(mergeRun(run({ status: "replied", final_reply: "OK" }), failed)).toBe(failed);
    const cancelled = run({ status: "cancelled" });
    expect(mergeRun(cancelled, run({ status: "failed" }))).toBe(cancelled);
    const failedFirst = run({ status: "failed" });
    expect(mergeRun(failedFirst, run({ status: "replied" }))).toBe(failedFirst);
  });

  test("upsertRun：新记录插到最前，旧记录不回退", () => {
    const list = [run({ id: "a", status: "running" })];
    const added = upsertRun(list, run({ id: "b", status: "queued" }));
    expect(added.map((r) => r.id)).toEqual(["b", "a"]);
    expect(upsertRun(added, run({ id: "a", status: "queued" }))).toBe(added);
  });

  test("addStep 按 seq 去重排序", () => {
    const s = addStep(addStep([step(2, "b")], step(1, "a")), step(2, "b"));
    expect(s.map((x) => x.seq)).toEqual([1, 2]);
  });
});
