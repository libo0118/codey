import { invoke } from "../../api";
import type { BuiltinToolsStatus } from "../../builtinTools";
import type { PonytailMode, PonytailStatus } from "./ponytail";

export const builtinToolsRequests = {
  status: () => invoke<BuiltinToolsStatus>("builtin_tools", { action: "status" }),
  setEnabled: (enabled: boolean) => invoke<BuiltinToolsStatus>("builtin_tools", { action: "set_enabled", enabled }),
};

export const ponytailRequests = {
  status: () => invoke<PonytailStatus>("builtin_tools", { tool: "ponytail", action: "status" }),
  setEnabled: (enabled: boolean) => invoke<PonytailStatus>("builtin_tools", { tool: "ponytail", action: "set_enabled", enabled }),
  save: (defaultMode: PonytailMode, revision: string) => invoke<PonytailStatus>("builtin_tools", { tool: "ponytail", action: "save_settings", defaultMode, revision }),
};
