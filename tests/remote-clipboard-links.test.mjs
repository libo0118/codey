import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { createModuleGraph } from "./helpers/jsx-tree.mjs";

const clipboard = createModuleGraph(new URL("../src/remote/clipboard.tsx", import.meta.url)).exports;
const adapter = readFileSync(new URL("./fixtures/published-mobile-clipboard.js", import.meta.url), "utf8");
const id = "12345678-1234-1234-1234-123456789abc";

function browser(t, { modern = false, exec = true, iframe = false } = {}) {
  const calls = [], listeners = new Map(), nodes = [];
  const document = {
    activeElement: null, getSelection: () => null,
    addEventListener(name, fn) { if (!listeners.has(name)) listeners.set(name, []); listeners.get(name).push(fn); },
    removeEventListener(name, fn) { listeners.set(name, (listeners.get(name) || []).filter(listener => listener !== fn)); },
    dispatchEvent(event) { for (const fn of listeners.get(event.type) || []) fn(event); },
    createElement(tag) { return new Element(tag); },
    execCommand(name) { calls.push({ method: name, text: document.activeElement?.value }); return exec; },
    body: { append(node) { nodes.push(node); } },
  };
  class Element {
    constructor(tag) { this.tag = tag; this.disabled = false; this.isConnected = true; this.textContent = "复制"; this.children = []; }
    append(...items) { this.children.push(...items); }
    closest() { return null; }
    focus() { document.activeElement = this; }
    select() { this.selected = true; }
    setSelectionRange() {}
    setAttribute() {}
    remove() { this.isConnected = false; }
    showModal() { this.open = true; }
    close() { this.open = false; this.onclose?.(); }
  }
  const window = { prompt(command, text) { calls.push({ method: "native", command, text }); return "copied"; }, setTimeout() {} };
  window.top = iframe ? {} : window;
  const navigator = modern ? { clipboard: { async writeText(text) { calls.push({ method: "modern", text }); } } } : {};
  for (const [name, value] of Object.entries({ window, document, navigator, HTMLInputElement: Element, HTMLTextAreaElement: Element })) {
    const previous = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { configurable: true, value, writable: true });
    t.after(() => previous ? Object.defineProperty(globalThis, name, previous) : delete globalThis[name]);
  }
  const event = (trusted, match = true) => document.dispatchEvent({ type: "click", isTrusted: trusted, target: { closest: () => match ? {} : null } });
  const native = () => vm.runInNewContext(adapter, { window, document, CustomEvent, setTimeout() {} });
  return { window, document, navigator, calls, nodes, event, native, button: new Element("button") };
}

test("原生会话深链只使用合法本地 UUID，不读取 URL 或携带 secret", () => {
  assert.equal(clipboard.sessionDeepLink(id), `codex://threads/${id}`);
  for (const invalid of ["", "thread", `${id}?token=secret`, `${id}#pair=secret`, `${id}/file`]) assert.equal(clipboard.sessionDeepLink(invalid), null);
  for (const host of ["endpoint-digest", "remote-ssh", "wsl:Ubuntu", "https://server/?sig=secret"]) assert.equal(clipboard.sessionDeepLink(id, host), null);
  assert.equal(clipboard.sessionDeepLink(id, "local", "wsl"), null);
});

test("复制按钮首次渲染即提供已发布壳捕获所需 class", () => {
  assert.equal(clipboard.CopyButton({ text: "code", code: true }).props.className, "code-copy");
  assert.equal(clipboard.CopyButton({ text: clipboard.sessionDeepLink(id) }).props.className, "message-copy");
});

test("安全 Clipboard 与 HTTP execCommand 使用同一公开 copy 接口，记录 Mock 而不读系统剪贴板", async t => {
  const h = browser(t, { modern: true }); clipboard.installRemoteClipboard();
  const link = clipboard.sessionDeepLink(id);
  await h.window.BridgeClipboard.copy(() => link, h.button);
  assert.deepEqual(h.calls, [{ method: "modern", text: link }]); assert.equal(h.button.textContent, "已复制");
  delete h.navigator.clipboard; h.calls.length = 0;
  await h.window.BridgeClipboard.copy(() => "Mock code", h.button);
  assert.deepEqual(h.calls, [{ method: "copy", text: "Mock code" }]);
});

test("HTTP 自动复制失败提供可选择的手动文本，而非假报成功；过长内容不写入", async t => {
  const h = browser(t, { exec: false }); clipboard.installRemoteClipboard();
  await h.window.BridgeClipboard.copy(() => "manual Mock", h.button);
  const dialog = h.nodes.find(node => node.tag === "dialog"); assert.equal(dialog.open, true);
  const field = dialog.children.find(node => node.tag === "textarea"); assert.equal(field.value, "manual Mock"); assert.equal(field.selected, true);
  assert.notEqual(h.button.textContent, "已复制");
  h.calls.length = 0; await h.window.BridgeClipboard.copy(() => "a".repeat(262145), h.button);
  assert.equal(h.calls.length, 0); assert.match(h.button.textContent, /失败/);
});

test("真实已发布适配器 Mock：首击接管、一次消费，非可信/无匹配按钮不触发 native", async t => {
  const h = browser(t); clipboard.installRemoteClipboard(); const original = h.window.BridgeClipboard.copy;
  h.native(); assert.notEqual(h.window.BridgeClipboard.copy, original);
  const node = clipboard.CopyButton({ text: clipboard.sessionDeepLink(id) });
  h.event(true); node.props.onClick({ currentTarget: h.button }); await Promise.resolve(); await Promise.resolve();
  assert.deepEqual(h.calls, [{ method: "native", command: "codexbridge-copy:Mock", text: `codex://threads/${id}` }]);
  await h.window.BridgeClipboard.copy(() => "replay", h.button);
  h.event(false); await h.window.BridgeClipboard.copy(() => "synthetic", h.button);
  h.event(true, false); await h.window.BridgeClipboard.copy(() => "wrong target", h.button);
  assert.equal(h.calls.length, 1);
});

test("已发布适配器在 iframe 不安装 native 接口，bootstrap 也不覆盖现有接管", async t => {
  const h = browser(t, { iframe: true }); clipboard.installRemoteClipboard(); const original = h.window.BridgeClipboard.copy;
  h.native(); assert.equal(h.window.BridgeClipboard.copy, original);
  const installed = async () => true; h.window.BridgeClipboard.copy = installed; clipboard.installRemoteClipboard();
  assert.equal(h.window.BridgeClipboard.copy, installed); assert.equal(h.calls.length, 0);
});

test("已发布壳复制失败的公开事件会显示按钮反馈，而不是静默恢复原文案", async t => {
  const h = browser(t); clipboard.installRemoteClipboard(); h.window.prompt = () => "denied"; h.native();
  h.event(true); clipboard.CopyButton({ text: "Mock" }).props.onClick({ currentTarget: h.button });
  for (let i = 0; i < 4; i++) await Promise.resolve();
  assert.equal(h.button.disabled, false); assert.match(h.button.textContent, /复制失败/); assert.match(h.button.title, /复制失败/);
});
