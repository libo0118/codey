import assert from "node:assert/strict";
import test from "node:test";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";
import { collectElements, createModuleGraph, createStub, elementProps, elementType, textContent } from "./helpers/jsx-tree.mjs";

const project = { id: "project", name: "Project", cwd: "/project" };
const thread = { id: "thread", title: "Thread", cwd: project.cwd, updatedAt: 1 };
const view = { id: thread.id, title: "Last known view", status: "active", historyComplete: false,
  turns: [{ id: "turn", status: "inProgress", messages: [] }],
  requests: [{ id: 7, method: "item/commandExecution/requestApproval", supported: true, params: {} }] };
const invalidFrames = [["malformed JSON", "{"], ["invalid state schema", { type: "state", state: { ...view, turns: null } }], ["mismatched thread", { type: "state", state: { ...view, id: "wrong-thread" } }], ["v2 first revision is not 1", { type: "state", streamVersion: 2, revision: 2, state: view }]];

class RemoteError extends Error {
  constructor(message, status) { super(message); this.status = status; }
}

function fixture(t) {
  let h;
  t.after(() => h?.unmount());
  const browser = installControllerWindow(t);
  const previous = { WebSocket: globalThis.WebSocket, location: globalThis.location };
  const sockets = [], calls = [], journal = new Map();
  let sequence = 0, unauthorized = 0;
  globalThis.location = { protocol: "https:", host: "mobile.invalid" };
  globalThis.WebSocket = class {
    constructor(url) { this.url = url; this.closed = 0; this.sent = []; sockets.push(this); }
    send(data) { this.sent.push(data); }
    frame(value) { this.onmessage?.({ data: typeof value === "string" ? value : JSON.stringify(value) }); }
    close() { this.closed++; this.onclose?.(); }
    end() { this.onclose?.(); }
  };
  const transport = {
    RemoteError,
    remoteRequest(path, args) { const call = { path, args, ...deferred() }; calls.push(call); return call.promise; },
    requestId: () => `request-${++sequence}`,
    readSubmission: key => journal.get(key) ?? null,
    rememberSubmission(key, args) { const pending = { id: `submission-${++sequence}`, args }; journal.set(key, pending); return pending; },
    forgetSubmission: key => journal.delete(key),
  };
  h = createControllerHarness(new URL("../src/features/remote-workspace/useRemoteWorkspaceController.ts", import.meta.url), "useRemoteWorkspaceController", {
    "../../remote/transport": transport,
  });
  const options = { active: true, onUnauthorized: () => unauthorized++ };
  const render = () => h.render(options);
  const last = path => calls.findLast(call => call.path === path);
  async function ready(connected = true) {
    render(); browser.tick();
    last("/remote/threads").resolve([thread]); last("/remote/projects").resolve([project]); last("/remote/models").resolve({ models: ["model"] });
    await h.flush(); render().open(thread); render();
    if (connected) { sockets.at(-1).frame({ type: "state", state: view }); await h.flush(); }
    render().changeDraft("保留草稿"); render();
  }
  t.after(() => {
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete globalThis[key]; else globalThis[key] = value;
    }
  });
  return { ...h, render, ready, browser, options, sockets, calls, journal, last, get unauthorized() { return unauthorized; } };
}

