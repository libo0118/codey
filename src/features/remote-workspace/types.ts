import type { DraftImage } from "../../remote/images";
import type { ThreadSettings, View } from "../../remote/workspace-data";

export type WorkspaceDraft = {
  text: string;
  images: DraftImage[];
  uncertain: boolean;
  settings: ThreadSettings;
};

export type Submission = { id: string; args: Record<string, unknown> };
export type CreationResult = { id: string; firstTurn: string };
export type ConversationEvent = { type: "state"; state: View } | { type: "disconnected"; message: string };
