import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";
import { autoStubModule, textContent } from "./helpers/jsx-tree.mjs";

test("remote console requests only the approved read-only summaries", async t => {
  installControllerWindow(t);
  const calls = [];
  const h = createControllerHarness(new URL("../src/remote/ReadOnlyConsole.tsx", import.meta.url), "ReadOnlyConsole", {
    "../api": { invoke(command, args) { const call = { command, args, ...deferred() }; calls.push(call); return call.promise; } },
    "@tabler/icons-react": autoStubModule("icons"),
  });
  h.render({ active: false }); assert.equal(calls.length, 0);
  h.render({ active: true });
  assert.deepEqual(calls.map(call => call.command), ["runtime_status", "load_codey_config", "builtin_tools", "builtin_tools", "list_codey_plugins"]);
  for (const call of calls.filter(call => call.command === "builtin_tools")) assert.equal(call.args.action, "status");
  calls[0].resolve({ running: true, codexAppVersion: "test", clientPlatform: "windows", fastContextToolsActive: true });
  calls[1].resolve({ config: { defaultModel: "example", fastContextTools: true, profiles: [{ id: "p", name: "safe-route", enabled: true }], apiKey: "must-not-render" } });
  calls[2].resolve({ installed: true, version: "1.6.2", mcp: { enabled: true } });
  calls[3].resolve({ installed: true, enabled: true, version: "5.3.3" });
  calls[4].resolve({ plugins: [] }); await h.flush();
  const text = textContent(h.render()); assert.match(text, /safe-route/); assert.match(text, /只读查看/); assert.doesNotMatch(text, /must-not-render/); h.unmount();
  const entry = readFileSync(new URL("../src/remote/main.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(entry, /import \{ App \}|<App\b|remote\/pair|pairingCode/);
});

test("a late read-only console response cannot overwrite a reopened page", async t => {
  installControllerWindow(t);
  const calls = [];
  const h = createControllerHarness(new URL("../src/remote/ReadOnlyConsole.tsx", import.meta.url), "ReadOnlyConsole", {
    "../api": { invoke() { const call = deferred(); calls.push(call); return call.promise; } }, "@tabler/icons-react": autoStubModule("icons"),
  });
  h.render({ active: true }); h.render({ active: false }); h.render({ active: true });
  calls.slice(0, 5).forEach(call => call.reject(new Error("stale failure"))); await h.flush();
  assert.doesNotMatch(textContent(h.render()), /stale failure/);
  h.unmount(); calls.slice(5).forEach(call => call.reject(new Error("late failure"))); await h.flush(); assert.equal(h.writesAfterUnmount, 0);
});
