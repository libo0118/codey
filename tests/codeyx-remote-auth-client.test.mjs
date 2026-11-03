import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";

const remote = await loadTypeScriptModule(new URL("../src/remote/transport.ts", import.meta.url));
const settings = await loadTypeScriptModule(new URL("../src/features/remote-control/state.ts", import.meta.url));

test("remote account settings validate the local boundary without inventing credentials", () => {
  const draft = { username: "admin", password: "twelve-characters", port: "18787", publicUrl: "https://codey.example.com" };
  assert.deepEqual(settings.remoteSettings(draft, false), { ...draft, port: 18787 });
  assert.equal("password" in settings.remoteSettings({ ...draft, password: "" }, true), false);
  for (const change of [{ username: "" }, { password: "short" }, { password: "" }, { port: "0" }, { port: "65536" }, { port: "1.2" }, { publicUrl: "javascript:alert(1)" }, { publicUrl: "https://user:secret@example.com" }, { publicUrl: "https://example.com/path" }, { publicUrl: "https://example.com?q=1" }]) {
    assert.throws(() => settings.remoteSettings({ ...draft, ...change }, false));
  }
});

test("remembered login uses same-origin cookies and in-memory CSRF, never a token URL", async t => {
  const oldFetch = globalThis.fetch, oldWindow = globalThis.window;
  const calls = []; const events = new EventTarget(); globalThis.window = events;
  remote.clearRemoteSession();
  globalThis.fetch = async (path, options) => {
    calls.push({ path, options });
    return { ok: true, status: 200, json: async () => path === "/remote/login"
      ? { authenticated: true, csrfToken: "csrf-fixture", expiresAt: "2026-12-03T00:00:00Z" }
      : path === "/remote/session" ? { authenticated: false } : { status: "ok" } };
  };
  t.after(() => { globalThis.fetch = oldFetch; globalThis.window = oldWindow; remote.clearRemoteSession(); });
  await assert.rejects(remote.remoteRequest("/remote/action", {}), /先登录/);
  assert.equal(calls.length, 0);
  await remote.remoteRequest("/remote/login", { username: "admin", password: "fake-only", remember: true });
  assert.equal(calls[0].options.credentials, "same-origin");
  assert.equal(calls[0].options.redirect, "error");
  assert.equal(calls[0].options.headers["X-CSRF-Token"], undefined);
  await remote.remoteRequest("/api/builtin_tools", { action: "status" });
  assert.equal(calls[1].options.headers["X-CSRF-Token"], "csrf-fixture");
  assert.equal(calls[1].path, "/api/builtin_tools");
  await assert.rejects(remote.remoteRequest("https://other.invalid/remote/action", {}), /不允许/);
  await remote.remoteRequest("/remote/session");
  await assert.rejects(remote.remoteRequest("/remote/logout", {}), /先登录/);
});

test("unauthorized non-JSON responses revoke local authentication state before decoding", async t => {
  const oldFetch = globalThis.fetch, oldWindow = globalThis.window;
  const events = new EventTarget(); globalThis.window = events;
  let unauthorized = 0; events.addEventListener("codey-remote-unauthorized", () => unauthorized++);
  globalThis.fetch = async () => ({ ok: true, status: 200, json: async () => ({ authenticated: true, csrfToken: "old" }) });
  await remote.remoteRequest("/remote/session");
  globalThis.fetch = async () => ({ ok: false, status: 401, json: async () => { throw new Error("body must not be echoed"); } });
  t.after(() => { globalThis.fetch = oldFetch; globalThis.window = oldWindow; remote.clearRemoteSession(); });
  await assert.rejects(remote.remoteRequest("/api/runtime_status", {}), /无效响应/);
  assert.equal(unauthorized, 1);
  await assert.rejects(remote.remoteRequest("/remote/action", {}), /先登录/);
});

function authFixture(t) {
  installControllerWindow(t);
  const calls = []; let cleared = 0;
  const h = createControllerHarness(new URL("../src/remote/useRemoteAuth.ts", import.meta.url), "useRemoteAuth", {
    "./transport": { remoteRequest(path, args) { const call = { path, args, ...deferred() }; calls.push(call); return call.promise; },
      clearRemoteSession() {}, clearSubmissions() { cleared++; }, RemoteError: remote.RemoteError },
  });
  return { ...h, calls, get cleared() { return cleared; } };
}

test("login and logout guard duplicate submissions, clear password and retain no credential storage", async t => {
  const h = authFixture(t); h.render(); h.calls[0].resolve({ authenticated: false }); await h.flush();
  h.render().changeUsername(" admin "); h.render().changePassword("fake-password");
  const work = h.render().login(); await h.render().login();
  assert.equal(h.calls.length, 2);
  assert.deepEqual(h.calls[1].args, { username: "admin", password: "fake-password", remember: true });
  h.calls[1].resolve({ authenticated: true, username: "admin", csrfToken: "fixture" }); await work; await h.flush();
  assert.equal(h.render().password, ""); assert.equal(h.render().session.authenticated, true);
  const logout = h.render().logout(); h.calls[2].resolve({ authenticated: false }); await logout; await h.flush();
  assert.equal(h.render().session, null); assert.equal(h.cleared, 1); h.unmount();
  const source = readFileSync(new URL("../src/remote/useRemoteAuth.ts", import.meta.url), "utf8");
  assert.doesNotMatch(source, /localStorage|sessionStorage/);
});

test("remote auth ignores late results after revocation and unmount", async t => {
  const h = authFixture(t); h.render(); h.calls[0].resolve({ authenticated: false }); await h.flush();
  h.render().changeUsername("admin"); h.render().changePassword("fake-password"); const login = h.render().login();
  h.render().unauthorized(); h.calls[1].resolve({ authenticated: true, csrfToken: "late" }); await login; await h.flush();
  assert.equal(h.render().session, null);
  h.render().changePassword("retry-password"); const retry = h.render().login(); h.unmount();
  h.calls.at(-1).reject(new Error("late failure")); await retry; await h.flush(); assert.equal(h.writesAfterUnmount, 0);
});
