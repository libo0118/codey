import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "@tabler/icons-react";
import { renderToStaticMarkup } from "react-dom/server";
import { readFileSync } from "node:fs";
import { collectElements, createModuleGraph, textContent } from "./helpers/jsx-tree.mjs";
import { createControllerHarness } from "./helpers/controller-harness.mjs";

const entry = new URL("../src/remote/cindy/GoalSurface.tsx", import.meta.url);
const baseGoal = { objective: "验证目标\n完整正文", status: "active", tokensUsed: 24001, tokenBudget: 100000, timeUsedSeconds: 841 };
const settle = async () => { await Promise.resolve(); await Promise.resolve(); };
const control = (tree, name) => collectElements(tree, node => node.props["data-cindy-control"] === `goal-${name}`)[0];
const element = (tree, type) => collectElements(tree, node => node.type === type)[0];

function mount(overrides = {}) {
  const calls = [], focused = [];
  class FakeElement { isConnected = true; focus() { focused.push(this); } getClientRects() { return [1]; } }
  const first = new FakeElement(), opener = new FakeElement(), document = { activeElement: opener };
  const dialog = { shown: 0, closed: 0, showModal() { this.shown++; }, close() { this.closed++; }, querySelector() { return first; } };
  const props = { goal: baseGoal, open: false, onOpenChange: open => { props.open = open; }, connected: true, busy: false, running: false, error: "", uncertain: false,
    onRead: async () => { calls.push("read"); return true; },
    onCommand: async (action, args) => { calls.push([action, args]); return true; },
    onConfirmUnknown: () => calls.push("confirmed"), ...overrides };
  const h = createControllerHarness(entry, exports => {
    const tree = exports.GoalSurface(props), sheet = element(tree, "dialog");
    if (sheet) sheet.props.ref.current = dialog;
    return tree;
  }, { "@tabler/icons-react": icons });
  function withDom(action) {
    const previous = { document: globalThis.document, HTMLElement: globalThis.HTMLElement };
    Object.assign(globalThis, { document, HTMLElement: FakeElement });
    try { return action(); } finally { Object.assign(globalThis, previous); }
  }
  const render = () => withDom(() => h.render());
  return { calls, props, render, withDom, document, FakeElement, first, opener, focused, dialog,
    async open() { render(); props.onOpenChange(true); render(); await settle(); return render(); } };
}

test("无目标、未读与已完成目标关闭时不渲染占位，未完成与未知写入仍保留摘要", () => {
  for (const goal of [null, undefined, { ...baseGoal, status: "complete" }]) {
    const h = mount({ goal }); assert.equal(h.render(), null); assert.deepEqual(h.calls, []);
  }
  for (const status of ["active", "paused", "blocked", "usageLimited", "budgetLimited"]) {
    const h = mount({ goal: { ...baseGoal, status } });
    assert.ok(control(h.render(), "open")); assert.deepEqual(h.calls, []);
  }
  for (const goal of [null, undefined, { ...baseGoal, status: "complete" }]) {
    const h = mount({ goal, uncertain: true });
    assert.match(textContent(h.render()), /目标状态待核对/);
    assert.equal(collectElements(h.render(), node => node.props["data-goal-status"] === "unknown").length, 1);
  }
});

test("摘要只读打开详情，六态中文、完整正文与真实数字不混入 HAP chrome 合同", async () => {
  const labels = { active: "目标进行中", paused: "目标已暂停", blocked: "目标等待处理", usageLimited: "目标用量受限", budgetLimited: "目标预算受限", complete: "目标已完成" };
  for (const [status, label] of Object.entries(labels)) {
    const h = mount({ goal: { ...baseGoal, status } });
    assert.deepEqual(h.calls, []);
    assert.equal(element(h.render(), "dialog"), undefined);
    const tree = await h.open();
    assert.deepEqual(h.calls, ["read"]);
    assert.match(textContent(tree), new RegExp(label));
    assert.match(textContent(tree), /验证目标\n完整正文/);
    assert.match(textContent(tree), /24,001 \/ 100,000 Token/);
    assert.match(textContent(tree), /14 分 1 秒/);
    assert.equal(collectElements(tree, n => n.props["data-codey-action"] || n.props["data-codey-chrome"]).length, 0);
    assert.equal(element(tree, "dialog").props["aria-modal"], "true");
    assert.equal(control(tree, "back").props["aria-label"], "返回对话");
    assert.equal(collectElements(tree, n => n.type === "button" && /关闭/.test(n.props["aria-label"] || "")).length, 0);
    assert.equal(!!control(tree, "toggle"), status !== "complete");
  }
});

