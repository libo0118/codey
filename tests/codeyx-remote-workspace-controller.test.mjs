import assert from "node:assert/strict";
import test from "node:test";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";

const projects = [{ id: "p", name: "Project", cwd: "/project" }, { id: "q", name: "Other", cwd: "/other" }];
const thread = { id: "thread", title: "Thread", cwd: "/project", updatedAt: 1 };
const view = { id: thread.id, status: "idle", turns: [], requests: [], historyComplete: false };
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
  async function ready(threads = [thread]) {
    render(); browser.tick(180); last("threads").resolve(threads); last("projects").resolve(projects); last("models").resolve(catalog); await h.flush();
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
  h.options.active = true; h.render(); h.browser.tick(180);
  h.last("threads").resolve([thread]); h.last("projects").resolve(projects); h.last("models").resolve(catalog); await h.flush();
  old.find(call => call.name === "threads").resolve([]); old.find(call => call.name === "projects").resolve([]);
  old.find(call => call.name === "models").reject({ status: 401 }); c = await h.flush();
  assert.equal(c.draft, "保留"); assert.equal(c.threads.length, 1); assert.equal(c.models.length, 1); assert.equal(c.error, ""); assert.equal(h.unauthorized, 0);
  h.unmount();
});

test("筛选、手动刷新各自作废旧读取的成功、错误和 finally", async t => {
  const h = fixture(t); await h.ready(); h.render().refresh(); const old = h.calls.slice(-3);
  h.render().changeSearch("new"); h.render(); h.browser.tick(180);
  assert.deepEqual(h.last("threads").args.slice(0, 2), ["new", false]);
  old[0].reject(new Error("old")); old[1].resolve([]); old[2].resolve({ models: ["old"] }); await h.flush();
  assert.equal(h.render().loading, true); assert.equal(h.render().error, "");
  h.last("threads").resolve([]); h.last("projects").resolve(projects); await h.flush();
  h.render().changeArchived(true); h.render(); h.browser.tick(180); assert.deepEqual(h.last("threads").args.slice(0, 2), ["new", true]);
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
  first.handlers.close(); await h.flush(); assert.equal(h.render().connected, false); h.browser.tick(2000); assert.equal(h.sockets.length, 2);
  const second = h.sockets[1]; first.handlers.message({ type: "state", state: { ...view, title: "late" } }); assert.equal(h.render().view.title, undefined);
  second.handlers.message({ type: "state", state: { ...view, id: "other" } }); assert.equal(h.render().connected, false);
  second.handlers.invalid(); assert.match(h.render().connectionError, /数据无效/);
  second.handlers.message({ type: "state", state: view }); assert.equal(h.render().connected, false);
  h.browser.tick(2000); assert.equal(h.sockets.length, 2);
  h.render().reconnect(); h.render(); const third = h.sockets[2];
  third.handlers.message({ type: "state", state: view }); assert.equal(h.render().connected, true);
  third.handlers.message({ type: "disconnected", message: "桌面离线" }); await h.flush(); h.browser.tick(2000);
  assert.equal(h.sockets.length, 3); assert.equal(h.render().connectionError, "桌面离线"); assert.equal(h.render().connected, false);
  h.render().reconnect(); h.render(); assert.equal(h.sockets.length, 4);
  h.sockets.at(-1).handlers.close(); h.last("session").reject({ status: 401 }); await h.flush(); h.browser.tick(2000);
  assert.equal(h.unauthorized, 1); assert.equal(h.sockets.length, 4); assert.equal(h.calls.filter(call => call.name === "action").length, 0); h.unmount();
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
    h.sockets.at(-1).handlers.message({ type: "state", state: { ...view, status: "active", turns: [{ id: "turn", status: "inProgress", messages: [] }] } });
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

test("5 秒列表轮询保持单 flight，失败清除旧绿点，释放后重试且不重拉项目或模型", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }]);
  assert.ok([...h.browser.delays.values()].includes(5000));
  h.browser.tick(4999); assert.equal(h.calls.filter(call => call.name === "threads").length, 1);
  h.browser.tick(5000); const pending = h.last("threads");
  assert.equal(h.render().loading, false);
  assert.equal(h.calls.filter(call => call.name === "threads").length, 2);
  h.browser.tick(4999); h.browser.dispatch("focus"); h.browser.dispatch("focus");
  assert.equal(h.calls.filter(call => call.name === "threads").length, 2);
  pending.reject(new Error("status unavailable")); await h.flush();
  assert.equal(h.render().threads[0].running, null); assert.equal(h.render().error, "status unavailable");
  assert.equal(pending.args[2].aborted, true);
  h.browser.tick(5000); h.last("threads").resolve([{ ...thread, running: false }]); await h.flush();
  assert.equal(h.render().threads[0].running, false);
  assert.equal(h.calls.filter(call => call.name === "projects").length, 1);
  assert.equal(h.calls.filter(call => call.name === "models").length, 1);
  const count = h.calls.length; h.unmount(); h.browser.dispatch("focus"); h.browser.tick(5000);
  assert.equal(h.calls.length, count); assert.equal(h.browser.timers.size, 0); assert.equal(h.writesAfterUnmount, 0);
});

