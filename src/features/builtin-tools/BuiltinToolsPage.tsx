import type { Config, FastContextToolsStatus, RuntimeStatus } from "../../App.types";
import { FastContextToolsCard } from "./FastContextToolsCard";
import { IconRefresh, IconTools } from "@tabler/icons-react";
import { Badge, Button, Switch } from "../../components/ui";
import { SettingsPageHeader } from "../../SettingsPageHeader";
import { useBuiltinToolsController } from "./useBuiltinToolsController";
import { PonytailCard } from "./PonytailCard";

type Props = { active: boolean; isBusy: boolean; config: Config; fastContextToolsStatus: FastContextToolsStatus; runtimeStatus: Pick<RuntimeStatus, "running" | "fastContextToolsActive">; onConfigChange: (config: Config) => void };

export function BuiltinToolsPage({ active, isBusy, config, fastContextToolsStatus, runtimeStatus, onConfigChange }: Props) {
  const { status, busy, error, notice, running, blocked, userManaged, setEnabled, refresh } = useBuiltinToolsController({ active, isBusy });
  return (
    <section className="secondary-section" aria-labelledby="builtin-tools-title">
      <SettingsPageHeader id="builtin-tools-title" title="内置工具" icon={<IconTools size={15} />}
        description="统一管理文件检索、代码索引与开发辅助工具。" />
      <div className="flex flex-col gap-4">
        <FastContextToolsCard active={active} config={config} fastContextToolsStatus={fastContextToolsStatus} runtimeStatus={runtimeStatus} isBusy={isBusy} onConfigChange={onConfigChange} />
        <article className={`feature-card ${status?.mcp.enabled ? "active" : ""}`} aria-busy={Boolean(running)}>
          <div className="feature-card-header">
            <div className="feature-card-title"><strong>CodeGraph 代码索引</strong>
              <Badge variant={status?.installed ? "brand" : "secondary"}>{status ? status.installed ? `v${status.version}` : "待安装" : "检测中"}</Badge>
            </div>
            <Switch checked={Boolean(status?.mcp.enabled && status.mcp.automatic)} disabled={blocked || userManaged}
              aria-label="启用 CodeGraph 自动索引" onCheckedChange={enabled => void setEnabled(enabled)} />
          </div>
          <div className="feature-card-body flex flex-col gap-3">
            <p className="m-0 text-xs leading-relaxed">理解符号、调用链与模块依赖。启用后，仅在工作区已添加的本地项目中聊天时自动建立索引，并持续同步代码变化。</p>
            {status?.mcp.id && <small>{status.mcp.managed ? "Codey 管理的 MCP" : `已配置的 MCP：${status.mcp.id}`} · {status.mcp.enabled ? "已启用" : "已停用"}</small>}
            {userManaged && <small>已检测到你配置的 CodeGraph。请在 MCP 管理中使用或调整，内置版本不会重复接入。</small>}
            {status?.mcp.managed && !status.mcp.automatic && <small>开启自动索引后，已有接入将升级为随聊天项目自动运行。</small>}
            <small>首次启用会下载官方运行包，下一轮项目聊天生效。无需填写路径或执行命令；首次索引期间可继续聊天，关闭后保留已有索引。</small>
            <div className="flex flex-wrap gap-2">
              {status?.job?.phase === "failed" && <Button size="sm" disabled={blocked || userManaged} onClick={() => void setEnabled(true)}>重试启用</Button>}
              <Button size="sm" variant="outline" disabled={busy || isBusy} onClick={refresh}>
                <IconRefresh size={14} aria-hidden="true" />刷新</Button>
            </div>
            {status?.job && <div role={status.job.phase === "failed" ? "alert" : "status"} className={`rounded-lg p-3 text-xs break-words ${status.job.phase === "failed" ? "bg-red-500/10 text-danger" : "bg-default/40"}`}>
              {status.job.message}
            </div>}
            {error && <p role="alert" className="m-0 text-xs text-danger break-words">{error}</p>}
            {notice && <p role="status" className="m-0 text-xs">{notice}</p>}
          </div>
        </article>
        <PonytailCard active={active} isBusy={isBusy} />
      </div>
    </section>
  );
}
