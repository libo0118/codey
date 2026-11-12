import { memo, type CSSProperties } from "react";
import {
  IconAdjustments,
  IconHelpCircle,
  IconInfoCircle,
  IconPhotoSearch,
  IconPlus,
  IconTerminal2,
  IconTrash,
  IconUsersGroup,
  IconWorldSearch,
} from "@tabler/icons-react";

import type {
  Config,
  Confirmation,
} from "./App.types";
import {
  Button,
  Badge,
  Input,
  NumberInput,
  Select,
  Switch,
  TextArea,
  Tooltip,
} from "./components/ui";
import { ModelCombobox } from "./components/ModelCombobox";
import {
  resolveSubagentModelOption,
  type SubagentModelOption,
} from "./subagentModels";
import { SettingsPageHeader } from "./SettingsPageHeader";
import type { DiagnosticStorageTarget } from "./diagnosticStorage";
import { NotificationChannelsCard } from "./notifications/NotificationChannelsCard";
import type { NotificationChannel } from "./notifications/types";
import { BUILTIN_SUBAGENT_ROLES, CUSTOM_SUBAGENT_FIELDS, enabledSubagentCount, isCustomSubagent, subagentSelection } from "./features/subagents/state";
import { useSubagentsController } from "./features/subagents/useSubagentsController";

const GPU_LAUNCH_MODES = [
  { value: "off", label: "关闭" },
  { value: "disableGpu", label: "禁用 GPU" },
  { value: "disableGpuRasterization", label: "禁用 GPU 栅格化" },
] as const satisfies ReadonlyArray<{
  value: Config["gpuLaunchMode"];
  label: string;
}>;
const REASONING_EFFORT_LABELS: Record<string, string> = {
  none: "无",
  minimal: "最低",
  low: "低",
  medium: "中",
  high: "高",
  xhigh: "极高",
  max: "最大",
  ultra: "超高",
};
const DEFAULT_SUBAGENT_TASK = {
  id: "default",
  name: "默认子代理",
  icon: IconUsersGroup,
  description: "未指定专用角色时使用的模型与思考深度。",
} as const;
const ROLE_ICONS = [IconWorldSearch, IconPhotoSearch, IconTerminal2];
const SUBAGENT_TASK_TYPES = BUILTIN_SUBAGENT_ROLES.map((task, index) => ({ ...task, icon: ROLE_ICONS[index] }));

export type SubagentPolicyCardProps = {
  active?: boolean;
  config: Config;
  isBusy: boolean;
  subagentModelOptions: SubagentModelOption[];
  onConfigChange: (config: Config) => void;
  onSubagentOptimizationChange: (checked: boolean) => void;
  onRequestConfirmation: (confirmation: Confirmation) => void;
};

