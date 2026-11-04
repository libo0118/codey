import { imageUrl, messageUrl, type FileChange, type Message, type Turn } from "./workspace-data";
import { FileLink } from "./files";
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

export function FileChanges({ changes }: { changes: FileChange[] }) {
  return <div className="remote-changes">{changes.map((change, index) => {
    const lines = change.diff.split("\n");
    return <section className="remote-diff" key={`${change.path}:${index}`} aria-label={`文件变更 ${change.path}`}>
      <header><span>{change.path}</span><small>{({ add: "新增", create: "新增", delete: "删除", update: "修改", move: "移动" } as Record<string, string>)[change.kind || ""] || "变更"} · Unified</small></header>
      <pre tabIndex={0} aria-label="统一差异，可横向滚动"><code>{lines.map((line, i) => <span key={i} className={line.startsWith("@@") ? "diff-hunk" : line.startsWith("+") ? "diff-add" : line.startsWith("-") ? "diff-delete" : "diff-context"}>{line}{i < lines.length - 1 ? "\n" : ""}</span>)}</code></pre>
    </section>;
  })}</div>;
}

function Attachments({ message }: { message: Message }) {
  return message.attachments?.map((attachment, index) => {
    const reference = attachment.id || attachment.url || "";
    const dataImage = attachment.url && /^data:image\//i.test(attachment.url) && imageUrl(attachment.url);
    const external = attachment.url && messageUrl(attachment.url);
    return <div className="remote-message-attachment" key={attachment.id || `${attachment.name}:${index}`}>
      {dataImage ? <img src={dataImage} alt={attachment.name} loading="lazy" />
        : external && !attachment.id && !attachment.downloadUrl ? <a href={external} target="_blank" rel="noopener noreferrer">{attachment.name}</a>
        : <FileLink reference={reference} attachment={attachment}>{attachment.name}</FileLink>}
    </div>;
  });
}

function Activity({ message }: { message: Message }) {
  const collaborative = message.kind === "collabAgentToolCall" || message.kind === "subAgentActivity";
  const actions = message.kind === "subAgentActivity" ? lifecycleActions : collaborationActions;
  const collaboration = collaborative && message.collaboration && Object.prototype.hasOwnProperty.call(actions, message.collaboration.action) ? message.collaboration : undefined;
  const known = Object.prototype.hasOwnProperty.call(activityLabels, message.kind) && (!collaborative || Boolean(collaboration));
  return <details className={`remote-activity remote-activity-${known ? message.kind : "unknown"}`} data-message-id={message.id || undefined}>
    <summary><span className="remote-activity-heading"><span>{known ? activityLabels[message.kind] : "未支持的活动"}{collaboration ? ` · ${actions[collaboration.action]}` : known && message.tool && ` · ${message.tool.name}`}</span>{message.status && <small>{statusLabel(message.status)}</small>}</span>{collaboration && <span className="remote-agent-summary">{collaboration.agents.map(agent => <span className="remote-agent-state" key={agent.id}><span>{agent.name || "未命名子代理"}</span><small>{agentStatusLabel(agent.status)}</small></span>)}</span>}</summary>
    {!known ? <p className="remote-content-notice">此活动类型暂不支持，请在电脑端查看。</p>
      : collaboration ? <div className="remote-collaboration">{collaboration.agents.length ? collaboration.agents.map(agent => <section key={agent.id}><h4>{agent.name || "未命名子代理"}</h4>{agent.message?.trim() ? <div className="remote-agent-result" tabIndex={0} role="region" aria-label={`${agent.name || "未命名子代理"}的公开结果，可滚动`}><RichText text={agent.message} /></div> : <p className="remote-content-notice">暂无公开结果。</p>}</section>) : <p className="remote-content-notice">暂无公开子代理状态或结果。</p>}</div>
      : message.kind === "commandExecution" ? message.command ? <><p className="remote-command-cwd">{message.command.cwd}</p><CodeBlock text={message.command.command} language="命令" />{message.command.output && <pre className="remote-command-output" tabIndex={0} aria-label="命令输出，可横向滚动">{message.command.output}</pre>}{message.command.exitCode !== undefined && <p className={message.command.exitCode ? "remote-client-error" : "remote-content-notice"}>退出码 {message.command.exitCode}</p>}</> : <CodeBlock text={message.text} language="命令与输出" />
      : message.kind === "fileChange" ? message.changes?.length ? <FileChanges changes={message.changes} /> : <pre tabIndex={0}>{message.text || "文件变更详情请在电脑端查看。"}</pre>
      : message.kind === "plan" ? message.plan?.length ? <ol className="remote-plan">{message.plan.map((step, index) => <li key={index} data-status={step.status}><span>{step.step}</span><small>{statusLabel(step.status, "待处理")}</small></li>)}</ol> : <RichText text={message.text} />
      : message.kind === "reasoning" ? <RichText text={message.text} />
      : message.tool ? <div className="remote-tool">{message.tool.input && <section><h4>输入</h4><pre tabIndex={0}>{message.tool.input}</pre></section>}{message.tool.output && <section><h4>结果</h4><RichText text={message.tool.output} /></section>}{message.tool.error && <p className="remote-client-error" role="status">{message.tool.error}</p>}{!message.tool.input && !message.tool.output && !message.tool.error && <p className="remote-content-notice">暂无公开结果。</p>}</div>
      : <p className="remote-content-notice">公开结果请在电脑端查看。</p>}
    {known && <Attachments message={message} />}{message.truncated && <p className="remote-content-notice">内容已截断，完整内容请在电脑端查看。</p>}
  </details>;
}

export function ConversationTurn({ turn }: { turn: Turn }) {
  const elapsed = turn.startedAt && turn.completedAt ? Math.max(0, Math.round((turn.completedAt - turn.startedAt) / 1000)) : 0;
  return <section className="remote-turn">{turn.messages.map((message, index) => {
    const key = message.id || `${message.role}:${message.kind}:${index}`;
    if (message.role === "activity") return <Activity key={key} message={message} />;
    if (!textKinds.has(message.kind) || !["user", "assistant", "error"].includes(message.role)) return <p key={key} className="remote-content-notice">未支持的消息，请在电脑端查看。</p>;
    return <article className={`remote-message ${message.role}`} key={key} data-message-id={message.id || undefined} aria-label={message.role === "user" ? "你" : "Codex"}>
      {message.role === "user" ? <div className="remote-user-text">{message.text}</div> : <RichText text={message.text} />}
      <Attachments message={message} />{message.truncated && <p className="remote-content-notice">内容已截断，完整内容请在电脑端查看。</p>}
    </article>;
  })}{elapsed > 0 && <p className="remote-turn-status">用时 {elapsed >= 60 ? `${Math.floor(elapsed / 60)} 分 ` : ""}{elapsed % 60} 秒</p>}{turn.status === "failed" && <p className="remote-client-error" role="alert">任务失败：{typeof turn.error === "string" ? turn.error : "请查看电脑端详情"}</p>}{turn.status === "interrupted" && <p className="remote-turn-status">已停止</p>}</section>;
}