test("可见且联网时挂起的列表读取 5 秒超时清除旧绿点，释放 single flight 并拒绝迟到状态", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }]);
  h.browser.tick(5000); const hung = h.last("threads");
  h.browser.tick(4999); assert.equal(hung.args[2].aborted, false); assert.equal(h.render().threads[0].running, true);
  h.browser.tick(5000); await h.flush();
  assert.equal(hung.args[2].aborted, true); assert.equal(h.render().threads[0].running, null);
  assert.match(h.render().error, /列表刷新超时/); assert.notEqual(h.last("threads"), hung);
  const fresh = h.last("threads"); hung.resolve([{ ...thread, running: true }]); await h.flush();
  assert.equal(h.render().threads[0].running, null); assert.equal(fresh.args[2].aborted, false);
  fresh.resolve([{ ...thread, running: false }]); await h.flush(); assert.equal(h.render().threads[0].running, false);
  h.unmount(); assert.equal(h.browser.timers.size, 0);
});

test("挂起读取的超时不覆盖请求开始后更新的有效 WS 状态，其余旧绿点仍清除", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }, { ...thread, id: "other", running: true }]); await h.open();
  h.browser.tick(5000); const hung = h.last("threads");
  h.sockets.at(-1).handlers.message({ type: "state", state: { ...view, status: "active", turns: [{ id: "turn", status: "inProgress", messages: [] }] } });
  await h.flush(); h.browser.tick(5000); await h.flush();
  assert.equal(h.render().threads[0].running, true); assert.equal(h.render().threads[1].running, null);
  hung.resolve([{ ...thread, running: false }]); await h.flush(); assert.equal(h.render().threads[0].running, true);
  h.unmount(); assert.equal(h.browser.timers.size, 0);
});

test("操作开始作废旧列表请求时一并取消，写操作等待期间不会占住状态查询 single flight", async t => {
  const h = fixture(t); await h.ready(); await h.open();
  h.browser.tick(5000); const old = h.last("threads");
  h.render().changeDraft("Mock send"); const work = h.render().send(); assert.equal(old.args[2].aborted, true);
  h.browser.tick(5000); assert.notEqual(h.last("threads"), old);
  h.last("threads").resolve([{ ...thread, running: false }]); h.last("submit").resolve({}); await work; await h.flush();
  h.unmount(); assert.equal(h.browser.timers.size, 0);
});

test("隐藏与离线立即清除绿点并取消读取，恢复可见或联网和 focus 立即刷新", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }]);
  h.browser.tick(5000); const hiddenRead = h.last("threads");
  h.browser.visibility("hidden"); await h.flush();
  assert.equal(hiddenRead.args[2].aborted, true); assert.equal(h.render().threads[0].running, null);
  const count = h.calls.length; h.browser.tick(5000); h.browser.dispatch("focus");
  assert.equal(h.calls.length, count); assert.equal(h.browser.timers.size, 0);
  h.browser.visibility("visible"); const visibleRead = h.last("threads"); assert.notEqual(visibleRead, hiddenRead);
  visibleRead.resolve([{ ...thread, running: false }]); await h.flush();
  hiddenRead.resolve([{ ...thread, running: true }]); await h.flush(); assert.equal(h.render().threads[0].running, false);
  h.browser.dispatch("focus"); h.last("threads").resolve([{ ...thread, running: true }]); await h.flush();
  h.browser.online(false); await h.flush(); assert.equal(h.render().threads[0].running, null);
  const offline = h.calls.length; h.browser.tick(5000); h.browser.dispatch("focus"); assert.equal(h.calls.length, offline);
  h.browser.online(true); h.last("threads").resolve([{ ...thread, running: null }]); await h.flush();
  assert.equal(h.render().threads[0].running, null); h.unmount();
});

