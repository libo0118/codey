import assert from "node:assert/strict";
import test from "node:test";
import * as icons from "@tabler/icons-react";
import { createControllerHarness } from "./helpers/controller-harness.mjs";
import { collectElements } from "./helpers/jsx-tree.mjs";

test("图片查看左侧返回，原尺寸与手势只缩放图片，退出还原焦点和销毁旧引用", t => {
  const children = [];
  function element(tagName) {
    const classes = new Set();
    return { tagName, children: [], attrs: {}, style: {}, handlers: {}, isConnected: true,
      clientWidth: 640, clientHeight: 480, naturalWidth: 1600, naturalHeight: 1200, scrollLeft: 0, scrollTop: 0,
      classList: { toggle(name, force = !classes.has(name)) { if (force) classes.add(name); else classes.delete(name); return force; }, contains: name => classes.has(name) },
      append(...nodes) { this.children.push(...nodes); }, setAttribute(name, value) { this.attrs[name] = value; },
      addEventListener(name, handler, options) { this.handlers[name] = { handler, options }; },
      getBoundingClientRect() { return { left: 0, top: 0, width: this.clientWidth, height: this.clientHeight }; },
      remove() { this.removed = true; }, showModal() { this.open = true; }, close() { this.open = false; this.onclose?.(); },
    };
  }
  const previous = globalThis.document, previousObserver = globalThis.ResizeObserver;
  globalThis.ResizeObserver = class { observe() {} disconnect() {} };
  globalThis.document = { createElement: element, body: { append: node => children.push(node) } };
  t.after(() => { if (previous === undefined) delete globalThis.document; else globalThis.document = previous; if (previousObserver === undefined) delete globalThis.ResizeObserver; else globalThis.ResizeObserver = previousObserver; });
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
  assert.match(header.children[0].textContent, /返回对话/);
  assert.equal(header.children.some(child => child.textContent === "关闭"), false, "右上角不再提供冲突的关闭按钮");
  const image = stage.children[0]; image.onload();
  const zoom = header.children.find(child => child.textContent === "原始尺寸");
  zoom.onclick(); assert.equal(stage.classList.contains("is-original"), true);
  assert.equal(image.style.width, "1600px");
  zoom.onclick(); assert.equal(stage.classList.contains("is-original"), false);
  assert.equal(image.style.width, "640px");
  const touch = distance => ({ touches: [{ clientX: 200, clientY: 200 }, { clientX: 200 + distance, clientY: 200 }], preventDefault() { this.prevented = true; } });
  const start = touch(100), move = touch(200);
  stage.handlers.touchstart.handler(start); stage.handlers.touchmove.handler(move);
  assert.equal(move.prevented, true); assert.equal(stage.handlers.touchmove.options.passive, false);
  assert.equal(image.style.width, "1280px");
  const wheel = { ctrlKey: true, deltaY: -100, clientX: 320, clientY: 240, preventDefault() { this.prevented = true; } };
  modal.handlers.wheel.handler(wheel); assert.equal(wheel.prevented, true); assert.equal(modal.handlers.wheel.options.passive, false);
  assert.ok(parseFloat(image.style.width) > 1280);
  header.children[0].onclick();
  assert.equal(modal.removed, true); assert.equal(focused, 1);
  button.props.onClick({ currentTarget: trigger }); h.unmount();
  assert.equal(children[1].removed, true);
});
