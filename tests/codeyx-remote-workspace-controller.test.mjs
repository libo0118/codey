import assert from "node:assert/strict";
import test from "node:test";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";

const projects = [{ id: "p", name: "Project", cwd: "/project" }, { id: "q", name: "Other", cwd: "/other" }];
const thread = { id: "thread", title: "Thread", cwd: "/project", updatedAt: 1 };
const view = { id: thread.id, turns: [], requests: [], historyComplete: false };
const catalog = { models: ["model"], default_model: "model", model_metadata: [{ model: "model", supported_reasoning_efforts: ["low", "high"], default_reasoning_effort: "high" }] };

function fixture(t, active = true) {
  const browser = installControllerWindow(t);
  const calls = [], sockets = [], journal = new Map();
  let sequence = 0, unauthorized = 0;
  const options = { active, onUnauthorized: () => unauthorized++ };
  const requests = Object.fromEntries(["threads", "projects", "models", "defaults", "session", "action", "submit", "create"].map(name => [name, (...args) => {
    const call = { name, args, ...deferred() }; calls.push(call); return call.promise;
  }]));
  Object.assign(requests, {
    unauthorized: cause => cause?.status === 401,
    readSubmission: key => journal.get(key) ?? null,
    rememberSubmission(key, args) { const record = { id: `submission-${++sequence}`, args }; journal.set(key, record); return record; },
    forgetSubmission: key => journal.delete(key),
    connect(id, handlers) { const socket = { id, handlers, closed: 0, close() { this.closed++; } }; sockets.push(socket); return socket; },
  });
  const h = createControllerHarness(new URL("../src/features/remote-workspace/useRemoteWorkspaceController.ts", import.meta.url), "useRemoteWorkspaceController", { "./requests": { remoteWorkspaceRequests: requests } });
  const render = () => h.render(options);
  const last = name => calls.findLast(call => call.name === name);
  async function ready() {
    render(); browser.tick(); last("threads").resolve([thread]); last("projects").resolve(projects); last("models").resolve(catalog); await h.flush();
  }
  async function open() {
    render().open(thread); render(); sockets.at(-1).handlers.message({ type: "state", state: view }); await h.flush();
  }
  return { ...h, render, browser, options, calls, sockets, journal, last, ready, open, get unauthorized() { return unauthorized; } };
}

test("隐藏期间不请求，重开保留草稿，过期列表、模型与错误均不回写", async t => {
  const h = fixture(t, false); let c = h.render();
  c.refresh(); c.reloadModels(); c.startCreating(); await c.send(); assert.equal(h.calls.length, 0);
  h.options.active = true; await h.ready(); await h.open(); h.render().changeDraft("保留"); h.render();
  h.render().refresh(); const old = h.calls.slice(-3);
  h.options.active = false; h.render(); assert.equal(h.sockets[0].closed, 1);
  h.options.active = true; h.render(); h.browser.tick();
  h.last("threads").resolve([thread]); h.last("projects").resolve(projects); h.last("models").resolve(catalog); await h.flush();
  old.find(call => call.name === "threads").resolve([]); old.find(call => call.name === "projects").resolve([]);
  old.find(call => call.name === "models").reject({ status: 401 }); c = await h.flush();
  assert.equal(c.draft, "保留"); assert.equal(c.threads.length, 1); assert.equal(c.models.length, 1); assert.equal(c.error, ""); assert.equal(h.unauthorized, 0);
  h.unmount();
});

test("筛选、手动刷新各自作废旧读取的成功、错误和 finally", async t => {
  const h = fixture(t); await h.ready(); h.render().refresh(); const old = h.calls.slice(-3);
  h.render().changeSearch("new"); h.render(); h.browser.tick();
  assert.deepEqual(h.last("threads").args, ["new", false]);
  old[0].reject(new Error("old")); old[1].resolve([]); old[2].resolve({ models: ["old"] }); await h.flush();
  assert.equal(h.render().loading, true); assert.equal(h.render().error, "");
  h.last("threads").resolve([]); h.last("projects").resolve(projects); await h.flush();
  h.render().changeArchived(true); h.render(); h.browser.tick(); assert.deepEqual(h.last("threads").args, ["new", true]);
  h.last("threads").reject(new Error("read failed")); h.last("projects").resolve(projects); await h.flush();
  assert.equal(h.render().loading, false); assert.equal(h.render().error, "read failed"); h.unmount();
});

