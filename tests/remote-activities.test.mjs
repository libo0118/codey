import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "@tabler/icons-react";
import MarkdownIt from "markdown-it";
import { renderToStaticMarkup } from "react-dom/server";
import { collectElements, createModuleGraph, elementProps } from "./helpers/jsx-tree.mjs";
import { createControllerHarness, installControllerWindow } from "./helpers/controller-harness.mjs";
import { activityMessages, completedActivity, startedActivity } from "./fixtures/remote-activities.mjs";

const real = { react: React, "react/jsx-runtime": jsx, "@tabler/icons-react": icons, "markdown-it": MarkdownIt };
const { ConversationTurn } = createModuleGraph(new URL("../src/remote/messages.tsx", import.meta.url), { stubs: real }).exports;
const turn = messages => ({ id: "turn", status: "completed", messages });
const render = messages => renderToStaticMarkup(React.createElement(ConversationTurn, { turn: turn(messages) }));

test("真实活动组件呈现工具合同的 name，不以 text 或虚构 displayTitle 替换", () => {
  const html = render(activityMessages);
  for (const name of ["inspect_local_file", "glob", "mcp__codey_fastctx__grep"]) assert.ok(html.includes(name), name);
  const tool = { id: "tool", role: "activity", kind: "dynamicToolCall", text: "PRIVATE_CANARY", displayTitle: "PRIVATE_CANARY", tool: { name: "public_name", output: "公开工具结果" } };
  assert.match(render([tool]), /public_name/);
  assert.doesNotMatch(render([tool]), /PRIVATE_CANARY/);
});

test("native started/completed/interrupted/interacted 在顶层无状态时仍显示每个子代理的真实状态", () => {
  const cases = [["started", "running", "开始工作", "运行中"], ["completed", "completed", "已完成", "已完成"], ["interrupted", "interrupted", "已中止", "已中止"], ["interacted", undefined, "交互", "未提供状态"]];
  for (const [action, status, label, state] of cases) {
    const html = render([{ ...startedActivity, collaboration: { action, agents: [{ id: "review", name: "审查消息展示", ...(status && { status }) }] } }]);
    assert.ok(html.includes(label), action); assert.ok(html.includes(state), action);
    assert.match(html, /审查消息展示/); assert.match(html, /暂无公开结果/);
    assert.doesNotMatch(html, /未支持的活动|公开结果请在电脑端/);
    if (action === "started") assert.doesNotMatch(html, /已完成/);
  }
});

test("协作调用的所有公开 action 可呈现，多子代理状态互不混淆，结果复用真实 RichText", () => {
  const actions = { spawnAgent: "开始工作", sendInput: "发送输入", resumeAgent: "恢复工作", wait: "等待结果", closeAgent: "关闭子代理", sendMessage: "发送消息", followupTask: "追加任务", interruptAgent: "中止工作", listAgents: "查看子代理" };
  for (const [action, label] of Object.entries(actions)) assert.ok(render([{ ...completedActivity, collaboration: { ...completedActivity.collaboration, action } }]).includes(label), action);
  const statuses = { pendingInit: "初始化中", running: "运行中", interrupted: "已中止", completed: "已完成", errored: "失败", shutdown: "已关闭", notFound: "未找到", futureState: "状态未知" };
  const html = render([{ ...completedActivity, collaboration: { action: "wait", agents: Object.entries(statuses).map(([status], index) => ({ id: String(index), name: `agent-${index}`, status, message: index === 3 ? "**公开结果**\n\n```ts\nconst checked = true;\n```" : undefined })) } }]);
  for (const [index, label] of Object.values(statuses).entries()) assert.ok(html.includes(`<span>agent-${index}</span><small>${label}</small>`), label);
  assert.match(html, /<strong>公开结果<\/strong>/); assert.match(html, /code-copy/);
  assert.match(html, /class="remote-agent-result"[^>]*tabindex="0"[^>]*role="region"/);
  assert.match(html, /公开结果，可滚动/); assert.doesNotMatch(html, /futureState/);
});

test("未知协作 action/缺失投影及未知 kind 只显示安全 placeholder，不回显私有 canary", () => {
  for (const kind of ["collabAgentToolCall", "subAgentActivity", "futurePrivateItem"]) {
    for (const collaboration of [undefined, ...["futureAction", "constructor", "toString"].map(action => ({ action, agents: [{ id: "PRIVATE_CANARY", name: "PRIVATE_CANARY", status: "PRIVATE_CANARY", message: "PRIVATE_CANARY" }] }))]) {
      const html = render([{ id: "unknown", kind, role: "activity", text: '{"secret":"PRIVATE_CANARY"}', tool: { name: "PRIVATE_CANARY", output: "PRIVATE_CANARY" }, attachments: [{ name: "PRIVATE_CANARY", url: "https://example.invalid/PRIVATE_CANARY" }], collaboration }]);
      assert.match(html, /请在电脑端查看/); assert.doesNotMatch(html, /PRIVATE_CANARY/);
    }
  }
  const empty = render([{ ...completedActivity, collaboration: { action: "wait", agents: [] } }]);
  assert.match(empty, /暂无公开子代理状态或结果/);
});

