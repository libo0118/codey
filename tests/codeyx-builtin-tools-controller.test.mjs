import assert from "node:assert/strict";
import test from "node:test";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";
import { autoStubModule, collectElements, elementProps, elementType, textContent } from "./helpers/jsx-tree.mjs";

const ready = { installed: true, version: "test", projects: [], mcp: { id: "codey", enabled: false, managed: true, automatic: true }, job: null };
const enabled = { ...ready, mcp: { ...ready.mcp, enabled: true }, message: "已启用" };
function fixture(extra = {}, component = false) {
  const calls = [], changes = [], ui = autoStubModule("ui");
  const options = { active: true, runtimeStatus: { running: false, fastContextToolsActive: false }, config: { fastContextTools: false, retained: "draft" }, fastContextToolsStatus: { userConfigured: false, detectionFailed: false }, isBusy: false, onConfigChange: next => changes.push(next), ...extra };
  const h = createControllerHarness(new URL(component ? "../src/features/builtin-tools/BuiltinToolsPage.tsx" : "../src/features/builtin-tools/useBuiltinToolsController.ts", import.meta.url), component ? "BuiltinToolsPage" : "useBuiltinToolsController", {
    "../../api": { invoke(command, args) {
      if (args.tool === "ponytail") return Promise.resolve({ installed: false, enabled: false, settings: { defaultMode: "full", revision: "missing" }, job: null });
      const call = { command, args, ...deferred() }; calls.push(call); return call.promise;
    } },
    "../../SettingsPageHeader": autoStubModule("header"), "../../components/ui": ui, "./components/ui": ui, "@tabler/icons-react": autoStubModule("icons"),
  });
  return { ...h, calls, changes, ui, options, render: () => h.render(options) };
}

test("CodeGraph 仅在活动页读取，不会在挂载时自动安装", async t => {
  installControllerWindow(t);
  const h = fixture({ active: false }); h.render(); await h.render().setEnabled(true);
  assert.equal(h.calls.length, 0);
  h.options.active = true; h.render();
  assert.equal(h.calls.length, 1); assert.equal(h.calls[0].args.action, "status");
  h.unmount();
});

test("首次检测后保留结果，空闲和切换页面不再检测，手动刷新更新状态", async t => {
  const browser = installControllerWindow(t);
  const h = fixture({ active: false }); h.render();
  assert.equal(h.calls.length, 0);
  h.options.active = true; h.render(); h.calls[0].resolve(ready); await h.flush();
  assert.equal(browser.timers.size, 0); browser.tick();
  h.options.active = false; h.render(); h.options.active = true; h.render();
  assert.equal(h.calls.length, 1); assert.deepEqual(h.render().status, ready);
  h.render().refresh(); h.render(); assert.equal(h.calls.length, 2);
  h.calls[1].resolve(enabled); assert.equal((await h.flush()).status.mcp.enabled, true);
  assert.equal(browser.timers.size, 0); h.unmount();
});

test("安装任务只在活动页跟踪，切页后恢复并在完成时停止轮询", async t => {
  const browser = installControllerWindow(t);
  const h = fixture(); h.render(); h.calls[0].resolve(ready); await h.flush();
  const work = h.render().setEnabled(true);
  const installing = { ...ready, job: { phase: "running" } };
  h.calls.at(-1).resolve(installing); await work; await h.flush();
  h.calls.at(-1).resolve(installing); await h.flush(); assert.equal(browser.timers.size, 1);
  h.options.active = false; h.render(); assert.equal(browser.timers.size, 0);
  const count = h.calls.length; browser.tick(); assert.equal(h.calls.length, count);
  h.options.active = true; h.render(); assert.equal(h.calls.length, count + 1);
  h.calls.at(-1).resolve(installing); await h.flush(); browser.tick();
  h.calls.at(-1).resolve(enabled); assert.equal((await h.flush()).status.mcp.enabled, true);
  assert.equal(browser.timers.size, 0);
  h.options.active = false; h.render(); h.options.active = true; h.render();
  assert.equal(h.calls.length, count + 2); h.unmount();
});

test("检测中、全局忙碌、后台安装和用户自管状态禁止启停", async t => {
  installControllerWindow(t);
  for (const mode of ["loading", "busy", "job", "user"]) {
    const h = fixture({ isBusy: mode === "busy" }); h.render();
    if (mode !== "loading") {
      h.calls[0].resolve(mode === "user" ? { ...ready, mcp: { ...ready.mcp, managed: false } } : mode === "job" ? { ...ready, job: { phase: "running" } } : ready);
      await h.flush();
    }
    await h.render().setEnabled(true); assert.equal(h.calls.length, 1); h.unmount();
  }
});

