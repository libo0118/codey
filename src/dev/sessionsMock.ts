import type { CleanupPolicy, SessionEntry } from "../features/session-management/types";

export function createSessionsPreview() {
  let revision = 1;
  let pendingCount = 0;
  let cleanupPolicy: CleanupPolicy = { enabled: false, retentionDays: 30 };
  let cleanupPolicyRevision = 1;
  let sessions: SessionEntry[] = Array.from({ length: 25 }, (_, index) => ({
    id: `11111111-1111-4111-8111-${String(index + 1).padStart(12, "0")}`,
    title: index === 0 ? "管理面板功能开发" : index === 1 ? "已归档的界面方案" : `项目会话 ${index + 1}`,
    cwd: index % 2 ? "\\\\?\\UNC\\Server\\Share\\example" : "\\\\?\\E:\\code\\Codey",
    archived: index % 2 === 1,
    updatedAt: Date.now() - (index < 2 ? 7 : 45) * 86_400_000,
    bytes: 1024 * (index + 1),
  }));
  return (command: string, args?: Record<string, unknown>) => {
    if (command === "list_codex_sessions") return { sessions: [...sessions], revision: String(revision), pendingCount,
      cleanupPolicy: { ...cleanupPolicy }, cleanupPolicyRevision: String(cleanupPolicyRevision) };
    if (command === "save_codex_session_cleanup_policy") {
      const request = args?.request as { policy: CleanupPolicy; revision: string; confirmed: boolean };
      if (request?.revision !== String(cleanupPolicyRevision)) throw new Error("自动清理设置已变化，请刷新后重试");
      const days = request.policy?.retentionDays;
      if (!Number.isInteger(days) || days < 1 || days > 36_500) throw new Error("保留天数须为 1 至 36500 的整数");
      if (request.policy.enabled && !request.confirmed) throw new Error("请先确认启用自动清理");
      cleanupPolicy = { ...request.policy }; cleanupPolicyRevision++;
      const cutoff = Date.now() - cleanupPolicy.retentionDays * 86_400_000;
      const selected = cleanupPolicy.enabled ? sessions.filter(session => session.updatedAt > 0 && session.updatedAt < cutoff) : [];
      const ids = new Set(selected.map(session => session.id));
      sessions = sessions.filter(session => !ids.has(session.id));
      pendingCount += selected.length;
      if (selected.length) revision++;
      return { policy: { ...cleanupPolicy }, revision: String(cleanupPolicyRevision), cleanup: cleanupPolicy.enabled
        ? { selectedCount: selected.length, restartRequired: selected.length > 0, errors: [] } : null, cleanupError: null };
    }
    const request = args?.request as { ids: string[]; revision: string; confirmed: boolean };
    if (!request?.confirmed || request.revision !== String(revision)) throw new Error("会话列表已变化，请刷新后重试");
    const selected = new Set(request.ids);
    const selectedCount = sessions.filter(session => selected.has(session.id)).length;
    sessions = sessions.filter(session => !selected.has(session.id));
    pendingCount += selectedCount;
    revision++;
    return { selectedCount, restartRequired: true, errors: [] };
  };
}