export function SubagentPolicyCardComponent({
  config,
  isBusy,
  subagentModelOptions,
  onConfigChange,
  onSubagentOptimizationChange,
  onRequestConfirmation,
}: SubagentPolicyCardProps) {
  const controller = useSubagentsController({ config, isBusy, subagentModelOptions, onConfigChange });
  const subagentPolicyControlsDisabled = isBusy;
  const preferredProviderId = controller.preferredProviderId;
  const enabledRoleCount = enabledSubagentCount(config);
  const customRoles = Object.keys(config.subagentRoles).filter(isCustomSubagent);
  const analysisTasks = SUBAGENT_TASK_TYPES.filter(
    (task) => task.kind === "analysis",
  );
  const implementationTasks = SUBAGENT_TASK_TYPES.filter(
    (task) => task.kind === "implementation",
  );
  const enabledAnalysisCount = analysisTasks.filter(
    ({ id }) => config.subagentRoles[id]?.enabled !== false,
  ).length;
  const enabledImplementationCount = implementationTasks.filter(
    ({ id }) => config.subagentRoles[id]?.enabled !== false,
  ).length;

  const renderRoleCard = (
    task: { id: string; name: string; icon: typeof IconUsersGroup; description: string },
  ) => {
    const isDefaultRole = task.id === "default";
    const custom = isCustomSubagent(task.id);
    const TaskIcon = task.icon;
    const selection = subagentSelection(config, task.id);
    const selectedModel = resolveSubagentModelOption(
      subagentModelOptions,
      selection.model,
      preferredProviderId,
    );
    const reasoningEfforts = selectedModel?.supportedReasoningEfforts ?? [];
    const reasoningOptions = reasoningEfforts.map((effort) => ({
      label: REASONING_EFFORT_LABELS[effort] ?? effort,
      value: effort,
    }));
    const updateRole = (next: Partial<typeof selection>) => controller.updateRole(task.id, next);
    const roleDisabled = !isDefaultRole && !selection.enabled;
    const isSingleEnabledRole = selection.enabled && enabledRoleCount <= 1;

    return (
      <div
        key={task.id}
        id={task.id}
        className={`subagent-role-item ${custom ? "subagent-role-item--custom" : ""} ${
          roleDisabled ? "subagent-role-disabled" : ""
        }`}
      >
        {!isDefaultRole && (
          <div className="subagent-role-switch-wrap">
            <span
              className="inline-flex"
              title={
                isSingleEnabledRole ? "至少需要保留一个启用的调度角色" : undefined
              }
            >
              <Switch
                size="sm"
                checked={selection.enabled}
                disabled={
                  subagentPolicyControlsDisabled || isSingleEnabledRole
                }
                onCheckedChange={(enabled) => updateRole({ enabled })}
                aria-label={`${selection.enabled ? "关闭" : "启用"}${task.name}角色`}
              />
            </span>
          </div>
        )}

        <div className="subagent-role-identity">
          <div
            className={`subagent-role-icon-box subagent-role-icon-box--${custom ? "default" : task.id}`}
          >
            <TaskIcon size={16} stroke={1.9} aria-hidden="true" />
          </div>
          <div className="subagent-role-meta">
            <div className="subagent-role-title-row">
              <h4 className="subagent-role-name">{task.name}</h4>
              <Tooltip content={task.description} position="top" asChild>
                <button
                  type="button"
                  className="subagent-role-help"
                  aria-label={`${task.name}说明`}
                >
                  <IconHelpCircle size={14} aria-hidden="true" />
                </button>
              </Tooltip>
              {custom && (
                <Button
                  size="icon-sm"
                  variant="ghost"
                  disabled={isBusy || isSingleEnabledRole}
                  title={isSingleEnabledRole ? "至少需要保留一个启用的调度角色" : `删除${task.name}`}
                  aria-label={`删除${task.name}`}
                  onClick={() => onRequestConfirmation({
                    action: "delete-subagent",
                    title: `删除子代理「${task.name}」？`,
                    description: "将从当前配置移除这个自定义角色。点击侧栏“保存更改”后生效；保存前可放弃全部更改以恢复。",
                    confirmLabel: "删除子代理",
                    run: () => controller.removeRole(task.id),
                  })}
                >
                  <IconTrash size={15} aria-hidden="true" />
                </Button>
              )}
              {roleDisabled && (
                <span className="subagent-role-status-chip">已停用</span>
              )}
            </div>
          </div>
        </div>

        <div className="subagent-control-group subagent-control-group--model">
          <span className="subagent-control-label" aria-hidden="true">指定模型</span>
          <div className="subagent-control-field">
            <ModelCombobox
              aria-label={`${task.name}模型`}
              value={selection.model}
              placeholder={
                subagentModelOptions.length === 0
                  ? "所有线路均暂无模型"
                  : "请选择模型"
              }
              disabled={
                subagentPolicyControlsDisabled ||
                roleDisabled ||
                subagentModelOptions.length === 0
              }
              options={subagentModelOptions}
              preferredProviderId={preferredProviderId}
              onChange={(value) => controller.changeModel(task.id, value)}
            />
          </div>
        </div>

        <div className="subagent-control-group subagent-control-group--effort">
          <span className="subagent-control-label" aria-hidden="true">思考深度</span>
          <div className="subagent-control-field">
            <Select
              className="w-full min-w-0"
              aria-label={`${task.name}思考深度`}
              value={
                reasoningEfforts.includes(selection.reasoningEffort)
                  ? selection.reasoningEffort
                  : undefined
              }
              placeholder="暂无可选深度"
              disabled={
                subagentPolicyControlsDisabled ||
                roleDisabled ||
                reasoningEfforts.length === 0
              }
              optionList={reasoningOptions}
              filter={false}
              onChange={(value) =>
                updateRole({
                  model: selectedModel?.value ?? selection.model,
                  reasoningEffort: String(value ?? ""),
                })
              }
            />
          </div>
        </div>
        {custom && (
          <div className="subagent-custom-fields">
            {CUSTOM_SUBAGENT_FIELDS.map(({ key, label, limit }) => {
              const value = selection[key] ?? "";
              const bytes = new TextEncoder().encode(value).length;
              const fieldId = `${task.id}-${key}`;
              const invalid = !value.trim() || bytes > limit;
              const fieldProps = {
                id: fieldId,
                value,
                disabled: isBusy,
                "aria-label": `${task.name}${label}`,
                "aria-invalid": invalid,
                "aria-describedby": `${fieldId}-hint`,
                onChange: (event: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => updateRole({ [key]: event.target.value }),
              };
              return (
                <div key={key} className={`subagent-custom-field ${key === "developerInstructions" ? "subagent-custom-field--wide" : ""}`}>
                  <label htmlFor={fieldId}>{label}</label>
                  {key === "developerInstructions" ? <TextArea {...fieldProps} rows={3} /> : <Input {...fieldProps} />}
                  <small id={`${fieldId}-hint`} className={invalid ? "subagent-field-error" : "field-hint"}>
                    {bytes} / {limit} 字节{invalid ? " · 必填且不能超限" : ""}
                  </small>
                </div>
              );
            })}
            <div className="subagent-custom-field subagent-custom-field--wide">
              <label id={`${task.id}-access-label`}>读写能力</label>
              <Select
                aria-labelledby={`${task.id}-access-label`}
                aria-label={`${task.name}读写能力`}
                value={selection.access ?? "readOnly"}
                disabled={isBusy || roleDisabled}
                filter={false}
                optionList={[{ value: "readOnly", label: "只读" }, { value: "readWrite", label: "可读写" }]}
                onChange={(value) => {
                  if (value === "readOnly" || value === "readWrite") updateRole({ access: value });
                }}
              />
              <small className="field-hint">受父任务权限与工具门禁约束，不提供独立安全沙箱；角色指令不能提升权限。</small>
            </div>
          </div>
        )}
      </div>
    );
  };

  return (
    <section className="secondary-section subagent-section" aria-labelledby="subagent-title">
      <div className="subagent-settings">
        <SettingsPageHeader
          id="subagent-title"
          title="子代理优化"
          icon={<IconUsersGroup size={15} />}
          description="保留默认与三类内置角色，按任务添加自己的子代理。"
          actions={
            <Switch
              checked={config.subagentOptimization}
              disabled={isBusy}
              onCheckedChange={(checked) => onSubagentOptimizationChange(checked)}
              aria-label="启用子代理优化"
            />
          }
        />
        <div className="module-card-body subagent-policy-body">
          {config.subagentOptimization ? (
            <>
              <div className="subagent-group-card codey-card p-4">
                <div className="grid gap-2">
                  <span className="text-xs font-semibold">最大并发子代理数量</span>
                  <NumberInput aria-label="最大并发子代理数量"
                    aria-describedby="subagent-max-threads-hint" minValue={1} maxValue={64} step={1}
                    value={config.subagentMaxThreads ?? 2} disabled={isBusy} onChange={controller.changeMaxThreads} />
                  <small id="subagent-max-threads-hint" className="field-hint">不包含主代理；保存并重启 Codex 后生效。仅修改上限不会中止已运行任务，重启前请先等待任务结束。</small>
                </div>
              </div>
              <div className="subagent-group-card subagent-group-card--default codey-card">
                {renderRoleCard(DEFAULT_SUBAGENT_TASK)}
              </div>
              <div className="subagent-group-card codey-card">
                <div className="subagent-group-header">
                  <div className="subagent-group-title-wrap">
                    <span className="subagent-group-title">分析角色</span>
                    <div className="subagent-stat-badge subagent-stat-badge--readonly">
                      <span className="subagent-stat-indicator" aria-hidden="true" />
                      <span>已启用</span>
                      <strong>
                        {enabledAnalysisCount} / {analysisTasks.length}
                      </strong>
                    </div>
                  </div>
                  <span className="subagent-group-desc">
                    检索角色只读；复核角色可按任务运行测试、构建和渲染检查
                  </span>
                </div>
                <div className="subagent-table-head" aria-hidden="true">
                  <span>启用</span>
                  <span>角色</span>
                  <span>模型</span>
                  <span>思考深度</span>
                </div>
                <div className="subagent-group-items">
                  {analysisTasks.map(renderRoleCard)}
                </div>
              </div>

              <div className="subagent-group-card codey-card">
                <div className="subagent-group-header">
                  <div className="subagent-group-title-wrap">
                    <span className="subagent-group-title">开发角色</span>
                    <div className="subagent-stat-badge subagent-stat-badge--write">
                      <span className="subagent-stat-indicator" aria-hidden="true" />
                      <span>已启用</span>
                      <strong>
                        {enabledImplementationCount} / {implementationTasks.length}
                      </strong>
                    </div>
                  </div>
                  <span className="subagent-group-desc">
                    负责前后端各类实现，受父任务权限约束
                  </span>
                </div>
                <div className="subagent-table-head" aria-hidden="true">
                  <span>启用</span>
                  <span>角色</span>
                  <span>模型</span>
                  <span>思考深度</span>
                </div>
                <div className="subagent-group-items">
                  {implementationTasks.map(renderRoleCard)}
                </div>
              </div>

              <div className="subagent-group-card subagent-custom-group codey-card">
                <div className="subagent-group-header">
                  <div className="subagent-group-title-wrap">
                    <span className="subagent-group-title">自定义子代理</span>
                    <span className="subagent-stat-badge subagent-stat-badge--readonly">
                      已启用 {customRoles.filter(id => config.subagentRoles[id].enabled).length} / {customRoles.length}
                    </span>
                  </div>
                  <Button size="sm" disabled={isBusy || customRoles.length >= 32} onClick={controller.addRole}>
                    <IconPlus size={15} aria-hidden="true" />添加子代理
                  </Button>
                </div>
                <div className="subagent-group-items">
                  {customRoles.length ? customRoles.map(id => renderRoleCard({
                    id,
                    name: config.subagentRoles[id].name || "未命名子代理",
                    description: config.subagentRoles[id].description || "请填写角色用途，帮助主代理选择合适的角色。",
                    icon: IconUsersGroup,
                  })) : (
                    <p className="subagent-custom-empty">最多添加 32 个专项角色，默认只读；修改后点击侧栏“保存更改”。</p>
                  )}
                </div>
              </div>
              {controller.error && <p role="alert" className="subagent-field-error">{controller.error}</p>}
              <div className={`subagent-policy-callout${subagentModelOptions.length === 0 ? " is-warning" : ""}`}>
                <IconInfoCircle
                  size={16}
                  className="subagent-callout-icon"
                  aria-hidden="true"
                />
                <div className="subagent-callout-text">
                  {subagentModelOptions.length === 0
                    ? "请先在模型管理中为任一可用线路启用模型。"
                    : "检索角色使用只读沙箱，开发与复核角色可写入工作区；复核仅在任务允许时运行构建、测试或渲染检查。角色启停、读写能力、模型和思考深度保存后对下次派生生效，正在运行的子代理沿用原配置；停用或删除不会中止已运行任务。实际操作仍受 Codex 原生权限约束。"}
                  {" "}自定义角色由 Codey 管理，仅供通过 Codey 启动的 Codex 加载；新增角色需保存并重启 Codex。配置随 Codey 备份保存，实际操作不越过父任务权限。
                </div>
              </div>
            </>
          ) : (
            <div className="module-disabled-placeholder">
              <div className="module-disabled-icon">
                <IconUsersGroup size={20} aria-hidden="true" />
              </div>
              <div className="module-disabled-text">
                <strong>子代理优化已关闭</strong>
                <p>开启后选择性委派给三类内置角色或自定义角色，并配置并发数量、模型与思考深度。</p>
              </div>
              <Button
                variant="default"
                size="sm"
                className="mt-1"
                disabled={isBusy}
                onClick={() => onSubagentOptimizationChange(true)}
              >
                开启子代理优化
              </Button>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

export const SubagentPolicyCard = memo(SubagentPolicyCardComponent, (prev, next) => {
  if (prev.active === false && next.active === false) return true;
  if (prev.active !== next.active) return false;
  return (
    prev.config === next.config &&
    prev.isBusy === next.isBusy &&
    prev.subagentModelOptions === next.subagentModelOptions &&
    prev.onConfigChange === next.onConfigChange &&
    prev.onSubagentOptimizationChange === next.onSubagentOptimizationChange
  );
});

type FeaturePolicyCardProps = {
  active?: boolean;
  config: Config;
  isMacClient: boolean;
  isWindowsClient: boolean;
  cleanupBusy: boolean;
  onAnalyzeDiagnosticStorage: (target: DiagnosticStorageTarget) => void;
  popupContainer: HTMLElement | null;
  isBusy: boolean;
  configEditingDisabled: boolean;
  onConfigChange: (config: Config) => void;
  onAddChannel?: (channel: NotificationChannel) => Promise<boolean>;
  onChannelChange?: (
    channelId: string,
    patch: Partial<NotificationChannel>,
  ) => Promise<boolean>;
  onRequestRemoveChannel?: (channel: NotificationChannel) => void;
};

function FeaturePolicyCardComponent({
  config,
  isMacClient,
  isWindowsClient,
  cleanupBusy,
  onAnalyzeDiagnosticStorage,
  popupContainer,
  isBusy,
  configEditingDisabled,
  onConfigChange,
  onAddChannel,
  onChannelChange,
  onRequestRemoveChannel,
}: FeaturePolicyCardProps) {
  const configuredGpuLaunchModeIndex = GPU_LAUNCH_MODES.findIndex(
    ({ value }) => value === config.gpuLaunchMode,
  );
  const gpuLaunchModeIndex = Math.max(configuredGpuLaunchModeIndex, 0);
  const gpuLaunchMode = GPU_LAUNCH_MODES[gpuLaunchModeIndex];
  const gpuLaunchModeStyle = {
    "--gpu-mode-offset": `${gpuLaunchModeIndex * 100}%`,
  } as CSSProperties;

  return (
    <>
      <section className="secondary-section" aria-labelledby="runtime-title">
        <div className="feature-policy-heading">
          <div className="flex items-center gap-2">
            <span className="section-title-icon" aria-hidden="true">
              <IconAdjustments size={14} />
            </span>
            <h3 id="runtime-title" className="settings-section-heading">Codex 功能策略</h3>
          </div>
          <p>按需精简客户端模块和界面行为。</p>
        </div>
        <div className="runtime-settings">
          <div className="feature-grid">
            {/* GPU 渲染模式：仅 Windows 客户端展示 */}
            {isWindowsClient && (
              <div className={`feature-card gpu-mode-card full-width-card ${gpuLaunchMode.value !== "off" ? "active" : ""}`}>
                <div className="feature-card-header">
                  <div className="feature-card-title">
                    <strong>GPU 渲染模式</strong>
                    <Badge variant="warning">实验性</Badge>
                  </div>
                </div>
                <div className="feature-card-body gpu-mode-card-body">
                  <fieldset
                    className="gpu-mode-fieldset"
                    disabled={configEditingDisabled}
                    aria-describedby="gpu-launch-mode-description"
                  >
                    <legend className="sr-only">Codex GPU 启动模式</legend>
                    <div className="gpu-mode-slider" style={gpuLaunchModeStyle}>
                      <span className="gpu-mode-slider-thumb" aria-hidden="true" />
                      {GPU_LAUNCH_MODES.map((mode) => (
                        <label
                          key={mode.value}
                          className={`gpu-mode-option ${gpuLaunchMode.value === mode.value ? "selected" : ""}`}
                        >
                          <input
                            type="radio"
                            name="codey-gpu-launch-mode"
                            value={mode.value}
                            checked={gpuLaunchMode.value === mode.value}
                            onChange={() =>
                              onConfigChange({
                                ...config,
                                gpuLaunchMode: mode.value,
                              })
                            }
                          />
                          <span>{mode.label}</span>
                        </label>
                      ))}
                    </div>
                  </fieldset>
                  <small id="gpu-launch-mode-description" aria-live="polite">
                    {gpuLaunchMode.value === "disableGpu"
                      ? "启动 Codex 时附加 --disable-gpu；可能增加 CPU 占用"
                      : gpuLaunchMode.value === "disableGpuRasterization"
                        ? "启动 Codex 时附加 --disable-gpu-rasterization；仅将栅格化移到 CPU"
                        : "保持 Codex 默认 GPU 渲染，不附加诊断参数"}
                  </small>
                </div>
              </div>
            )}

            <div
              className={`feature-card ${config.slimCodexPet ? "active" : ""}`}
            >
              <div className="feature-card-header">
                <strong>精简 Codex 宠物模块</strong>
                <Switch
                  checked={config.slimCodexPet}
                  disabled={configEditingDisabled}
                  onCheckedChange={(checked) =>
                    onConfigChange({ ...config, slimCodexPet: checked })
                  }
                  aria-label="精简 Codex 宠物模块"
                />
              </div>
              <div className="feature-card-body">
                <small>
                  {config.slimCodexPet
                    ? "已收起宠物并取消隐藏窗口预热；语音功能仍按需启用"
                    : "保留 Codex 宠物的完整功能"}
                </small>
              </div>
            </div>

            <div
              className={`feature-card ${config.scopeGuard ? "active" : ""}`}
            >
              <div className="feature-card-header">
                <strong>防御性编程控制</strong>
                <Switch
                  checked={config.scopeGuard}
                  disabled={configEditingDisabled}
                  onCheckedChange={(checked) =>
                    onConfigChange({ ...config, scopeGuard: checked })
                  }
                  aria-label="防御性编程控制"
                />
              </div>
              <div className="feature-card-body">
                <small>
                  {config.scopeGuard
                    ? "新会话和压缩后向代理注入防护尺度规则，减少不必要的兼容层、兜底和重复检查"
                    : "不注入额外规则，保持 Codex 默认行为"}
                </small>
              </div>
            </div>

            <div
              className={`feature-card ${config.disableTraceLogWrites ? "active" : ""}`}
            >
              <div className="feature-card-header">
                <strong>Trace 日志写盘保护</strong>
                <div className="feature-card-actions">
                  <Button
                    className="feature-action-btn"
                    size="xs"
                    disabled={isBusy}
                    loading={cleanupBusy}
                    onClick={() => onAnalyzeDiagnosticStorage("trace")}
                    aria-label="分析并清理 Trace 日志"
                  >
                    分析并清理
                  </Button>
                  <Switch
                    checked={config.disableTraceLogWrites}
                    disabled={configEditingDisabled}
                    onCheckedChange={(checked) =>
                      onConfigChange({
                        ...config,
                        disableTraceLogWrites: checked,
                      })
                    }
                    aria-label="启用 Codex Trace 日志写盘保护"
                  />
                </div>
              </div>
              <div className="feature-card-body">
                <small>阻止 Trace 日志持续写入数据库影响硬盘寿命</small>
              </div>
            </div>

            {isMacClient && (
              <div
                className={`feature-card ${config.protectCrashpadPending ? "active" : ""}`}
              >
                <div className="feature-card-header">
                  <strong>Crashpad 磁盘保护</strong>
                  <div className="feature-card-actions">
                    <Button
                      className="feature-action-btn"
                      size="xs"
                      disabled={isBusy}
                      loading={cleanupBusy}
                      onClick={() => onAnalyzeDiagnosticStorage("crashpad")}
                      aria-label="分析并清理 Crashpad 报告"
                    >
                      分析并清理
                    </Button>
                    <Switch
                      checked={config.protectCrashpadPending}
                      disabled={configEditingDisabled}
                      onCheckedChange={(checked) =>
                        onConfigChange({
                          ...config,
                          protectCrashpadPending: checked,
                        })}
                      aria-label="启用 Codex Crashpad 磁盘保护"
                    />
                  </div>
                </div>
                <div className="feature-card-body">
                  <small>
                    {config.protectCrashpadPending
                      ? "待处理崩溃报告超过安全上限时自动收敛，并保留最近写入"
                      : "仅显示占用和提供手动清理，不执行自动容量保护"}
                  </small>
                </div>
              </div>
            )}

            <div
              className={`feature-card ${config.hideFullAccessWarning ? "active" : ""}`}
            >
              <div className="feature-card-header">
                <strong>屏蔽完全访问安全提示</strong>
                <Switch
                  checked={config.hideFullAccessWarning}
                  disabled={configEditingDisabled}
                  onCheckedChange={(checked) =>
                    onConfigChange({ ...config, hideFullAccessWarning: checked })
                  }
                  aria-label="屏蔽完全访问安全提示"
                />
              </div>
              <div className="feature-card-body">
                <small>
                  {config.hideFullAccessWarning
                    ? "自动隐藏完全访问模式和 Ultra 的原生安全提示"
                    : "保留 Codex 原生安全提示"}
                </small>
              </div>
            </div>
          </div>
        </div>
      </section>

      {onAddChannel && onChannelChange && onRequestRemoveChannel && (
        <section className="secondary-section notification-section" aria-labelledby="notification-title">
          <NotificationChannelsCard
            config={config}
            container={popupContainer ?? null}
            popupContainer={popupContainer ?? null}
            isBusy={isBusy}
            onAddChannel={onAddChannel}
            onChannelChange={onChannelChange}
            onRequestRemoveChannel={onRequestRemoveChannel}
          />
        </section>
      )}
    </>
  );
}

export const FeaturePolicyCard = memo(FeaturePolicyCardComponent, (prev, next) => {
  if (prev.active === false && next.active === false) return true;
  if (prev.active !== next.active) return false;
  return (
    prev.config === next.config &&
    prev.isMacClient === next.isMacClient &&
    prev.isWindowsClient === next.isWindowsClient &&
    prev.cleanupBusy === next.cleanupBusy &&
    prev.onAnalyzeDiagnosticStorage === next.onAnalyzeDiagnosticStorage &&
    prev.popupContainer === next.popupContainer &&
    prev.isBusy === next.isBusy &&
    prev.configEditingDisabled === next.configEditingDisabled &&
    prev.onConfigChange === next.onConfigChange &&
    prev.onAddChannel === next.onAddChannel &&
    prev.onChannelChange === next.onChannelChange &&
    prev.onRequestRemoveChannel === next.onRequestRemoveChannel
  );
});
