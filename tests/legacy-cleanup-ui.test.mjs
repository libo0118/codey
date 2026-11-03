import assert from "node:assert/strict";
import test from "node:test";
import { createControllerHarness, deferred, installControllerWindow } from "./helpers/controller-harness.mjs";
import { autoStubModule, collectElements, elementProps, elementType, textContent } from "./helpers/jsx-tree.mjs";

const existingId = "11111111-1111-4111-8111-000000000001";
const ordinaryId = "11111111-1111-4111-8111-000000000002";
const missingId = "11111111-1111-4111-8111-000000000003";
const entries = [existingId, ordinaryId].map((id, index) => ({
  id, title: index ? "普通会话" : "需重新确认的会话", cwd: "/mock/project", archived: false, updatedAt: 1, bytes: 10,
}));
const inventory = (extra = {}) => ({
  sessions: entries, revision: "r1", pendingCount: 0, legacyPendingIds: [existingId, missingId],
  cleanupPolicy: { enabled: false, retentionDays: 30 }, cleanupPolicyRevision: "p1", ...extra,
});
const ui = autoStubModule("ui");
const byType = (tree, type) => collectElements(tree, node => elementType(node) === type).map(elementProps);
const button = (tree, label) => {
  const found = byType(tree, ui.Button).find(props => textContent(props.children) === label);
  assert.ok(found, `未找到按钮：${label}`);
  return found;
};
const check = (tree, label) => {
  const found = byType(tree, "input").find(props => props["aria-label"] === label);
  assert.ok(found, `未找到选择框：${label}`);
  return found;
};
function fixture(controller = false) {
  const calls = [];
  const h = createControllerHarness(new URL(controller
    ? "../src/features/session-management/useSessionsController.ts"
    : "../src/features/session-management/SessionsPage.tsx", import.meta.url), controller ? "useSessionsController" : "SessionsPage", {
    "../../api": { invoke(command, args) { const call = { command, args, ...deferred() }; calls.push(call); return call.promise; } },
    "../../components/ui": ui,
    "../../SettingsPageHeader": autoStubModule("header"),
    "@tabler/icons-react": autoStubModule("icons"),
  });
  const options = { active: true, isBusy: false, onRestart() {} };
  return { ...h, calls, options, render: () => h.render(options),
    async ready(next = inventory()) { h.render(options); await h.flush(); calls.at(-1).resolve(next); await h.flush(); },
    async finish(result, next) { calls.at(-1).resolve(result); await h.flush(); calls.at(-1).resolve(next); await h.flush(); },
  };
}

test("旧版任务独立展示且默认不选，现存与缺失 ID 经确认绑定同一版本，取消不请求", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready();
  assert.match(textContent(h.render()), /旧版清理任务需重新确认/);
  assert.match(textContent(h.render()), /会话已不存在，仅清除标记/);
  assert.match(textContent(h.render()), new RegExp(missingId));
  assert.equal(check(h.render(), "选择旧版清理任务 需重新确认的会话").checked, false);
  assert.equal(check(h.render(), `选择旧版清理任务 ${missingId}`).checked, false);
  assert.equal(button(h.render(), "确认所选旧版任务").disabled, true);
  assert.equal(byType(h.render(), ui.Button).some(props => textContent(props.children) === "重启 Codex"), false);
  check(h.render(), "选择会话 普通会话").onChange();
  check(h.render(), "选择旧版清理任务 需重新确认的会话").onChange();
  check(h.render(), `选择旧版清理任务 ${missingId}`).onChange();
  button(h.render(), "确认所选旧版任务").onClick();
  const description = textContent(byType(h.render(), ui.DialogDescription)[0].children);
  assert.match(description, /1 个现存会话.*手动清理.*重启.*永久删除/);
  assert.match(description, /1 个会话已不存在.*仅移除.*不安排以后删除.*不删除任何其他文件/);
  button(h.render(), "取消").onClick();
  assert.equal(h.calls.length, 1);
  assert.equal(byType(h.render(), ui.Dialog).length, 0);
  button(h.render(), "确认所选旧版任务").onClick();
  const submit = button(h.render(), "确认旧版清理任务"); submit.onClick(); submit.onClick();
  assert.equal(h.calls.length, 2);
  assert.deepEqual(h.calls[1].args, { request: { ids: [existingId, missingId], revision: "r1", confirmed: true } });
  await h.finish({ selectedCount: 2, scheduledCount: 1, clearedLegacyCount: 1, restartRequired: true, errors: [] }, inventory({ legacyPendingIds: [], revision: "r2", pendingCount: 1 }));
  assert.match(textContent(h.render()), /已安排 1 个会话清理/);
  assert.match(textContent(h.render()), /已清除 1 个.*旧版任务标记/);
  assert.doesNotMatch(textContent(h.render()), /已安排 2 个|已删除 2 个/);
  assert.ok(button(h.render(), "重启 Codex"));
  assert.equal(check(h.render(), "选择会话 普通会话").checked, false);
  h.unmount();
});