test("新 URL 协商 v2，旧服务完整帧兼容；历史优先只读 WS，显式补读 Native 后仍扩展窗口", async t => {
  const h = fixture(t); await h.ready(false); const socket = h.sockets[0];
  assert.equal(socket.url, "wss://mobile.invalid/remote/events/thread?stream=2");
  socket.frame({ type: "ack", requestId: "Mock-ack" }); socket.frame({ type: "error", message: "Mock ignored" });
  assert.equal(h.render().connected, false);
  const initial = { ...view, hasEarlierMessages: true, model: "Mock-model", effort: "high", permissionMode: "read-only", serviceTier: null,
    turns: [{ id: "turn", status: "inProgress", messages: [{ id: "last", role: "assistant", kind: "agentMessage", text: "Mock recent" }] }] };
  socket.frame({ type: "state", streamVersion: 2, revision: 1, state: initial });
  let c = h.render(); assert.equal(c.connected, true); await c.loadHistory(); await c.loadHistory();
  assert.deepEqual(socket.sent, ['{"type":"history"}']); assert.equal(mutations(h).length, 0); assert.equal(h.sockets.length, 1);
  socket.frame({ type: "delta", baseRevision: 1, revision: 2, patches: [{ op: "add", path: "/turns/0/messages/0", value: { id: "first", role: "user", kind: "userMessage", text: "Mock earlier" } }, { op: "replace", path: "/hasEarlierMessages", value: false }] });
  c = h.render(); assert.equal(c.loadingEarlier, false); assert.equal(c.draft, "保留草稿"); assert.equal(c.composerView.permissionMode, "read-only");
  assert.deepEqual(c.view.turns[0].messages.map(message => message.id), ["first", "last"]);
  assert.equal(mutations(h).length, 0, "窗口已展开也不自动读取全部 Native 历史");
  const work = c.loadHistory(); assert.equal(h.last("/remote/action").args.action, "history");
  h.last("/remote/action").resolve({}); await work; h.render(); assert.equal(h.sockets.length, 1, "v2 Native 历史读取也不为分页重连");
  h.render().reconnect(); h.render(); assert.equal(h.sockets.length, 2);
  assert.equal(h.render().connected, false); h.sockets[1].frame({ type: "state", state: view }); assert.equal(h.render().connected, true);
  assert.equal(h.render().draft, "保留草稿"); assert.deepEqual(socket.sent, ['{"type":"history"}', '{"type":"history"}']);
});

for (const [label, bad] of [
  ["旧 revision", { type: "delta", baseRevision: 2, revision: 3, patches: [] }],
  ["坏 patch", { type: "delta", baseRevision: 1, revision: 2, patches: [{ op: "replace", path: "/turns/99", value: {} }] }],
  ["坏 schema", { type: "delta", baseRevision: 1, revision: 2, patches: [{ op: "replace", path: "/turns", value: null }] }],
  ["错 thread", { type: "delta", baseRevision: 1, revision: 2, patches: [{ op: "replace", path: "/id", value: "other" }] }],
  ["v2 混入 legacy", { type: "state", state: view }],
  ["重复 v2 首帧", { type: "state", streamVersion: 2, revision: 1, state: view }],
]) test(`真实 v2 传输 ${label} 同步只读，保留草稿且不自动重连，手动重连隔离晚到帧`, async t => {
  const h = fixture(t); await h.ready(false); const socket = h.sockets[0];
  socket.frame({ type: "state", streamVersion: 2, revision: 1, state: view });
  const c = h.render(), oldMessage = socket.onmessage, fileGeneration = c.fileGeneration;
  socket.frame(bad); await assertBlocked(h, c);
  h.browser.tick(30000); assert.equal(h.sockets.length, 1); assert.equal(h.render().connected, false);
  assert.deepEqual(h.render().view, view); assert.equal(h.render().draft, "保留草稿"); assert.match(h.render().connectionError, /数据无效/);
  h.render().reconnect(); h.render(); const fresh = h.sockets[1];
  fresh.frame({ type: "state", streamVersion: 2, revision: 1, state: { ...view, title: "Mock fresh" } });
  oldMessage({ data: JSON.stringify({ type: "delta", baseRevision: 1, revision: 2, patches: [{ op: "replace", path: "/title", value: "Mock stale" }] }) });
  assert.equal(h.render().view.title, "Mock fresh"); assert.equal(h.render().connected, true); assert.equal(h.render().draft, "保留草稿");
  assert.notEqual(h.render().fileGeneration, fileGeneration); assert.equal(mutations(h).length, 0); assert.equal(socket.sent.length, 0);
});

const mutations = h => h.calls.filter(call => call.path === "/remote/action" || call.path === "/remote/create");

async function assertBlocked(h, c) {
  for (const invoke of [c => c.send(), c => c.respond(7, { decision: "accept" }), c => c.stop(), c => c.loadHistory(), c => c.changeSettings({ effort: "high" })]) {
    const count = mutations(h).length;
    const result = invoke(c);
    assert.equal(mutations(h).length, count, "失效连接不得发送新操作，包括重新渲染前保留的回调");
    await result;
  }
}