test("长子代理名字和公开结果完整保留在有键盘滚动入口的结果区域，不读取隐藏标题或 prompt", () => {
  const name = "long-public-agent-name-".repeat(8), message = "公开结果\n\n".repeat(100);
  const html = render([{ ...completedActivity, collaboration: { action: "wait", agents: [{ id: "child", name, status: "completed", message, title: "PRIVATE_CANARY", prompt: "PRIVATE_CANARY" }, { id: "unnamed" }] } }]);
  assert.ok(html.includes(name)); assert.equal((html.match(/<p>公开结果<\/p>/g) || []).length, 100);
  assert.match(html, /未命名子代理/); assert.match(html, /未提供状态/); assert.match(html, /tabindex="0" role="region"/);
  assert.doesNotMatch(html, /PRIVATE_CANARY/);
});

test("真实消息树保持工具/回答/协作原序与稳定 key，流式更新不控制 details 的展开值", () => {
  const graph = createModuleGraph(new URL("../src/remote/messages.tsx", import.meta.url), { stubs: { "@tabler/icons-react": icons, "markdown-it": MarkdownIt } });
  const tree = graph.exports.ConversationTurn({ turn: turn(activityMessages) });
  const children = collectElements(tree, node => elementProps(node).key !== undefined && Boolean(elementProps(node).message || elementProps(node)["data-message-id"]));
  assert.deepEqual(children.map(node => elementProps(node).key), activityMessages.map(message => message.id));
  const before = children.find(node => elementProps(node).message?.id === "collab-wait");
  const details = before.type(before.props);
  assert.equal(details.type, "details"); assert.equal(details.props.open, undefined); assert.equal(details.props.onToggle, undefined);
  const streamed = activityMessages.map(message => message.id === "collab-wait" ? { ...message, status: "inProgress", collaboration: { ...message.collaboration, agents: [...message.collaboration.agents, { id: "extra", name: "新增子代理", status: "running" }] } } : message);
  graph.restart();
  const after = collectElements(graph.exports.ConversationTurn({ turn: turn([{ id: "new", role: "assistant", kind: "agentMessage", text: "新增" }, ...streamed]) }), node => elementProps(node).message?.id === "collab-wait")[0];
  assert.equal(after.type, before.type); assert.equal(after.props.key, before.props.key);
  const html = render(activityMessages);
  const positions = activityMessages.map(message => html.indexOf(`data-message-id="${message.id}"`));
  assert.ok(positions.every((position, index) => position >= 0 && (!index || position > positions[index - 1])));
});

test("流式消息输入边界接受公开协作合同并拒绝错误类型，不丢弃现有工具字段", () => {
  const { conversationEvent } = createModuleGraph(new URL("../src/features/remote-workspace/state.ts", import.meta.url)).exports;
  const event = message => ({ type: "state", state: { id: "thread", historyComplete: true, requests: [], turns: [turn([message])] } });
  for (const message of activityMessages) assert.equal(conversationEvent(event(message)).state.turns[0].messages[0], message);
  for (const collaboration of [null, {}, { action: 1, agents: [] }, { action: "wait", agents: {} }, { action: "wait", agents: [null] }, ...[{ id: 1 }, { id: "a", name: {} }, { id: "a", status: null }, { id: "a", message: {} }].map(agent => ({ action: "wait", agents: [agent] }))]) {
    assert.throws(() => conversationEvent(event({ ...startedActivity, collaboration })), /数据无效/);
  }
});

test("真实 Workspace hook 接收并更新协作投影，只保留父会话列表，不发起子代理控制请求", async t => {
  const browser = installControllerWindow(t), sockets = [], calls = [];
  const thread = { id: "parent", title: "父会话", cwd: "/Mock", updatedAt: 1 };
  const requests = {
    threads: async () => [thread], projects: async () => [], models: async () => ({}), readSubmission: () => null,
    unauthorized: () => false,
    connect(id, handlers) { const socket = { id, handlers, close() {} }; sockets.push(socket); return socket; },
    action: async (...args) => calls.push(args),
  };
  const h = createControllerHarness(new URL("../src/features/remote-workspace/useRemoteWorkspaceController.ts", import.meta.url), "useRemoteWorkspaceController", { "./requests": { remoteWorkspaceRequests: requests } });
  const { conversationEvent } = createModuleGraph(new URL("../src/features/remote-workspace/state.ts", import.meta.url)).exports;
  h.render({ active: true, onUnauthorized() { assert.fail("unexpected unauthorized"); } }); browser.tick(); await h.flush();
  h.render().open(thread); h.render();
  const event = messages => conversationEvent({ type: "state", state: { id: thread.id, requests: [], historyComplete: true, turns: [turn(messages)] } });
  sockets[0].handlers.message(event([startedActivity])); await h.flush();
  assert.equal(h.render().view.turns[0].messages[0].collaboration.agents[0].status, "running");
  sockets[0].handlers.message(event([startedActivity, completedActivity])); await h.flush();
  assert.equal(h.render().view.turns[0].messages[1].collaboration.agents[0].message, completedActivity.collaboration.agents[0].message);
  assert.deepEqual(h.render().threads, [{ ...thread, running: null }]); assert.equal(calls.length, 0); assert.equal(sockets.length, 1);
  h.unmount();
});
