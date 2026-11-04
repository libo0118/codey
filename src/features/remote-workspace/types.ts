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
export type ViewPatch = { op: "add" | "replace"; path: string; value: unknown } | { op: "remove"; path: string };
export type ConversationEvent = { type: "state"; state: View; streamVersion?: 2; revision?: number }
  | { type: "delta"; baseRevision: number; revision: number; patches: ViewPatch[] }
  | { type: "disconnected"; message: string };
