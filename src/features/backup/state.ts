import type { WebDavDraft, WebDavSettings } from "./types";

export const MAX_BACKUP_BYTES = 4 * 1024 * 1024;
export const EMPTY_WEBDAV: WebDavSettings = { url: "", username: "", passwordConfigured: false, allowHttp: false, revision: "" };
export function webdavDraft(settings: WebDavSettings): WebDavDraft {
  return { ...settings, password: "", clearPassword: false };
}

export function validateWebdav(draft: WebDavDraft): string {
  let url: URL;
  try { url = new URL(draft.url.trim()); } catch { return "请输入完整的 WebDAV 文件地址"; }
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || url.search || url.hash || !url.pathname.endsWith(".json"))
    return "请填写以 .json 结尾的 HTTP(S) 文件地址，用户名和密码请单独填写";
  if (url.protocol === "http:" && !draft.allowHttp) return "使用 HTTP 前请明确允许未加密连接";
  if (draft.username.includes(":")) return "WebDAV 用户名不能包含冒号";
  return "";
}

export function backupFilename(date: Date): string {
  return `Codey-backup-${date.toISOString().replace(/[:.]/g, "-")}.json`;
}
