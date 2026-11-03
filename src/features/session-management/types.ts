export type SessionEntry = {
  id: string;
  title: string;
  cwd: string;
  updatedAt: number;
  archived: boolean;
  bytes: number;
};
export type CleanupPolicy = { enabled: boolean; retentionDays: number };
export type CleanupPolicyDraft = { enabled: boolean; retentionDays: string };
export type SessionInventory = {
  sessions: SessionEntry[]; revision: string; pendingCount: number;
  legacyPendingIds?: string[];
  cleanupPolicy: CleanupPolicy; cleanupPolicyRevision: string;
};
export type CleanupResult = { selectedCount: number; scheduledCount?: number; clearedLegacyCount?: number; restartRequired: boolean; errors: string[] };
export type CleanupPolicySaveResult = { policy: CleanupPolicy; revision: string; cleanup: CleanupResult | null; cleanupError: string | null };
export type SessionFilter = "all" | "active" | "archived";
export type CleanupConfirmation = { ids: string[]; revision: string; archivedCount: number; all: boolean; legacyMissingCount?: number };
