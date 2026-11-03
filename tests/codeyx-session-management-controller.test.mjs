import assert from "node:assert/strict";
import test from "node:test";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";

const entries = Array.from({ length: 23 }, (_, index) => ({ id: `id-${index}`, title: `会话 ${index}`, cwd: index % 2 ? "/alpha" : "/beta", archived: index % 2 === 1, updatedAt: 1, bytes: 10 }));
const inventory = (sessions = entries, revision = "r1", pendingCount = 0, cleanupPolicy = { enabled: false, retentionDays: 30 }, cleanupPolicyRevision = "p1") =>
  ({ sessions, revision, pendingCount, cleanupPolicy, cleanupPolicyRevision });
function fixture(active = true) {
  const calls = [], restarts = [];
  const h = createControllerHarness(new URL("../src/features/session-management/useSessionsController.ts", import.meta.url), "useSessionsController", {
    "../../api": { invoke(command, args) { const call = { command, args, ...deferred() }; calls.push(call); return call.promise; } },
  });
  const options = { active, isBusy: false, onRestart: () => restarts.push(true) };
  return { ...h, calls, restarts, options, render: () => h.render(options),
    async ready(next = inventory()) { h.render(options); await h.flush(); calls.at(-1).resolve(next); await h.flush(); },
  };
}

test("本机会话支持搜索、归档筛选、分页与本页选择，全部清理不受筛选限制", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready();
  assert.equal(h.render().visible.length, 20); assert.equal(h.render().pages, 2);
  h.render().toggleVisible(); assert.equal(h.render().selected.size, 20);
  h.render().changePage(1); assert.equal(h.render().visible.length, 3);
  h.render().changeFilter("archived"); assert.equal(h.render().selected.size, 0); assert.equal(h.render().page, 0);
  assert.equal(h.render().visible.length, 11);
  h.render().changeQuery("alpha"); assert.equal(h.render().visible.length, 11);
  h.render().askRemove(true); assert.equal(h.render().confirmation.ids.length, 23);
  assert.equal(h.render().confirmation.archivedCount, 11);
  h.render().cancelRemove(); assert.equal(h.render().confirmation, null);
  h.unmount();
});

test("自动清理草稿校验和时间筛选重置选择，启用经确认后使用策略版本保存", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready();
  assert.deepEqual(h.render().policyDraft, { enabled: false, retentionDays: "30" });
  h.render().toggleVisible(); h.render().changePage(1); h.render().askRemove(true);
  h.render().changeOlderOnly(true);
  assert.equal(h.render().selected.size, 0); assert.equal(h.render().page, 0); assert.equal(h.render().confirmation, null);
  h.render().changePolicy("retentionDays", ""); h.render().askSavePolicy();
  assert.match(h.render().policyProblem, /整数/); assert.equal(h.calls.length, 1); assert.equal(h.render().visible.length, 0);
  h.render().changePolicy("retentionDays", "30"); h.render().changePolicy("enabled", true); h.render().askSavePolicy();
  assert.deepEqual(h.render().policyConfirmation, { policy: { enabled: true, retentionDays: 30 }, revision: "p1" });
  h.render().cancelSavePolicy(); assert.equal(h.calls.length, 1);
  h.render().askSavePolicy(); const operation = h.render().confirmSavePolicy(); await h.render().confirmSavePolicy();
  assert.equal(h.calls.length, 2);
  assert.equal(h.calls[1].command, "save_codex_session_cleanup_policy");
  assert.deepEqual(h.calls[1].args, { request: { policy: { enabled: true, retentionDays: 30 }, revision: "p1", confirmed: true } });
  h.calls[1].resolve({ policy: { enabled: true, retentionDays: 30 }, revision: "p2", cleanup: { selectedCount: 23, restartRequired: true, errors: [] }, cleanupError: null });
  await operation; await h.flush();
  h.calls.at(-1).resolve(inventory([], "r2", 23, { enabled: true, retentionDays: 30 }, "p2")); await h.flush();
  assert.equal(h.render().policyDirty, false); assert.equal(h.render().result.restartRequired, true);
  assert.match(h.render().notice, /设置已保存/); assert.equal(h.render().selected.size, 0); h.unmount();
});

