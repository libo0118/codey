import type { DraftImage } from "../../remote/images";
import type { Approval, Message, ThreadSettings, Turn, View } from "../../remote/workspace-data";
import type { ConversationEvent, Submission, WorkspaceDraft } from "./types";

const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const optionalString = (value: unknown) => value === undefined || typeof value === "string";
const nullableString = (value: unknown) => value === null || optionalString(value);

function draftImage(value: unknown): value is DraftImage {
  return record(value) && typeof value.name === "string" && typeof value.url === "string"
    && typeof value.size === "number" && Number.isFinite(value.size) && value.size >= 0;
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
    images: previous?.images ?? (Array.isArray(args?.images) ? args.images.filter(draftImage) : []),
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
    && (value.attachments === undefined || (Array.isArray(value.attachments) && value.attachments.every(attachment =>
      record(attachment) && typeof attachment.name === "string" && nullableString(attachment.url))));
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
  return [params.command, params.reason, params.message, params.planContent].every(nullableString)
    && (params.availableDecisions == null || (Array.isArray(params.availableDecisions) && params.availableDecisions.every(item => typeof item === "string")))
    && (params.questions == null || (Array.isArray(params.questions) && params.questions.every(question =>
      record(question) && typeof question.id === "string" && typeof question.question === "string"
      && (question.options == null || (Array.isArray(question.options) && question.options.every(option =>
        record(option) && typeof option.label === "string" && nullableString(option.description)))))));
}

function conversation(value: unknown): value is View {
  return record(value) && typeof value.id === "string" && typeof value.historyComplete === "boolean"
    && [value.title, value.cwd, value.status, value.model, value.effort].every(nullableString) && optionalString(value.permissionMode)
    && (value.serviceTier === null || optionalString(value.serviceTier))
    && Array.isArray(value.turns) && value.turns.every(turn) && Array.isArray(value.requests) && value.requests.every(approval);
}

export function conversationEvent(value: unknown): ConversationEvent | null {
  if (!record(value)) throw new Error("会话数据无效，请重新连接");
  if (value.type === "state") {
    if (!conversation(value.state)) throw new Error("会话数据无效，请重新连接");
    return { type: "state", state: value.state };
  }
  if (value.type === "disconnected") {
    if (typeof value.message !== "string") throw new Error("会话数据无效，请重新连接");
    return { type: "disconnected", message: value.message };
  }
  return null;
}
