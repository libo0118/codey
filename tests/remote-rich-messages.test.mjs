import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "@tabler/icons-react";
import MarkdownIt from "markdown-it";
import { renderToStaticMarkup } from "react-dom/server";
import { createModuleGraph } from "./helpers/jsx-tree.mjs";

const real = { react: React, "react/jsx-runtime": jsx, "@tabler/icons-react": icons, "markdown-it": MarkdownIt };
const graph = createModuleGraph(new URL("../src/remote/messages.tsx", import.meta.url), { stubs: real });
const { RichText, ConversationTurn } = graph.exports;
const render = (Component, props) => renderToStaticMarkup(React.createElement(Component, props));

test("真实 Markdown 组件呈现标题、嵌套列表、表格、行内代码和围栏", () => {
  const html = render(RichText, { text: "## 结果\n\n- 第一项 **完成**\n  - 子项 `ok`\n\n| 文件 | 状态 |\n| --- | --- |\n| a.ts | 成功 |\n\n```ts\nconst answer = 42;\n```" });
  assert.match(html, /<h2>结果<\/h2>/);
  assert.match(html, /<strong>完成<\/strong>/);
  assert.match(html, /<table>/);
  assert.match(html, /<code>ok<\/code>/);
  assert.match(html, /class="[^"]*code-copy/);
});

test("交错活动保留真实消息顺序和独立 identity，不把后续活动移到首项", () => {
  const messages = [
    { id: "a", role: "activity", kind: "commandExecution", text: "先执行" },
    { id: "b", role: "assistant", kind: "agentMessage", text: "中间回答" },
    { id: "c", role: "activity", kind: "reasoning", text: "后续公开摘要" },
    { id: "d", role: "assistant", kind: "agentMessage", text: "最终回答" },
  ];
  const html = render(ConversationTurn, { turn: { id: "turn", status: "completed", messages } });
  const order = messages.map(message => html.indexOf(message.text));
  assert.ok(order.every((position, index) => position >= 0 && (!index || position > order[index - 1])), `实际位置 ${order}`);
});