test("切换会话和项目隔离草稿，默认值失败可重试，晚到的项目默认值不能覆盖当前项目", async t => {
  const h = fixture(t); await h.ready(); await h.open(); h.render().changeDraft("会话草稿"); h.render();
  h.render().startCreating(projects[0]); h.render(); const old = h.last("defaults"); h.render().changeDraft("项目草稿"); h.render();
  h.render().startCreating(projects[1]); h.render(); h.last("defaults").reject(new Error("offline")); await h.flush();
  assert.match(h.render().catalogError, /默认设置/); assert.equal(await h.render().send(), false);
  h.render().reloadModels(); h.render(); h.last("defaults").resolve({ model: "model", effort: "low", serviceTier: null }); h.last("models").resolve(catalog); await h.flush();
  old.resolve({ model: "old", serviceTier: "priority" }); await h.flush(); assert.equal(h.render().composerView.serviceTier, null);
  h.render().startCreating(projects[0]); h.render(); assert.equal(h.render().draft, "项目草稿");
  h.render().back(); h.render(); h.render().open(thread); h.render(); assert.equal(h.render().draft, "会话草稿"); h.unmount();
});

test("连接按会话隔离、隐藏清理、断线退避重连且不重放写入，桌面不可用停止自动重试", async t => {
  const h = fixture(t); await h.ready(); await h.open(); const first = h.sockets[0];
  first.handlers.close(); await h.flush(); assert.equal(h.render().connected, false); h.browser.tick(); assert.equal(h.sockets.length, 2);
  const second = h.sockets[1]; first.handlers.message({ type: "state", state: { ...view, title: "late" } }); assert.equal(h.render().view.title, undefined);
  second.handlers.message({ type: "state", state: { ...view, id: "other" } }); assert.equal(h.render().connected, false);
  second.handlers.invalid(); assert.match(h.render().connectionError, /数据无效/);
  second.handlers.message({ type: "state", state: view }); assert.equal(h.render().connected, true);
  second.handlers.message({ type: "disconnected", message: "桌面离线" }); await h.flush(); h.browser.tick();
  assert.equal(h.sockets.length, 2); assert.equal(h.render().connectionError, "桌面离线"); assert.equal(h.render().connected, false);
  h.render().reconnect(); h.render(); assert.equal(h.sockets.length, 3);
  h.sockets.at(-1).handlers.close(); h.last("session").reject({ status: 401 }); await h.flush(); h.browser.tick();
  assert.equal(h.unauthorized, 1); assert.equal(h.sockets.length, 3); assert.equal(h.calls.filter(call => call.name === "action").length, 0); h.unmount();
});

test("发送立即锁定重复提交，失败保留草稿和防重记录，显式核对后才能再次发送", async t => {
  const h = fixture(t); await h.ready(); await h.open(); h.render().changeDraft("hello"); let c = h.render();
  const work = c.send(); await c.send(); c.open({ ...thread, id: "other" }); c.confirmSubmission();
  assert.equal(h.calls.filter(call => call.name === "submit").length, 1); assert.equal(h.render().selectedId, thread.id);
  h.last("submit").reject(new Error("outcome unknown")); await work; c = await h.flush();
  assert.equal(c.draft, "hello"); assert.equal(c.uncertain, true); assert.equal(c.busy, false); assert.equal(h.journal.size, 1);
  await c.send(); assert.equal(h.calls.filter(call => call.name === "submit").length, 1);
  c.confirmSubmission(); c = h.render(); const retry = c.send(); h.last("submit").resolve({}); await retry; c = await h.flush();
  assert.equal(c.draft, ""); assert.equal(c.uncertain, false); assert.equal(h.journal.size, 0); h.unmount();
});

