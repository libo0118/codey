import type { SessionEntry, SessionFilter } from "./types";

export const PAGE_SIZE = 20;
export function parseRetentionDays(value: string): number | null {
  const days = Number(value);
  return /^\d+$/.test(value) && Number.isInteger(days) && days >= 1 && days <= 36_500 ? days : null;
}

export function isSessionExpired(timestamp: number, days: number, now: number) {
  return Number.isFinite(timestamp) && timestamp > 0 && timestamp < now - days * 86_400_000;
}

export function filterSessions(sessions: SessionEntry[], query: string, filter: SessionFilter, olderThanDays: number | null = null, now = 0) {
  const search = query.trim().toLocaleLowerCase();
  return sessions.filter(session => (filter === "all" || session.archived === (filter === "archived"))
    && (olderThanDays === null || isSessionExpired(session.updatedAt, olderThanDays, now))
    && (!search || [session.title, session.cwd, session.id].some(value => value.toLocaleLowerCase().includes(search))));
}

export function sessionDate(timestamp: number) {
  return timestamp > 0 ? new Date(timestamp).toLocaleString("zh-CN", { hour12: false }) : "时间未知";
}

export function sessionPath(path: string) {
  return path.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "");
}
