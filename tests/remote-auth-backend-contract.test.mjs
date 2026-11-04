import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";

const adapter = fs.readFileSync(new URL("../public/codey-remote-control.js", import.meta.url), "utf8");
const server = fs.readFileSync(new URL("../backend/src/remote_control/server.rs", import.meta.url), "utf8");
const auth = fs.readFileSync(new URL("../backend/src/remote_control/auth.rs", import.meta.url), "utf8");

function runtime(manager = {}) {
  const window = {
    setTimeout, clearTimeout,
    __codeyModelWhitelistPatch: { ensureRequestGate: async () => true, requestGateReady: () => true },
    __codeyLoadCodexSessionController: Object.assign(async () => ({ kind: "manager", manager }), { remoteVersion: 1 }),
    __codeyAppServerManagerResolverFromModule: () => () => manager,
    __codeyAppServerManagerRegistryResolverFromModule: () => null,
    __codeyAppServerManagerFromReact: resolver => resolver({ get: () => null }, "local"),
  };
  const originalController = window.__codeyLoadCodexSessionController;
  const context = vm.createContext({ window, document: { scripts: [], querySelectorAll: () => [] }, performance: { getEntriesByType: () => [] }, URL });
  vm.runInContext(adapter, context);
  return { window, context, originalController };
}

test("remote resume reuses the existing native controller and never replaces hooks", async () => {
  const calls = [];
  const manager = { resumeConversation: async args => { calls.push(args); return { status: "ready" }; } };
  const app = runtime(manager);
  const result = await app.window.__codeyRemoteControl({ action: "resume", threadId: "thread" }, Date.now() + 10000);
  assert.equal(result.status, "ok");
  assert.equal(calls.length, 1);
  assert.equal(calls[0].conversationId, "thread");
  assert.equal(calls[0].model, null);
  assert.equal(calls[0].serviceTier, null);
  assert.equal(app.window.__codeyLoadCodexSessionController, app.originalController);
  const first = app.window.__codeyRemoteControl;
  vm.runInContext(adapter, app.context);
  assert.equal(app.window.__codeyRemoteControl, first);
});

test("expired, disposed or unconfirmed operations cannot submit or silently replay", async () => {
  let calls = 0;
  const app = runtime({ resumeConversation: async () => { calls++; throw new Error("lost acknowledgement"); } });
  await assert.rejects(app.window.__codeyRemoteControl({ action: "resume" }, Date.now() - 1), /过期/);
  assert.equal(calls, 0);
  await assert.rejects(app.window.__codeyRemoteControl({ action: "resume" }, Date.now() + 10000), /lost acknowledgement/);
  assert.equal(calls, 1);
  const original = app.window.__codeyRemoteControl;
  original.dispose();
  await assert.rejects(original({ action: "resume" }, Date.now() + 10000), /过期/);
  assert.equal(calls, 1);
});

test("native creation retains creation-input tools while defaults expose no credentials", async () => {
  const creationInputs = { tools: ["existing-tool"], instructions: "native instructions" };
  let submitted;
  const manager = {
    sendRequest: async () => ({ config: { model: "current-model", model_reasoning_effort: "high", service_tier: "default", api_key: "secret" } }),
    startConversation: async (params, options) => {
      submitted = { params, inputs: await options.readThreadCreationInputs() };
      return { status: "created", conversationId: "created", firstTurn: { status: "started" } };
    },
  };
  const app = runtime(manager);
  delete app.window.__codeyModelWhitelistPatch;
  app.context.document.scripts.push({ src: "app://codex/assets/app-initial-test.js" });
  app.window.__codeyImportCodexAsset = async () => ({
    factory: function (scope, hostId) {
      return { scope: scope, hostId: hostId, requestClient: scope.get("client"), dynamicTools: new Map(), params: { scope }, readCreationInputs: async () => creationInputs };
    },
  });
  const defaults = await app.window.__codeyRemoteControl({ action: "defaults", cwd: "/project" }, Date.now() + 10000);
  assert.deepEqual(JSON.parse(JSON.stringify(defaults)), { model: "current-model", effort: "high", serviceTier: "default" });
  assert.ok(!JSON.stringify(defaults).includes("secret"));
  app.window.__codeyModelWhitelistPatch = { ensureRequestGate: async () => true, requestGateReady: () => true };
  const params = { input: ["task"], useAppServerPermissionDefault: true };
  const result = await app.window.__codeyRemoteControl({ action: "create", params }, Date.now() + 10000);
  assert.equal(result.id, "created");
  assert.equal(submitted.params, params);
  assert.equal(submitted.inputs, creationInputs);
});

