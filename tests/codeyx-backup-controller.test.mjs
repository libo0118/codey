import assert from "node:assert/strict";
import test from "node:test";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";
import { createModuleGraph } from "./helpers/jsx-tree.mjs";

const state = createModuleGraph(new URL("../src/features/backup/state.ts", import.meta.url)).exports;
const savedSettings = { url: "https://dav.example.com/backup.json", username: "tester", passwordConfigured: true, allowHttp: false, revision: "dav1" };
const summary = { createdAt: "2026-10-18T00:00:00Z", routeCount: 2, channelCount: 0, settingsRevision: 7 };
const file = { name: "backup.json", size: 100, text: async () => "backup-content" };
function fixture() {
  const calls = [], adopted = []; let reloads = 0, appBusy = null;
  const options = { active: true, remote: false, isBusy: false, dirty: false,
    setBusy(value) { appBusy = typeof value === "function" ? value(appBusy) : value; },
    onSaved(value) { adopted.push(value); return true; }, onReload: async () => { reloads++; } };
  const harness = createControllerHarness(new URL("../src/features/backup/useBackupController.ts", import.meta.url), "useBackupController", {
    "../../api": { invoke(command, args) { const call = { command, args, ...deferred() }; calls.push(call); return call.promise; }, isCodeyApiError: error => error.explicit === true },
  });
  const render = () => harness.render(options);
  const ready = async () => { render(); calls.at(-1).resolve(savedSettings); await harness.flush(); return render(); };
  const inspect = async () => { const work = render().importFile(file); await harness.flush(); calls.at(-1).resolve(summary); await work; await harness.flush(); };
  return { ...harness, render, ready, inspect, options, calls, adopted, get reloads() { return reloads; }, get appBusy() { return appBusy; } };
}

test("备份文件名兼容 Windows，WebDAV 仅接受明确文件地址与 HTTP 授权", () => {
  assert.match(state.backupFilename(new Date(summary.createdAt)), /^Codey-backup-[\dTZ-]+\.json$/);
  const draft = state.webdavDraft(savedSettings); assert.equal(draft.password, "");
  assert.equal(state.validateWebdav(draft), "");
  for (const url of ["", "file:///backup.json", "https://name:secret@host/a.json", "https://host/", "https://host/a.json?key=x", "http://host/a.json"]) {
    assert.notEqual(state.validateWebdav({ ...draft, url }), "");
  }
  assert.equal(state.validateWebdav({ ...draft, url: "http://host/a.json", allowHttp: true }), "");
});

test("仅活动桌面页面读取设置，旧读取不能覆盖新会话", async t => {
  installControllerWindow(t);
  for (const key of ["active", "remote"]) {
    const h = fixture(); h.options[key] = key === "remote"; await h.render().exportBackup(); assert.equal(h.calls.length, 0); h.unmount();
  }
  const h = fixture(); h.render(); h.options.active = false; h.render(); h.options.active = true; h.render();
  h.calls[1].resolve(savedSettings); await h.flush(); h.calls[0].reject(new Error("stale")); await h.flush();
  assert.equal(h.render().error, ""); assert.equal(h.render().draft.url, savedSettings.url); h.unmount();
});

test("保存即时防重复，失败保留连接草稿及密码", async t => {
  installControllerWindow(t); const h = fixture(); (await h.ready()).change("password", "fake-password");
  const c = h.render(); const work = c.saveSettings(); await c.saveSettings();
  assert.equal(h.calls.length, 2); assert.equal(h.appBusy, "backup");
  h.calls[1].reject(new Error("写入失败")); await work; await h.flush();
  assert.equal(h.render().draft.password, "fake-password"); assert.equal(h.appBusy, null);
  assert.match(h.render().error, /写入失败/); assert.equal(h.render().cloudBlocked, true); h.unmount();
});

test("未保存配置和超大空文件均不发送备份请求", async t => {
  installControllerWindow(t); const h = fixture(); await h.ready();
  h.options.dirty = true; await h.render().exportBackup(); await h.render().importFile(file); await h.render().download();
  assert.equal(h.calls.length, 1); h.options.dirty = false;
  for (const size of [0, state.MAX_BACKUP_BYTES + 1]) { await h.render().importFile({ ...file, size }); await h.flush(); }
  assert.equal(h.calls.length, 1); assert.match(h.render().error, /4 MiB/); h.unmount();
});