test("缩短已启用保留期需重新确认，关闭直接保存，清理失败与保存成功分别呈现", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready(inventory(entries, "r1", 0, { enabled: true, retentionDays: 60 }));
  h.render().changePolicy("retentionDays", "30"); h.render().askSavePolicy();
  assert.equal(h.render().policyConfirmation.policy.retentionDays, 30);
  h.render().changePolicy("retentionDays", "31"); assert.equal(h.render().policyConfirmation, null);
  h.render().askSavePolicy(); const operation = h.render().confirmSavePolicy();
  h.calls.at(-1).resolve({ policy: { enabled: true, retentionDays: 31 }, revision: "p2", cleanup: null, cleanupError: "数据库繁忙" });
  await operation; await h.flush();
  h.calls.at(-1).resolve(inventory(entries, "r1", 0, { enabled: true, retentionDays: 31 }, "p2")); await h.flush();
  assert.match(h.render().error, /设置已保存.*数据库繁忙/); assert.equal(h.render().policyDirty, false);
  h.render().changePolicy("enabled", false); h.render().askSavePolicy();
  assert.equal(h.render().policyConfirmation, null); assert.equal(h.calls.at(-1).command, "save_codex_session_cleanup_policy");
  h.calls.at(-1).resolve({ policy: { enabled: false, retentionDays: 31 }, revision: "p3", cleanup: null, cleanupError: null });
  await h.flush(); h.calls.at(-1).resolve(inventory(entries, "r1", 0, { enabled: false, retentionDays: 31 }, "p3")); await h.flush();
  assert.match(h.render().notice, /已关闭/); h.unmount();
});

test("保存失败和隐藏重开保留清理草稿，重开使用最新策略版本", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready();
  h.render().changePolicy("retentionDays", "45"); h.render().askSavePolicy();
  h.calls.at(-1).reject(new Error("保存失败")); await h.flush();
  h.calls.at(-1).resolve(inventory()); await h.flush();
  assert.equal(h.render().policyDraft.retentionDays, "45"); assert.match(h.render().error, /保存失败/);
  h.options.active = false; h.render(); h.options.active = true; h.render(); await h.flush();
  h.calls.at(-1).resolve(inventory(entries, "r1", 0, { enabled: false, retentionDays: 60 }, "p2")); await h.flush();
  assert.equal(h.render().policyDraft.retentionDays, "45");
  h.render().askSavePolicy(); assert.equal(h.calls.at(-1).args.request.revision, "p2");
  h.calls.at(-1).resolve({ policy: { enabled: false, retentionDays: 45 }, revision: "p3", cleanup: null, cleanupError: null });
  await h.flush(); h.calls.at(-1).resolve(inventory(entries, "r1", 0, { enabled: false, retentionDays: 45 }, "p3")); await h.flush(); h.unmount();
});

test("策略保存超时保持底层写入锁，隐藏重开与卸载不采纳旧保存响应", async t => {
  installControllerWindow(t); t.mock.timers.enable({ apis: ["setTimeout"] });
  for (const unmount of [false, true]) {
    const h = fixture(); await h.ready(); h.render().changePolicy("enabled", true); h.render().askSavePolicy();
    const operation = h.render().confirmSavePolicy(), call = h.calls.at(-1);
    if (unmount) h.unmount();
    else { h.options.active = false; h.render(); h.options.active = true; h.render(); }
    assert.equal(h.calls.length, 2);
    t.mock.timers.tick(120_000); await operation; await h.flush();
    if (!unmount) { h.render().askRemove(true); await h.render().remove(); assert.equal(h.calls.length, 2); }
    call.resolve({ policy: { enabled: true, retentionDays: 30 }, revision: "p2", cleanup: null, cleanupError: null }); await h.flush();
    if (unmount) assert.equal(h.writesAfterUnmount, 0);
    else {
      h.calls.at(-1).resolve(inventory(entries, "r1", 0, { enabled: true, retentionDays: 30 }, "p2")); await h.flush();
      assert.equal(h.render().notice, ""); assert.equal(h.render().policyDirty, false); h.unmount();
    }
  }
});

