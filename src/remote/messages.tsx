import { memo, useMemo, useState } from "react";
import { imageUrl, messageUrl, type FileChange, type Message, type Turn } from "./workspace-data";
import { FileImage, FileLink } from "./files";
import { CodeBlock, RichText } from "./markdown";
export { RichText } from "./markdown";

const activityLabels: Record<string, string> = { commandExecution: "执行命令", fileChange: "文件变更", reasoning: "公开思考摘要", mcpToolCall: "MCP 工具", dynamicToolCall: "调用工具", webSearch: "搜索网页", plan: "工作计划", collabAgentToolCall: "协作任务", subAgentActivity: "子代理" };
const statusLabels: Record<string, string> = { inProgress: "进行中", in_progress: "进行中", pending: "待处理", completed: "已完成", failed: "失败", interrupted: "已停止", declined: "已拒绝" };
const statusLabel = (status: string | null | undefined, fallback = "状态未知") => status && Object.prototype.hasOwnProperty.call(statusLabels, status) ? statusLabels[status] : fallback;
const collaborationActions: Record<string, string> = { spawnAgent: "开始工作", sendInput: "发送输入", resumeAgent: "恢复工作", wait: "等待结果", closeAgent: "关闭子代理", sendMessage: "发送消息", followupTask: "追加任务", interruptAgent: "中止工作", listAgents: "查看子代理" };
const lifecycleActions: Record<string, string> = { started: "开始工作", interacted: "交互", completed: "已完成", interrupted: "已中止" };
const agentStatuses: Record<string, string> = { pendingInit: "初始化中", running: "运行中", interrupted: "已中止", completed: "已完成", errored: "失败", shutdown: "已关闭", notFound: "未找到" };
const agentStatusLabel = (status: string | undefined) => status === undefined ? "未提供状态" : Object.prototype.hasOwnProperty.call(agentStatuses, status) ? agentStatuses[status] : "状态未知";
const textKinds = new Set(["userMessage", "steeringUserMessage", "agentMessage", "assistantMessage", "planImplementation", "error"]);

function UserText({ text }: { text: string }) {
  let body = text, context = "";
  const trimmed = text.trim();
  const marker = /\r?\n## My request:\r?\n/.exec(trimmed);
  if (marker && (trimmed.startsWith('# Files mentioned by the user:') || trimmed.startsWith('<in-app-browser-context source="ambient-ui-state">'))) {
    context = trimmed.slice(0, marker.index).trim();
    body = trimmed.slice(marker.index + marker[0].length).trim();
  }
  let replies: { questionItemId: string; question: string; answer: string }[] | undefined;
  const encoded = /^<send_user_message_question_reply>\s*([\s\S]*)\s*<\/send_user_message_question_reply>$/.exec(body.trim());
  if (encoded) {
    try {
      const value: unknown = JSON.parse(encoded[1]);
      const entries = Array.isArray(value) ? value : [value];
      if (entries.length && entries.every(entry => entry && typeof entry === "object" && [entry.questionItemId, entry.question, entry.answer].every(part => typeof part === "string"))) replies = entries;
    } catch { /* 未识别的消息保留原文。 */ }
  }
  return <>{replies ? replies.map((reply, index) => <div className="remote-question-reply" key={`${reply.questionItemId}:${index}`}><div className="remote-reply-question" title={reply.question}>{reply.question}</div><div className="remote-user-text">{reply.answer}</div></div>) : body && <div className="remote-user-text">{body}</div>}{context && <details className="remote-user-context"><summary>附加上下文</summary><pre>{context}</pre></details>}</>;
}

export const FileChanges = memo(function FileChanges({ changes }: { changes: FileChange[] }) {
  return <div className="remote-changes">{changes.map((change, index) => {
    const lines = change.diff.split("\n");
    return <section className="remote-diff" key={`${change.path}:${index}`} aria-label={`文件变更 ${change.path}`}>
      <header><span>{change.path}</span><small>{({ add: "新增", create: "新增", delete: "删除", update: "修改", move: "移动" } as Record<string, string>)[change.kind || ""] || "变更"} · Unified</small></header>
      <pre tabIndex={0} aria-label="统一差异，可横向滚动"><code>{lines.map((line, i) => <span key={i} className={line.startsWith("@@") ? "diff-hunk" : line.startsWith("+") ? "diff-add" : line.startsWith("-") ? "diff-delete" : "diff-context"}>{line}{i < lines.length - 1 ? "\n" : ""}</span>)}</code></pre>
    </section>;
  })}</div>;
});

