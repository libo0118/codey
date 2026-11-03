import assert from "node:assert/strict";
import test from "node:test";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";

const empty = { running: false, urls: [], port: 18787, publicUrl: "", accountConfigured: false, username: "", revision: "", sessions: [] };
const configured = { ...empty, username: "admin", accountConfigured: true, revision: "r1" };
function fixture(t, options = {}) {
  installControllerWindow(t);
  const calls = [];
  const props = { active: true, isBusy: false, ...options };
  const h = createControllerHarness(new URL("../src/features/remote-control/useRemoteControlController.ts", import.meta.url), "useRemoteControlController", {
    "../../api": { invoke(command, args) { const call = { command, args, ...deferred() }; calls.push(call); return call.promise; } },
  });
  return { ...h, props, calls, render: () => h.render(props), async ready(status = empty) { h.render(props); calls.at(-1).resolve(status); await h.flush(); } };
}

test("remote account configuration is local-only, inactive pages never start a service", async t => {
  const h = fixture(t, { active: false }); h.render(); await h.render().start(); assert.equal(h.calls.length, 0);
  h.props.active = true; h.render(); assert.equal(h.calls[0].command, "remote_control_status");
  h.calls[0].resolve(empty); await h.flush(); await h.render().start(); assert.equal(h.calls.length, 1); h.unmount();
  const remote = fixture(t); window.__codeyRemoteMode = true; remote.render(); await remote.render().start();
  assert.equal(remote.calls.length, 0); assert.equal(remote.render().local, false); remote.unmount();
});

test("first account setup validates, confirms and submits the exact revision once", async t => {
  const h = fixture(t); await h.ready();
  h.render().change("username", "admin"); h.render().change("password", "secure-fixture-password"); h.render().change("port", "0");
  h.render().askSave(); assert.match(h.render().error, /端口/); assert.equal(h.calls.length, 1);
  h.render().change("port", "18787"); h.render().askSave(); assert.equal(h.render().confirmation, "save"); assert.equal(h.calls.length, 1);
  const save = h.render().confirm(); await h.render().confirm();
  assert.equal(h.calls.length, 2); assert.equal(h.calls[1].command, "configure_remote_control");
  assert.deepEqual(h.calls[1].args, { settings: { username: "admin", password: "secure-fixture-password", port: 18787, publicUrl: "" }, revision: "", confirmed: true });
  h.calls[1].resolve(configured); await save; await h.flush();
  assert.equal(h.render().draft.password, ""); assert.equal(h.render().status.accountConfigured, true); h.unmount();
});

test("polling preserves edited settings and blank passwords are not sent", async t => {
  const h = fixture(t); await h.ready(configured);
  h.render().change("port", "18788"); await h.render().start(); assert.equal(h.calls.length, 1);
  h.render().refresh(); h.calls.at(-1).resolve(configured); await h.flush(); assert.equal(h.render().draft.port, "18788");
  h.render().askSave(); const save = h.render().confirm();
  const request = h.calls.find(call => call.command === "configure_remote_control");
  assert.equal(request.args.settings.port, 18788); assert.equal("password" in request.args.settings, false);
  request.reject(new Error("配置版本已变化")); await save; await h.flush();
  assert.equal(h.render().draft.port, "18788"); assert.match(h.render().error, /版本已变化/); h.unmount();
});

test("running service settings are locked and late responses cannot restore hidden state", async t => {
  const running = { ...configured, running: true, urls: ["http://127.0.0.1:18787"] };
  const h = fixture(t); await h.ready(running); h.render().change("username", "other");
  assert.equal(h.render().draft.username, "admin"); await h.render().start(); assert.equal(h.calls.length, 1);
  const stop = h.render().stop(); h.props.active = false; h.render(); h.props.active = true; h.render();
  h.calls[1].resolve(configured); await stop; await h.flush();
  assert.equal(h.render().status.running, true); assert.equal(h.calls.at(-1).command, "remote_control_status");
  h.unmount(); h.calls.at(-1).reject(new Error("late read")); await h.flush(); assert.equal(h.writesAfterUnmount, 0);
});
