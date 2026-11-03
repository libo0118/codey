import type { ConfigCommandResult } from "../../configCommandResult";

export type WebDavSettings = { url: string; username: string; passwordConfigured: boolean; allowHttp: boolean; revision: string };
export type WebDavDraft = WebDavSettings & { password: string; clearPassword: boolean };
export type BackupSummary = { createdAt: string; routeCount: number; channelCount: number; settingsRevision: number };
export type RestoreResult = ConfigCommandResult & { recoveryPath: string; restored?: boolean; applyWarning?: string; warning?: string; modelHotReloadError?: string; subagentConfigHotReloadError?: string; routeRequestLogHotReloadError?: string };
export type BackupPreview = BackupSummary & { content: string; source: string };
