import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";

const adapter = fs.readFileSync(new URL("../public/codey-remote-control.js", import.meta.url), "utf8");
const server = fs.readFileSync(new URL("../backend/src/remote_control/server.rs", import.meta.url), "utf8");
const auth = fs.readFileSync(new URL("../backend/src/remote_control/auth.rs", import.meta.url), "utf8");

function runtime(manager = {}) {
  const window = {
    __codeyLoadCodexSessionController: async () => ({ kind: "manager", manager }),
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
  app.context.document.scripts.push({ src: "app://codex/assets/app-initial-test.js" });
  app.window.__codeyImportCodexAsset = async () => ({
    factory: function (scope, hostId) {
      return { scope: scope, hostId: hostId, requestClient: scope.get("client"), dynamicTools: new Map(), params: { scope }, readCreationInputs: async () => creationInputs };
    },
  });
  const defaults = await app.window.__codeyRemoteControl({ action: "defaults", cwd: "/project" }, Date.now() + 10000);
  assert.deepEqual(JSON.parse(JSON.stringify(defaults)), { model: "current-model", effort: "high", serviceTier: "default" });
  assert.ok(!JSON.stringify(defaults).includes("secret"));
  const params = { input: ["task"], useAppServerPermissionDefault: true };
  const result = await app.window.__codeyRemoteControl({ action: "create", params }, Date.now() + 10000);
  assert.equal(result.id, "created");
  assert.equal(submitted.params, params);
  assert.equal(submitted.inputs, creationInputs);
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
