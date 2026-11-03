import { IconAlertTriangle, IconDeviceMobile, IconLogout, IconRefresh, IconServer } from "@tabler/icons-react";
import { Badge, Button, Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Input, PasswordInput } from "../../components/ui";
import { SettingsPageHeader } from "../../SettingsPageHeader";
import { useRemoteControlController } from "./useRemoteControlController";

export function RemoteControlPanel({ active, isBusy = false, container }: { active: boolean; isBusy?: boolean; container?: HTMLElement | null }) {
  const c = useRemoteControlController({ active, isBusy });
  if (!c.local) return <section aria-labelledby="remote-title"><SettingsPageHeader id="remote-title" title="远程控制" icon={<IconDeviceMobile size={15} />} description="本机远程服务设置" /><p role="status">管理员账号、监听服务与登录会话设置请在本机操作。</p></section>;
  return <section className="secondary-section" aria-labelledby="remote-title" aria-busy={!!c.busy}>
    <SettingsPageHeader id="remote-title" title="远程控制" icon={<IconDeviceMobile size={15} />} description="通过浏览器登录并继续电脑上的 Codex 会话。" />
    <div className="flex flex-col gap-4">
      <div className="codey-callout" data-tone="warning"><IconAlertTriangle size={16} aria-hidden="true" /><div className="codey-callout-body"><p>启用后监听局域网 HTTP，密码和会话内容的传输未加密，仅用于可信网络。</p><p>不内置 HTTPS 代理或内网穿透；外部入口由你自行保护。记住登录有效期为 30 天，注销、改密或撤销后失效。</p></div></div>
      {c.error && <p role="alert" className="codey-callout" data-tone="danger">{c.error}</p>}
      {c.notice && <p role="status" className="codey-callout" data-tone="success">{c.notice}</p>}
      <section className="codey-panel" aria-labelledby="remote-account-title">
        <div className="codey-panel-heading"><h3 id="remote-account-title" className="settings-section-heading">管理员与监听设置</h3><p>首次启用前在本机设置账号密码；不提供默认凭证。修改监听设置前请先停止服务。</p></div>
        <form className="flex flex-col gap-3" onSubmit={event => { event.preventDefault(); c.askSave(); }}>
          <fieldset disabled={c.blocked || c.status?.running} className="m-0 flex min-w-0 flex-col gap-3 border-0 p-0">
            <div className="grid grid-cols-1 min-[640px]:grid-cols-2 gap-3">
              <label className="codey-field-label"><span>管理员账号</span><Input autoComplete="username" maxLength={128} value={c.draft.username} onChange={event => c.change("username", event.target.value)} /></label>
              <label className="codey-field-label"><span>{c.status?.accountConfigured ? "新密码（留空保留）" : "管理员密码"}</span><PasswordInput autoComplete="new-password" value={c.draft.password} placeholder="至少 12 个字符" onChange={event => c.change("password", event.target.value)} /></label>
            </div>
            <div className="grid grid-cols-1 min-[640px]:grid-cols-2 gap-3">
              <label className="codey-field-label"><span>监听地址</span><Input value="0.0.0.0" readOnly aria-describedby="remote-bind-help" /></label>
              <label className="codey-field-label"><span>监听端口</span><Input type="number" min={1} max={65535} step={1} inputMode="numeric" value={c.draft.port} onChange={event => c.change("port", event.target.value)} /></label>
            </div>
            <small id="remote-bind-help" className="text-muted">允许局域网访问；默认端口 18787。服务不会代你修改防火墙或建立穿透连接。</small>
            <label className="codey-field-label"><span>外部访问地址（可选）</span><Input autoComplete="off" placeholder="https://codey.example.com" value={c.draft.publicUrl} onChange={event => c.change("publicUrl", event.target.value)} /><small className="text-muted">仅用于你自建入口的来源校验，不配置或启动任何代理；只填协议、主机与端口。</small></label>
          </fieldset>
          <div className="flex flex-wrap gap-2"><Button type="submit" size="sm" disabled={c.blocked || c.status?.running || !c.dirty}>保存管理员与端口</Button><Button type="button" size="sm" variant="outline" disabled={c.blocked} onClick={c.refresh}><IconRefresh size={14} />刷新</Button></div>
        </form>
      </section>
      <section className="codey-panel" aria-labelledby="remote-service-title">
        <div className="flex flex-wrap items-center justify-between gap-3"><h3 id="remote-service-title" className="settings-section-heading inline-flex items-center gap-2"><IconServer size={16} />监听服务 <Badge variant={c.status?.running ? "brand" : "secondary"}>{c.status?.running ? "运行中" : "已关闭"}</Badge></h3>
          <Button size="sm" variant={c.status?.running ? "outline" : "default"} disabled={c.blocked || (!c.status?.running && (!c.status?.accountConfigured || c.dirty))} loading={!!c.busy} onClick={() => void (c.status?.running ? c.stop() : c.start())}>{c.status?.running ? "停止远程服务" : "启用远程服务"}</Button></div>
        {!c.status?.accountConfigured && <p className="m-0 text-xs text-muted">请先保存管理员账号和密码，再启用远程服务。</p>}
        {c.dirty && <p className="m-0 text-xs text-muted">有未保存的管理员或端口设置，请先保存。</p>}
        {!!c.status?.urls.length && <div className="flex flex-col gap-2" aria-label="访问地址">{c.status.urls.filter(url => /^https?:\/\//i.test(url)).map(url => <a className="break-all text-sm underline" key={url} href={url} target="_blank" rel="noopener noreferrer">{url}</a>)}</div>}
      </section>
      <section className="codey-panel" aria-labelledby="remote-sessions-title">
        <div className="flex flex-wrap items-center justify-between gap-3"><h3 id="remote-sessions-title" className="settings-section-heading">已登录会话</h3><Button size="sm" variant="outline" disabled={c.blocked || !c.status?.sessions.length} onClick={c.askRevokeAll}>撤销全部登录</Button></div>
        {!c.status?.sessions.length && <p className="m-0 text-xs text-muted">暂无有效浏览器登录。</p>}
        {c.status?.sessions.map(session => <div key={session.id} className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-[var(--codey-border)] p-3"><div className="min-w-0 text-xs"><p className="m-0 break-all font-mono">{session.id}</p><p className="mb-0 text-muted">登录：{new Date(session.createdAt).toLocaleString()} · 到期：{new Date(session.expiresAt).toLocaleString()}</p></div><Button size="sm" variant="outline" disabled={c.blocked} onClick={() => void c.revoke(session.id)}><IconLogout size={14} />撤销</Button></div>)}
      </section>
    </div>
    {active && c.confirmation && <Dialog open onOpenChange={open => { if (!open) c.cancelConfirmation(); }}><DialogContent container={container}><DialogHeader><DialogTitle>{c.confirmation === "save" ? "保存管理员与监听设置？" : "撤销所有浏览器登录？"}</DialogTitle><DialogDescription>{c.confirmation === "save" ? "更改账号或密码会撤销已有登录；密码不明文保存，且不会随配置备份导出。" : "所有浏览器将需要重新输入账号密码；不会停止 Codex 正在执行的任务。"}</DialogDescription></DialogHeader><DialogFooter><Button variant="outline" disabled={!!c.busy} onClick={c.cancelConfirmation}>取消</Button><Button disabled={c.blocked} onClick={() => void c.confirm()}>确认</Button></DialogFooter></DialogContent></Dialog>}
  </section>;
}