test("只有缺失旧 ID 时只确认清除标记，结果不误报删除或要求重启", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready(inventory({ sessions: [], legacyPendingIds: [missingId] }));
  check(h.render(), `选择旧版清理任务 ${missingId}`).onChange();
  button(h.render(), "确认所选旧版任务").onClick();
  const description = textContent(byType(h.render(), ui.DialogDescription)[0].children);
  assert.match(description, /仅移除.*旧版任务标记.*不删除任何其他文件/);
  assert.doesNotMatch(description, /将转为手动清理|重启.*永久删除/);
  button(h.render(), "确认清除旧标记").onClick();
  assert.deepEqual(h.calls[1].args.request.ids, [missingId]);
  await h.finish({ selectedCount: 1, scheduledCount: 0, clearedLegacyCount: 1, restartRequired: false, errors: [] }, inventory({ sessions: [], legacyPendingIds: [], revision: "r2" }));
  assert.match(textContent(h.render()), /已清除 1 个.*旧版任务标记.*未删除任何其他文件/);
  assert.doesNotMatch(textContent(h.render()), /已安排|已删除 1 个|清理请求已保存/);
  assert.equal(byType(h.render(), ui.Button).some(props => textContent(props.children) === "重启 Codex"), false);
  h.unmount();
});

test("旧任务 revision 冲突后只刷新不重试，并清空旧确认及选择", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready();
  check(h.render(), `选择旧版清理任务 ${missingId}`).onChange();
  button(h.render(), "确认所选旧版任务").onClick(); button(h.render(), "确认清除旧标记").onClick();
  h.calls[1].reject(new Error("会话列表已变化，请刷新后重试")); await h.flush();
  assert.deepEqual(h.calls.map(call => call.command), ["list_codex_sessions", "delete_codex_sessions", "list_codex_sessions"]);
  h.calls[2].resolve(inventory({ revision: "r2" })); await h.flush();
  assert.match(textContent(h.render()), /会话列表已变化/);
  assert.equal(check(h.render(), `选择旧版清理任务 ${missingId}`).checked, false);
  assert.equal(byType(h.render(), ui.Dialog).length, 0);
  assert.equal(h.calls.length, 3);
  check(h.render(), `选择旧版清理任务 ${missingId}`).onChange();
  button(h.render(), "确认所选旧版任务").onClick(); button(h.render(), "确认清除旧标记").onClick();
  assert.equal(h.calls[3].args.request.revision, "r2");
  await h.finish({ selectedCount: 1, scheduledCount: 0, clearedLegacyCount: 1, restartRequired: false, errors: [] }, inventory({ revision: "r3", legacyPendingIds: [existingId] }));
  h.unmount();
});