for (const [label, invalid] of invalidFrames) {
  test(`first ${label} closes the subscription without a fabricated state or automatic retry`, async t => {
    const h = fixture(t); await h.ready(false);
    const socket = h.sockets[0], lateMessage = socket.onmessage, lateClose = socket.onclose;
    socket.frame(invalid);
    assert.equal(socket.closed, 1); assert.equal(socket.onmessage, null); assert.equal(socket.onclose, null);
    let c = h.render(); assert.equal(c.connected, false); assert.equal(c.view, null); assert.match(c.connectionError, /数据无效/);
    lateMessage({ data: JSON.stringify({ type: "state", state: view }) }); lateClose();
    h.last("/remote/session").resolve({ authenticated: true }); await h.flush(); h.browser.tick();
    c = h.render(); assert.equal(c.connected, false); assert.equal(c.view, null); assert.equal(c.draft, "保留草稿");
    assert.match(c.connectionError, /数据无效/); assert.equal(h.sockets.length, 1);
    assert.equal(h.calls.filter(call => call.path === "/remote/session").length, 1); await assertBlocked(h, c);
  });

  test(`valid state followed by ${label} becomes read-only immediately and ignores late frames`, async t => {
    const h = fixture(t); await h.ready(); const beforeInvalid = h.render();
    const socket = h.sockets[0], lateMessage = socket.onmessage, lateClose = socket.onclose;
    assert.equal(beforeInvalid.connected, true); assert.equal(beforeInvalid.threads[0].running, true); socket.frame(invalid);
    await assertBlocked(h, beforeInvalid);
    lateMessage({ data: JSON.stringify({ type: "disconnected", message: "late generic disconnect" }) });
    lateMessage({ data: JSON.stringify({ type: "state", state: { ...view, title: "must not replace last view" } }) });
    lateMessage({ data: "{" }); lateClose();
    h.last("/remote/session").reject(new Error("network unavailable")); await h.flush(); h.browser.tick();
    const c = h.render(); assert.equal(c.connected, false); assert.deepEqual(c.view, view); assert.equal(c.draft, "保留草稿");
    assert.notEqual(c.threads[0].running, true);
    assert.match(c.connectionError, /数据无效/); assert.equal(socket.closed, 1); assert.equal(h.sockets.length, 1);
    assert.equal(h.calls.filter(call => call.path === "/remote/session").length, 1); await assertBlocked(h, c);
  });
}

test("manual reconnect requires a matching new state and ignores old frames and session checks", async t => {
  const h = fixture(t); await h.ready(); const old = h.sockets[0], oldMessage = old.onmessage, oldClose = old.onclose;
  old.frame("{"); const oldSession = h.last("/remote/session");
  assert.ok(oldSession, "失效帧必须进入断线会话校验");
  h.render().reconnect(); h.render(); const rejected = h.sockets[1];
  assert.equal(h.render().connected, false);
  rejected.frame({ type: "state", state: { ...view, id: "wrong-thread" } }); assert.equal(h.render().connected, false); assert.equal(rejected.closed, 1);
  h.render().reconnect(); h.render(); const next = h.sockets[2];
  next.frame({ type: "state", state: { ...view, title: "New connection" } }); assert.equal(h.render().connected, true);
  oldMessage({ data: "{" }); oldClose(); oldSession.reject(new RemoteError("old revoked session", 401));
  await h.flush(); h.browser.tick();
  const c = h.render(); assert.equal(c.connected, true); assert.equal(c.view.title, "New connection"); assert.equal(c.connectionError, "");
  assert.equal(c.draft, "保留草稿"); assert.equal(next.closed, 0); assert.equal(h.unauthorized, 0); assert.equal(mutations(h).length, 0);
});

test("a stale network-close session check cannot revoke an automatically reconnected generation", async t => {
  const h = fixture(t); await h.ready(); h.sockets[0].end(); const oldSession = h.last("/remote/session");
  assert.equal(h.render().connected, false); h.browser.tick(); const next = h.sockets[1];
  next.frame({ type: "state", state: view }); oldSession.reject(new RemoteError("old revoked session", 401));
  await h.flush(); h.browser.tick();
  assert.equal(h.render().connected, true); assert.equal(next.closed, 0); assert.equal(h.unauthorized, 0); assert.equal(mutations(h).length, 0);
});

