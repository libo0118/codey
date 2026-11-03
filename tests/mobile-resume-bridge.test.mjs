import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const cdp = readFileSync(new URL("../backend/src/cdp.rs", import.meta.url), "utf8");
const remote = cdp.slice(cdp.indexOf("pub(crate) async fn remote_control_request("), cdp.indexOf("pub async fn refresh_model_whitelist("));
const adapter = readFileSync(new URL("../public/codey-remote-control.js", import.meta.url), "utf8");

// Evaluate the actual Rust-embedded JS, not a second implementation of its policy.
function formatScript(name, variables) {
  const match = remote.match(new RegExp(`let ${name} = format!\\(\\s*r#"([\\s\\S]*?)"#`));
  assert.ok(match, `${name} renderer script is present`);
  return match[1].replace(/\{\{|\}\}|\{(\w+)\}/g, (value, key) => {
    if (value === "{{") return "{";
    if (value === "}}") return "}";
    assert.ok(Object.hasOwn(variables, key), `known Rust format argument: ${key}`);
    return variables[key];
  });
}

async function response(window, action = "resume") {
  const script = formatScript("script", {
    args: JSON.stringify({ action, threadId: "thread-1" }),
    expires_at: String(Date.now() + 10_000),
  });
  return JSON.parse(await vm.runInNewContext(script, { window }));
}

test("CDP remote diagnostics expose only allowlisted stage messages", async () => {
  const messages = {
    codey_remote_controller_not_ready: "桌面会话接口尚未就绪，请稍后重试",
    codey_remote_unsupported: "当前 Codex 暂不支持远程读取会话，请更新 Codey 后重试",
    codey_remote_not_ready: "桌面尚未就绪，暂时无法读取此会话",
    codey_remote_disconnected: "Codex 宿主会话接口未连接，请稍后重试",
    codey_remote_restart_required: "桌面会话工具版本已更新，请在本机通过 Codey 重启 Codex 后重新连接",
  };
  for (const [code, message] of Object.entries(messages)) {
    const error = Object.assign(Error("private prompt https://private.invalid/?token=secret"), { code });
    let calls = 0;
    const result = await response({ __codeyRemoteControl: async () => { calls += 1; throw error; } });
    assert.deepEqual(result, { status: "failed", code, message });
    assert.equal(calls, 1);
    assert.ok(!JSON.stringify(result).includes("private"));
  }
});

test("CDP keeps unknown operation outcomes unconfirmed and never leaks or replays native errors", async () => {
  for (const code of [undefined, "native_failure", "__proto__", "constructor", "toString"]) {
    let calls = 0;
    const error = Object.assign(Error("secret native stack and prompt"), { code });
    const result = await response({ __codeyRemoteControl: async () => { calls += 1; throw error; } }, "create");
    assert.deepEqual(result, { status: "failed", message: "桌面未确认远程操作，请检查会话状态后重试" });
    assert.equal(calls, 1);
    assert.ok(!JSON.stringify(result).includes("secret"));
  }
});

test("CDP distinguishes a missing controller and preserves successful native payloads", async () => {
  const missing = await response({});
  assert.equal(missing.code, "codey_remote_controller_not_ready");
  const window = {};
  vm.runInNewContext(adapter, { window });
  assert.equal((await response(window)).code, "codey_remote_controller_not_ready");
  for (const payload of [{ status: "ok" }, { id: "created", firstTurn: "outcome-unknown" }]) {
    assert.deepEqual(await response({ __codeyRemoteControl: async () => payload }), payload);
  }
});

test("CDP readiness requires the current remote and session-controller generations", () => {
  const probe = remote.match(/let ready =[\s\S]*?r#"([\s\S]*?)"#/);
  assert.ok(probe);
  const window = {
    __codeyRemoteControl: Object.assign(() => {}, { version: 1 }),
    __codeyLoadCodexSessionController: Object.assign(() => {}, { remoteVersion: 1 }),
    __codeySessionToolsInjectLoaded: true,
  };
  const ready = () => vm.runInNewContext(probe[1], { window });
  assert.equal(ready(), false, "an old remote adapter is not ready");
  window.__codeyRemoteControl.version = 2;
  assert.equal(ready(), true);
  delete window.__codeyLoadCodexSessionController.remoteVersion;
  assert.equal(ready(), false, "a legacy session loader is not ready");
  window.__codeyLoadCodexSessionController.remoteVersion = 1;
  window.__codeySessionToolsInjectLoaded = false;
  assert.equal(ready(), false, "a failed installation must be reloaded");
});

test("CDP refuses an installed legacy generation before loading or disposing desktop session tools", () => {
  const load = formatScript("session_tools_load", { script: "window.loads += 1;" });
  for (const [loaded, generation] of [[true, undefined], [true, 1], [false, undefined], [undefined, undefined]]) {
    const state = { selectedSession: "thread-1", selectedMessages: ["turn-1"] };
    const window = {
      __codeySessionToolsInjectLoaded: loaded,
      __codeyLoadCodexSessionController: Object.assign(() => {}, { remoteVersion: generation }),
      __codeySessionToolsInstall: { dispose() { assert.fail("must not dispose desktop controls"); } },
      loads: 0,
      state,
    };
    const stage = vm.runInNewContext(load, { window });
    const restartRequired = loaded === true && generation !== 1;
    assert.equal(stage, restartRequired ? "codey_remote_restart_required" : "");
    assert.equal(window.loads, restartRequired ? 0 : 1);
    assert.equal(window.__codeySessionToolsInjectLoaded, loaded);
    assert.equal(window.state, state);
    assert.deepEqual(state, { selectedSession: "thread-1", selectedMessages: ["turn-1"] });
  }
  assert.match(remote, /runtime_value\(&prepared\)[\s\S]*?Some\("codey_remote_restart_required"\)[\s\S]*?return Err\("桌面会话工具版本已更新，请在本机通过 Codey 重启 Codex 后重新连接"\.into\(\)\)/);
});
