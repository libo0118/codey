export const PONYTAIL_MODES = [
  { value: "lite", label: "Lite · 轻量" },
  { value: "full", label: "Full · 标准（官方默认）" },
  { value: "ultra", label: "Ultra · 强化" },
  { value: "off", label: "Off · 新聊天不自动激活" },
] as const;

export type PonytailMode = typeof PONYTAIL_MODES[number]["value"];
export function isPonytailMode(value: unknown): value is PonytailMode {
  return PONYTAIL_MODES.some(mode => mode.value === value);
}

export type PonytailStatus = {
  installed: boolean;
  enabled: boolean;
  version: string | null;
  conflict: string | null;
  message?: string;
  settings: {
    defaultMode: PonytailMode;
    effectiveMode: PonytailMode;
    environmentOverride: PonytailMode | null;
    revision: string;
    configPath: string;
  };
  job: { id: string; phase: "running" | "succeeded" | "failed"; message: string } | null;
};