test("启停防止重复提交和旧读取覆盖，失败保留草稿并支持显式重试", async t => {
  installControllerWindow(t);
  const h = fixture(); h.render(); h.calls[0].resolve(ready); await h.flush();
  h.render().refresh(); h.render(); const old = h.calls.at(-1);
  const c = h.render(); const work = c.setEnabled(true); await c.setEnabled(false); c.refresh();
  assert.deepEqual(h.calls.at(-1).args, { action: "set_enabled", enabled: true }); assert.equal(h.calls.length, 3);
  old.reject(new Error("old read")); await h.flush(); assert.equal(h.render().error, ""); assert.equal(h.render().busy, true);
  h.calls.at(-1).reject(new Error("write failed")); await work; await h.flush();
  h.calls.at(-1).resolve(ready); await h.flush();
  assert.equal(h.render().error, "write failed"); assert.equal(h.options.config.retained, "draft");
  const retry = h.render().setEnabled(true); h.calls.at(-1).resolve(enabled); await retry;
  const result = await h.flush(); assert.equal(result.notice, "已启用"); assert.equal(result.error, ""); assert.equal(result.status.mcp.enabled, true); h.unmount();
});

test("刷新作废旧读结果，当前读取失败停止轮询并可手动恢复", async t => {
  const browser = installControllerWindow(t);
  for (const failed of [false, true]) {
    const h = fixture(); h.render().refresh(); h.render();
    h.calls[1].reject(new Error("read failed")); await h.flush();
    if (failed) h.calls[0].reject(new Error("stale")); else h.calls[0].resolve(enabled);
    assert.equal((await h.flush()).error, "read failed"); assert.equal(browser.timers.size, 0);
    h.options.active = false; h.render(); h.options.active = true; h.render();
    assert.equal(h.calls.length, 2); assert.equal(h.render().error, "read failed");
    h.render().refresh(); h.render(); h.calls.at(-1).resolve(ready);
    assert.equal((await h.flush()).error, ""); assert.equal(browser.timers.size, 0); h.unmount();
  }
});

test("隐藏重开作废旧读取与写入，写入锁保持到完成，随后重新读取", async t => {
  installControllerWindow(t);
  for (const mutation of [false, true]) {
    for (const failed of [false, true]) {
      const h = fixture(); h.render();
      if (mutation) { h.calls[0].resolve(ready); await h.flush(); }
      const work = mutation ? h.render().setEnabled(true) : Promise.resolve(); const old = h.calls.at(-1);
      h.render(); h.options.active = false; h.render(); h.options.active = true; h.render();
      const count = h.calls.length; await h.render().setEnabled(true); assert.equal(h.calls.length, count);
      if (failed) old.reject(new Error("late")); else old.resolve(enabled);
      await work; const c = await h.flush();
      assert.equal(c.status?.mcp.enabled ?? false, false); assert.equal(c.notice, ""); assert.equal(c.error, "");
      h.calls.at(-1).resolve(enabled); assert.equal((await h.flush()).status.mcp.enabled, true); h.unmount();
    }
  }
});

test("卸载清理轮询，迟到成功、失败与 finally 不再写入", async t => {
  const browser = installControllerWindow(t);
  for (const mutation of [false, true]) {
    for (const failed of [false, true]) {
      const h = fixture(); h.render();
      if (mutation) { h.calls[0].resolve(ready); await h.flush(); }
      const work = mutation ? h.render().setEnabled(true) : Promise.resolve(); const count = h.calls.length;
      h.unmount(); assert.equal(browser.timers.size, 0);
      if (failed) h.calls.at(-1).reject(new Error("late")); else h.calls.at(-1).resolve(enabled);
      await work; await h.flush(); assert.equal(h.writesAfterUnmount, 0); assert.equal(h.calls.length, count);
    }
  }
});

test("内置工具视图接线保留开关状态、错误提示、重试和刷新行为", async t => {
  installControllerWindow(t);
  const h = fixture({}, true); h.render(); h.calls[0].resolve(ready); await h.flush();
  const find = (type, label) => collectElements(h.render(), node => elementType(node) === type && (elementProps(node)["aria-label"] === label || textContent(node) === label))[0];
  elementProps(find(h.ui.Switch, "启用 CodeGraph 自动索引")).onCheckedChange(true);
  assert.equal(elementProps(find(h.ui.Switch, "启用 CodeGraph 自动索引")).disabled, true);
  h.calls.at(-1).resolve({ ...ready, job: { phase: "failed", message: "install failed" } }); await h.flush();
  elementProps(find(h.ui.Button, "重试启用")).onClick(); h.render(); h.calls.at(-1).reject(new Error("retry failed")); await h.flush();
  assert.match(textContent(h.render()), /retry failed/);
  elementProps(find(h.ui.Button, "刷新")).onClick(); h.render(); h.calls.at(-1).resolve(enabled); await h.flush();
  assert.doesNotMatch(textContent(h.render()), /retry failed/); assert.equal(elementProps(find(h.ui.Switch, "启用 CodeGraph 自动索引")).checked, true); h.unmount();
});