function Attachments({ message }: { message: Message }) {
  return message.attachments?.map((attachment, index) => {
    const reference = attachment.id || attachment.url || "";
    const dataImage = attachment.url && /^data:image\//i.test(attachment.url) && imageUrl(attachment.url);
    const external = attachment.url && messageUrl(attachment.url);
    return <div className="remote-message-attachment" key={attachment.id || `${attachment.name}:${index}`}>
      {dataImage ? <FileImage url={dataImage} alt={attachment.name} />
        : external && !attachment.id && !attachment.downloadUrl ? <a href={external} target="_blank" rel="noopener noreferrer">{attachment.name}</a>
        : <FileLink reference={reference} attachment={attachment}>{attachment.name}</FileLink>}
    </div>;
  });
}

const Activity = memo(function Activity({ message }: { message: Message }) {
  const [expanded, setExpanded] = useState(false);
  const collaborative = message.kind === "collabAgentToolCall" || message.kind === "subAgentActivity";
  const actions = message.kind === "subAgentActivity" ? lifecycleActions : collaborationActions;
  const collaboration = collaborative && message.collaboration && Object.prototype.hasOwnProperty.call(actions, message.collaboration.action) ? message.collaboration : undefined;
  const known = Object.prototype.hasOwnProperty.call(activityLabels, message.kind) && (!collaborative || Boolean(collaboration));
  return <details className={`remote-activity remote-activity-${known ? message.kind : "unknown"}`} data-message-id={message.id || undefined} onToggle={event => { if (event.target === event.currentTarget) setExpanded(event.currentTarget.open); }}>
    <summary><span className="remote-activity-heading"><span>{known ? activityLabels[message.kind] : "未支持的活动"}{collaboration ? ` · ${actions[collaboration.action]}` : known && message.tool && ` · ${message.tool.name}`}</span>{message.status && <small>{statusLabel(message.status)}</small>}</span>{collaboration && <span className="remote-agent-summary">{collaboration.agents.map(agent => <span className="remote-agent-state" key={agent.id}><span>{agent.name || "未命名子代理"}</span><small>{agentStatusLabel(agent.status)}</small></span>)}</span>}</summary>
    {(!known || expanded) && <>{!known ? <p className="remote-content-notice">此活动类型暂不支持，请在电脑端查看。</p>
      : collaboration ? <div className="remote-collaboration">{collaboration.agents.length ? collaboration.agents.map(agent => <section key={agent.id}><h4>{agent.name || "未命名子代理"}</h4>{agent.message?.trim() ? <div className="remote-agent-result" tabIndex={0} role="region" aria-label={`${agent.name || "未命名子代理"}的公开结果，可滚动`}><RichText text={agent.message} /></div> : <p className="remote-content-notice">暂无公开结果。</p>}</section>) : <p className="remote-content-notice">暂无公开子代理状态或结果。</p>}</div>
      : message.kind === "commandExecution" ? message.command ? <><p className="remote-command-cwd">{message.command.cwd}</p><CodeBlock text={message.command.command} language="命令" />{message.command.output && <pre className="remote-command-output" tabIndex={0} aria-label="命令输出，可横向滚动">{message.command.output}</pre>}{message.command.exitCode !== undefined && <p className={message.command.exitCode ? "remote-client-error" : "remote-content-notice"}>退出码 {message.command.exitCode}</p>}</> : <CodeBlock text={message.text} language="命令与输出" />
      : message.kind === "fileChange" ? message.changes?.length ? <FileChanges changes={message.changes} /> : <pre tabIndex={0}>{message.text || "文件变更详情请在电脑端查看。"}</pre>
      : message.kind === "plan" ? message.plan?.length ? <ol className="remote-plan">{message.plan.map((step, index) => <li key={index} data-status={step.status}><span>{step.step}</span><small>{statusLabel(step.status, "待处理")}</small></li>)}</ol> : <RichText text={message.text} />
      : message.kind === "reasoning" ? message.text.trim() ? <RichText text={message.text} /> : <p className="remote-content-notice">暂无公开思考摘要。</p>
      : message.tool ? <div className="remote-tool">{message.tool.input && <section><h4>输入</h4><pre tabIndex={0}>{message.tool.input}</pre></section>}{message.tool.output && <section><h4>结果</h4><RichText text={message.tool.output} /></section>}{message.tool.error && <p className="remote-client-error" role="status">{message.tool.error}</p>}{!message.tool.input && !message.tool.output && !message.tool.error && <p className="remote-content-notice">暂无公开结果。</p>}</div>
      : <p className="remote-content-notice">公开结果请在电脑端查看。</p>}
    {known && <Attachments message={message} />}{message.truncated && <p className="remote-content-notice">内容已截断，完整内容请在电脑端查看。</p>}</>}
  </details>;
});

