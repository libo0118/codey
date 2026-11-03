import { invoke } from "../../api";
export { isCodeyApiError } from "../../api";
import type { BackupSummary, RestoreResult, WebDavDraft, WebDavSettings } from "./types";
import { MAX_BACKUP_BYTES } from "./state";

export const backupRequests = {
  settings: () => invoke<WebDavSettings>("config_backup", { action: "webdav_get" }),
  saveSettings: (settings: WebDavDraft) => invoke<WebDavSettings>("config_backup", { action: "webdav_save", settings }),
  export: () => invoke<{ content: string }>("config_backup", { action: "export", confirmed: true }),
  inspect: (content: string) => invoke<BackupSummary>("config_backup", { action: "inspect", content }),
  restore: (content: string, settingsRevision: number) => invoke<RestoreResult>("config_backup", { action: "restore", content, settingsRevision, confirmed: true }),
  upload: () => invoke<{ status: "ok" }>("config_backup", { action: "webdav_upload", confirmed: true }),
  download: () => invoke<{ content: string }>("config_backup", { action: "webdav_download" }),
  async readFile(file: File) {
    if (!file.size || file.size > MAX_BACKUP_BYTES) throw new Error("请选择非空 JSON 文件，且不超过 4 MiB");
    return file.text();
  },
};