test("控制器拒绝混入未知 ID，去重后仅发送明确确认的旧任务", async t => {
  installControllerWindow(t);
  const h = fixture(true); await h.ready();
  h.render().reconfirmLegacy([missingId, "unknown"]);
  assert.equal(h.render().confirmation, null); assert.equal(h.calls.length, 1);
  h.render().reconfirmLegacy([missingId, missingId]);
  assert.deepEqual(h.render().confirmation.ids, [missingId]);
  assert.equal(h.render().confirmation.revision, "r1");
  const operation = h.render().remove();
  assert.deepEqual(h.calls[1].args.request.ids, [missingId]);
  await h.finish({ selectedCount: 1, scheduledCount: 0, clearedLegacyCount: 1, restartRequired: false, errors: [] }, inventory({ revision: "r2", legacyPendingIds: [existingId] }));
  await operation; h.unmount();
});

test("未提供新字段时不伪造旧任务，混合旧 pending 只保守提示状态需核验", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready(inventory({ legacyPendingIds: undefined, pendingCount: 2 }));
  assert.doesNotMatch(textContent(h.render()), /旧版清理任务需重新确认|清理请求已保存/);
  assert.match(textContent(h.render()), /明细.*请.*刷新.*核验/);
  assert.equal(byType(h.render(), ui.Button).some(props => textContent(props.children) === "重启 Codex"), false);
  h.unmount();
});

test("现存会话缺少标题时以 ID 展示，不误报会话已不存在", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready(inventory({ sessions: [{ ...entries[0], title: "" }], legacyPendingIds: [existingId] }));
  assert.ok(check(h.render(), `选择旧版清理任务 ${existingId}`));
  assert.doesNotMatch(textContent(h.render()), /会话已不存在/);
  h.unmount();
});

test("旧任务选择受忙态保护，刷新或隐藏后不保留旧选择与确认", async t => {
  installControllerWindow(t);
  const h = fixture(true); await h.ready();
  h.render().toggleLegacy(existingId);
  h.options.isBusy = true;
  h.render().toggleLegacy(missingId); h.render().reconfirmLegacy([existingId]);
  assert.deepEqual([...h.render().legacySelected], [existingId]);
  assert.equal(h.render().confirmation, null); assert.equal(h.calls.length, 1);
  h.options.isBusy = false;
  h.render().reconfirmLegacy([existingId]); h.render().refresh(); await h.flush();
  assert.equal(h.render().legacySelected.size, 0); assert.equal(h.render().confirmation, null);
  h.render().reconfirmLegacy([missingId]); assert.equal(h.render().confirmation, null);
  h.calls.at(-1).resolve(inventory({ revision: "r2" })); await h.flush();
  h.render().toggleLegacy(existingId); h.render().reconfirmLegacy([existingId]);
  h.options.active = false; h.render();
  assert.equal(h.render().legacySelected.size, 0); assert.equal(h.render().confirmation, null);
  h.render().reconfirmLegacy([missingId]); assert.equal(h.calls.length, 2);
  h.unmount();
});

test("可执行 pending 与未确认 legacy 同时存在时仍分别告知，普通清理保持原请求合同", async t => {
  installControllerWindow(t);
  const h = fixture(); await h.ready(inventory({ pendingCount: 2 }));
  assert.ok(button(h.render(), "重启 Codex"));
  assert.match(textContent(h.render()), /旧版清理任务需重新确认/);
  check(h.render(), "选择会话 普通会话").onChange(); button(h.render(), "清理所选").onClick();
  assert.match(textContent(byType(h.render(), ui.DialogDescription)[0].children), /永久清理 1 个本机会话/);
  button(h.render(), "确认永久清理").onClick();
  assert.deepEqual(h.calls[1].args, { request: { ids: [ordinaryId], revision: "r1", confirmed: true } });
  await h.finish({ selectedCount: 1, restartRequired: true, errors: [] }, inventory({ pendingCount: 3, revision: "r2" }));
  assert.ok(button(h.render(), "重启 Codex"));
  assert.doesNotMatch(textContent(h.render()), /已清除 .*旧版任务标记/);
  h.unmount();
});
