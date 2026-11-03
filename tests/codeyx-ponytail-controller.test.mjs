import assert from "node:assert/strict";
import test from "node:test";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";
import { autoStubModule, collectElements, elementProps, elementType, textContent } from "./helpers/jsx-tree.mjs";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

const ready = { installed: false, enabled: false, version: null, conflict: null, job: null,
  settings: { defaultMode: "full", effectiveMode: "full", environmentOverride: null, revision: "revision1", configPath: "fixture" } };
const installed = { ...ready, installed: true, enabled: true, version: "5.3.2", message: "已启用" };
function fixture(extra = {}, component = false) {
  const calls = [], ui = autoStubModule("ui");
  const options = { active: true, isBusy: false, ...extra };
  const h = createControllerHarness(new URL(component ? "../src/features/builtin-tools/PonytailCard.tsx" : "../src/features/builtin-tools/usePonytailController.ts", import.meta.url), component ? "PonytailCard" : "usePonytailController", {
    "../../api": { invoke(command, args) { const call = { command, args, ...deferred() }; calls.push(call); return call.promise; } }, "../../components/ui": ui,
  });
  return { ...h, calls, ui, options, render: () => h.render(options) };
}

test("Ponytail 模式只允许官方四种默认值", async () => {
  const { isPonytailMode } = await loadTypeScriptModule(new URL("../src/features/builtin-tools/ponytail.ts", import.meta.url));
  for (const mode of ["lite", "full", "ultra", "off"]) assert.equal(isPonytailMode(mode), true);
  for (const mode of ["review", "", null, true, "FULL"]) assert.equal(isPonytailMode(mode), false);
});

test("Ponytail 活动页读取、安装互斥和失败后重试", async t => {
  installControllerWindow(t);
  const h = fixture({ active: false }); h.render(); await h.render().setEnabled(true); assert.equal(h.calls.length, 0);
  h.options.active = true; h.render(); h.calls[0].resolve(ready); await h.flush();
  const c = h.render(); const work = c.setEnabled(true); await c.setEnabled(true);
  assert.equal(h.calls.length, 2); assert.deepEqual(h.calls.at(-1).args, { tool: "ponytail", action: "set_enabled", enabled: true });
  h.calls.at(-1).reject(new Error("install failed")); await work; await h.flush();
  h.calls.at(-1).resolve(ready); await h.flush(); assert.equal(h.render().error, "install failed");
  const retry = h.render().setEnabled(true); h.calls.at(-1).resolve(installed); await retry;
  assert.equal(h.render().notice, "已启用"); assert.equal(h.render().status.installed, true); h.unmount();
});

test("草稿与版本绑定：轮询、保存失败和配置冲突保留用户修改", async t => {
  const browser = installControllerWindow(t); const h = fixture(); h.render(); h.calls[0].resolve(ready); await h.flush();
  h.render().changeMode("ultra"); assert.equal(h.render().dirty, true);
  browser.tick(); h.calls.at(-1).resolve({ ...ready, settings: { ...ready.settings, defaultMode: "lite", revision: "revision2" } }); await h.flush();
  assert.equal(h.render().mode, "ultra");
  const work = h.render().save(); assert.deepEqual(h.calls.at(-1).args, { tool: "ponytail", action: "save_settings", defaultMode: "ultra", revision: "revision1" });
  h.calls.at(-1).reject(new Error("配置已被修改")); await work; await h.flush(); assert.equal(h.render().mode, "ultra");
  h.calls.at(-1).resolve({ ...ready, settings: { ...ready.settings, defaultMode: "lite", revision: "revision2" } }); await h.flush();
  h.render().reset(); assert.equal(h.render().mode, "lite"); assert.equal(h.render().dirty, false);
  h.render().changeMode("off"); const save = h.render().save();
  assert.equal(h.calls.at(-1).args.revision, "revision2");
  h.calls.at(-1).resolve({ ...ready, settings: { ...ready.settings, defaultMode: "off", revision: "revision3" }, message: "saved" }); await save;
  assert.equal(h.render().dirty, false); assert.equal(h.render().mode, "off"); h.unmount();
});

