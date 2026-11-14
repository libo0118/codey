import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "@tabler/icons-react";
import * as nativeWeb from "react-native-web";
import MarkdownIt from "markdown-it";
import { renderToStaticMarkup } from "react-dom/server";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";
import { collectElements, createModuleGraph, textContent } from "./helpers/jsx-tree.mjs";
import { largeRemoteView, remoteWindow } from "./fixtures/bounded-remote-stream.mjs";

const state = createModuleGraph(new URL("../src/features/remote-workspace/state.ts", import.meta.url)).exports;
const full = largeRemoteView(), initial = remoteWindow(full);
const real = { react: React, "react/jsx-runtime": jsx, "react-native-web": nativeWeb, "@tabler/icons-react": icons, "markdown-it": MarkdownIt };
const message = { id: "new", role: "assistant", kind: "agentMessage", text: "Mock new answer" };
const delta = (patches, baseRevision = 1) => ({ type: "delta", baseRevision, revision: baseRevision + 1, patches });

test("有界真实消息组件不创建闭合正文、图片或空历史 section", () => {
  const { ConversationTurn } = createModuleGraph(new URL("../src/remote/messages.tsx", import.meta.url), { stubs: real }).exports;
  const html = renderToStaticMarkup(React.createElement("div", null, ...initial.turns.map(turn => React.createElement(ConversationTurn, { key: turn.id, turn }))));
  assert.equal((html.match(/class="remote-turn"/g) || []).length, 1);
  assert.equal((html.match(/<details /g) || []).length, 1);
  assert.doesNotMatch(html, /Mock lazy detail body|remote-agent-result|remote-tool|<img /);
  assert.match(html, /工作过程/); assert.match(html, /19 项活动/);
  assert.ok((html.match(/<[a-z][^>]*>/g) || []).length < 250);
});

