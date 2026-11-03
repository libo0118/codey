import { invoke } from "../../api";
import type { CleanupPolicy, CleanupPolicySaveResult, CleanupResult, SessionInventory } from "./types";

let pending: Promise<unknown> | null = null;
function bounded<T>(operation: Promise<T>, mutation = false): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(mutation
      ? "会话操作等待超时，后台可能仍在执行。结果尚不确定，请刷新确认后再操作。"
      : "读取会话超时，请稍后刷新。")), mutation ? 120_000 : 30_000);
    operation.then(resolve, reject).finally(() => clearTimeout(timer));
  });
}

function mutate<T>(command: "delete_codex_sessions" | "save_codex_session_cleanup_policy", request: unknown) {
  if (pending) return Promise.reject(new Error("上次会话操作仍在执行，请刷新确认后再操作。"));
  const operation = invoke<T>(command, { request });
  pending = operation;
  const release = () => { if (pending === operation) pending = null; };
  void operation.then(release, release);
  return bounded(operation, true);
}

export const sessionRequests = {
  waitForPending: () => pending ? bounded(pending.catch(() => undefined)) : Promise.resolve(),
  list: () => bounded(invoke<SessionInventory>("list_codex_sessions")),
  remove(ids: string[], revision: string) {
    return mutate<CleanupResult>("delete_codex_sessions", { ids, revision, confirmed: true });
  },
  savePolicy(policy: CleanupPolicy, revision: string) {
    return mutate<CleanupPolicySaveResult>("save_codex_session_cleanup_policy", { policy, revision, confirmed: true });
  },
};