test("invalid connection renders an unavailable header and read-only conversation controls", async t => {
  const h = fixture(t); await h.ready(); h.sockets[0].frame("{"); const c = h.render();
  const Composer = createStub("Composer");
  const graph = createModuleGraph(new URL("../src/features/remote-workspace/Workspace.tsx", import.meta.url), { autoStub: true, stubs: {
    "./useRemoteWorkspaceController": { useRemoteWorkspaceController: () => c },
    "../../remote/cindy/HomeSurface": { HomeSurface: createStub("HomeSurface") },
    "../../remote/cindy/NewSessionSurface": { NewSessionSurface: createStub("NewSessionSurface") },
    "../../remote/composer": { Composer },
    "../../remote/messages": { ConversationTurn: createStub("ConversationTurn"), RichText: createStub("RichText") },
  } });
  const tree = graph.exports.Workspace({ active: true, onUnauthorized() {}, onPanel() {}, onLogout() {} });
  const find = predicate => collectElements(tree, predicate)[0];
  assert.match(textContent(find(node => elementProps(node).className === "remote-chat-header")), /未连接/);
  assert.match(textContent(find(node => elementProps(node).className === "remote-connection")), /数据无效/);
  assert.equal(elementProps(find(node => elementType(node) === Composer)).connected, false);
  assert.equal(elementProps(find(node => elementType(node)?.name === "ApprovalCard")).disabled, true);
  assert.equal(elementProps(find(node => elementType(node) === "button" && textContent(node) === "加载完整对话记录")).disabled, true);
});

for (const action of ["history", "settings"]) {
  test(`an in-flight ${action} success cannot reopen an invalid subscription`, async t => {
    const h = fixture(t); await h.ready();
    const work = action === "history" ? h.render().loadHistory() : h.render().changeSettings({ effort: "high" });
    const operation = h.last("/remote/action"); h.sockets[0].frame("{");
    operation.resolve({}); await work; await h.flush(); h.browser.tick();
    assert.equal(h.sockets.length, 1); assert.equal(h.render().connected, false); assert.match(h.render().connectionError, /数据无效/);
    assert.equal(h.render().draft, "保留草稿"); assert.equal(mutations(h).length, 1);
  });
}

for (const reason of ["invalid", "disconnected", "closed"]) {
  test(`current ${reason} still checks 401 and stops connection callbacks and retries`, async t => {
    const h = fixture(t); await h.ready(); const socket = h.sockets[0], oldMessage = socket.onmessage, oldClose = socket.onclose;
    if (reason === "invalid") socket.frame("{");
    else if (reason === "disconnected") socket.frame({ type: "disconnected", message: "桌面离线" });
    else socket.end();
    const session = h.last("/remote/session"); assert.ok(session, "断线后必须校验登录状态");
    session.reject(new RemoteError("revoked session", 401)); await h.flush();
    oldMessage({ data: JSON.stringify({ type: "state", state: view }) }); oldClose(); h.browser.tick();
    assert.equal(h.unauthorized, 1); assert.equal(h.render().connected, false); assert.equal(h.sockets.length, 1);
    assert.equal(socket.closed, 1); assert.equal(h.browser.timers.size, 0); assert.equal(h.calls.filter(call => call.path === "/remote/session").length, 1);
    if (reason === "disconnected") assert.equal(h.render().connectionError, "桌面离线");
    await assertBlocked(h, h.render());
  });
}

for (const outcome of ["accepted", "unknown"]) {
  test(`invalid frames and reconnect never replay an ${outcome} submission`, async t => {
    const h = fixture(t); await h.ready(); const work = h.render().send(); const submission = h.last("/remote/action");
    if (outcome === "accepted") { submission.resolve({}); await work; await h.flush(); }
    h.sockets[0].frame("{"); h.render();
    if (outcome === "unknown") { submission.reject(new Error("outcome unknown")); await work; await h.flush(); }
    assert.equal(h.render().draft, outcome === "accepted" ? "" : "保留草稿"); assert.equal(h.render().uncertain, outcome === "unknown");
    assert.equal(h.journal.size, outcome === "accepted" ? 0 : 1);
    h.render().reconnect(); h.render(); h.sockets.at(-1).frame({ type: "state", state: view }); await h.flush(); h.browser.tick();
    await h.render().send(); assert.equal(mutations(h).length, 1); assert.equal(h.sockets.length, 2);
    assert.equal(h.render().draft, outcome === "accepted" ? "" : "保留草稿"); assert.equal(h.render().uncertain, outcome === "unknown");
  });
}
