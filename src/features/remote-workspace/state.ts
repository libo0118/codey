import type { DraftImage } from "../../remote/images";
import type { Approval, Message, ThreadSettings, Turn, View } from "../../remote/workspace-data";
import { isAttachment } from "../../remote/workspace-data";
import { isGoal } from "../../remote/goals";
import type { ConversationEvent, Submission, ViewPatch, WorkspaceDraft } from "./types";

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const optionalString = (value: unknown) => value === undefined || typeof value === "string";
const nullableString = (value: unknown) => value === null || optionalString(value);

function draftImage(value: unknown): value is DraftImage {
  return record(value) && typeof value.name === "string" && typeof value.url === "string"
    && typeof value.size === "number" && Number.isFinite(value.size) && value.size >= 0 && (value.kind === undefined || value.kind === "file");
}

export function restoreWorkspaceDraft(previous: WorkspaceDraft | undefined, pending: Submission | null): WorkspaceDraft {
  const args = pending?.args;
  const settings: ThreadSettings = {};
  for (const key of ["model", "effort", "permissionMode"] as const) {
    if (typeof args?.[key] === "string") settings[key] = args[key];
  }
  if (args?.serviceTier === null || typeof args?.serviceTier === "string") settings.serviceTier = args.serviceTier;
  return {
    text: previous?.text ?? (typeof args?.text === "string" ? args.text : ""),
    images: previous?.images ?? [...(Array.isArray(args?.images) ? args.images.filter(draftImage) : []), ...(Array.isArray(args?.files) ? args.files.filter(draftImage).map(file => ({ ...file, kind: "file" as const })) : [])],
    uncertain: Boolean(previous?.uncertain || pending),
    settings: previous?.settings ?? settings,
  };
}

export function canSendDraft(draft: WorkspaceDraft): boolean {
  return !draft.uncertain && Boolean(draft.text.trim() || draft.images.length) && draft.images.every(image => Boolean(image.url));
}

export function firstTurnDraft(draft: WorkspaceDraft, firstTurn: string): WorkspaceDraft {
  if (firstTurn === "accepted") return { text: "", images: [], uncertain: false, settings: {} };
  return { ...draft, uncertain: firstTurn !== "not-started" && firstTurn !== "not-requested" };
}

function message(value: unknown): value is Message {
  return record(value) && nullableString(value.id) && typeof value.role === "string" && typeof value.kind === "string"
    && typeof value.text === "string" && nullableString(value.status)
    && (value.attachments === undefined || Array.isArray(value.attachments) && value.attachments.every(isAttachment))
    && (value.truncated === undefined || typeof value.truncated === "boolean")
    && (value.command === undefined || record(value.command) && typeof value.command.command === "string"
      && optionalString(value.command.cwd) && typeof value.command.output === "string"
      && (value.command.exitCode === undefined || Number.isInteger(value.command.exitCode)))
    && (value.tool === undefined || record(value.tool) && typeof value.tool.name === "string"
      && [value.tool.input, value.tool.output, value.tool.error].every(optionalString))
    && (value.collaboration === undefined || record(value.collaboration) && typeof value.collaboration.action === "string"
      && Array.isArray(value.collaboration.agents) && value.collaboration.agents.every(agent =>
        record(agent) && typeof agent.id === "string" && [agent.name, agent.status, agent.message].every(optionalString)))
    && (value.changes === undefined || Array.isArray(value.changes) && value.changes.every(change =>
      record(change) && typeof change.path === "string" && optionalString(change.kind) && typeof change.diff === "string"))
    && (value.plan === undefined || Array.isArray(value.plan) && value.plan.every(step =>
      record(step) && typeof step.step === "string" && optionalString(step.status)));
}

function turn(value: unknown): value is Turn {
  return record(value) && (value.id === null || typeof value.id === "string") && (value.status === null || typeof value.status === "string")
    && (value.startedAt == null || typeof value.startedAt === "number")
    && (value.completedAt == null || typeof value.completedAt === "number")
    && Array.isArray(value.messages) && value.messages.every(message);
}

function approval(value: unknown): value is Approval {
  if (!record(value) || (typeof value.id !== "string" && typeof value.id !== "number")
    || typeof value.method !== "string" || typeof value.supported !== "boolean" || !record(value.params)) return false;
  const params = value.params;
  return [params.command, params.cwd, params.grantRoot, params.reason, params.message, params.planContent].every(nullableString)
    && (params.networkApprovalContext === undefined || record(params.networkApprovalContext) && typeof params.networkApprovalContext.host === "string" && typeof params.networkApprovalContext.protocol === "string")
    && (params.availableDecisions == null || (Array.isArray(params.availableDecisions) && params.availableDecisions.every(item => typeof item === "string")))
    && (params.questions == null || (Array.isArray(params.questions) && params.questions.every(question =>
      record(question) && typeof question.id === "string" && typeof question.question === "string"
      && (question.options == null || (Array.isArray(question.options) && question.options.every(option =>
        record(option) && typeof option.label === "string" && nullableString(option.description)))))));
}