test("隐藏重开期间保留写锁和未知结果，旧成功不清空草稿也不自动重发", async t => {
  const h = fixture(t); await h.ready(); await h.open(); h.render().changeDraft("pending"); const work = h.render().send();
  h.options.active = false; h.render(); h.options.active = true; h.render(); await h.render().send();
  h.last("submit").resolve({}); await work; const c = await h.flush();
  assert.equal(c.draft, "pending"); assert.equal(c.uncertain, true); assert.equal(c.busy, false); assert.equal(h.journal.size, 1);
  assert.equal(h.calls.filter(call => call.name === "submit").length, 1); h.unmount();
});

test("新建的首轮接受、未启动及结果不明分别清理或保护草稿，失败不重复创建", async t => {
  const h = fixture(t); await h.ready();
  for (const firstTurn of ["accepted", "not-started", "outcome-unknown", "failed"]) {
    h.render().startCreating(projects[0]); h.render(); h.last("defaults").resolve({ serviceTier: "priority" }); await h.flush();
    h.render().changeDraft(firstTurn); h.render().changeSettings({ effort: "low" }); const c = h.render();
    const work = c.send(); await c.send(); const call = h.last("create");
    assert.equal(call.args[0].args.effort, "low"); assert.equal(call.args[0].args.serviceTier, "priority");
    if (firstTurn === "failed") call.reject(new Error("unknown")); else call.resolve({ id: `new-${firstTurn}`, firstTurn });
    await work; const next = await h.flush();
    assert.equal(next.draft, firstTurn === "accepted" ? "" : firstTurn);
    assert.equal(next.uncertain, ["outcome-unknown", "failed"].includes(firstTurn));
    assert.equal(next.creating, firstTurn === "failed");
  }
  assert.equal(h.calls.filter(call => call.name === "create").length, 4); h.unmount();
});

test("历史、审批、设置和中断通过业务动作适配参数，断线和重复操作被阻止", async t => {
  const h = fixture(t); await h.ready(); await h.open();
  const actions = [[c => c.loadHistory(), "history", {}], [c => c.respond(7, { decision: "accept" }), "respond", { decision: "accept", approvalId: 7 }], [c => c.stop(), "interrupt", { turnId: "turn" }]];
  for (const [invoke, action, args] of actions) {
    h.sockets.at(-1).handlers.message({ type: "state", state: { ...view, turns: [{ id: "turn", status: "inProgress", messages: [] }] } });
    const work = invoke(h.render()); await invoke(h.render()); assert.deepEqual(h.last("action").args, [thread.id, action, args]);
    h.last("action").resolve({}); await work; await h.flush();
  }
  h.sockets.at(-1).handlers.message({ type: "state", state: view }); h.render().changeSettings({ serviceTier: "default" });
  assert.deepEqual(h.last("action").args, [thread.id, "settings", { serviceTier: "default" }]); h.last("action").resolve({}); await h.flush();
  const count = h.calls.length; await h.render().loadHistory(); assert.equal(h.calls.length, count); h.unmount();
});

test("卸载后的读取、写入、连接成功与错误均不能回写或继续调度", async t => {
  const h = fixture(t); await h.ready(); await h.open(); h.render().changeDraft("pending"); const work = h.render().send();
  h.render().refresh(); const late = h.calls.slice(-3); const socket = h.sockets.at(-1); h.unmount();
  h.last("submit").reject({ status: 401 }); late[0].resolve([]); late[1].resolve([]); late[2].reject({ status: 401 });
  socket.handlers.message({ type: "state", state: view }); socket.handlers.close(); socket.handlers.invalid(); await work; await h.flush();
  assert.equal(h.writesAfterUnmount, 0); assert.equal(h.unauthorized, 0); assert.equal(h.browser.timers.size, 0);
});