test("unknown、读取失败和结果待核对均不当成 none，也不会自动重发", async () => {
  for (const goal of [undefined, null]) {
    const h = mount({ goal, onRead: async () => false });
    const tree = await h.open();
    assert.equal(control(tree, "save"), undefined);
    assert.equal(control(tree, "create"), undefined);
    assert.equal(control(tree, "open"), undefined);
    assert.match(textContent(tree), /读取成功前不能创建目标/);
    assert.equal(collectElements(tree, n => n.props["data-goal-status"] === "none").length, 0);
  }
  const h = mount({ goal: null, uncertain: true });
  let tree = await h.open();
  assert.match(textContent(tree), /桌面核对目标状态和启动消息.*不会自动重发/);
  assert.equal(control(tree, "save"), undefined);
  control(tree, "confirm").props.onClick();
  tree = h.render();
  assert.equal(control(tree, "save"), undefined);
  assert.deepEqual(h.calls, ["read", "confirmed"]);
});

test("读取异常显示错误且阻止创建，成功重读后才可进入表单", async () => {
  const h = mount({ goal: null, onRead: async () => { throw new Error("synthetic read failure"); } });
  let tree = await h.open();
  assert.match(textContent(tree), /读取目标失败：synthetic read failure/);
  assert.equal(control(tree, "save"), undefined);
  h.props.onRead = async () => true;
  control(h.render(), "refresh").props.onClick(); await settle();
  tree = h.render();
  assert.ok(control(tree, "save"));
});

test("读取返回成功不代替权威 goal，未确认状态不能进入新建", async () => {
  const h = mount({ goal: undefined }); let tree = await h.open();
  assert.equal(control(tree, "save"), undefined); assert.equal(control(tree, "create"), undefined);
  assert.match(textContent(tree), /读取成功前不能创建目标/);
  h.props.goal = { ...baseGoal, status: "complete" }; tree = h.render();
  assert.equal(element(tree, "form"), undefined); assert.ok(control(tree, "create"));
  assert.deepEqual(h.calls, ["read"]);
});

test("新建默认折叠高级预算，仅显式提交原生字段；失败保留草稿，成功转详情", async () => {
  const h = mount({ goal: null, onCommand: async (action, args) => { h.calls.push([action, args]); h.props.error = "synthetic rejected"; return false; } });
  let tree = await h.open();
  assert.equal(element(tree, "details").props.open, undefined);
  assert.equal(element(tree, "input").props.placeholder, "不限");
  assert.equal(control(tree, "save").props.disabled, true);
  assert.match(textContent(tree), /当前会话的模型和权限.*消耗 Token/);
  assert.doesNotMatch(textContent(tree), /轮数|无进展|百分比/);
  element(tree, "textarea").props.onChange({ target: { value: "  保留我的草稿  " } });
  element(h.render(), "input").props.onChange({ target: { value: "500000" } });
  tree = h.render();
  assert.equal(control(tree, "save").props.disabled, false);
  element(tree, "form").props.onSubmit({ preventDefault() {} }); await settle();
  tree = h.render();
  assert.equal(element(tree, "textarea").props.value, "  保留我的草稿  ");
  assert.match(textContent(tree), /synthetic rejected/);
  assert.deepEqual(h.calls, ["read", ["create", { objective: "保留我的草稿", tokenBudget: 500000 }]]);
  h.props.onCommand = async (action, args) => { h.calls.push([action, args]); h.props.goal = { ...baseGoal, objective: args.objective }; h.props.error = ""; return true; };
  element(h.render(), "form").props.onSubmit({ preventDefault() {} }); await settle();
  tree = h.render();
  assert.equal(element(tree, "form"), undefined);
  assert.match(textContent(tree), /保留我的草稿/);
});

