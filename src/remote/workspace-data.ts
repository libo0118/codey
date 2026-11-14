import type { Goal, GoalAvailability } from "./goals";
export type Thread = { id: string; title: string; cwd: string; updatedAt: number; running?: boolean | null };
export type Project = { id: string; name: string; cwd: string; rootPaths?: string[] };
export type Question = { id: string; question: string; options?: { label: string; description?: string | null }[] | null };
export type RequestedPermissions = { network?: { enabled: boolean }; fileSystem?: { read?: string[]; write?: string[] } };
export type Approval = { id: string | number; method: string; supported: boolean; params: { command?: string | null; cwd?: string | null; grantRoot?: string | null; networkApprovalContext?: { host: string; protocol: string }; reason?: string | null; questions?: Question[] | null; availableDecisions?: string[] | null; message?: string | null; permissions?: unknown; changes?: unknown; planContent?: string | null } };
export type Attachment = { name: string; url?: string | null; id?: string; downloadUrl?: string; size?: number; mime?: string; unavailableReason?: string };
export type FileChange = { path: string; kind?: string; diff: string };
export type Message = {
  id?: string | null; role: string; kind: string; text: string; status?: string | null; attachments?: Attachment[];
  command?: { command: string; cwd?: string; output: string; exitCode?: number };
  tool?: { name: string; input?: string; output?: string; error?: string };
  collaboration?: { action: string; agents: { id: string; name?: string; status?: string; message?: string }[] };
  changes?: FileChange[]; plan?: { step: string; status?: string }[]; truncated?: boolean;
};
export type Turn = { id: string | null; status: string | null; error?: unknown; startedAt?: number | null; completedAt?: number | null; messages: Message[] };
export type ThreadSettings = { model?: string | null; effort?: string | null; serviceTier?: string | null; permissionMode?: string };
export type View = ThreadSettings & { id: string; title?: string | null; cwd?: string | null; status?: string | null; turns: Turn[]; requests: Approval[]; historyComplete: boolean; hasEarlierMessages?: boolean; historyRevision?: number; threadGoal?: Goal | null; goalAvailability?: GoalAvailability };
export type ModelOption = { id: string; label: string; route: string; efforts: string[]; defaultEffort: string };
export type ModelCatalog = { models?: string[]; default_model?: string; clear_models?: boolean; model_metadata?: { model: string; display_name?: string; model_display_name?: string; route_name?: string; supported_reasoning_efforts?: string[]; default_reasoning_effort?: string }[] };

export const effortLabels: Record<string, string> = { none: "无", minimal: "最低", low: "低", medium: "中", high: "高", xhigh: "超高", max: "Max", ultra: "Ultra" };
export const permissionLabels: Record<string, string> = { "read-only": "只读", auto: "默认权限", "full-access": "完全访问", custom: "自定义权限" };

function normalizedPath(path: string): string {
  const normalized = path.replace(/\\/g, "/").replace(/^\/\/\?\/UNC\//i, "//").replace(/^\/\/\?\//, "").replace(/\/+$/, "");
  return /^[a-z]:/i.test(normalized) || normalized.startsWith("//") ? normalized.toLowerCase() : normalized;
}

export function projectForThread(thread: Thread, projects: readonly Project[]): Project | undefined {
  const cwd = normalizedPath(thread.cwd);
  let best: Project | undefined;
  let length = -1;
  for (const project of projects) {
    for (const path of project.rootPaths?.length ? project.rootPaths : [project.cwd]) {
      const root = normalizedPath(path);
      if (path && (cwd === root || cwd.startsWith(`${root}/`)) && root.length > length) {
        best = project; length = root.length;
      }
    }
  }
  return best;
}

export function groupThreads(threads: readonly Thread[], projects: readonly Project[]) {
  const groups = projects.map(project => ({ project, threads: [] as Thread[] }));
  const byId = new Map(groups.map(group => [group.project.id, group]));
  const recent: Thread[] = [];
  for (const thread of [...threads].sort((a, b) => b.updatedAt - a.updatedAt)) {
    const project = projectForThread(thread, projects);
    const group = project && byId.get(project.id);
    (group ? group.threads : recent).push(thread);
  }
  return { groups, recent };
}

export function modelOptions(catalog: ModelCatalog): ModelOption[] {
  if (catalog.clear_models) return [];
  const metadata = new Map(catalog.model_metadata?.map(entry => [entry.model, entry]));
  return [...new Set(catalog.models || [])].map(id => {
    const entry = metadata.get(id);
    const efforts = [...new Set(entry?.supported_reasoning_efforts || [])].filter(value => value in effortLabels);
    return { id, label: entry?.model_display_name || entry?.display_name || id, route: entry?.route_name || "模型", efforts, defaultEffort: efforts.includes(entry?.default_reasoning_effort || "") ? entry!.default_reasoning_effort! : efforts[0] || "" };
  });
}

export function defaultThreadSettings(catalog: ModelCatalog, saved: ThreadSettings): ThreadSettings {
  const models = modelOptions(catalog);
  const model = models.find(model => model.id === saved.model)
    || models.find(model => model.id === catalog.default_model) || models[0];
  if (!model) return saved;
  return {
    ...saved, model: model.id,
    effort: model.efforts.includes(saved.effort || "") ? saved.effort : model.defaultEffort || undefined,
  };
}

export function relativeTime(timestamp: number, now = Date.now()): string {
  if (!Number.isFinite(timestamp) || timestamp <= 0) return "";
  const minutes = Math.max(0, Math.floor((now - timestamp) / 60000));
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)} 小时`;
  if (minutes < 10080) return `${Math.floor(minutes / 1440)} 天`;
  return new Date(timestamp).toLocaleDateString("zh-CN", { month: "numeric", day: "numeric" });
}

// Local references are resolved separately; never navigate to a model's path.
export function messageUrl(url: string): string {
  if (/[\u0000-\u0020\u007f]/.test(url) || !/^(https?:\/\/|mailto:)/i.test(url)) return "";
  try { const parsed = new URL(url); return parsed.username || parsed.password ? "" : url; } catch { return ""; }
}

export function imageUrl(url: string): string {
  return /^https:\/\//i.test(url) || /^data:image\/(png|jpeg|gif|webp);base64,[a-z0-9+/=]+$/i.test(url) ? url : "";
}

export function isAttachment(value: unknown): value is Attachment {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const file = value as Record<string, unknown>;
  return typeof file.name === "string" && (file.url == null || typeof file.url === "string")
    && [file.id, file.downloadUrl, file.mime, file.unavailableReason].every(part => part === undefined || typeof part === "string")
    && (file.size === undefined || typeof file.size === "number" && Number.isSafeInteger(file.size) && file.size >= 0);
}

export function isRequestedPermissions(value: unknown): value is RequestedPermissions {
  const record = (part: unknown): part is Record<string, unknown> => !!part && typeof part === "object" && !Array.isArray(part);
  if (!record(value) || !Object.keys(value).length || Object.keys(value).some(key => key !== "network" && key !== "fileSystem")) return false;
  if (value.network !== undefined && (!record(value.network) || typeof value.network.enabled !== "boolean" || Object.keys(value.network).some(key => key !== "enabled"))) return false;
  if (value.fileSystem !== undefined && (!record(value.fileSystem) || !Object.keys(value.fileSystem).length || Object.entries(value.fileSystem).some(([key, paths]) => key !== "read" && key !== "write" || !Array.isArray(paths) || paths.some(path => typeof path !== "string")))) return false;
  return value.network !== undefined || value.fileSystem !== undefined;
}
