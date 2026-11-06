import type { Config, FastContextToolsStatus, RuntimeStatus } from "../../App.types";
import { Badge, Switch, Tooltip } from "../../components/ui";

type Props = {
  active: boolean;
  config: Config;
  fastContextToolsStatus: FastContextToolsStatus;
  runtimeStatus: Pick<RuntimeStatus, "running" | "fastContextToolsActive">;
  isBusy: boolean;
  configEditingDisabled?: boolean;
  onConfigChange: (config: Config) => void;
};

export function FastContextToolsCard({ active, config, fastContextToolsStatus, runtimeStatus, isBusy, configEditingDisabled = isBusy, onConfigChange }: Props) {
  const fastctxStatusBlocksEmbedded =
    fastContextToolsStatus.userConfigured ||
    fastContextToolsStatus.detectionFailed;
  const fastContextToolsEnabled =
    config.fastContextTools && !fastctxStatusBlocksEmbedded;
  const fastctxBlockedReason = fastContextToolsStatus.detectionFailed
    ? "暂时无法确认 Codex 配置中的 FastCtx 状态，为避免重复加载，Codey 内置 FastCtx 不可开启"
    : fastContextToolsStatus.userConfigured
      ? `已检测到 Codex 配置中的 FastCtx${
          fastContextToolsStatus.serverId
            ? `（${fastContextToolsStatus.serverId}）`
            : ""
        }，为避免重复加载，Codey 内置 FastCtx 不可开启`
      : "";
  const fastContextToolsSwitch = (
    <Switch
      checked={fastContextToolsEnabled}
      disabled={configEditingDisabled || fastctxStatusBlocksEmbedded}
      onCheckedChange={(checked) =>
        active && !configEditingDisabled && !fastctxStatusBlocksEmbedded && onConfigChange({ ...config, fastContextTools: checked })
      }
      aria-label="启用 FastCtx 上下文工具"
    />
  );

  return (
    <article
      aria-label="FastCtx 上下文工具"
      className={`feature-card ${fastContextToolsEnabled ? "active" : ""}`}
    >
      <div className="feature-card-header">
        <div className="feature-card-title">
          <strong>FastCtx 上下文工具</strong>
          <Badge variant="secondary" className="font-mono">v0.2.6</Badge>
        </div>
        {fastctxStatusBlocksEmbedded ? (
          <Tooltip
            content={fastctxBlockedReason}
            position="top"
          >
            <span
              className="fastctx-disabled-switch-tooltip"
              tabIndex={0}
              aria-label={fastctxBlockedReason}
            >
              {fastContextToolsSwitch}
            </span>
          </Tooltip>
        ) : (
          fastContextToolsSwitch
        )}
      </div>
      <div className="feature-card-body flex flex-col gap-2">
        <p className="m-0 text-xs">读取本地文件、搜索内容与定位路径，无需预先建立索引。</p>
        <small>
          {fastctxStatusBlocksEmbedded
            ? fastContextToolsStatus.detectionFailed
              ? "暂时无法确认 FastCtx 状态，内置工具保持关闭"
              : "已检测到已配置的 FastCtx，Codey 不会重复加载内置工具"
            : config.fastContextTools
              ? "下次启动加载 Codey 内置 FastCtx 文件工具"
              : "保持 Codex 默认文件工具，不加载额外 MCP"}
        </small>
        <small role="status">{runtimeStatus.running ? runtimeStatus.fastContextToolsActive && !fastctxStatusBlocksEmbedded ? "当前运行实例已加载 Codey 内置 FastCtx" : "当前运行实例未加载内置 FastCtx；启用并保存后重启 Codex 生效" : "Codex 尚未运行"}</small>
      </div>
    </article>
  );
}