test("导入先预览，取消不恢复；确认使用预览版本并采纳恢复结果", async t => {
  installControllerWindow(t); const h = fixture(); await h.ready(); await h.inspect();
  assert.equal(h.calls.at(-1).args.action, "inspect"); assert.equal(h.render().preview.settingsRevision, 7);
  h.render().cancelRestore(); await h.render().restore(); assert.equal(h.calls.length, 2);
  await h.inspect(); const work = h.render().restore(); await h.render().restore();
  assert.deepEqual(h.calls.at(-1).args, { action: "restore", content: "backup-content", settingsRevision: 7, confirmed: true });
  const result = { recoveryPath: "before-restore.json", restartRequired: true, warning: "热更新需重启" };
  h.calls.at(-1).resolve(result); await work; await h.flush();
  assert.deepEqual(h.adopted, [result]); assert.equal(h.render().preview, null);
  assert.match(h.render().notice, /before-restore.json/); assert.match(h.render().notice, /热更新需重启/); h.unmount();
});

test("隐藏后恢复回包不采纳旧状态，重开后刷新；卸载后不回写", async t => {
  installControllerWindow(t);
  for (const unmount of [false, true]) {
    const h = fixture(); await h.ready(); await h.inspect(); const work = h.render().restore();
    if (unmount) h.unmount(); else { h.options.active = false; h.render(); h.options.active = true; h.render(); }
    h.calls.at(-1).resolve({ recoveryPath: "snapshot.json" }); await work; await h.flush();
    assert.equal(h.adopted.length, 0); assert.equal(h.writesAfterUnmount, 0);
    if (!unmount) { assert.equal(h.reloads, 1); assert.equal(h.calls.at(-1).args.action, "webdav_get"); h.unmount(); }
  }
});

test("恢复明确失败与结果不明分别提示，均不自动重放", async t => {
  installControllerWindow(t);
  for (const explicit of [false, true]) {
    const h = fixture(); await h.ready(); await h.inspect(); const work = h.render().restore();
    h.calls.at(-1).reject(Object.assign(new Error("配置版本过期"), { explicit })); await work; await h.flush();
    assert.match(h.render().error, explicit ? /配置版本过期/ : /恢复结果未确认/);
    await h.render().restore(); assert.equal(h.calls.filter(c => c.args.action === "restore").length, 1); h.unmount();
  }
});

test("WebDAV 上传必须确认，下载先校验且不会直接恢复", async t => {
  installControllerWindow(t); const h = fixture(); await h.ready();
  await h.render().upload(); assert.equal(h.calls.length, 1);
  h.render().askUpload(); h.render().cancelUpload(); await h.render().upload(); assert.equal(h.calls.length, 1);
  h.render().askUpload(); const upload = h.render().upload(); h.calls.at(-1).resolve({ status: "ok" }); await upload; await h.flush();
  assert.match(h.render().notice, /已备份/);
  const download = h.render().download(); h.calls.at(-1).resolve({ content: "cloud-content" }); await h.flush();
  assert.equal(h.calls.at(-1).args.action, "inspect"); h.calls.at(-1).resolve(summary); await download; await h.flush();
  assert.equal(h.render().preview.content, "cloud-content"); assert.equal(h.adopted.length, 0); h.unmount();
});

test("本地导出须先确认敏感数据且不会在取消后发送请求", async t => {
  installControllerWindow(t); const h = fixture(); await h.ready();
  await h.render().exportBackup(); assert.equal(h.calls.length, 1);
  h.render().askExport(); assert.equal(h.render().exportConfirmation, true);
  h.render().cancelExport(); await h.render().exportBackup(); assert.equal(h.calls.length, 1);
  h.render().askExport(); const work = h.render().exportBackup();
  assert.deepEqual(h.calls.at(-1).args, { action: "export", confirmed: true });
  h.calls.at(-1).reject(new Error("fixture export unavailable")); await work; await h.flush();
  assert.equal(h.render().exportConfirmation, false); assert.match(h.render().error, /fixture/); h.unmount();
});

test("WebDAV 保存保留读取版本，已提交的恢复警告不被当成失败重放", async t => {
  installControllerWindow(t); const h = fixture(); (await h.ready()).change("password", "new-fixture-secret");
  const save = h.render().saveSettings(); assert.equal(h.calls.at(-1).args.settings.revision, "dav1");
  h.calls.at(-1).resolve({ ...savedSettings, revision: "dav2" }); await save; await h.flush();
  assert.equal(h.render().draft.password, ""); assert.equal(h.render().draft.revision, "dav2");
  await h.inspect(); const restore = h.render().restore();
  h.calls.at(-1).resolve({ restored: true, recoveryPath: "before.json", applyWarning: "已恢复，后台应用状态待确认" });
  await restore; await h.flush(); assert.match(h.render().notice, /后台应用状态待确认/);
  assert.equal(h.render().error, ""); assert.equal(h.adopted.length, 1); h.unmount();
});
