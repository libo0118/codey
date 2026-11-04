export const startedActivity = {
  id: "native-started", kind: "subAgentActivity", role: "activity", text: "", status: null, attachments: [],
  collaboration: { action: "started", agents: [{ id: "review", name: "审查消息展示", status: "running" }] },
};

export const completedActivity = {
  id: "collab-wait", kind: "collabAgentToolCall", role: "activity", text: "", status: "completed", attachments: [],
  collaboration: { action: "wait", agents: [
    { id: "review", name: "审查消息展示", status: "completed", message: "## 公开审查结果\n\n**检查完成**：消息顺序与单栏布局保持不变。\n\n```ts\nconst result = 'public';\n```" },
    { id: "layout", name: "检查布局", status: "running" },
  ] },
};

export const activityMessages = [
  startedActivity,
  { id: "inspect", kind: "mcpToolCall", role: "activity", status: "completed", text: "", tool: { name: "inspect_local_file", input: "src/remote/messages.tsx", output: "已读取公开文件内容（Mock）。" } },
  { id: "middle", kind: "agentMessage", role: "assistant", text: "工具和子代理活动按原消息顺序展示（Mock）。" },
  { id: "glob", kind: "mcpToolCall", role: "activity", status: "completed", text: "", tool: { name: "glob", input: "src/remote/*.tsx", output: "已匹配文件路径（Mock）。" } },
  { id: "multi-start", kind: "collabAgentToolCall", role: "activity", status: "completed", text: "", collaboration: { action: "spawnAgent", agents: [
    { id: "files", name: "实现远程文件活动展示", status: "running" },
    { id: "layout", name: "检查布局", status: "running" },
  ] } },
  { id: "fastctx", kind: "mcpToolCall", role: "activity", status: "completed", text: "", tool: { name: "mcp__codey_fastctx__grep", input: "remote-activity", output: "已使用 Codey Fastctx 集成（Mock）。" } },
  completedActivity,
  { id: "native-completed", kind: "subAgentActivity", role: "activity", status: null, text: "", collaboration: { action: "completed", agents: [{ id: "files", name: "实现远程文件活动展示", status: "completed" }] } },
];
