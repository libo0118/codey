import assert from "node:assert/strict";
import test from "node:test";
import { createModuleGraph } from "./helpers/jsx-tree.mjs";

const { isGoal, goalResult, goalExpected } = createModuleGraph(new URL("../src/remote/goals.ts", import.meta.url)).exports;
const goal = { objective: "测试目标", status: "paused", createdAt: 12, tokensUsed: 100, tokenBudget: null };

test("Goal 网络边界拒绝未知状态、类型转换、非法数值和 UTF-8 超长内容", () => {
  assert.equal(isGoal(goal), true);
  for (const patch of [{ status: ["active"] }, { status: "future" }, { createdAt: "12" }, { tokensUsed: -1 }, { tokenBudget: 1.5 }, { timeUsedSeconds: Infinity }, { objective: "中".repeat(6667) }]) assert.equal(isGoal({ ...goal, ...patch }), false);
  assert.equal(isGoal(null), false);
  assert.deepEqual(goalExpected(goal), { objective: goal.objective, status: goal.status, createdAt: 12 });
  assert.equal(goalExpected(null), null);
});

test("未知能力与缺失 Goal 不伪装 none，持久回执重放须单独重读当前状态", () => {
  const result = { status: "ok", availability: "ready", goal, activation: "not-requested" };
  assert.equal(goalResult(result), result);
  assert.equal(goalResult({ ...result, goal: null }).goal, null);
  for (const patch of [{ goal: undefined }, { goal: { ...goal, status: "future" } }, { activation: "started" }, { availability: ["ready"] }]) assert.throws(() => goalResult({ ...result, ...patch }), /目标响应无效/);
  assert.equal(goalResult({ ...result, goal: undefined, replayed: true }).replayed, true);
});