test("不安全 URL、原始 HTML 和未知活动都不会成为可执行节点或 JSON dump", () => {
  const html = render(RichText, { text: '<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n\n[x](javascript:alert(1)) [y](data:text/html,hi) [z](https://user:password@example.invalid/) [普通链接](https://example.invalid/) [邮件](mailto:team@example.invalid)' });
  assert.doesNotMatch(html, /<(?:script|img)\b|href="(?:javascript:|data:|https:\/\/user:)/i);
  assert.match(html, /&lt;script&gt;/);
  assert.match(html, /href="https:\/\/example.invalid\/"/);
  assert.match(html, /href="mailto:team@example.invalid"/);
  for (const kind of ["futurePrivateItem", "toString", "constructor"]) {
    const unknown = render(ConversationTurn, { turn: { id: "turn", status: "completed", messages: [{ id: "private", role: "activity", kind, text: '{"secret":"NEVER_DUMP"}', tool: { name: "NEVER_DUMP" } }] } });
    assert.match(unknown, /未支持的活动/);
    assert.doesNotMatch(unknown, /NEVER_DUMP/);
  }
});

test("公开命令、工具、计划、统一 Diff、图片与截断提示按类型呈现", () => {
  const messages = [
    { id: "cmd", role: "activity", kind: "commandExecution", status: "completed", text: "", command: { command: "node test.js", cwd: "D:/Mock", output: "2 passed", exitCode: 0 } },
    { id: "tool", role: "activity", kind: "mcpToolCall", status: "failed", text: "", tool: { name: "lookup", input: "public query", output: "**公开结果**", error: "Mock tool failed" } },
    { id: "plan", role: "activity", kind: "plan", text: "", plan: [{ step: "检查", status: "completed" }, { step: "验证", status: "in_progress" }] },
    { id: "diff", role: "activity", kind: "fileChange", text: "", changes: [{ path: "src/a.ts", kind: "update", diff: "@@ -1 +1 @@\n-old\n+new" }] },
    { id: "image", role: "user", kind: "userMessage", text: "图片", attachments: [{ name: "Mock.png", url: "data:image/png;base64,YQ==" }], truncated: true },
  ];
  const html = render(ConversationTurn, { turn: { id: "turn", status: "completed", messages } });
  for (const text of ["node test.js", "2 passed", "退出码 0", "MCP 工具", "lookup", "public query", "<strong>公开结果</strong>", "Mock tool failed", "data-status=\"in_progress\"", "Unified", "diff-add", "diff-delete", 'alt="Mock.png"', "内容已截断"]) assert.ok(html.includes(text), text);
});

test("Markdown 生成文件是候选链接而非本地导航，HTTPS 图片不偷偷发背景请求", () => {
  const html = render(RichText, { text: "[下载报告](sandbox:/mnt/data/report.pdf) [源文件](file:///tmp/test.ts) [Windows文件](<C:\\work\\report.pdf>) ![远程图](https://example.invalid/pixel.png)" });
  assert.match(html, /下载报告/);
  assert.match(html, /文件需在已连接的本地会话中下载/);
  assert.match(html, /Windows文件<small/);
  assert.doesNotMatch(html, /href="(?:sandbox:|file:)|src="https:/);
  assert.match(html, /href="https:\/\/example.invalid\/pixel.png"/);
});

test("Markdown 图片链接保留单个可访问链接，不创建嵌套 anchor 或重复下载入口", () => {
  const html = render(RichText, { text: "[![报告预览](https://example.invalid/preview.png)](https://example.invalid/report.pdf)" });
  assert.equal((html.match(/<a /g) || []).length, 1); assert.match(html, /报告预览/); assert.doesNotMatch(html, /<img /);
});

test("消息输入边界验证新增字段类型，超限附件仍能展示后台明确的不可用原因", () => {
  const { conversationEvent } = createModuleGraph(new URL("../src/features/remote-workspace/state.ts", import.meta.url)).exports;
  const message = { id: "item", role: "activity", kind: "commandExecution", text: "", command: { command: "echo Mock", output: "ok", exitCode: 0 }, tool: { name: "Mock", input: "public", output: "result" }, changes: [{ path: "a.ts", diff: "+ok" }], plan: [{ step: "test", status: "completed" }], truncated: false, attachments: [{ name: "large.pdf", size: 201 * 1024 * 1024, unavailableReason: "超过 200 MiB 上限" }] };
  const event = next => ({ type: "state", state: { id: "thread", historyComplete: true, requests: [], turns: [{ id: "turn", status: "completed", messages: [next] }] } });
  assert.equal(conversationEvent(event(message)).state.turns[0].messages[0], message);
  for (const bad of [{ command: { command: "echo", output: {} } }, { tool: { name: "Mock", input: {} } }, { changes: [{ path: "a", diff: 1 }] }, { plan: [{ step: 1 }] }, { truncated: "true" }, { attachments: [{ name: "a", size: -1 }] }]) assert.throws(() => conversationEvent(event({ ...message, ...bad })), /数据无效/);
});

test("结构化附件只呈现同一会话的白名单下载 URL", () => {
  // Use one graph so Provider and FileLink share the actual production context.
  const all = createModuleGraph(new URL("../src/remote/files.tsx", import.meta.url), { stubs: real }).exports;
  const threadId = "12345678-1234-1234-1234-123456789abc";
  const url = `/api/sessions/${threadId}/files/${"a".repeat(64)}`;
  const make = downloadUrl => renderToStaticMarkup(React.createElement(all.MessageFilesContext.Provider, { value: { threadId, enabled: true } }, React.createElement(all.FileLink, { reference: "opaque", attachment: { id: "opaque", name: "report.pdf", downloadUrl, size: 42, mime: "application/pdf" } })));
  assert.match(make(url), new RegExp(`href="${url}" download="report.pdf"`));
  for (const bad of [`https://evil.invalid${url}`, `${url}?token=secret`, url.replace(threadId, "ffffffff-ffff-ffff-ffff-ffffffffffff"), "file:///private", "/api/sessions/bad/files/a"]) assert.doesNotMatch(make(bad), /href=/);
});

test("真实审批卡按公开范围呈现，不 dump 未知权限或为无详情请求提供盲批", () => {
  const { ApprovalCard } = createModuleGraph(new URL("../src/features/remote-workspace/Workspace.tsx", import.meta.url), { stubs: { ...real, "./useRemoteWorkspaceController": {} } }).exports;
  const make = (method, params) => render(ApprovalCard, { request: { id: 1, method, params, supported: true }, disabled: false, onRespond: async () => {} });
  const permission = make("item/permissions/requestApproval", { permissions: { network: { enabled: true }, fileSystem: { read: ["D:/Mock"], write: ["D:/Mock/output"] } } });
  for (const text of ["网络访问", "允许", "读取路径", "D:/Mock", "写入路径", "本次允许"]) assert.ok(permission.includes(text), text);
  const command = make("item/commandExecution/requestApproval", { command: "node test.js", cwd: "D:/Mock", networkApprovalContext: { host: "example.invalid", protocol: "https" } });
  assert.match(command, /example.invalid/); assert.match(command, /工作目录/);
  for (const [method, params] of [["unknown", { config: "NEVER_DUMP" }], ["item/permissions/requestApproval", { permissions: { config: "NEVER_DUMP" } }], ["item/commandExecution/requestApproval", {}]]) {
    const html = make(method, params); assert.match(html, /电脑端处理/); assert.doesNotMatch(html, /NEVER_DUMP|本次允许/);
  }
});
