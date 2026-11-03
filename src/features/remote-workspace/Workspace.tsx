import { useEffect, useRef, useState } from "react";
import { IconArrowDown, IconArrowLeft, IconDeviceLaptop, IconRefresh, IconSquarePlus } from "@tabler/icons-react";
import type { Approval } from "../../remote/workspace-data";
import { Sidebar } from "../../remote/sidebar";
import { Composer } from "../../remote/composer";
import { ConversationTurn, RichText } from "../../remote/messages";
import { useRemoteWorkspaceController } from "./useRemoteWorkspaceController";

export function Workspace({ active, onUnauthorized, onPanel, onLogout }: { active: boolean; onUnauthorized: () => void; onPanel: () => void; onLogout: () => void }) {
  const c = useRemoteWorkspaceController({ active, onUnauthorized });
  const { threads, projects, selected, selectedId, selectedProject, search, archived, project, creating,
    draftScope, draftProject, defaultsReady, composerView, view, connected, connectionError, activeTurn,
    models, busy, loading, error, draft, images, uncertain, catalogError } = c;
  const [atBottom, setAtBottom] = useState(true);
  const scroll = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  useEffect(() => { pinned.current = true; setAtBottom(true); }, [draftScope]);
  useEffect(() => { if (pinned.current && scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight; }, [view]);

  return <div className={`remote-codex-workspace${selected || creating ? " has-selection" : ""}`}>
    <Sidebar threads={threads} projects={projects} selectedId={selectedId} busy={busy} loading={loading} search={search} archived={archived} error={!selected && !creating ? error : ""} onSearch={c.changeSearch} onArchived={c.changeArchived} onOpen={c.open} onCreate={c.startCreating} onRefresh={c.refresh} onPanel={onPanel} onLogout={onLogout} />
    <main className="remote-chat">{!selected && !creating ? <div className="remote-welcome"><IconDeviceLaptop size={36} stroke={1.25} /><h2>今天想做些什么？</h2><p>选择项目或会话，继续电脑上的工作。</p><button className="remote-primary" onClick={() => c.startCreating()}><IconSquarePlus size={18} />新建会话</button></div> : <>
      <header className="remote-chat-header">
        <button className="remote-back remote-icon-button" aria-label="返回会话列表" disabled={busy} onClick={c.back}><IconArrowLeft size={22} /></button>
        <div><h2>{creating ? "新会话" : view?.title || selected?.title}</h2>{creating
          ? <label className="remote-draft-project">项目<select aria-label="项目" value={project} disabled={busy} onChange={event => c.startCreating(projects.find(item => item.id === event.target.value))}>{projects.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>
          : <p><span className={connected ? "remote-chat-status-dot is-connected" : "remote-chat-status-dot"} />{selectedProject?.name || selected?.cwd.split(/[\\/]/).filter(Boolean).pop() || "本地电脑"}<span>·</span>{connected ? activeTurn ? "正在工作" : "已连接" : connectionError ? "未连接" : "连接中"}</p>}
        </div>
        <button className="remote-icon-button" disabled={busy} aria-label="新建会话" onClick={() => c.startCreating(selectedProject)}><IconSquarePlus size={20} /></button>
        {!creating && <button className="remote-icon-button" disabled={busy} aria-label="重新连接" onClick={c.reconnect}><IconRefresh size={18} /></button>}
      </header>
      {connectionError && <p className="remote-connection" role="status">{connectionError}</p>}
      <div className="remote-transcript">
        <div className="remote-messages" ref={scroll} onScroll={event => { const element = event.currentTarget; pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 100; setAtBottom(pinned.current); }}>
          {view && !view.historyComplete && <button disabled={busy || !connected} onClick={() => void c.loadHistory()}>加载完整历史</button>}
          {!creating && !view && <p className="remote-chat-loading">{connectionError ? "暂时无法读取会话，请点击重新连接。" : "正在获取桌面会话…"}</p>}
          {(creating || view?.turns.length === 0) && <div className="remote-chat-empty"><h2>从一个想法开始</h2><p>{creating && !draftProject ? "请先在电脑 Codex 中保存本地项目。" : `描述任务，让 Codex 在 ${draftProject && creating ? draftProject.name : selectedProject?.name || "当前工作区"} 中开始工作。`}</p></div>}
          {view?.turns.map((turn, index) => <ConversationTurn key={turn.id || index} turn={turn} />)}
          {view?.requests.map(request => <ApprovalCard key={String(request.id)} request={request} disabled={busy || !connected} onRespond={args => c.respond(request.id, args)} />)}
          {activeTurn && <p className="remote-working" role="status"><span className="remote-chat-status-dot is-connected" />Codex 正在工作…</p>}
        </div>
        {!atBottom && <button className="remote-jump remote-icon-button" aria-label="滚动到最新消息" onClick={() => { if (scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight; pinned.current = true; setAtBottom(true); }}><IconArrowDown size={20} /></button>}
      </div>
      {error && <p className="remote-client-error" role="alert">{error}</p>}
      {uncertain && !busy && <div className="remote-uncertain"><p>消息未确认。请查看会话是否已收到，确认后再决定是否重新发送。</p><button onClick={c.confirmSubmission}>已核对，允许再次发送</button></div>}
      <Composer key={draftScope} draft={draft} onDraft={c.changeDraft} images={images} onImages={c.changeImages} view={composerView} models={models} connected={creating ? !!draftProject && defaultsReady : connected} busy={busy} uncertain={uncertain}
        onSend={event => { event.preventDefault(); void c.send().then(started => { if (started) pinned.current = true; }); }}
        onStop={() => void c.stop()} onSettings={c.changeSettings} catalogError={catalogError} onReloadModels={c.reloadModels} />
    </>}</main>
  </div>;
}

function ApprovalCard({ request, disabled, onRespond }: { request: Approval; disabled: boolean; onRespond: (args: Record<string, unknown>) => Promise<unknown> }) {
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [revision, setRevision] = useState("");
  if (!request.supported) return <section className="remote-approval"><p>{request.params.message || "请在电脑端处理此请求"}</p></section>;
  if (request.method === "item/plan/requestImplementation") return <section className="remote-approval"><h3>确认工作计划</h3><RichText text={request.params.planContent || ""} /><label>修改意见<textarea value={revision} disabled={disabled} onChange={event => setRevision(event.target.value)} /></label><div><button className="remote-primary" disabled={disabled || !request.params.planContent} onClick={() => void onRespond({ decision: "implement" })}>执行计划</button><button disabled={disabled || !revision.trim()} onClick={() => void onRespond({ decision: "revise", text: revision })}>修改计划</button></div></section>;
  if (request.params.questions) return <form className="remote-approval" onSubmit={event => { event.preventDefault(); void onRespond({ answers }); }}><h3>Codex 需要你的回答</h3>{request.params.questions.map(question => <label key={question.id}>{question.question}{question.options?.map(option => <button key={option.label} type="button" disabled={disabled} onClick={() => setAnswers(current => ({ ...current, [question.id]: option.label }))}>{option.label}{option.description && <small>{option.description}</small>}</button>)}<input required disabled={disabled} value={answers[question.id] || ""} onChange={event => setAnswers(current => ({ ...current, [question.id]: event.target.value }))} /></label>)}<button disabled={disabled} className="remote-primary">提交回答</button></form>;
  const available = request.params.availableDecisions || ["accept", "decline"];
  return <section className="remote-approval"><h3>等待你的批准</h3><p>{request.params.reason}</p>{request.params.command && <pre>{request.params.command}</pre>}{request.params.permissions !== undefined && <pre>{JSON.stringify(request.params.permissions, null, 2)}</pre>}{request.params.changes !== undefined && <pre>{JSON.stringify(request.params.changes, null, 2)}</pre>}<div>{available.includes("accept") && <button disabled={disabled} className="remote-primary" onClick={() => void onRespond({ decision: "accept" })}>本次允许</button>}{available.includes("decline") && <button disabled={disabled} onClick={() => void onRespond({ decision: "decline" })}>拒绝</button>}{available.includes("cancel") && <button disabled={disabled} onClick={() => void onRespond({ decision: "cancel" })}>取消</button>}</div></section>;
}