test("确认绑定清理集合和版本，禁止重复提交，清理结果提供重启并重新读取", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready();
  h.render().toggle("id-1"); h.render().askRemove(false);
  const operation = h.render().remove(); await h.render().remove();
  assert.equal(h.calls.length, 2);
  assert.deepEqual(h.calls[1].args, { request: { ids: ["id-1"], revision: "r1", confirmed: true } });
  h.render().restart(); assert.equal(h.restarts.length, 0);
  h.calls[1].resolve({ selectedCount: 1, restartRequired: true, errors: [] }); await operation; await h.flush();
  assert.equal(h.calls.at(-1).command, "list_codex_sessions");
  h.calls.at(-1).resolve(inventory(entries.filter(entry => entry.id !== "id-1"), "r2", 1)); await h.flush();
  assert.equal(h.render().selected.size, 0); assert.equal(h.render().confirmation, null);
  assert.equal(h.render().result.restartRequired, true);
  h.render().restart(); assert.equal(h.restarts.length, 1);
  h.unmount();
});

test("读取失败和删除冲突不使用旧列表再次删除，刷新后恢复", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready();
  h.render().askRemove(true); const operation = h.render().remove();
  h.calls.at(-1).reject(new Error("版本已变化")); await operation; await h.flush();
  h.calls.at(-1).reject(new Error("读取失败")); await h.flush();
  assert.equal(h.render().blocked, true); h.render().askRemove(true); assert.equal(h.render().confirmation, null);
  h.render().refresh(); await h.flush(); h.calls.at(-1).resolve(inventory()); await h.flush();
  assert.equal(h.render().blocked, false); assert.equal(h.render().error, ""); h.unmount();
});

test("隐藏时禁止读取与删除，重开不采纳旧列表", async t => {
  installControllerWindow(t);
  const h = fixture(false); h.render(); await h.flush(); assert.equal(h.calls.length, 0);
  h.options.active = true; h.render(); await h.flush(); const old = h.calls[0];
  h.options.active = false; h.render(); h.options.active = true; h.render(); await h.flush();
  h.calls[1].resolve(inventory([], "new")); await h.flush();
  old.resolve(inventory(entries, "old")); await h.flush();
  assert.equal(h.render().inventory.revision, "new"); h.unmount();
});

test("删除在途隐藏重开仍保留写入锁，完成后刷新；卸载后无状态回写", async t => {
  installControllerWindow(t);
  for (const unmount of [false, true]) {
    const h = fixture(); await h.ready(); h.render().askRemove(true); const operation = h.render().remove();
    const call = h.calls.at(-1);
    if (unmount) h.unmount(); else { h.options.active = false; h.render(); h.options.active = true; h.render(); }
    assert.equal(h.calls.length, 2);
    call.resolve({ selectedCount: 23, restartRequired: true, errors: [] }); await operation; await h.flush();
    if (unmount) assert.equal(h.writesAfterUnmount, 0);
    else {
      h.calls.at(-1).resolve(inventory([], "new", 23)); await h.flush();
      assert.equal(h.render().result, null); assert.equal(h.render().inventory.pendingCount, 23); h.unmount();
    }
  }
});

test("删除超时不释放底层请求锁，也不把超时当作取消", async t => {
  installControllerWindow(t); t.mock.timers.enable({ apis: ["setTimeout"] });
  const h = fixture(); await h.ready(); h.render().askRemove(true); const operation = h.render().remove();
  const call = h.calls.at(-1); t.mock.timers.tick(120_000); await operation; await h.flush();
  assert.match(h.render().error, /结果尚不确定/); assert.equal(h.calls.length, 2);
  h.render().askRemove(true); await h.render().remove(); assert.equal(h.calls.length, 2);
  call.resolve({ selectedCount: 23, restartRequired: true, errors: [] }); await h.flush();
  h.calls.at(-1).resolve(inventory([], "new", 23)); await h.flush();
  assert.equal(h.render().inventory.pendingCount, 23); h.unmount();
});
