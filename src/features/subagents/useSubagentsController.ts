import type { Config, SubagentRoleConfig } from "../../App.types";
import { routeProviderId } from "../../modelRoutes";
import { resolveSubagentModelOption, type SubagentModelOption } from "../../subagentModels";
import { useStableEvent } from "../../useStableEvent";
import { addCustomSubagent, removeCustomSubagent, selectSubagentModel, updateSubagent, validateSubagents } from "./state";

type SubagentsControllerProps = {
  config: Config;
  isBusy: boolean;
  subagentModelOptions: SubagentModelOption[];
  onConfigChange: (config: Config) => void;
};

export function useSubagentsController({ config, isBusy, subagentModelOptions, onConfigChange }: SubagentsControllerProps) {
  const preferredProfile = config.profiles.find(profile => profile.id === config.activeProfileId) ?? config.profiles[0];
  const preferredProviderId = preferredProfile ? routeProviderId(preferredProfile) : undefined;
  const commit = useStableEvent((next: Config) => {
    if (!isBusy && next !== config) onConfigChange(next);
  });
  const addRole = useStableEvent(() => commit(addCustomSubagent(config,
    resolveSubagentModelOption(subagentModelOptions, config.subagentModel, preferredProviderId) ?? subagentModelOptions[0])));
  const updateRole = useStableEvent((id: string, patch: Partial<SubagentRoleConfig>) => commit(updateSubagent(config, id, patch)));
  const removeRole = useStableEvent((id: string) => commit(removeCustomSubagent(config, id)));
  const changeModel = useStableEvent((id: string, value: string) => {
    const option = subagentModelOptions.find(candidate => candidate.value === value);
    if (option) commit(selectSubagentModel(config, id, option));
  });
  return { preferredProviderId, addRole, updateRole, removeRole, changeModel, error: validateSubagents(config) };
}