test("批量状态每次替换，未知或缺失字段不继承上次运行状态，401 仍通知退出", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }]);
  for (const result of [{ ...thread, running: false }, { ...thread, running: true }, { ...thread, running: null }, { ...thread, running: true }, thread]) {
    h.browser.tick(5000); h.last("threads").resolve([result]); await h.flush();
    assert.equal(h.render().threads[0].running, result.running);
  }
  h.browser.tick(5000); h.last("threads").reject({ status: 401 }); await h.flush();
  assert.equal(h.unauthorized, 1); assert.notEqual(h.render().threads[0].running, true); h.unmount();
});

test("真实会话流由 active 点亮，终态熄灭且晚到 HTTP 不能反转原生状态", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }]); await h.open();
  const socket = h.sockets.at(-1);
  assert.equal(h.render().connected, true); assert.equal(h.render().threads[0].running, false);
  for (const status of ["completed", "failed", "interrupted"]) {
    socket.handlers.message({ type: "state", state: { ...view, status: "active", turns: [{ id: "turn", status: "inProgress", messages: [] }] } });
    await h.flush(); assert.equal(h.render().threads[0].running, true);
    const sameRunning = h.render().threads;
    socket.handlers.message({ type: "state", state: { ...view, status: "active", turns: [{ id: "turn", status: "inProgress", messages: [{ id: "token", kind: "agentMessage", role: "assistant", text: "stream", attachments: [] }] }] } });
    await h.flush(); assert.equal(h.render().threads, sameRunning);
    h.browser.tick(5000); const pending = h.last("threads");
    socket.handlers.message({ type: "state", state: { ...view, turns: [{ id: "turn", status, messages: [] }] } });
    await h.flush(); assert.equal(h.render().threads[0].running, false);
    pending.resolve([{ ...thread, running: true }]); await h.flush(); assert.equal(h.render().threads[0].running, false);
  }
  socket.handlers.message({ type: "state", state: { ...view, turns: [{ id: "turn", status: null, messages: [] }] } });
  await h.flush(); assert.notEqual(h.render().threads[0].running, true); h.unmount();
});

test("原生执行状态优先于缓存轮次，未知状态不能从历史 inProgress 推断成运行", async t => {
  const h = fixture(t); await h.ready(); await h.open();
  for (const [status, expected] of [["active", true], ["idle", false], ["systemError", false], ["notLoaded", null], [null, null], [undefined, null]]) {
    h.sockets.at(-1).handlers.message({ type: "state", state: { ...view, status, turns: [{ id: "cached", status: "inProgress", messages: [] }] } });
    await h.flush(); assert.equal(h.render().threads[0].running, expected);
  }
  h.unmount();
});

test("新批量结果可清除更早的 WS 运行状态，不让已连接但未更新的旧 view 粘住绿点", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }]); await h.open();
  for (const result of [{ ...thread, running: false }, { ...thread, running: null }, thread]) {
    h.sockets.at(-1).handlers.message({ type: "state", state: { ...view, status: "active", turns: [{ id: "turn", status: "inProgress", messages: [] }] } });
    await h.flush(); assert.equal(h.render().threads[0].running, true);
    h.browser.tick(5000); h.last("threads").resolve([result]); await h.flush();
    assert.equal(h.render().threads[0].running, result.running);
  }
  h.unmount();
});

test("断线和无效会话流清除旧状态，旧 HTTP 或离线期间的批量 true 不重新点亮所选行", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }, { ...thread, id: "other", running: true }]); await h.open();
  const socket = h.sockets.at(-1);
  socket.handlers.message({ type: "state", state: { ...view, status: "active", turns: [{ id: "turn", status: "inProgress", messages: [] }] } });
  await h.flush(); assert.equal(h.render().threads[0].running, true);
  h.browser.tick(5000); const pending = h.last("threads"); socket.handlers.close(); await h.flush();
  assert.ok(h.render().threads.every(item => item.running !== true));
  assert.equal(pending.args[2].aborted, true);
  pending.resolve([{ ...thread, running: true }, { ...thread, id: "other", running: true }]); await h.flush();
  assert.ok(h.render().threads.every(item => item.running !== true));
  h.browser.visibility("hidden"); h.browser.visibility("visible"); h.last("threads").resolve([{ ...thread, running: true }]); await h.flush();
  assert.equal(h.render().threads[0].running, null);
  h.render().reconnect(); h.render(); const next = h.sockets.at(-1);
  next.handlers.message({ type: "state", state: { ...view, status: "active", turns: [{ id: "turn", status: "inProgress", messages: [] }] } });
  await h.flush(); assert.equal(h.render().threads[0].running, true);
  next.handlers.invalid(); await h.flush(); assert.equal(h.render().threads[0].running, null);
  next.handlers.message({ type: "state", state: { ...view, status: "active", turns: [{ id: "turn", status: "inProgress", messages: [] }] } });
  await h.flush(); assert.equal(h.render().threads[0].running, null); h.unmount();
});

