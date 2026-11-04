import type { BuiltinSubagentRoleId, Config, SubagentRoleConfig } from "../../App.types";
import type { SubagentModelOption } from "../../subagentModels";

export const BUILTIN_SUBAGENT_ROLES = [
  { id: "codey_quick_scan", name: "快速定位", kind: "analysis", description: "用于精确位置、重复性检查、低风险事实查找和小范围快速检索。" },
  { id: "codey_deep_research", name: "深度检索", kind: "analysis", description: "用于跨文件、日志、代码和文档的宽范围检索、归纳与架构探索。" },
  { id: "codey_visual_analysis", name: "界面核验", kind: "analysis", description: "用于截图、页面、GUI、PDF 的视觉证据观察，以及前端改动的独立核验。" },
  { id: "codey_worker", name: "逻辑开发", kind: "implementation", description: "用于后端、数据、算法、命令行和脚本等不改变可见界面的逻辑实现。" },
  { id: "codey_visual_worker", name: "前端开发", kind: "implementation", description: "用于界面、布局、样式与交互实现，客户端可截图时附渲染证据。" },
] as const satisfies ReadonlyArray<{ id: BuiltinSubagentRoleId; name: string; kind: "analysis" | "implementation"; description: string }>;

export const CUSTOM_SUBAGENT_FIELDS = [
  { key: "name", label: "名称", limit: 80 },
  { key: "description", label: "用途说明", limit: 1000 },
  { key: "developerInstructions", label: "角色指令", limit: 4000 },
] as const;

export function isCustomSubagent(id: string) {
  return id !== "default" && !BUILTIN_SUBAGENT_ROLES.some(role => role.id === id);
}

export function subagentSelection(config: Config, id: string): SubagentRoleConfig {
  return config.subagentRoles[id] ?? {
    enabled: true,
    model: config.subagentModel,
    reasoningEffort: config.subagentReasoningEffort,
  };
}

export function enabledSubagentCount(config: Config) {
  return BUILTIN_SUBAGENT_ROLES.filter(({ id }) => subagentSelection(config, id).enabled).length
    + Object.entries(config.subagentRoles).filter(([id, role]) => isCustomSubagent(id) && role.enabled).length;
}

export function addCustomSubagent(config: Config, model?: SubagentModelOption): Config {
  if (Object.keys(config.subagentRoles).filter(isCustomSubagent).length >= 32) return config;
  let index = 1;
  while (Object.prototype.hasOwnProperty.call(config.subagentRoles, `custom_agent_${index}`)
    || config.deletedSubagentRoles?.includes(`custom_agent_${index}`)) index++;
  const id = `custom_agent_${index}`;
  return {
    ...config,
    subagentRoles: {
      ...config.subagentRoles,
      [id]: {
        enabled: true,
        name: `自定义子代理 ${index}`,
        description: "用于主代理明确指定的专项调查与核验。",
        developerInstructions: "根据主代理交付的目标进行调查，返回结论、证据与未完成项。",
        access: "readOnly",
        model: model?.value ?? config.subagentModel,
        reasoningEffort: model?.defaultReasoningEffort ?? config.subagentReasoningEffort,
      },
    },
  };
}

export function updateSubagent(config: Config, id: string, patch: Partial<SubagentRoleConfig>): Config {
  if (isCustomSubagent(id) && !Object.prototype.hasOwnProperty.call(config.subagentRoles, id)) return config;
  const previous = subagentSelection(config, id);
  if (patch.enabled === false && (id === "default" || (previous.enabled && enabledSubagentCount(config) <= 1))) return config;
  const selection = { ...previous, ...patch };
  return {
    ...config,
    ...(id === "default" ? { subagentModel: selection.model, subagentReasoningEffort: selection.reasoningEffort } : {}),
    subagentRoles: { ...config.subagentRoles, [id]: selection },
  };
}

export function removeCustomSubagent(config: Config, id: string): Config {
  if (!isCustomSubagent(id) || !Object.prototype.hasOwnProperty.call(config.subagentRoles, id)) return config;
  if (config.subagentRoles[id].enabled && enabledSubagentCount(config) <= 1) return config;
  const roles = { ...config.subagentRoles };
  delete roles[id];
  return { ...config, subagentRoles: roles, deletedSubagentRoles: [...new Set([...(config.deletedSubagentRoles ?? []), id])] };
}

export function selectSubagentModel(config: Config, id: string, option: SubagentModelOption): Config {
  const selection = subagentSelection(config, id);
  return updateSubagent(config, id, {
    model: option.value,
    reasoningEffort: option.supportedReasoningEfforts.includes(selection.reasoningEffort)
      ? selection.reasoningEffort : option.defaultReasoningEffort,
  });
}

export function validateSubagents(config: Config): string | null {
  const limit = config.subagentMaxThreads ?? 2;
  if (!Number.isInteger(limit) || limit < 1 || limit > 64) return "最大子代理数量必须是 1 到 64 之间的整数";
  if (config.subagentOptimization && enabledSubagentCount(config) === 0) return "至少需要保留一个启用的子代理角色";
  const custom = Object.entries(config.subagentRoles).filter(([id]) => isCustomSubagent(id));
  if (custom.length > 32) return "最多可配置 32 个自定义子代理";
  for (const [id, role] of custom) {
    for (const { key, label, limit } of CUSTOM_SUBAGENT_FIELDS) {
      const value = role[key];
      if (!value?.trim() || new TextEncoder().encode(value).length > limit) {
        return `${role.name || id}的${label}不能为空且不能超过 ${limit} 字节`;
      }
    }
    if (role.access !== undefined && role.access !== "readOnly" && role.access !== "readWrite") return `${role.name || id}的读写能力无效`;
  }
  return null;
}