function conversation(value: unknown): value is View {
  return record(value) && typeof value.id === "string" && typeof value.historyComplete === "boolean"
    && (value.hasEarlierMessages === undefined || typeof value.hasEarlierMessages === "boolean")
    && [value.title, value.cwd, value.status, value.model, value.effort].every(nullableString) && optionalString(value.permissionMode)
    && (value.serviceTier === null || optionalString(value.serviceTier))
    && (value.threadGoal == null || isGoal(value.threadGoal))
    && (value.goalAvailability === undefined || typeof value.goalAvailability === "string" && ["ready", "unavailable", "unsupported"].includes(value.goalAvailability))
    && Array.isArray(value.turns) && value.turns.every(turn) && Array.isArray(value.requests) && value.requests.every(approval);
}

const invalidConversation = () => new Error("会话数据无效，请重新连接");
const own = (value: object, key: string) => Object.prototype.hasOwnProperty.call(value, key);
const revision = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value > 0;

function pointer(path: string): string[] {
  if (path === "") return [];
  if (!path.startsWith("/") || /~(?![01])/.test(path)) throw invalidConversation();
  const keys = path.slice(1).split("/").map(key => key.replace(/~1/g, "/").replace(/~0/g, "~"));
  if (keys.some(key => key === "__proto__" || key === "constructor" || key === "prototype")) throw invalidConversation();
  return keys;
}

function viewPatch(value: unknown): value is ViewPatch {
  if (!record(value) || typeof value.path !== "string" || value.op !== "add" && value.op !== "remove" && value.op !== "replace") return false;
  pointer(value.path);
  return value.op === "remove" || own(value, "value");
}

// Copy only the path being changed; memoized history keeps its existing identities.
export function applyConversationPatches(view: View, patches: ViewPatch[]): View {
  let next: unknown = view;
  for (const patch of patches) {
    if (!viewPatch(patch)) throw invalidConversation();
    const keys = pointer(patch.path);
    function update(value: unknown, depth: number): unknown {
      if (depth === keys.length) {
        if (patch.op !== "replace") throw invalidConversation();
        return patch.value;
      }
      const key = keys[depth], last = depth === keys.length - 1;
      if (Array.isArray(value)) {
        const index = key === "-" && last && patch.op === "add" ? value.length : /^(0|[1-9]\d*)$/.test(key) ? Number(key) : NaN;
        if (!Number.isSafeInteger(index) || index < 0 || index > value.length || (index === value.length && (!last || patch.op !== "add"))) throw invalidConversation();
        const copy = value.slice();
        if (!last) copy[index] = update(value[index], depth + 1);
        else if (patch.op === "add") copy.splice(index, 0, patch.value);
        else if (patch.op === "remove") copy.splice(index, 1);
        else copy[index] = patch.value;
        return copy;
      }
      if (!record(value) || (!own(value, key) && (!last || patch.op !== "add"))) throw invalidConversation();
      const copy = { ...value };
      if (!last) copy[key] = update(value[key], depth + 1);
      else if (patch.op === "remove") delete copy[key];
      else copy[key] = patch.value;
      return copy;
    }
    next = update(next, 0);
  }
  if (!conversation(next)) throw invalidConversation();
  return next;
}

export function conversationEvent(value: unknown): ConversationEvent | null {
  if (!record(value)) throw invalidConversation();
  if (value.type === "state") {
    if (!conversation(value.state)) throw invalidConversation();
    if (value.streamVersion === undefined && value.revision === undefined) return { type: "state", state: value.state };
    if (value.streamVersion !== 2 || !revision(value.revision)) throw invalidConversation();
    return { type: "state", streamVersion: 2, revision: value.revision, state: value.state };
  }
  if (value.type === "delta") {
    if (!revision(value.baseRevision) || !revision(value.revision) || value.revision !== value.baseRevision + 1
      || !Array.isArray(value.patches) || !value.patches.every(viewPatch)) throw invalidConversation();
    return { type: "delta", baseRevision: value.baseRevision, revision: value.revision, patches: value.patches };
  }
  if (value.type === "disconnected") {
    if (typeof value.message !== "string") throw invalidConversation();
    return { type: "disconnected", message: value.message };
  }
  return null;
}
