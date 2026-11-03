import { useEffect, useState } from "react";
import { IconRefresh } from "@tabler/icons-react";
import { invoke } from "../api";
import type { RuntimeStatus } from "../App.types";
import type { BuiltinToolsStatus } from "../builtinTools";
import type { PonytailStatus } from "../features/builtin-tools";
import { useOperationScope } from "../useOperationScope";

type Snapshot = {
  runtime?: RuntimeStatus;
  config?: { config: { defaultModel: string; fastContextTools?: boolean; profiles: { id: string; name: string; enabled?: boolean }[] } };
  graph?: BuiltinToolsStatus;
  ponytail?: PonytailStatus;
  plugins?: { plugins: { id: string; name: string; enabled: boolean }[] };
};

export function ReadOnlyConsole({ active }: { active: boolean }) {
  const scope = useOperationScope(active);
  const [snapshot, setSnapshot] = useState<Snapshot>({});
  const [errors, setErrors] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    if (!active) return;
    const current = scope.read();
    setBusy(true);
    const reads = [
      ["runtime", () => invoke<RuntimeStatus>("runtime_status")],
      ["config", () => invoke<Snapshot["config"]>("load_codey_config")],
      ["graph", () => invoke<BuiltinToolsStatus>("builtin_tools", { action: "status" })],
      ["ponytail", () => invoke<PonytailStatus>("builtin_tools", { tool: "ponytail", action: "status" })],
      ["plugins", () => invoke<Snapshot["plugins"]>("list_codey_plugins")],
    ] as const;
    void Promise.allSettled(reads.map(async ([key, request]) => [key, await request()] as const)).then(results => {
      if (!current()) return;
      const next: Record<string, unknown> = {};
      const failures: string[] = [];
      results.forEach(result => {
        if (result.status === "fulfilled") next[result.value[0]] = result.value[1];
        else failures.push(result.reason instanceof Error ? result.reason.message : "读取控制台状态失败");
      });
      setSnapshot(next); setErrors(failures); setBusy(false);
    });
    return () => scope.invalidateReads();
  }, [active, revision, scope]);

  return <main className="remote-readonly-console" aria-busy={busy}>
    <div className="remote-readonly-heading"><div><h1>远程控制台</h1><p>只读查看当前状态；会话操作请返回 Codex 工作区。</p></div><button className="remote-icon-button" aria-label="刷新控制台状态" disabled={busy} onClick={() => { scope.invalidateReads(); setRevision(value => value + 1); }}><IconRefresh size={18} /></button></div>
    <p className="codey-callout" role="note">安装、账号配置、备份、清理、更新安装、重启和监听服务设置均请在本机操作。此页面不提供这些高权限操作。</p>
    {busy && <p role="status">正在读取状态…</p>}
    {errors.map((error, index) => <p className="codey-callout" data-tone="danger" role="alert" key={index}>{error}</p>)}
    <section className="codey-panel" aria-labelledby="remote-runtime-title"><h2 id="remote-runtime-title">运行状态</h2><p>{snapshot.runtime ? snapshot.runtime.running ? "Codex 运行中" : "Codex 未运行" : "未读取"}</p>{snapshot.runtime && <p className="remote-readonly-muted">客户端：{snapshot.runtime.codexAppVersion || "未知版本"} · {snapshot.runtime.clientPlatform || "未知平台"}</p>}</section>
    <section className="codey-panel" aria-labelledby="remote-routes-title"><h2 id="remote-routes-title">线路与模型</h2>{snapshot.config && <p className="remote-readonly-muted">默认模型：{snapshot.config.config.defaultModel || "未指定"}</p>}{snapshot.config?.config.profiles.map(route => <div className="remote-readonly-row" key={route.id}><strong>{route.name}</strong><span>{route.enabled === false ? "已停用" : "已启用"}</span></div>)}{!snapshot.config && <p className="remote-readonly-muted">暂未读取线路信息。</p>}</section>
    <section className="codey-panel" aria-labelledby="remote-tools-title"><h2 id="remote-tools-title">内置工具</h2><div className="remote-readonly-row"><strong>FastCtx</strong><span>{snapshot.runtime?.fastContextToolsActive === true ? "当前运行实例已加载" : snapshot.config?.config.fastContextTools === false ? "已停用" : "未读取运行状态"}</span></div><div className="remote-readonly-row"><strong>CodeGraph</strong><span>{snapshot.graph ? `${snapshot.graph.installed ? `v${snapshot.graph.version}` : "未安装"} · ${snapshot.graph.mcp.enabled ? "已启用" : "已停用"}` : "未读取"}</span></div><div className="remote-readonly-row"><strong>Ponytail</strong><span>{snapshot.ponytail ? `${snapshot.ponytail.installed ? `v${snapshot.ponytail.version}` : "未安装"} · ${snapshot.ponytail.enabled ? "已启用" : "已停用"}` : "未读取"}</span></div></section>
    <section className="codey-panel" aria-labelledby="remote-plugins-title"><h2 id="remote-plugins-title">插件</h2>{snapshot.plugins?.plugins.map(plugin => <div className="remote-readonly-row" key={plugin.id}><strong>{plugin.name}</strong><span>{plugin.enabled ? "已启用" : "已停用"}</span></div>)}{!snapshot.plugins?.plugins.length && <p className="remote-readonly-muted">没有可显示的插件。</p>}</section>
  </main>;
}
