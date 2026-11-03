import { IconAlertTriangle, IconLoader2, IconMessages, IconMessageOff, IconRefresh, IconTrash } from "@tabler/icons-react";
import { Badge, Button, Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Input, Switch } from "../../components/ui";
import { SettingsPageHeader } from "../../SettingsPageHeader";
import { sessionDate, sessionPath } from "./state";
import type { SessionFilter } from "./types";
import { useSessionsController } from "./useSessionsController";

export function SessionsPage({ active, isBusy, onRestart, container }: {
  active: boolean;
  isBusy: boolean;
  onRestart: () => void;
  container?: HTMLElement | null;
}) {
  const remote = window.__codeyRemoteMode === true;
  const c = useSessionsController({ active: active && !remote, isBusy, onRestart });
  if (remote) return <section aria-labelledby="sessions-title"><SettingsPageHeader id="sessions-title" title="会话管理" icon={<IconMessages size={15} />} description="本机会话清理" /><p role="status">会话批量清理与自动清理设置请在本机操作；远程工作区仍可查看与继续会话。</p></section>;
  const pending = c.result?.restartRequired || (c.inventory?.legacyPendingIds !== undefined && !!c.inventory.pendingCount);
  const pendingDetailsUnavailable = c.inventory?.legacyPendingIds === undefined && !!c.inventory?.pendingCount && !pending;
  const legacyConfirmation = c.confirmation?.legacyMissingCount !== undefined;
  return <section className="secondary-section" aria-labelledby="sessions-title" aria-busy={!!c.busy}>
    <SettingsPageHeader id="sessions-title" title="会话管理" icon={<IconMessages size={15} />}
      description="按时间筛选本机会话，手动清理或设置自动清理。" />
    <div className="flex flex-col gap-4">
      <p className="codey-callout m-0" data-tone="warning"><IconAlertTriangle size={15} aria-hidden="true" />
        <span>永久删除会话记录、归档及关联历史缓存，无法撤销。仅处理本机数据；项目文件、账号配置和云端会话不在清理范围内。</span></p>
      {pending && <div role="status" className="codey-callout" data-tone="warning">
        <IconRefresh size={15} aria-hidden="true" />
        <div className="codey-callout-body">
          <p>{c.result?.errors.length ? "部分数据仍待清理。" : "清理请求已保存。"}会话文件尚未删除；请通过 Codey 重启 Codex，在旧客户端停止后完成清理并刷新侧边栏。重启前请先停止正在运行的任务。</p>
          <div><Button size="sm" disabled={!!c.busy || isBusy} onClick={c.restart}>重启 Codex</Button></div>
        </div>
      </div>}
      {pendingDetailsUnavailable && <p role="status" className="codey-callout" data-tone="warning">存在待处理清理任务，但当前后端未提供任务明细。请更新后刷新并核验任务状态，不能仅凭重启认定已清理。</p>}
      {c.error && <p role="alert" className="codey-callout" data-tone="danger">{c.error}</p>}
      {c.notice && <p role="status" className="codey-callout" data-tone="success">{c.notice}</p>}
      {!!c.result?.scheduledCount && <p role="status" className="codey-callout" data-tone="success">已安排 {c.result.scheduledCount} 个会话清理，会话文件尚未删除。</p>}
      {!!c.result?.clearedLegacyCount && <p role="status" className="codey-callout" data-tone="success">已清除 {c.result.clearedLegacyCount} 个会话已不存在的旧版任务标记，未删除任何其他文件。</p>}
      {c.result?.errors.map((error, index) => <p role="alert" key={index} className="codey-callout" data-tone="danger">{error}</p>)}
      {c.legacyPendingIds.length > 0 && <section className="codey-panel" aria-labelledby="session-legacy-title">
        <div className="codey-panel-heading">
          <h3 id="session-legacy-title" className="settings-section-heading">旧版清理任务需重新确认</h3>
          <p>这些任务来源无法确认，已保留且不会自动执行。请逐项选择并重新确认；仅重启 Codex 不会执行这些旧版任务。</p>
        </div>
        <div className="flex max-h-64 flex-col gap-2 overflow-y-auto" aria-label="旧版清理任务列表">
          {c.legacyPendingIds.map(id => {
            const session = c.inventory?.sessions.find(entry => entry.id === id);
            return <label key={id} className="codey-select-row">
              <input type="checkbox" aria-label={`选择旧版清理任务 ${session?.title || id}`} checked={c.legacySelected.has(id)} disabled={c.blocked}
                onChange={() => c.toggleLegacy(id)} className="codey-native-check" />
              <span className="min-w-0 flex-1 space-y-1">
                <strong className="block break-words text-sm font-semibold">{session ? session.title || id : "会话已不存在，仅清除标记"}</strong>
                <span className="block break-all font-mono text-[11px] text-muted">{id}</span>
                {session && <span className="block text-xs text-muted">重新确认后转为手动清理，重启后永久删除。</span>}
              </span>
            </label>;
          })}
        </div>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <span className="text-xs text-muted" aria-live="polite">旧版任务 {c.legacyPendingIds.length} 个 · 已选 {c.legacySelected.size} 个</span>
          <Button size="sm" variant="destructive-light" disabled={c.blocked || !c.legacySelected.size}
            onClick={() => c.reconfirmLegacy([...c.legacySelected])}>确认所选旧版任务</Button>
        </div>
      </section>}
      <section className="codey-panel" aria-labelledby="session-retention-title">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="codey-panel-heading min-w-0 flex-1">
            <h3 id="session-retention-title" className="settings-section-heading">自动清理</h3>
            <p id="session-retention-description">按最后更新时间清理超过保留天数的本机会话，包含归档。启用后立即检查，之后每小时检查并安排清理；实际删除在停止旧客户端后的重启维护阶段完成，无法撤销。</p>
          </div>
          <Switch size="sm" aria-label="启用自动清理" checked={c.policyDraft.enabled} disabled={c.blocked}
            onCheckedChange={value => c.changePolicy("enabled", value)} />
        </div>
        <div className="flex flex-wrap items-end justify-between gap-3">
          <label className="codey-field-label"><span>保留天数</span>
            <div className="flex items-center gap-2"><Input type="number" min={1} max={36500} step={1} inputMode="numeric" aria-label="保留天数"
              aria-describedby="session-retention-description" aria-invalid={!!c.policyProblem} className="w-28" value={c.policyDraft.retentionDays}
              disabled={c.blocked} onChange={event => c.changePolicy("retentionDays", event.target.value)} /><span className="text-xs text-muted">天</span></div>
          </label>
          <Button size="sm" variant="outline" loading={c.busy === "policy"} disabled={c.blocked || !c.policyDirty || !!c.policyProblem}
            onClick={c.askSavePolicy}>保存清理设置</Button>
        </div>
        {c.policyProblem && <p role="alert" className="m-0 text-xs text-[var(--codey-red)]">{c.policyProblem}</p>}
      </section>
      <div className="flex flex-wrap gap-2 items-center">
        <input type="search" aria-label="搜索会话" placeholder="搜索标题、项目路径或会话 ID" value={c.query}
          disabled={c.busy === "delete" || c.busy === "policy"} onChange={event => c.changeQuery(event.target.value)}
          className="input min-w-[200px] flex-1" />
        <select aria-label="会话范围" value={c.filter} disabled={c.busy === "delete" || c.busy === "policy"}
          onChange={event => c.changeFilter(event.target.value as SessionFilter)} className="input codey-native-select w-auto">
          <option value="all">全部会话</option><option value="active">未归档</option><option value="archived">已归档</option>
        </select>
        <select aria-label="时间范围" value={c.olderOnly ? "expired" : "all"} disabled={c.busy === "delete" || c.busy === "policy" || !!c.policyProblem}
          onChange={event => c.changeOlderOnly(event.target.value === "expired")} className="input codey-native-select w-auto">
          <option value="all">全部时间</option><option value="expired">超过 {c.policyDraft.retentionDays || "自定义"} 天</option>
        </select>
        <Button variant="outline" loading={c.busy === "list"} disabled={!!c.busy || isBusy} onClick={c.refresh}>{c.busy !== "list" && <IconRefresh size={14} aria-hidden="true" />}刷新</Button>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <span className="text-xs text-muted tabular-nums" aria-live="polite">共 {c.inventory?.sessions.length ?? 0} 个 · 已选 <strong className={c.selected.size ? "text-[var(--codey-blue)]" : "font-normal"}>{c.selected.size}</strong> 个</span>
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="outline" disabled={c.blocked || !c.visible.length} onClick={c.toggleVisible}>
            {c.visible.length > 0 && c.visible.every(session => c.selected.has(session.id)) ? "取消本页选择" : "选择本页"}</Button>
          <Button size="sm" variant="destructive" disabled={c.blocked || !c.selected.size} onClick={() => c.askRemove(false)}><IconTrash size={14} aria-hidden="true" />清理所选</Button>
          <Button size="sm" variant="destructive-light" disabled={c.blocked || !c.inventory?.sessions.length} onClick={() => c.askRemove(true)}>清空全部（含归档）</Button>
        </div>
      </div>
      {c.busy && <p role="status" className="codey-inline-status"><IconLoader2 size={14} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />{c.busy === "delete" ? "正在保存会话清理请求…" : c.busy === "policy" ? "正在保存清理设置并检查会话…" : "正在读取本机会话…"}</p>}
      {!c.busy && c.inventory && !c.visible.length && <p className="codey-empty-state"><IconMessageOff size={22} stroke={1.6} aria-hidden="true" />{c.inventory.sessions.length ? "没有符合条件的会话。" : "暂无本机会话。"}</p>}
      <div className="flex flex-col gap-2" aria-label="本机会话列表">
        {c.visible.map(session => <label key={session.id} className="codey-select-row">
          <input type="checkbox" aria-label={`选择会话 ${session.title}`} checked={c.selected.has(session.id)} disabled={c.blocked}
            onChange={() => c.toggle(session.id)} className="codey-native-check" />
          <span className="min-w-0 flex-1 space-y-1">
            <span className="flex flex-wrap items-center gap-2"><strong className="text-sm font-semibold break-words min-w-0">{session.title}</strong><Badge variant={session.archived ? "secondary" : "info"}>{session.archived ? "已归档" : "未归档"}</Badge></span>
            <span className="block break-all text-xs text-muted">{sessionPath(session.cwd) || "未记录项目路径"}</span>
            <span className="block break-all text-[11px] text-muted tabular-nums">{sessionDate(session.updatedAt)} · <span className="font-mono">{session.id}</span></span>
          </span>
        </label>)}
      </div>
      {c.filteredCount > 20 && <div className="flex items-center justify-end gap-3 text-xs">
        <Button size="sm" variant="outline" disabled={c.blocked || c.page === 0} onClick={() => c.changePage(c.page - 1)}>上一页</Button>
        <span className="tabular-nums text-muted">{c.page + 1} / {c.pages}</span>
        <Button size="sm" variant="outline" disabled={c.blocked || c.page + 1 === c.pages} onClick={() => c.changePage(c.page + 1)}>下一页</Button>
      </div>}
    </div>
    {active && c.confirmation && <Dialog open onOpenChange={open => { if (!open) c.cancelRemove(); }}>
      <DialogContent container={container} onEscapeKeyDown={event => { if (c.busy === "delete") event.preventDefault(); }}>
        <DialogHeader><DialogTitle>{legacyConfirmation ? "重新确认旧版清理任务？" : c.confirmation.all ? "清空全部本机会话？" : "永久清理所选会话？"}</DialogTitle>
          <DialogDescription>{legacyConfirmation ? <>
            本次选择 {c.confirmation.ids.length} 个旧版任务。
            {c.confirmation.ids.length > (c.confirmation.legacyMissingCount ?? 0) && <>其中 {c.confirmation.ids.length - (c.confirmation.legacyMissingCount ?? 0)} 个现存会话将转为手动清理，确认后仅保存待清理集合；停止旧客户端并重启 Codex 后永久删除其会话内容、归档及关联历史缓存，无法撤销。</>}
            {!!c.confirmation.legacyMissingCount && <>其中 {c.confirmation.legacyMissingCount} 个会话已不存在，仅移除旧版任务标记，不安排以后删除，也不删除任何其他文件。</>}
          </> : <>将永久清理 {c.confirmation.ids.length} 个本机会话，其中归档 {c.confirmation.archivedCount} 个，包括会话内容及关联历史缓存。{c.confirmation.all ? "此操作包含当前筛选条件以外的全部会话。" : ""}无法撤销。确认后仅保存待清理集合；实际文件在停止旧客户端后的重启维护中永久删除。</>}</DialogDescription></DialogHeader>
        <DialogFooter><Button variant="outline" disabled={c.busy === "delete"} onClick={c.cancelRemove}>取消</Button>
          <Button variant="destructive" loading={c.busy === "delete"} onClick={() => void c.remove()}>{legacyConfirmation
            ? c.confirmation.legacyMissingCount === c.confirmation.ids.length ? "确认清除旧标记" : "确认旧版清理任务"
            : "确认永久清理"}</Button></DialogFooter>
      </DialogContent>
    </Dialog>}
    {active && c.policyConfirmation && <Dialog open onOpenChange={open => { if (!open) c.cancelSavePolicy(); }}>
      <DialogContent container={container} onEscapeKeyDown={event => { if (c.busy === "policy") event.preventDefault(); }}>
        <DialogHeader><DialogTitle>启用此自动清理规则？</DialogTitle>
          <DialogDescription>保存后立即检查，之后 Codey 运行期间每小时安排清理最后更新时间超过 {c.policyConfirmation.policy.retentionDays} 天的本机会话，包含归档且不受当前筛选影响。会话内容及关联历史缓存将在停止旧客户端后的重启维护中删除，无法撤销。</DialogDescription></DialogHeader>
        <DialogFooter><Button variant="outline" disabled={c.busy === "policy"} onClick={c.cancelSavePolicy}>取消</Button>
          <Button variant="destructive" loading={c.busy === "policy"} onClick={() => void c.confirmSavePolicy()}>确认启用自动清理</Button></DialogFooter>
      </DialogContent>
    </Dialog>}
  </section>;
}
