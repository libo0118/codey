// Cindy ContextSheetGoalView.tsx visual reference at deef17354321527fd857acf62662955139ef2491, Apache-2.0.
import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { IconArrowLeft, IconChevronDown, IconChevronRight, IconPencil, IconPlayerPause, IconPlayerPlay, IconRefresh, IconTarget } from "@tabler/icons-react";
import "./goals.css";
import type { Goal } from "../goals";
export type { Goal } from "../goals";

export type GoalSurfaceProps = {
  goal: Goal | null | undefined;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  connected: boolean;
  busy: boolean;
  running: boolean;
  error: string;
  uncertain: boolean;
  /** true 表示读取成功；状态仍以 goal prop 为准。 */
  onRead: () => Promise<boolean>;
  onCommand: (command: "create" | "edit" | "pause" | "resume", args: { objective?: string; tokenBudget?: number | null }) => Promise<boolean>;
  onConfirmUnknown: () => void;
};

const statusLabels: Record<Goal["status"], string> = {
  active: "目标进行中", paused: "目标已暂停", blocked: "目标等待处理",
  usageLimited: "目标用量受限", budgetLimited: "目标预算受限", complete: "目标已完成",
};

function trapFocus(event: KeyboardEvent<HTMLDialogElement>) {
  if (event.key !== "Tab") return;
  const controls = [...event.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), textarea:not(:disabled), summary, [tabindex]:not([tabindex="-1"])')]
    .filter(element => element.getClientRects().length > 0);
  const first = controls[0], last = controls[controls.length - 1];
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
}

