import type { BuiltinToolsStatus } from "../builtinTools";
import type { PonytailStatus } from "../features/builtin-tools";

export function createBuiltinToolsPreview() {
  const state: BuiltinToolsStatus = { installed: false, version: "1.6.2", mcp: { enabled: false, managed: false, automatic: false }, job: null };
  let finishAt = 0;
  const ponytail: PonytailStatus = { installed: false, enabled: false, version: null, conflict: null, job: null,
    settings: { defaultMode: "full", effectiveMode: "full", environmentOverride: null, revision: "missing", configPath: "C:/Users/Preview/AppData/Roaming/ponytail/config.json" } };
  let ponytailFinishAt = 0;
  return async (args: Record<string, unknown> = {}) => {
    if (args.tool === "ponytail") {
      if (ponytail.job?.phase === "running" && Date.now() >= ponytailFinishAt) {
        ponytail.installed = true; ponytail.enabled = true; ponytail.version = "5.3.3";
        ponytail.job = { ...ponytail.job, phase: "succeeded", message: "Ponytail 安装完成。请重启 Codex，在 /hooks 中检查并信任官方钩子，再开始新聊天。" };
      }
      if (args.action === "status") return structuredClone(ponytail);
      if (ponytail.job?.phase === "running") throw new Error("已有 Ponytail 操作进行中，请等待完成");
      if (args.action === "save_settings") {
        if (args.defaultMode !== "lite" && args.defaultMode !== "full" && args.defaultMode !== "ultra" && args.defaultMode !== "off") throw new Error("Ponytail 默认模式无效");
        if (args.revision !== ponytail.settings.revision) throw new Error("Ponytail 配置已被修改；请先撤销参数修改以加载最新配置，再重新选择并保存");
        ponytail.settings = { ...ponytail.settings, defaultMode: args.defaultMode, effectiveMode: ponytail.settings.environmentOverride ?? args.defaultMode, revision: String(Date.now()) };
        ponytail.message = "默认模式已保存，新聊天生效。";
      } else if (args.action === "set_enabled" && typeof args.enabled === "boolean") {
        if (args.enabled && !ponytail.installed) {
          ponytail.job = { id: String(Date.now()), phase: "running", message: "正在通过 Codex 安装 Ponytail 官方插件…" }; ponytailFinishAt = Date.now() + 2200;
        } else { ponytail.enabled = args.enabled; ponytail.job = null; ponytail.message = args.enabled ? "Ponytail 已启用，请重启 Codex。" : "Ponytail 已停用，安装文件和参数已保留。"; }
      } else throw new Error("未知 Ponytail 操作");
      return structuredClone(ponytail);
    }
    if (state.job?.phase === "running" && Date.now() >= finishAt) {
      state.installed = true;
      state.mcp = { id: "codey_codegraph", enabled: true, managed: true, automatic: true };
      state.job = { ...state.job, phase: "succeeded", message: "自动接入已启用。下一轮项目聊天会在后台建立索引并持续同步，无需选择目录。" };
    }
    if (args.action === "status") return structuredClone(state);
    if (args.action !== "set_enabled") throw new Error("未知内置工具操作");
    if (state.job?.phase === "running") throw new Error("已有 CodeGraph 操作进行中，请等待完成");
    if (args.enabled === false) {
      state.mcp.enabled = false;
      state.job = null;
    } else if (args.enabled === true) {
      state.job = { id: String(Date.now()), phase: "running", message: "正在准备 CodeGraph 自动接入，首次启用需下载官方运行包…" };
      finishAt = Date.now() + 2200;
    } else throw new Error("enabled 必须为布尔值");
    return structuredClone(state);
  };
}