test("可见性恢复后，新批量结果可更新仍有效的所选会话，不复用隐藏前的 WS 绿点", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }]); await h.open();
  h.sockets.at(-1).handlers.message({ type: "state", state: { ...view, status: "active", turns: [{ id: "turn", status: "inProgress", messages: [] }] } });
  await h.flush(); assert.equal(h.render().threads[0].running, true);
  h.browser.visibility("hidden"); await h.flush(); assert.equal(h.render().threads[0].running, null);
  h.browser.visibility("visible"); assert.equal(h.render().threads[0].running, null);
  h.last("threads").resolve([{ ...thread, running: true }]); await h.flush();
  assert.equal(h.render().threads[0].running, true); h.unmount();
});

test("401 立即终止列表状态刷新，登录状态尚未重新渲染时的旧 WS 也不能点亮", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }]); await h.open();
  h.browser.tick(5000); h.last("threads").reject({ status: 401 }); await h.flush();
  assert.equal(h.unauthorized, 1); assert.equal(h.render().loading, false);
  h.sockets.at(-1).handlers.message({ type: "state", state: { ...view, status: "active", turns: [{ id: "turn", status: "inProgress", messages: [] }] } });
  await h.flush(); assert.notEqual(h.render().threads[0].running, true);
  const count = h.calls.length; h.browser.tick(5000); h.browser.dispatch("focus"); h.render().refresh();
  assert.equal(h.calls.length, count); assert.equal(h.browser.timers.size, 0); h.unmount();
});

test("筛选、归档和登录更换取消旧状态查询，不让旧结果或错误污染新列表", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }]);
  h.browser.tick(5000); const searched = h.last("threads");
  h.render().changeSearch("filtered"); h.render(); h.browser.tick(180); const archived = h.last("threads");
  assert.equal(searched.args[2].aborted, true);
  h.render().changeArchived(true); h.render(); h.browser.tick(180);
  assert.equal(archived.args[2].aborted, true); assert.deepEqual(h.last("threads").args.slice(0, 2), ["filtered", true]);
  h.last("threads").resolve([{ ...thread, running: false }]); h.last("projects").resolve(projects); await h.flush();
  searched.resolve([{ ...thread, running: true }]); archived.reject({ status: 401 }); await h.flush();
  assert.equal(h.render().threads[0].running, false); assert.equal(h.unauthorized, 0);
  h.browser.tick(5000); const loggedOut = h.last("threads");
  h.options.active = false; h.render(); assert.equal(loggedOut.args[2].aborted, true);
  h.options.active = true; await h.ready([{ ...thread, running: null }]);
  loggedOut.resolve([{ ...thread, running: true }]); await h.flush();
  assert.equal(h.render().threads[0].running, null); assert.equal(h.unauthorized, 0); h.unmount();
});

test("轮询不改绑新会话项目或草稿；项目目录失败也取消同批读取并清除旧绿点", async t => {
  const h = fixture(t); await h.ready([{ ...thread, running: true }]);
  h.render().startCreating(projects[1]); h.render(); h.last("defaults").resolve({ model: "model" }); await h.flush();
  h.render().changeDraft("保留当前项目草稿"); h.render();
  h.browser.tick(5000); h.last("threads").resolve([{ ...thread, running: false }]); await h.flush();
  assert.equal(h.render().project, projects[1].id); assert.equal(h.render().draft, "保留当前项目草稿");
  assert.equal(h.calls.filter(call => call.name === "projects").length, 1);
  h.render().refresh(); const sibling = h.last("threads"); h.last("projects").reject(new Error("project directory unavailable"));
  await h.flush(); assert.equal(sibling.args[2].aborted, true); assert.match(h.render().error, /project directory unavailable/);
  assert.ok(h.render().threads.every(item => item.running !== true)); assert.equal(h.render().loading, false); h.unmount();
});
