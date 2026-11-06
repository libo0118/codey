import type { BuiltinSubagentRoleId, Config, SubagentRoleConfig } from "../../App.types";
import type { SubagentModelOption } from "../../subagentModels";

export const BUILTIN_SUBAGENT_ROLES = [
  { id: "codey_explorer", name: "检索", kind: "analysis", description: "只读检索代码、日志和文档，从快速定位到跨目录归纳，深浅由任务决定。" },
  { id: "codey_reviewer", name: "复核", kind: "analysis", description: "独立核验已完成的改动，包括差异、针对性测试和界面渲染效果；可按任务运行构建、测试和渲染检查。" },
  { id: "codey_worker", name: "开发", kind: "implementation", description: "负责前端、后端、数据和脚本等各类实现，改动界面时附渲染核验。" },
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