function modelWriteRuntime() {
  const writes = [];
  const inputs = { readCreationInputs: async () => "native inputs" };
  const app = runtime({
    async resumeConversation() { writes.push("resume"); return { status: "ready" }; },
    async startConversation(_params, options) {
      await options.readThreadCreationInputs();
      writes.push("create");
      return { status: "created", conversationId: "created" };
    },
  });
  app.context.document.scripts.push({ src: "app://codex/assets/app-initial-fixture.js" });
  app.window.__codeyImportCodexAsset = async () => ({
    factory: function (scope, hostId) {
      return { scope: scope, hostId: hostId, requestClient: scope.get("client"), dynamicTools: new Map(), readCreationInputs: (...args) => inputs.readCreationInputs(...args) };
    },
  });
  const run = action => app.window.__codeyRemoteControl({ action, threadId: "thread", params: {} }, Date.now() + 10_000);
  return { ...app, inputs, writes, run };
}

test("remote create and resume refuse missing, failed, false or uncertified routing before native submission", async () => {
  for (const action of ["resume", "create"]) {
    for (const invalid of ["missing-patch", "missing", "rejected", "false", "unverified", "not-ready"]) {
      const app = modelWriteRuntime();
      const patch = app.window.__codeyModelWhitelistPatch;
      let notices = 0;
      patch.notifyRequestGateUnavailable = () => { notices++; };
      if (invalid === "missing-patch") delete app.window.__codeyModelWhitelistPatch;
      if (invalid === "missing") delete patch.ensureRequestGate;
      if (invalid === "rejected") patch.ensureRequestGate = async () => { throw Error("PRIVATE_GATE_ERROR"); };
      if (invalid === "false") patch.ensureRequestGate = async () => false;
      if (invalid === "unverified") delete patch.requestGateReady;
      if (invalid === "not-ready") patch.requestGateReady = () => false;
      await assert.rejects(app.run(action), { code: "codey_remote_request_gate_not_ready", message: "模型线路尚未就绪，远程操作未发送，请稍后重试" });
      assert.deepEqual(app.writes, []);
      assert.equal(notices, invalid === "missing-patch" ? 0 : 1);
    }
  }
});

test("late or replaced gate results never resume, create, or automatically replay", async () => {
  for (const action of ["resume", "create"]) {
    for (const invalid of ["timeout", "disposed", "replaced"]) {
      const app = modelWriteRuntime();
      let finish, timeout, cleared = 0;
      app.window.setTimeout = callback => { timeout = callback; return 1; };
      app.window.clearTimeout = () => { cleared++; };
      app.window.__codeyModelWhitelistPatch.ensureRequestGate = () => new Promise(resolve => { finish = resolve; });
      const pending = app.run(action);
      await new Promise(setImmediate);
      assert.equal(typeof finish, "function");
      if (invalid === "timeout") timeout();
      else {
        if (invalid === "disposed") app.window.__codeyRemoteControl.dispose();
        else app.window.__codeyModelWhitelistPatch = { requestGateReady: () => true };
        finish(true);
      }
      await assert.rejects(pending, /过期|模型线路尚未就绪/);
      finish(true);
      await new Promise(setImmediate);
      assert.deepEqual(app.writes, []);
      assert.equal(cleared, 1);
    }
    const app = modelWriteRuntime();
    app.window.__codeyModelWhitelistPatch.requestGateReady = () => {
      queueMicrotask(() => { app.window.__codeyModelWhitelistPatch = { requestGateReady: () => true }; });
      return true;
    };
    await assert.rejects(app.run(action), /模型线路尚未就绪/);
    assert.deepEqual(app.writes, []);
  }
});

test("native creation input reads cannot continue after request-gate replacement", async () => {
  const app = modelWriteRuntime();
  app.inputs.readCreationInputs = async () => {
    app.window.__codeyModelWhitelistPatch = { requestGateReady: () => true };
    return "native inputs";
  };
  await assert.rejects(app.run("create"), /模型线路尚未就绪/);
  assert.deepEqual(app.writes, []);
});

test("backend account contracts use an existing KDF and separate CSRF instead of pairing", () => {
  assert.match(auth, /pbkdf2::verify\(\s*pbkdf2::PBKDF2_HMAC_SHA256/);
  assert.match(auth, /const ITERATIONS: u32 = 600_000/);
  assert.match(auth, /secret\(32\)/);
  assert.match(auth, /digest: digest\(&token\)/);
  assert.match(server, /x-csrf-token/);
  assert.match(server, /HttpOnly; SameSite=Strict/);
  assert.doesNotMatch(server, /\/remote\/pair|pairing_code|x-forwarded-host|x-forwarded-proto/);
  const allowlist = server.slice(server.indexOf("fn panel_command("), server.indexOf("fn panel_arguments_allowed("));
  for (const command of ["runtime_status", "load_codey_config", "builtin_tools", "list_codey_plugins"]) assert.ok(allowlist.includes(`"${command}"`));
  for (const command of ["save_codey_config", "config_backup", "delete_codex_sessions", "install_codey_plugin", "restart_codey", "configure_remote_control"]) assert.ok(!allowlist.includes(`"${command}"`));
});
