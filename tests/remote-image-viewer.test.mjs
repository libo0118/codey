import assert from "node:assert/strict";
import test from "node:test";
import * as icons from "@tabler/icons-react";
import { createControllerHarness } from "./helpers/controller-harness.mjs";
import { collectElements } from "./helpers/jsx-tree.mjs";

test("同源会话图片点击大图、原尺寸、关闭还原焦点，组件失效销毁大图而不保留旧引用", t => {
  const children = [];
  function element(tagName) {
    const classes = new Set();
    return { tagName, children: [], attrs: {}, isConnected: true,
      classList: { toggle(name) { if (classes.has(name)) { classes.delete(name); return false; } classes.add(name); return true; }, contains: name => classes.has(name) },
      append(...nodes) { this.children.push(...nodes); }, setAttribute(name, value) { this.attrs[name] = value; },
      remove() { this.removed = true; }, showModal() { this.open = true; }, close() { this.open = false; this.onclose?.(); },
    };
  }
  const previous = globalThis.document;
  globalThis.document = { createElement: element, body: { append: node => children.push(node) } };
  t.after(() => { if (previous === undefined) delete globalThis.document; else globalThis.document = previous; });
  const h = createControllerHarness(new URL("../src/remote/files.tsx", import.meta.url), "FileImage", { "@tabler/icons-react": icons });
  t.after(() => h.unmount());
  const url = `/api/sessions/12345678-1234-1234-1234-123456789abc/files/${"a".repeat(64)}`;
  const tree = h.render({ url, alt: "Mock image" });
  const button = collectElements(tree, node => node.type === "button" && node.props.className === "remote-image-open")[0];
  assert.ok(button, "图片本身必须有可访问的大图按钮");
  assert.equal(children.length, 0, "未点击不创建大图或发起额外图片读取");
  let focused = 0;
  const trigger = { isConnected: true, focus: () => focused++ };
  button.props.onClick({ currentTarget: trigger });
  const modal = children[0], [header, stage] = modal.children;
  assert.equal(modal.open, true); assert.equal(stage.children[0].src, url);
  const zoom = header.children.find(child => child.textContent === "原始尺寸");
  zoom.onclick(); assert.equal(stage.classList.contains("is-original"), true);
  zoom.onclick(); assert.equal(stage.classList.contains("is-original"), false);
  header.children.find(child => child.textContent === "关闭").onclick();
  assert.equal(modal.removed, true); assert.equal(focused, 1);
  button.props.onClick({ currentTarget: trigger }); h.unmount();
  assert.equal(children[1].removed, true);
});