export function GoalSurface({ goal, open, onOpenChange, connected, busy, running, error, uncertain, onRead, onCommand, onConfirmUnknown }: GoalSurfaceProps) {
  const [mode, setMode] = useState<"detail" | "create" | "edit">(goal === null ? "create" : "detail");
  const [objective, setObjective] = useState("");
  const [budget, setBudget] = useState("");
  const [reading, setReading] = useState(false);
  const [sending, setSending] = useState(false);
  const [readFailed, setReadFailed] = useState(false);
  const [readSucceeded, setReadSucceeded] = useState(false);
  const [localError, setLocalError] = useState("");
  const pendingRead = useRef(false), pendingCommand = useRef(false);
  const dialogRef = useRef<HTMLDialogElement>(null), editorRef = useRef<HTMLTextAreaElement>(null);
  const id = useId();
  const working = busy || sending || reading;
  const unconfirmed = goal === undefined || readFailed || (goal === null && !!error && !readSucceeded);
  const locked = !connected || working || uncertain || unconfirmed;
  const canPause = !!goal && ["active", "blocked", "usageLimited", "budgetLimited"].includes(goal.status);
  const canResume = goal?.status === "paused";
  const editing = mode === "edit" && goal?.status === "paused";
  const canCreate = goal === null || goal?.status === "complete";
  const creating = mode === "create" && canCreate && !uncertain && !unconfirmed;
  const parsedBudget = budget === "" ? null : Number(budget);
  const budgetValid = parsedBudget === null || (Number.isSafeInteger(parsedBudget) && parsedBudget > 0);
  const status = uncertain ? "目标状态待核对" : goal ? statusLabels[goal.status] : !unconfirmed ? "设置目标" : "目标状态未确认";

  useEffect(() => {
    if (!open) return;
    setMode(current => current === "edit" ? current : goal === null ? "create" : "detail");
    void read();
    const dialog = dialogRef.current!;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // showModal 让背景 inert，并将面板放入 top layer，避免 composer 的 overflow 裁切。
    dialog.showModal();
    return () => { dialog.close(); if (previous?.isConnected) previous.focus(); };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    if (editing) editorRef.current?.focus();
    else dialogRef.current?.querySelector<HTMLButtonElement>('[data-cindy-control="goal-back"]')?.focus();
  }, [open, mode, editing]);

  async function read() {
    if (!connected || pendingRead.current || pendingCommand.current || busy) return;
    pendingRead.current = true; setReading(true); setLocalError("");
    try {
      const ok = await onRead();
      setReadFailed(!ok);
      setReadSucceeded(ok);
      if (!ok) setLocalError("无法确认目标状态，请重新读取。");
    } catch (cause) {
      setReadFailed(true); setLocalError(`读取目标失败：${cause instanceof Error ? cause.message : String(cause)}`);
    } finally { pendingRead.current = false; setReading(false); }
  }

  async function command(action: "create" | "edit" | "pause" | "resume") {
    if (locked || pendingCommand.current || pendingRead.current) return;
    if ((action === "create" && (!creating || running)) || (action === "resume" && (!canResume || running)) ||
      (action === "pause" && !canPause) || (action === "edit" && !editing)) return;
    if ((action === "create" || action === "edit") && (!objective.trim() || !budgetValid)) return;
    pendingCommand.current = true; setSending(true); setLocalError("");
    try {
      const args = action === "create" || action === "edit" ? { objective: objective.trim(), tokenBudget: parsedBudget } : {};
      if (await onCommand(action, args)) setMode("detail");
    } catch (cause) {
      setLocalError(`目标操作失败：${cause instanceof Error ? cause.message : String(cause)}`);
    } finally { pendingCommand.current = false; setSending(false); }
  }

  function edit() {
    if (locked || goal?.status !== "paused") return;
    setObjective(goal.objective); setBudget(goal.tokenBudget == null ? "" : String(goal.tokenBudget)); setMode("edit");
  }

  const toggle = canPause ? "pause" : "resume";
  const toggleLabel = canPause ? "暂停目标" : "继续目标";
  const toggleDisabled = locked || (!canPause && running);
  const ToggleIcon = canPause ? IconPlayerPause : IconPlayerPlay;
  const showBar = uncertain || (!!goal && goal.status !== "complete");
  if (!showBar && !open) return null;
  return <>
    {showBar && <div className="cindy-goal-surface">
      <div className="cindy-goal-bar" data-goal-status={uncertain || unconfirmed ? "unknown" : goal?.status || "none"} aria-busy={working}>
        <button type="button" data-cindy-control="goal-open" className="cindy-goal-open" aria-haspopup="dialog" aria-expanded={open} onClick={() => onOpenChange(true)}>
          <IconTarget size={19} aria-hidden="true" /><span className="cindy-goal-copy"><strong>{status}</strong>{goal && <span title={goal.objective}>{goal.objective}</span>}</span>
        </button>
        {(canPause || canResume) && <button type="button" data-cindy-control="goal-toggle" className="cindy-goal-icon" aria-label={toggleLabel} disabled={toggleDisabled} onClick={() => void command(toggle)}><ToggleIcon size={18} aria-hidden="true" /></button>}
        <button type="button" data-cindy-control="goal-details" className="cindy-goal-icon" aria-label="查看目标详情" aria-haspopup="dialog" onClick={() => onOpenChange(true)}><IconChevronRight size={18} aria-hidden="true" /></button>
      </div>
    </div>}
    {open && <dialog ref={dialogRef} className="cindy-goal-sheet" aria-modal="true" aria-labelledby={`${id}-title`} aria-busy={working}
      onKeyDown={trapFocus} onCancel={event => { event.preventDefault(); onOpenChange(false); }} onClose={() => onOpenChange(false)}>
      <div className="cindy-goal-handle" aria-hidden="true" />
      <div className="cindy-goal-heading"><button type="button" data-cindy-control="goal-back" className="cindy-goal-icon" aria-label={editing ? "返回目标详情" : "返回对话"} onClick={() => editing ? setMode("detail") : onOpenChange(false)}><IconArrowLeft size={20} aria-hidden="true" /></button><h2 id={`${id}-title`}>{editing ? "修改目标" : creating ? "新建目标" : "会话目标"}</h2></div>
      {uncertain && <div className="cindy-goal-notice" role="status"><p>上次操作结果未知。请先在桌面核对目标状态和启动消息；不会自动重发。</p><button type="button" data-cindy-control="goal-confirm" className="cindy-goal-row" disabled={working} onClick={onConfirmUnknown}>已在桌面核对</button></div>}
      {creating || editing ? <form onSubmit={event => { event.preventDefault(); void command(editing ? "edit" : "create"); }}>
        <label className="cindy-goal-label" htmlFor={`${id}-objective`}>{editing ? "目标内容" : "希望 Codex 持续完成什么？"}</label>
        <textarea ref={editorRef} id={`${id}-objective`} className="cindy-goal-editor" value={objective} required disabled={working} onChange={event => setObjective(event.target.value)} placeholder="例如：补齐远程目标管理，并验证各状态与操作。" />
        <details className="cindy-goal-budget"><summary>高级设置<IconChevronDown size={17} aria-hidden="true" /></summary><label htmlFor={`${id}-budget`}>Token 预算<input id={`${id}-budget`} type="number" inputMode="numeric" min="1" step="1" max={Number.MAX_SAFE_INTEGER} value={budget} disabled={working} aria-invalid={!budgetValid} placeholder="不限" onChange={event => setBudget(event.target.value)} /></label><p className="cindy-goal-hint">留空表示不设置 Token 预算。</p></details>
        <p className="cindy-goal-hint" id={`${id}-warning`}>{editing ? "保存修改会重置目标用量统计，并保持暂停；继续执行需再次明确点击。" : "创建后使用当前会话的模型和权限执行，会消耗 Token，可随时暂停。"}</p>
        {running && !editing && <p className="cindy-goal-hint">当前回复进行中，结束后才能创建并开始。</p>}
        <button type="submit" data-cindy-control="goal-save" className="cindy-goal-primary" aria-describedby={`${id}-warning`} disabled={locked || (!editing && running) || !objective.trim() || !budgetValid}>{sending ? "正在保存…" : editing ? "保存修改" : "创建并开始"}</button>
      </form> : goal ? <>
        <p className="cindy-goal-label" data-goal-status={goal.status}><span className="cindy-goal-dot" aria-hidden="true" />{statusLabels[goal.status]}</p>
        <p className="cindy-goal-objective">{goal.objective}</p>
        <div className="cindy-goal-meta"><span>{goal.tokensUsed === undefined ? "Token 用量尚未提供" : `已用 ${goal.tokensUsed.toLocaleString("zh-CN")}${goal.tokenBudget == null ? "" : ` / ${goal.tokenBudget.toLocaleString("zh-CN")}`} Token`}</span><span>{goal.timeUsedSeconds === undefined ? "耗时尚未提供" : `已运行 ${Math.floor(goal.timeUsedSeconds / 60)} 分 ${Math.floor(goal.timeUsedSeconds % 60)} 秒`}</span></div>
        <div className="cindy-goal-actions">{(canPause || canResume) && <button type="button" data-cindy-control="goal-toggle-detail" className="cindy-goal-row" disabled={toggleDisabled} onClick={() => void command(toggle)}><ToggleIcon size={19} aria-hidden="true" /><span>{toggleLabel}</span></button>}
          {goal.status !== "complete" && <button type="button" data-cindy-control="goal-edit" className="cindy-goal-row" disabled={locked || goal.status !== "paused"} onClick={edit}><IconPencil size={19} aria-hidden="true" /><span>修改目标</span></button>}</div>
        <p className="cindy-goal-hint">{goal.status === "complete" ? "目标已完成，可以新建下一项目标。" : `${canPause ? "修改前请先暂停。" : "保存修改后保持暂停。"}暂停目标不会停止当前回复。`}{running && canResume ? "当前回复结束后才能继续目标。" : ""}</p>
      </> : <p className="cindy-goal-hint">{unconfirmed ? "尚未确认目标状态；读取成功前不能创建目标。" : "当前会话尚未设置目标。"}</p>}
      {!creating && !editing && canCreate && !uncertain && !unconfirmed && <button type="button" data-cindy-control="goal-create" className="cindy-goal-row" disabled={locked} onClick={() => { setObjective(""); setBudget(""); setMode("create"); }}><IconTarget size={19} aria-hidden="true" />{goal ? "新建目标" : "设置目标"}</button>}
      <button type="button" data-cindy-control="goal-refresh" className="cindy-goal-row cindy-goal-refresh" disabled={!connected || working} onClick={() => void read()}><IconRefresh size={18} aria-hidden="true" /><span>{reading ? "正在读取目标…" : "重新读取目标状态"}</span></button>
      {!connected && <p className="cindy-goal-hint" role="status">连接断开；恢复连接后才能读取或操作目标。</p>}
      {(localError || error) && <p className="cindy-goal-error" role="alert">{localError || error}</p>}
    </dialog>}
  </>;
}