test("预算只接受正安全整数，空白目标不提交，空预算传 null", async () => {
  const h = mount({ goal: null }); let tree = await h.open();
  element(tree, "textarea").props.onChange({ target: { value: "  " } });
  element(h.render(), "form").props.onSubmit({ preventDefault() {} }); await settle();
  assert.deepEqual(h.calls, ["read"]);
  element(h.render(), "textarea").props.onChange({ target: { value: "合法目标" } });
  for (const value of ["0", "-1", "1.5", "9007199254740992"]) {
    element(h.render(), "input").props.onChange({ target: { value } }); tree = h.render();
    assert.equal(control(tree, "save").props.disabled, true);
    element(tree, "form").props.onSubmit({ preventDefault() {} }); await settle();
  }
  element(h.render(), "input").props.onChange({ target: { value: "" } });
  element(h.render(), "form").props.onSubmit({ preventDefault() {} }); await settle();
  assert.deepEqual(h.calls, ["read", ["create", { objective: "合法目标", tokenBudget: null }]]);
});

test("active 必须先暂停；保存编辑警示重置用量，不自行修改权威状态", async () => {
  const h = mount(); let tree = await h.open();
  assert.equal(control(tree, "edit").props.disabled, true);
  control(tree, "edit").props.onClick(); assert.equal(element(h.render(), "form"), undefined);
  control(tree, "toggle-detail").props.onClick(); await settle();
  assert.deepEqual(h.calls, ["read", ["pause", {}]]);
  assert.match(textContent(h.render()), /目标进行中/);
  h.props.goal = { ...baseGoal, status: "paused" }; tree = h.render();
  control(tree, "edit").props.onClick(); tree = h.render();
  assert.equal(element(tree, "textarea").props.value, baseGoal.objective);
  assert.match(textContent(tree), /重置目标用量统计，并保持暂停/);
  assert.equal(control(tree, "back").props["aria-label"], "返回目标详情");
  element(tree, "form").props.onSubmit({ preventDefault() {} }); await settle();
  assert.deepEqual(h.calls.at(-1), ["edit", { objective: baseGoal.objective, tokenBudget: 100000 }]);
  tree = h.render(); assert.equal(element(tree, "form"), undefined);
  assert.match(textContent(tree), /24,001/);
});

test("回复活跃只禁止创建和继续，暂停仍可用；busy/断线/未知结果均禁写", async () => {
  const active = mount({ running: true }); let tree = await active.open();
  assert.equal(control(tree, "toggle-detail").props.disabled, false);
  control(tree, "toggle-detail").props.onClick(); await settle();
  assert.deepEqual(active.calls.at(-1), ["pause", {}]);
  for (const status of ["paused"]) {
    const h = mount({ running: true, goal: { ...baseGoal, status } }); tree = await h.open();
    assert.equal(control(tree, "toggle-detail").props.disabled, true);
    control(tree, "toggle-detail").props.onClick(); await settle(); assert.deepEqual(h.calls, ["read"]);
  }
  for (const status of ["blocked", "usageLimited", "budgetLimited"]) {
    const h = mount({ running: true, goal: { ...baseGoal, status } }); tree = await h.open();
    assert.equal(control(tree, "toggle-detail").props.disabled, false);
    assert.equal(control(tree, "toggle-detail").props["aria-label"], undefined);
    assert.equal(control(tree, "edit").props.disabled, true);
    control(tree, "toggle-detail").props.onClick(); await settle(); assert.deepEqual(h.calls.at(-1), ["pause", {}]);
  }
  const create = mount({ running: true, goal: null }); tree = await create.open();
  element(tree, "textarea").props.onChange({ target: { value: "目标" } }); tree = create.render();
  assert.equal(control(tree, "save").props.disabled, true);
  element(tree, "form").props.onSubmit({ preventDefault() {} }); await settle(); assert.deepEqual(create.calls, ["read"]);
  for (const override of [{ busy: true }, { connected: false }, { uncertain: true }]) {
    const h = mount(override); tree = await h.open();
    assert.equal(control(tree, "toggle-detail").props.disabled, true);
    control(tree, "toggle-detail").props.onClick(); await settle();
    assert.equal(h.calls.some(Array.isArray), false);
  }
});