const ActivityGroup = memo(function ActivityGroup({ messages, working }: { messages: Message[]; working: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const visible = messages.filter(message => message.kind !== "reasoning" || message.text.trim() || message.attachments?.length);
  return <details className="remote-process" data-message-id={messages[0].id || undefined} onToggle={event => { if (event.target === event.currentTarget) setExpanded(event.currentTarget.open); }}>
    <summary>{working ? "正在思考" : "工作过程"}{visible.length > 0 && <small> · {visible.length} 项活动</small>}</summary>
    {expanded && <div className="remote-process-body">{visible.length ? visible.map((message, index) => <Activity key={message.id || `${message.kind}:${index}`} message={message} />) : <p className="remote-content-notice">暂无公开思考摘要，原始思考内容不在远程端显示。</p>}</div>}
  </details>;
});

const ConversationMessage = memo(function ConversationMessage({ message }: { message: Message }) {
  if (!textKinds.has(message.kind) || !["user", "assistant", "error"].includes(message.role)) return <p className="remote-content-notice" data-message-id={message.id || undefined}>未支持的消息，请在电脑端查看。</p>;
  return <article className={`remote-message ${message.role}`} data-message-id={message.id || undefined} aria-label={message.role === "user" ? "你" : "Codex"}>
    {message.role === "user" ? <UserText text={message.text} /> : <RichText text={message.text} />}
    <Attachments message={message} />{message.truncated && <p className="remote-content-notice">内容已截断，完整内容请在电脑端查看。</p>}
  </article>;
});

export const ConversationTurn = memo(function ConversationTurn({ turn }: { turn: Turn }) {
  const groups = useMemo(() => {
    const groups: (Message | Message[])[] = [];
    for (const message of turn.messages) {
      const previous = groups[groups.length - 1];
      if (message.role !== "activity") groups.push(message);
      else if (Array.isArray(previous)) previous.push(message);
      else groups.push([message]);
    }
    return groups;
  }, [turn.messages]);
  if (!turn.messages.length) return null;
  const elapsed = turn.startedAt && turn.completedAt ? Math.max(0, Math.round((turn.completedAt - turn.startedAt) / 1000)) : 0;
  return <section className="remote-turn">{groups.map((message, index) => {
    if (Array.isArray(message)) return <ActivityGroup key={message[0].id || `activity:${index}`} messages={message} working={turn.status === "inProgress" && index === groups.length - 1} />;
    const key = message.id || `${message.role}:${message.kind}:${index}`;
    return <ConversationMessage key={key} message={message} />;
  })}{elapsed > 0 && <p className="remote-turn-status">用时 {elapsed >= 60 ? `${Math.floor(elapsed / 60)} 分 ` : ""}{elapsed % 60} 秒</p>}{turn.status === "failed" && <p className="remote-client-error" role="alert">任务失败：{typeof turn.error === "string" ? turn.error : "请查看电脑端详情"}</p>}{turn.status === "interrupted" && <p className="remote-turn-status">已停止</p>}</section>;
});