test("近期正文穿插密集活动时仍按原顺序以单栏正文呈现，活动正文保持惰性", () => {
  const { ConversationTurn } = createModuleGraph(new URL("../src/remote/messages.tsx", import.meta.url), { stubs: real }).exports;
  const messages = Array.from({ length: 20 }, (_, index) => [
    { id: `body-${index}`, role: index % 2 ? "assistant" : "user", kind: index % 2 ? "agentMessage" : "userMessage", text: `Mock 正文 ${index}` },
    { id: `reason-${index}`, role: "activity", kind: "reasoning", text: "Mock 折叠思考" },
    { id: `tool-${index}`, role: "activity", kind: "mcpToolCall", text: "", tool: { name: "Mock.tool", output: "Mock 折叠工具结果" } },
  ]).flat();
  const html = renderToStaticMarkup(React.createElement(ConversationTurn, { turn: { id: "turn", status: "inProgress", messages } }));
  assert.equal((html.match(/<article class="remote-message /g) || []).length, 20);
  assert.equal((html.match(/class="remote-process"/g) || []).length, 20);
  for (let index = 0; index < 20; index++) {
    assert.ok(html.indexOf(`Mock 正文 ${index}<`) < html.indexOf(`data-message-id="reason-${index}"`));
    if (index < 19) assert.ok(html.indexOf(`data-message-id="reason-${index}"`) < html.indexOf(`Mock 正文 ${index + 1}<`));
  }
  assert.doesNotMatch(html, /Mock 折叠思考|Mock 折叠工具结果/);
});

test("真实 Activity hook 仅在展开时创建正文，流更新不控制原生 details identity", () => {
  // Drive the actual nested Activity with the controller harness, not a second lazy-body algorithm.
  const a = createControllerHarness(new URL("../src/remote/messages.tsx", import.meta.url), (exports, props) => {
    const tree = exports.ConversationTurn({ turn: { id: "Mock-turn", status: "completed", messages: [props.message] } });
    const group = collectElements(tree, node => node.props.messages)[0];
    const outer = group.type(group.props);
    const child = collectElements(outer, node => node.props.message?.id === props.message.id)[0];
    return child ? child.type(child.props) : outer;
  }, { "react-native-web": nativeWeb, "@tabler/icons-react": icons, "markdown-it": MarkdownIt });
  const message = full.turns[0].messages[199];
  let body = a.render({ message });
  assert.equal(body.props.open, undefined); assert.doesNotMatch(textContent(body), /Mock lazy detail body/);
  const opened = { open: true };
  body.props.onToggle({ target: opened, currentTarget: opened }); body = a.render();
  body.props.onToggle({ target: opened, currentTarget: opened }); body = a.render();
  assert.ok(collectElements(body, node => node.props.className === "remote-agent-result").length);
  body = a.render({ message: { ...message, status: "completed" } });
  assert.ok(collectElements(body, node => node.props.className === "remote-agent-result").length);
  const closed = { open: false };
  body.props.onToggle({ target: closed, currentTarget: closed }); body = a.render();
  assert.equal(collectElements(body, node => node.props.className === "remote-agent-result").length, 0);
  a.unmount();
});

test("真实 Workspace prepend 保持阅读锚点、草稿和文件 scope；普通增量不跳回底部", () => {
  let history = 0;
  const c = { view: initial, composerView: initial, selected: { id: initial.id, title: "Mock", cwd: "D:/Mock" }, selectedId: initial.id,
    draftScope: initial.id, threads: [], projects: [], models: [], connected: true, busy: false, fileGeneration: 1, draft: "保留 Mock 草稿", images: [], loadHistory: () => history++ };
  const h = createControllerHarness(new URL("../src/features/remote-workspace/Workspace.tsx", import.meta.url), "Workspace", {
    "react-native-web": nativeWeb, "@tabler/icons-react": icons, "markdown-it": MarkdownIt, "./useRemoteWorkspaceController": { useRemoteWorkspaceController: () => c },
  });
  const props = { active: true, onUnauthorized() {}, onPanel() {}, onLogout() {} };
  let tree = h.render(props);
  const transcript = () => collectElements(tree, node => node.props.className === "remote-messages")[0];
  const fileScope = () => collectElements(tree, node => node.props.value?.threadId === initial.id)[0].props.value;
  const container = { scrollTop: 0, scrollHeight: 2000, clientHeight: 400, getBoundingClientRect: () => ({ top: 100 }) };
  let prepend = 0;
  const nodes = Array.from({ length: 20 }, (_, index) => ({ dataset: { messageId: `Mock-${index}` }, getBoundingClientRect: () => ({ top: 100 + prepend + index * 100 - container.scrollTop, bottom: 200 + prepend + index * 100 - container.scrollTop }) }));
  container.querySelectorAll = () => nodes;
  transcript().props.ref.current = container;
  container.scrollTop = 600; transcript().props.onScroll({ currentTarget: container }); tree = h.render();
  const scope = fileScope(), top = nodes[6].getBoundingClientRect().top;
  const button = collectElements(tree, node => node.type === "button" && textContent(node) === "加载更早对话记录")[0];
  button.props.onClick(); assert.equal(history, 1);
  prepend = 500; container.scrollHeight += 500; c.view = remoteWindow(full, 70); tree = h.render();
  assert.equal(container.scrollTop, 1100); assert.equal(nodes[6].getBoundingClientRect().top, top);
  assert.equal(fileScope(), scope, "同连接普通内容变化不更新 Provider scope");
  container.scrollHeight += 100; c.view = { ...c.view, title: "Mock delta" }; tree = h.render();
  assert.equal(container.scrollTop, 1100); assert.equal(nodes[6].getBoundingClientRect().top, top);
  assert.ok(collectElements(tree, node => node.props.draft === "保留 Mock 草稿").length);
  c.fileGeneration = 2; tree = h.render(); assert.notEqual(fileScope(), scope); assert.equal(fileScope().generation, 2);
  h.unmount();
});

test("查看过程暂停跟随，原生折叠回弹不恢复；主动滚到底部、跳转或发送才恢复", async () => {
  const c = { view: initial, composerView: initial, selected: { id: initial.id, title: "Mock", cwd: "D:/Mock" }, selectedId: initial.id,
    draftScope: initial.id, threads: [], projects: [], models: [], connected: true, busy: false, images: [], send: async () => true };
  const h = createControllerHarness(new URL("../src/features/remote-workspace/Workspace.tsx", import.meta.url), "Workspace", {
    "react-native-web": nativeWeb, "@tabler/icons-react": icons, "markdown-it": MarkdownIt, "./useRemoteWorkspaceController": { useRemoteWorkspaceController: () => c },
  });
  try {
    let tree = h.render({ active: true, onUnauthorized() {}, onPanel() {}, onLogout() {} });
    const transcript = () => collectElements(tree, node => node.props.className === "remote-messages")[0];
    const jump = () => collectElements(tree, node => node.props["aria-label"] === "滚动到最新消息")[0];
    const container = { scrollTop: 1600, scrollHeight: 2000, clientHeight: 400, getBoundingClientRect: () => ({ top: 0 }), querySelectorAll: () => [] };
    transcript().props.ref.current = container;
    const inspect = () => {
      transcript().props.onClickCapture({ target: { closest: () => ({ parentElement: { matches: () => true } }) } });
      tree = h.render();
    };
    inspect(); assert.ok(jump());
    // A native details collapse may emit scroll while already at the bottom.
    transcript().props.onScroll({ currentTarget: container }); tree = h.render(); assert.ok(jump());
    container.scrollHeight = 2300; c.view = { ...c.view }; tree = h.render(); assert.equal(container.scrollTop, 1600);
    transcript().props.onWheelCapture(); container.scrollTop = 1900;
    transcript().props.onScroll({ currentTarget: container }); tree = h.render(); assert.equal(jump(), undefined);
    container.scrollHeight = 2500; c.view = { ...c.view }; tree = h.render(); assert.equal(container.scrollTop, 2500);
    inspect(); jump().props.onClick(); tree = h.render(); assert.equal(jump(), undefined);
    inspect();
    collectElements(tree, node => node.props.onSend)[0].props.onSend({ preventDefault() {} });
    await h.flush(); tree = h.render(); assert.equal(jump(), undefined);
    inspect(); c.draftScope = "another-thread"; tree = h.render(); tree = h.render(); assert.equal(jump(), undefined);
  } finally { h.unmount(); }
});

test("增量回放重建完整 View，未修改 turn/message 引用保持，支持数组 prepend/remove 与根 replace", () => {
  const start = { ...initial, turns: [...initial.turns, { id: "extra", status: "completed", messages: [message] }] };
  const patches = [{ op: "replace", path: "/turns/13/messages/19/collaboration/agents/0/status", value: "running" }, { op: "add", path: "/turns/14/messages/0", value: { ...message, id: "before" } }, { op: "add", path: "/turns/14/messages/-", value: { ...message, id: "after" } }, { op: "remove", path: "/turns/14/messages/1" }, { op: "add", path: "/requests/-", value: { id: 7, method: "Mock", supported: false, params: { "a/b~c": "old" } } }, { op: "replace", path: "/requests/0/params/a~1b~0c", value: "new" }];
  const parsed = state.conversationEvent(delta(patches));
  assert.equal(parsed.type, "delta");
  const next = state.applyConversationPatches(start, parsed.patches);
  const expected = structuredClone(start);
  expected.turns[13].messages[19].collaboration.agents[0].status = "running";
  expected.turns[14].messages = [{ ...message, id: "before" }, { ...message, id: "after" }];
  expected.requests = [{ id: 7, method: "Mock", supported: false, params: { "a/b~c": "new" } }];
  assert.deepEqual(next, expected); assert.equal(next.turns[0], start.turns[0]);
  assert.equal(next.turns[13].messages[0], start.turns[13].messages[0]);
  assert.notEqual(next.turns[13].messages[19], start.turns[13].messages[19]);
  assert.equal(state.applyConversationPatches(next, [{ op: "replace", path: "", value: full }]), full);
  assert.deepEqual(state.applyConversationPatches(initial, [{ op: "replace", path: "", value: remoteWindow(full, 70) }]), remoteWindow(full, 70));
});

test("增量输入拒绝未知操作、坏 pointer、原型路径、越界、缺 value 和破坏 View 的值，失败不修改基底", () => {
  const before = JSON.stringify(initial);
  for (const patch of [{ op: "move", path: "/title", value: "bad" }, { op: "replace", path: "title", value: "bad" }, { op: "replace", path: "/ti~2tle", value: "bad" }, { op: "add", path: "/__proto__/polluted", value: true }, { op: "add", path: "/constructor/prototype/polluted", value: true }, { op: "replace", path: "/turns/01/messages", value: [] }, { op: "remove", path: "/turns/50" }, { op: "replace", path: "/turns/-", value: {} }, { op: "add", path: "/turns/15", value: {} }, { op: "replace", path: "/title" }, { op: "remove", path: "/unknown" }, { op: "replace", path: "/turns", value: null }, { op: "remove", path: "" }]) {
    assert.throws(() => state.applyConversationPatches(initial, [patch]), /数据无效/, JSON.stringify(patch));
    assert.equal(JSON.stringify(initial), before);
  }
  assert.equal({}.polluted, undefined);
  for (const frame of [{ type: "state", streamVersion: 3, revision: 1, state: initial }, { type: "state", streamVersion: 2, state: initial }, delta([], -1), delta([], 1.5), delta([], Number.MAX_SAFE_INTEGER), { ...delta([]), patches: {} }, { type: "state", state: { ...initial, hasEarlierMessages: "yes" } }]) assert.throws(() => state.conversationEvent(frame), /数据无效/);
});

function fixture(t) {
  let h;
  t.after(() => h?.unmount());
  const browser = installControllerWindow(t), sockets = [], reads = [];
  let unauthorized = 0;
  const requests = { session: async () => ({}), unauthorized: error => error?.status === 401,
    action(...args) { const call = { args, ...deferred() }; reads.push(call); return call.promise; },
    connect(id, handlers) { const socket = { id, handlers, closed: 0, historyCalls: 0, close() { this.closed++; }, history() { this.historyCalls++; } }; sockets.push(socket); return socket; } };
  h = createControllerHarness(new URL("../src/features/remote-workspace/useRemoteConversationController.ts", import.meta.url), "useRemoteConversationController", { "./requests": { remoteWorkspaceRequests: requests } });
  const options = { active: true, id: initial.id }, render = () => h.render(options.active, options.id, () => unauthorized++);
  const frame = (event, socket = sockets.at(-1)) => { const parsed = state.conversationEvent(event); if (parsed) socket.handlers.message(parsed); };
  render();
  return { ...h, render, sockets, frame, browser, options, reads, get unauthorized() { return unauthorized; } };
}

test("Native 历史未完整时一次点击补读并等待对话记录，不提前结束加载或重连", async t => {
  const h = fixture(t);
  h.frame({ type: "state", streamVersion: 2, revision: 1, state: { ...initial, hasEarlierMessages: false, historyComplete: false } });
  assert.equal(h.render().loadEarlier(), true); assert.equal(h.render().loadEarlier(), false);
  assert.deepEqual(h.reads[0].args, [initial.id, "history"]); assert.equal(h.sockets[0].historyCalls, 1);
  h.reads[0].resolve({ status: "ok" }); await h.flush();
  assert.equal(h.render().loadingEarlier, true, "RPC ACK 不是对话记录已送达");
  h.frame(delta([{ op: "replace", path: "", value: { ...remoteWindow(full, 70), historyComplete: true } }]));
  assert.equal(h.render().loadingEarlier, false); assert.equal(h.sockets.length, 1);
});

test("Native 历史读取失败仅结束加载并允许显式重试，普通聊天连接保持有效", async t => {
  const h = fixture(t);
  h.frame({ type: "state", streamVersion: 2, revision: 1, state: { ...initial, hasEarlierMessages: false, historyComplete: false } });
  h.render().loadEarlier(); h.reads[0].reject(new Error("Native history unavailable")); await h.flush();
  assert.equal(h.render().loadingEarlier, false); assert.equal(h.render().connected, true);
  assert.equal(h.render().historyError, "Native history unavailable"); assert.equal(h.reads.length, 1);
  assert.equal(h.render().loadEarlier(), true); assert.equal(h.reads.length, 2);
});

test("过程变化与首条实时回复不能提前结束尚无对话的 Native 历史加载", async t => {
  const h = fixture(t);
  const activity = { id: "live-step", role: "activity", kind: "reasoning", text: "public summary" };
  const current = { ...initial, hasEarlierMessages: false, historyComplete: false, turns: [{ id: "live", status: "inProgress", messages: [activity] }] };
  h.frame({ type: "state", streamVersion: 2, revision: 1, state: current }); h.render().loadEarlier();
  h.frame(delta([{ op: "add", path: "/turns/0/messages/0", value: { ...activity, id: "inserted-step" } }]));
  assert.equal(h.render().loadingEarlier, true);
  h.frame(delta([{ op: "add", path: "/turns/0/messages/-", value: { id: "live-answer", role: "assistant", kind: "agentMessage", text: "live answer" } }], 2));
  assert.equal(h.render().loadingEarlier, true); assert.equal(h.reads.length, 1);
  h.reads[0].resolve({ status: "ok" }); await h.flush();
  h.frame(delta([{ op: "replace", path: "", value: { ...current, historyComplete: true, hasEarlierMessages: true, turns: [{ ...current.turns[0], messages: [{ id: "old-answer", role: "assistant", kind: "agentMessage", text: "older answer" }, activity] }] } }], 3));
  assert.equal(h.render().loadingEarlier, false);
});

test("本地更早记录在加载期间消失时补读 Native，旧会话读取错误不污染新会话", async t => {
  const h = fixture(t);
  h.frame({ type: "state", streamVersion: 2, revision: 1, state: { ...initial, historyComplete: false } });
  h.render().loadEarlier(); assert.equal(h.reads.length, 0);
  h.frame(delta([{ op: "replace", path: "/hasEarlierMessages", value: false }]));
  assert.equal(h.reads.length, 1); assert.equal(h.render().loadingEarlier, true);
  h.options.id = "other"; h.render();
  h.frame({ type: "state", streamVersion: 2, revision: 1, state: { ...initial, id: "other", hasEarlierMessages: false, historyComplete: false } });
  h.render().loadEarlier(); h.reads[0].reject({ status: 401 }); await h.flush();
  assert.equal(h.unauthorized, 0); assert.equal(h.render().loadingEarlier, true); assert.equal(h.render().historyError, "");
  h.reads[1].resolve({ status: "ok" }); await h.flush();
  h.frame(delta([{ op: "replace", path: "/historyComplete", value: true }]));
  assert.equal(h.render().loadingEarlier, false);
});

test("新流只由历史读取回执结束加载，删除首条对话或过程更新不充当回执", t => {
  const h = fixture(t);
  h.frame({ type: "state", streamVersion: 2, revision: 1, state: { ...initial, historyRevision: 0 } });
  h.render().loadEarlier();
  h.frame(delta([{ op: "remove", path: "/turns/0" }]));
  assert.equal(h.render().loadingEarlier, true);
  h.frame(delta([{ op: "replace", path: "/historyRevision", value: 1 }], 2));
  assert.equal(h.render().loadingEarlier, false);
});

test("历史回执数值在网络边界校验，已声明的回执不能撤回或倒退", t => {
  for (const historyRevision of [-1, 0.5, "1", Number.MAX_SAFE_INTEGER + 1]) assert.throws(() => state.conversationEvent({ type: "state", streamVersion: 2, revision: 1, state: { ...initial, historyRevision } }), /数据无效/);
  for (const patch of [{ op: "remove", path: "/historyRevision" }, { op: "replace", path: "/historyRevision", value: 1 }]) {
    const h = fixture(t);
    h.frame({ type: "state", streamVersion: 2, revision: 1, state: { ...initial, historyRevision: 2 } });
    h.frame(delta([patch])); assert.equal(h.render().connected, false); assert.match(h.render().connectionError, /数据无效/); h.unmount();
  }
});

test("Native 补读的 401 立即撤销当前连接，迟到状态不再恢复在线", async t => {
  const h = fixture(t);
  h.frame({ type: "state", streamVersion: 2, revision: 1, state: { ...initial, hasEarlierMessages: false, historyComplete: false } });
  const socket = h.sockets[0]; h.render().loadEarlier(); h.reads[0].reject({ status: 401 }); await h.flush();
  assert.equal(h.unauthorized, 1); assert.equal(h.render().connected, false); assert.equal(socket.closed, 1);
  h.frame(delta([{ op: "replace", path: "/historyComplete", value: true }]), socket);
  assert.equal(h.render().connected, false); assert.equal(h.render().loadEarlier(), false);
});

test("真实订阅接受 v2 首帧与目标增量，历史只读单 flight，不重连或重放写操作", t => {
  const h = fixture(t);
  h.frame({ type: "state", streamVersion: 2, revision: 1, state: initial });
  const before = h.render(), generation = before.fileGeneration;
  assert.equal(before.connected, true); assert.equal(before.loadEarlier(), true); assert.equal(before.loadEarlier(), false);
  assert.equal(h.sockets[0].historyCalls, 1); assert.equal(h.render().loadingEarlier, true);
  h.frame(delta([{ op: "replace", path: "/turns/13/messages/0/text", value: "Mock streamed" }]));
  let next = h.render(); assert.equal(next.view.turns[13].messages[0].text, "Mock streamed");
  assert.equal(next.view.turns[12], before.view.turns[12]); assert.equal(next.fileGeneration, generation);
  assert.equal(next.loadingEarlier, true, "普通增量不提前释放历史 single flight");
  h.frame(delta([{ op: "replace", path: "", value: remoteWindow(full, 70) }], 2));
  next = h.render(); assert.equal(next.loadingEarlier, false); assert.equal(h.sockets.length, 1);
});

test("delta 在首帧前、legacy 后、旧 revision 或错会话均失败关闭，只能手动新鲜快照恢复", t => {
  for (const setup of [null, { type: "state", state: initial }, { type: "state", streamVersion: 2, revision: 1, state: initial }]) {
    const h = fixture(t); if (setup) h.frame(setup);
    const socket = h.sockets[0], saved = h.render().view;
    h.frame(delta([{ op: "replace", path: "/title", value: "bad" }], 4));
    assert.equal(h.render().connected, false); assert.equal(h.render().isConnected(), false); assert.equal(h.render().view, saved);
    assert.match(h.render().connectionError, /数据无效/); h.browser.tick(30000); assert.equal(h.sockets.length, 1);
    socket.handlers.message({ type: "state", state: full }); assert.equal(h.render().view, saved);
    h.render().reconnect(); h.render(); const fresh = h.sockets[1];
    h.frame({ type: "state", state: initial }, fresh); assert.equal(h.render().connected, true);
    socket.handlers.message({ type: "state", state: full }); assert.equal(h.render().view, initial);
    h.unmount();
  }
});

test("legacy 完整快照保持兼容，新连接重置 revision/file scope，旧登录帧不污染新会话", t => {
  const h = fixture(t); h.frame({ type: "state", state: initial }); const first = h.sockets[0], generation = h.render().fileGeneration;
  h.frame({ type: "state", state: full }); assert.equal(h.render().view, full); assert.equal(h.render().fileGeneration, generation);
  assert.equal(h.render().loadEarlier(), false);
  first.handlers.close(); assert.equal(h.render().connected, false); h.browser.tick(2000);
  h.frame({ type: "state", streamVersion: 2, revision: 1, state: initial });
  assert.notEqual(h.render().fileGeneration, generation); const next = h.render().view;
  h.frame(delta([{ op: "replace", path: "/id", value: "wrong-thread" }])); assert.equal(h.render().connected, false); assert.equal(h.render().view, next);
  h.options.active = false; h.render(); h.options.active = true; h.render(); h.frame({ type: "state", streamVersion: 2, revision: 1, state: initial });
  first.handlers.message({ type: "delta", baseRevision: 1, revision: 2, patches: [{ op: "replace", path: "", value: full }] });
  assert.equal(h.render().view, initial); assert.equal(h.render().connected, true);
});