test("完成目标保留详情并允许明确新建，不允许直接编辑旧目标", async () => {
  const h = mount({ goal: { ...baseGoal, status: "complete" } }); let tree = await h.open();
  assert.equal(control(tree, "edit"), undefined);
  assert.equal(control(tree, "toggle-detail"), undefined);
  control(tree, "create").props.onClick(); tree = h.render();
  control(tree, "back").props.onClick(); assert.equal(h.render(), null);
  tree = await h.open(); assert.equal(element(tree, "form"), undefined);
  assert.match(textContent(tree), /目标已完成/); assert.deepEqual(h.calls, ["read", "read"]);
  control(tree, "create").props.onClick(); tree = h.render();
  element(tree, "textarea").props.onChange({ target: { value: "下一项目标" } }); tree = h.render();
  element(tree, "form").props.onSubmit({ preventDefault() {} }); await settle();
  assert.deepEqual(h.calls.at(-1), ["create", { objective: "下一项目标", tokenBudget: null }]);
});

test("双击不重复发指令，异常保留编辑草稿并明确报错", async () => {
  let resolve;
  const h = mount({ goal: { ...baseGoal, status: "paused" }, onCommand: () => new Promise(done => { h.calls.push("command"); resolve = done; }) });
  let tree = await h.open(); control(tree, "edit").props.onClick(); tree = h.render();
  const submit = () => element(tree, "form").props.onSubmit({ preventDefault() {} });
  submit(); submit(); assert.deepEqual(h.calls, ["read", "command"]);
  resolve(false); await settle(); assert.ok(element(h.render(), "form"));
  h.props.onCommand = async () => { throw new Error("synthetic edit failure"); };
  element(h.render(), "form").props.onSubmit({ preventDefault() {} }); await settle(); tree = h.render();
  assert.equal(element(tree, "textarea").props.value, baseGoal.objective);
  assert.match(textContent(tree), /目标操作失败：synthetic edit failure/);
});

test("原生模态、Tab 首尾环绕、Escape/返回及关闭后焦点恢复", async () => {
  const h = mount(); let tree = h.render();
  control(tree, "open").props.onClick(); tree = h.render(); await settle(); tree = h.render();
  const dialog = element(tree, "dialog"), last = new h.FakeElement(), hidden = new h.FakeElement();
  hidden.getClientRects = () => [];
  assert.equal(h.dialog.shown, 1);
  for (const [current, shiftKey, expected] of [[last, false, h.first], [h.first, true, last]]) {
    h.document.activeElement = current; let prevented = false;
    h.withDom(() => dialog.props.onKeyDown({ key: "Tab", shiftKey, currentTarget: { querySelectorAll: () => [h.first, hidden, last] }, preventDefault() { prevented = true; } }));
    assert.equal(prevented, true); assert.equal(h.focused.at(-1), expected);
  }
  let canceled = false; dialog.props.onCancel({ preventDefault() { canceled = true; } });
  assert.equal(canceled, true); assert.equal(element(h.render(), "dialog"), undefined);
  assert.equal(h.dialog.closed, 1); assert.equal(h.focused.at(-1), h.opener);
  tree = await h.open(); control(tree, "back").props.onClick(); assert.equal(element(h.render(), "dialog"), undefined);
});

test("缺失用量不伪造零，实际 React 渲染转义正文，布局包含窄屏与深色约束", async () => {
  const h = mount({ goal: { objective: "<script>long</script>", status: "paused" } }); const tree = await h.open();
  assert.match(textContent(tree), /Token 用量尚未提供.*耗时尚未提供/);
  const { GoalSurface } = createModuleGraph(entry, { stubs: { react: React, "react/jsx-runtime": jsx, "@tabler/icons-react": icons } }).exports;
  const html = renderToStaticMarkup(React.createElement(GoalSurface, h.props));
  assert.doesNotMatch(html, /<script>/); assert.match(html, /&lt;script&gt;/);
  const css = readFileSync(new URL("../src/remote/cindy/goals.css", import.meta.url), "utf8");
  assert.match(css, /min-height: 44px/); assert.match(css, /max-width: 100%/);
  assert.match(css, /overflow-wrap: anywhere/); assert.match(css, /prefers-color-scheme: dark/);
  assert.match(css, /var\(--remote-fg/); assert.doesNotMatch(css, /--remote-text/);
});
