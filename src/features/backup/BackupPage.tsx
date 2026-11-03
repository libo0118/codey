import { useRef } from "react";
import { IconAlertCircle, IconAlertTriangle, IconCircleCheck, IconCloud, IconCloudDownload, IconCloudUpload, IconDatabaseExport, IconDownload, IconFileCode, IconLoader2, IconUpload } from "@tabler/icons-react";
import { Button, Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Input, PasswordInput } from "../../components/ui";
import { SettingsPageHeader } from "../../SettingsPageHeader";
import { useBackupController, type BackupOptions } from "./useBackupController";

export function BackupPage({ container, ...options }: BackupOptions & { container?: HTMLElement | null }) {
  const remote = window.__codeyRemoteMode === true;
  const c = useBackupController({ ...options, remote });
  const file = useRef<HTMLInputElement>(null);
  return <section className="secondary-section" aria-labelledby="backup-title" aria-busy={!!c.busy}>
    <SettingsPageHeader id="backup-title" title="备份与恢复" icon={<IconDatabaseExport size={15} />}
      description="将已保存的 Codey 配置导出为 JSON，或通过 WebDAV 保存与恢复云端备份。" />
    {remote ? <p role="status" className="text-sm text-muted">备份与恢复仅可在电脑端操作。</p> : <div className="flex flex-col gap-5">
      <div className="codey-callout" data-tone="warning">
        <IconAlertTriangle size={15} aria-hidden="true" />
        <div className="codey-callout-body">
          <p className="font-semibold text-[var(--codey-text)]">JSON 含线路 API Key、请求头和通知凭据，未加密，请仅存放在可信位置。</p>
          <p>备份包括线路、模型和功能配置。恢复时保留本机账号与插件线路、安装路径及脚本路径；账号凭据、插件文件、MCP / Skill、会话和日志需单独备份。WebDAV 连接设置独立保存，不随备份导出。</p>
        </div>
      </div>
      {options.dirty && <p role="status" className="codey-callout" data-tone="warning"><IconAlertCircle size={15} aria-hidden="true" /><span>有未保存的配置更改，请先保存或放弃后再备份与恢复。</span></p>}
      {c.error && <p role="alert" className="codey-callout" data-tone="danger"><IconAlertCircle size={15} aria-hidden="true" /><span>{c.error}</span></p>}
      {c.notice && <p role="status" className="codey-callout break-all" data-tone="success"><IconCircleCheck size={15} aria-hidden="true" /><span>{c.notice}</span></p>}
      {c.busy && <p role="status" className="codey-inline-status"><IconLoader2 size={14} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />正在{c.busy}…</p>}
      <section className="codey-panel" aria-labelledby="backup-local-title">
        <div className="codey-panel-heading">
          <h3 id="backup-local-title" className="settings-section-heading inline-flex items-center gap-2"><IconFileCode size={16} className="text-[var(--codey-blue)]" aria-hidden="true" />本地 JSON</h3>
          <p>导入后先预览，再确认覆盖配置。恢复前会自动保存当前配置的备份。</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button size="sm" disabled={c.blocked || options.dirty} onClick={c.askExport}><IconDownload size={15} />导出 JSON</Button>
          <Button size="sm" variant="outline" disabled={c.blocked || options.dirty} onClick={() => file.current?.click()}><IconUpload size={15} />导入 JSON</Button>
          <input ref={file} type="file" accept=".json,application/json" aria-label="选择 JSON 备份" hidden onChange={event => {
            const selected = event.currentTarget.files?.[0]; event.currentTarget.value = "";
            if (selected) void c.importFile(selected);
          }} />
        </div>
      </section>
      <section className="codey-panel" aria-labelledby="backup-cloud-title">
        <div className="codey-panel-heading">
          <h3 id="backup-cloud-title" className="settings-section-heading inline-flex items-center gap-2"><IconCloud size={16} className="text-[var(--codey-blue)]" aria-hidden="true" />WebDAV 云备份</h3>
          <p>填写完整的 JSON 文件地址，父目录需已存在。每次上传替换该文件，下载后可预览并恢复。</p>
        </div>
        <form className="flex flex-col gap-3" onSubmit={event => { event.preventDefault(); void c.saveSettings(); }}>
          <fieldset disabled={c.blocked} className="m-0 flex min-w-0 flex-col gap-3 border-0 p-0">
            <label className="codey-field-label"><span>WebDAV 文件地址</span><Input aria-label="WebDAV 文件地址" placeholder="https://dav.example.com/backup/codey.json" value={c.draft.url} onChange={event => c.change("url", event.target.value)} /></label>
            <div className="grid grid-cols-1 min-[640px]:grid-cols-2 gap-3">
              <label className="codey-field-label"><span>用户名</span><Input aria-label="WebDAV 用户名" autoComplete="off" value={c.draft.username} onChange={event => c.change("username", event.target.value)} /></label>
              <label className="codey-field-label"><span>密码 / 应用密码</span><PasswordInput aria-label="WebDAV 密码" autoComplete="new-password" placeholder={c.draft.passwordConfigured ? "已保存，留空保留" : "输入密码"} value={c.draft.password} onChange={event => c.change("password", event.target.value)} /></label>
            </div>
            <div className="flex flex-col gap-2">
              <label className="flex cursor-pointer items-start gap-2 text-xs text-[var(--codey-text-soft)]"><input type="checkbox" className="codey-native-check" checked={c.draft.clearPassword} onChange={event => c.change("clearPassword", event.target.checked)} />清除已保存的密码（匿名访问）</label>
              <label className="flex cursor-pointer items-start gap-2 text-xs text-[var(--codey-text-soft)]"><input type="checkbox" className="codey-native-check" checked={c.draft.allowHttp} onChange={event => c.change("allowHttp", event.target.checked)} />允许 HTTP 未加密连接（凭据及备份内容可能被截获）</label>
            </div>
          </fieldset>
          <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
            <p className="m-0 text-xs text-muted">连接凭据以本地私有文件保存；修改地址或用户名后请重新填写密码。</p>
            <Button type="submit" size="sm" variant="outline" disabled={c.blocked || !c.connectionDirty}>保存 WebDAV 设置</Button>
          </div>
        </form>
        <div className="flex flex-wrap gap-2 border-t border-[var(--codey-border-subtle)] pt-3.5">
          <Button size="sm" disabled={c.cloudBlocked || options.dirty} onClick={c.askUpload}><IconCloudUpload size={15} />备份到 WebDAV</Button>
          <Button size="sm" variant="outline" disabled={c.cloudBlocked || options.dirty} onClick={() => void c.download()}><IconCloudDownload size={15} />从 WebDAV 恢复</Button>
        </div>
      </section>
    </div>}
    {options.active && c.preview && <Dialog open onOpenChange={open => { if (!open) c.cancelRestore(); }}>
      <DialogContent container={container}>
        <DialogHeader><DialogTitle>恢复这份配置备份？</DialogTitle><DialogDescription>将覆盖当前可恢复的配置。恢复前自动保留一份本地备份；本机账号、插件线路及路径保持不变。</DialogDescription></DialogHeader>
        <dl className="m-0 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 rounded-lg border border-[var(--codey-border-subtle)] bg-[var(--codey-surface-muted)] p-3 text-sm break-words"><dt className="text-muted">来源</dt><dd className="m-0">{c.preview.source}</dd><dt className="text-muted">备份时间</dt><dd className="m-0 tabular-nums">{new Date(c.preview.createdAt).toLocaleString()}</dd><dt className="text-muted">配置内容</dt><dd className="m-0">{c.preview.routeCount} 条普通线路 · {c.preview.channelCount} 个通知渠道</dd></dl>
        <DialogFooter><Button variant="outline" disabled={!!c.busy} onClick={c.cancelRestore}>取消</Button><Button variant="destructive" disabled={c.blocked || options.dirty} onClick={() => void c.restore()}>确认恢复</Button></DialogFooter>
      </DialogContent>
    </Dialog>}
    {options.active && c.exportConfirmation && <Dialog open onOpenChange={open => { if (!open) c.cancelExport(); }}>
      <DialogContent container={container}><DialogHeader><DialogTitle>导出含密钥的配置备份？</DialogTitle><DialogDescription>JSON 未加密，可能包含线路 API Key、请求头和通知凭据。请只保存到可信位置，不要发送给他人或提交到代码仓库。</DialogDescription></DialogHeader><DialogFooter><Button variant="outline" disabled={!!c.busy} onClick={c.cancelExport}>取消</Button><Button disabled={c.blocked || options.dirty} onClick={() => void c.exportBackup()}>确认导出</Button></DialogFooter></DialogContent>
    </Dialog>}
    {options.active && c.uploadConfirmation && <Dialog open onOpenChange={open => { if (!open) c.cancelUpload(); }}>
      <DialogContent container={container}><DialogHeader><DialogTitle>覆盖 WebDAV 云端备份？</DialogTitle><DialogDescription>将上传当前已保存的配置及其中的密钥，替换此地址已有的文件。</DialogDescription></DialogHeader>
        <p className="m-0 rounded-lg bg-[var(--codey-surface-muted)] px-3 py-2 font-mono text-xs break-all">{c.settings?.url}</p><DialogFooter><Button variant="outline" onClick={c.cancelUpload}>取消</Button><Button disabled={c.cloudBlocked || options.dirty} onClick={() => void c.upload()}>确认上传</Button></DialogFooter>
      </DialogContent>
    </Dialog>}
  </section>;
}
