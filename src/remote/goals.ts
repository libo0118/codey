export type Goal = {
  objective: string;
  status: "active" | "paused" | "blocked" | "usageLimited" | "budgetLimited" | "complete";
  goalId?: string;
  createdAt?: number;
  updatedAt?: number;
  tokenBudget?: number | null;
  tokensUsed?: number;
  timeUsedSeconds?: number;
};
export type GoalCommand = "create" | "edit" | "pause" | "resume";
export type GoalAvailability = "ready" | "unavailable" | "unsupported";
export type GoalResult = { status: "ok"; availability: GoalAvailability; goal?: Goal | null; activation: "not-requested" | "accepted" | "unknown"; replayed?: boolean };

export function isGoal(value: unknown): value is Goal {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const goal = value as Record<string, unknown>;
  return typeof goal.objective === "string" && !!goal.objective.trim() && new TextEncoder().encode(goal.objective).length <= 20_000
    && typeof goal.status === "string" && ["active", "paused", "blocked", "usageLimited", "budgetLimited", "complete"].includes(goal.status)
    && (goal.goalId === undefined || typeof goal.goalId === "string" && !!goal.goalId && goal.goalId.length <= 512)
    && [goal.createdAt, goal.updatedAt, goal.tokensUsed, goal.timeUsedSeconds].every(number => number === undefined || typeof number === "number" && Number.isSafeInteger(number) && number >= 0)
    && (goal.tokenBudget == null || typeof goal.tokenBudget === "number" && Number.isSafeInteger(goal.tokenBudget) && goal.tokenBudget >= 0);
}

export function goalResult(value: unknown): GoalResult {
  const result = value as GoalResult | null;
  if (!result || typeof result !== "object" || result.status !== "ok" || !["ready", "unavailable", "unsupported"].includes(result.availability)
    || !["not-requested", "accepted", "unknown"].includes(result.activation) || result.replayed !== undefined && typeof result.replayed !== "boolean"
    || result.goal !== null && result.goal !== undefined && !isGoal(result.goal)
    || result.availability === "ready" && result.goal === undefined && result.replayed !== true) throw new Error("目标响应无效，请重新读取状态");
  return result;
}

export function goalExpected(goal: Goal | null) {
  if (goal === null) return null;
  return { objective: goal.objective, status: goal.status, ...(goal.goalId === undefined ? {} : { goalId: goal.goalId }), ...(goal.createdAt === undefined ? {} : { createdAt: goal.createdAt }) };
}
