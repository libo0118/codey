import { RemoteError, remoteRequest, requestId, readSubmission, rememberSubmission, forgetSubmission } from "../../remote/transport";
import type { ModelCatalog, Project, Thread, ThreadSettings } from "../../remote/workspace-data";
import { conversationEvent } from "./state";
import type { ConversationEvent, CreationResult, Submission } from "./types";

// 手机端复用带 HttpOnly Cookie 的传输；这些端点不属于桌面 invoke 命令。
export const remoteWorkspaceRequests = {
  threads: (search: string, archived: boolean) => remoteRequest<Thread[]>("/remote/threads", { search, archived }),
  projects: () => remoteRequest<Project[]>("/remote/projects", {}),
  models: () => remoteRequest<ModelCatalog>("/remote/models", {}),
  defaults: (projectId: string) => remoteRequest<ThreadSettings>("/remote/defaults", { projectId }),
  session: async () => { const result = await remoteRequest<{ authenticated: boolean }>("/remote/session"); if (!result.authenticated) throw new RemoteError("登录已过期或被撤销", 401); return result; },
  action: (threadId: string, action: string, args: Record<string, unknown> = {}) =>
    remoteRequest("/remote/action", { ...args, threadId, action, requestId: requestId() }),
  submit: (threadId: string, submission: Submission) =>
    remoteRequest("/remote/action", { ...submission.args, threadId, requestId: submission.id }),
  create: (submission: Submission) => remoteRequest<CreationResult>("/remote/create", { ...submission.args, requestId: submission.id }),
  readSubmission,
  rememberSubmission,
  forgetSubmission,
  unauthorized: (error: unknown) => error instanceof RemoteError && error.status === 401,
  connect(threadId: string, handlers: { message: (event: ConversationEvent) => void; invalid: () => void; close: () => void }) {
    const socket = new WebSocket(`${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/remote/events/${encodeURIComponent(threadId)}`);
    socket.onmessage = event => {
      let message: ConversationEvent | null;
      try { message = conversationEvent(JSON.parse(event.data)); }
      catch { handlers.invalid(); return; }
      if (message) handlers.message(message);
    };
    socket.onclose = handlers.close;
    return { close: () => { socket.onmessage = null; socket.onclose = null; socket.close(); } };
  },
};