test("安装进行中、加载和忙碌禁止修改，来源冲突禁止接管", async t => {
  installControllerWindow(t);
  for (const mode of ["loading", "busy", "running", "conflict"]) {
    const h = fixture({ isBusy: mode === "busy" }); h.render();
    if (mode !== "loading") { h.calls[0].resolve({ ...ready, job: mode === "running" ? { phase: "running" } : null, conflict: mode === "conflict" ? "custom source" : null }); await h.flush(); }
    await h.render().setEnabled(true); assert.equal(h.calls.length, 1); h.unmount();
  }
});

test("隐藏重开、卸载与旧结果不能覆盖当前状态或草稿", async t => {
  const browser = installControllerWindow(t);
  for (const mutation of [false, true]) for (const fail of [false, true]) {
    const h = fixture(); h.render();
    if (mutation) { h.calls[0].resolve(ready); await h.flush(); h.render().changeMode("lite"); }
    const work = mutation ? h.render().setEnabled(true) : Promise.resolve(); const old = h.calls.at(-1);
    h.options.active = false; h.render(); assert.equal(browser.timers.size, 0);
    h.options.active = true; h.render();
    if (fail) old.reject(new Error("late")); else old.resolve(installed);
    await work; await h.flush(); assert.equal(h.render().notice, ""); assert.equal(h.render().error, "");
    h.calls.at(-1).resolve(ready); await h.flush(); if (mutation) assert.equal(h.render().mode, "lite");
    const next = h.render().setEnabled(true); h.unmount(); h.calls.at(-1).resolve(installed); await next;
    assert.equal(h.writesAfterUnmount, 0); assert.equal(browser.timers.size, 0);
  }
});

test("读取失败禁止使用过时状态，刷新恢复且保留草稿", async t => {
  const browser = installControllerWindow(t); const h = fixture(); h.render(); h.calls[0].resolve(ready); await h.flush();
  h.render().changeMode("lite"); browser.tick(); h.calls.at(-1).reject(new Error("read failed")); await h.flush();
  const count = h.calls.length; await h.render().save(); assert.equal(h.calls.length, count); assert.equal(browser.timers.size, 0);
  h.render().refresh(); h.render(); h.calls.at(-1).resolve(ready); await h.flush();
  assert.equal(h.render().mode, "lite"); assert.equal(h.render().error, ""); h.unmount();
});

test("Ponytail 卡片展示环境覆盖并接线模式保存与安装", async t => {
  installControllerWindow(t); const h = fixture({}, true); h.render(); h.calls[0].resolve({ ...ready, settings: { ...ready.settings, environmentOverride: "off", effectiveMode: "off" } }); await h.flush();
  const find = (type, label) => collectElements(h.render(), node => elementType(node) === type && (elementProps(node)["aria-label"] === label || textContent(node) === label))[0];
  assert.match(textContent(h.render()), /PONYTAIL_DEFAULT_MODE/); assert.match(textContent(h.render()), /\/hooks/);
  elementProps(find(h.ui.Select, "Ponytail 默认模式")).onChange("ultra");
  elementProps(find(h.ui.Button, "保存 Ponytail 参数")).onClick();
  assert.equal(h.calls.at(-1).args.defaultMode, "ultra"); h.calls.at(-1).reject(new Error("save failed")); await h.flush();
  assert.match(textContent(h.render()), /save failed/); h.unmount();
});

test("预览后端模拟安装、独立开关、参数校验与版本冲突", async t => {
  t.mock.timers.enable({ apis: ["Date"], now: 1000 });
  const request = createControllerHarness(new URL("../src/dev/builtinToolsMock.ts", import.meta.url), "createBuiltinToolsPreview").render();
  const call = args => request({ tool: "ponytail", ...args });
  const initial = await call({ action: "status" }); assert.equal(initial.settings.defaultMode, "full");
  await call({ action: "set_enabled", enabled: true }); await assert.rejects(call({ action: "set_enabled", enabled: true }), /进行中/);
  t.mock.timers.tick(3000); assert.equal((await call({ action: "status" })).installed, true);
  await assert.rejects(call({ action: "save_settings", defaultMode: "review", revision: "missing" }), /无效/);
  await assert.rejects(call({ action: "save_settings", defaultMode: "lite", revision: "old" }), /已被修改/);
  assert.equal((await call({ action: "save_settings", defaultMode: "off", revision: "missing" })).settings.defaultMode, "off");
  const disabled = await call({ action: "set_enabled", enabled: false }); assert.equal(disabled.installed, true); assert.equal(disabled.enabled, false);
  assert.equal((await request({ action: "status" })).installed, false);
});
