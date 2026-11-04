import assert from "node:assert/strict";
import test from "node:test";
import * as icons from "@tabler/icons-react";
import { createControllerHarness, deferred } from "./helpers/controller-harness.mjs";
import { collectElements, createModuleGraph, textContent } from "./helpers/jsx-tree.mjs";

const first = "12345678-1234-1234-1234-123456789abc", second = "abcdef12-1234-1234-1234-123456789abc";
const file = threadId => ({ name: "report.pdf", id: "opaque", downloadUrl: `/api/sessions/${threadId}/files/${"a".repeat(64)}`, size: 10, mime: "application/pdf" });
function fixture() {
  const calls = [];
  const h = createControllerHarness(new URL("../src/remote/files.tsx", import.meta.url), "useResolvedFile", {
    "@tabler/icons-react": icons,
    "./transport": { remoteRequest(...args) { const call = { args, ...deferred() }; calls.push(call); args[2].addEventListener("abort", () => call.reject(args[2].reason), { once: true }); return call.promise; } },
  });
  return { ...h, calls };
}

test("文件解析先授权元数据，成功后才给直接下载链接，不发送聊天动作", async () => {
  const h = fixture();
  assert.equal(h.render(first, "sandbox:/mnt/data/report.pdf", undefined, true).loading, true);
  assert.equal(h.calls.length, 1);
  assert.deepEqual(h.calls[0].args.slice(0, 2), ["/remote/files/resolve", { threadId: first, reference: "sandbox:/mnt/data/report.pdf" }]);
  h.calls[0].resolve({ file: file(first) });
  const result = await h.flush();
  assert.equal(result.file.downloadUrl, file(first).downloadUrl);
  assert.equal(result.loading, false); assert.equal(result.error, undefined); h.unmount();
});

test("跨会话和卸载取消旧解析，迟到成功与失败不回写新目标", async () => {
  const h = fixture();
  h.render(first, "report.pdf", undefined, true);
  const old = h.calls[0];
  h.render(second, "report.pdf", undefined, true);
  assert.equal(old.args[2].aborted, true);
  old.resolve({ file: file(first) });
  assert.equal((await h.flush()).file, undefined);
  h.calls[1].resolve({ file: file(second) });
  assert.equal((await h.flush()).file.downloadUrl, file(second).downloadUrl);
  h.render(first, "old.pdf", undefined, true);
  h.render(second, "new.pdf", undefined, true);
  h.calls[2].reject(new Error("old denied"));
  assert.equal((await h.flush()).error, undefined);
  h.unmount(); h.calls[3].resolve({ file: file(second) }); await h.flush();
  assert.equal(h.writesAfterUnmount, 0);
});

test("同会话新连接作废旧元数据与解析，不因普通增量重查，断线取消等待且不请求", async () => {
  const h = fixture();
  h.render(first, "report.pdf", undefined, true, 1); const old = h.calls[0];
  h.render(first, "report.pdf", undefined, false, 1); assert.equal(old.args[2].aborted, true);
  await h.flush(); assert.equal(h.calls.length, 1);
  assert.equal(h.render(first, "report.pdf", undefined, true, 2).file, undefined);
  await h.flush(); assert.equal(h.calls.length, 2); const fresh = h.calls[1];
  old.resolve({ file: file(first) }); assert.equal((await h.flush()).file, undefined);
  const nextFile = { ...file(first), downloadUrl: file(first).downloadUrl.replace(/a{64}$/, "b".repeat(64)) };
  fresh.resolve({ file: nextFile }); assert.equal((await h.flush()).file.downloadUrl, nextFile.downloadUrl);
  h.render(first, "report.pdf", undefined, true, 2); await h.flush(); assert.equal(h.calls.length, 2);
  h.render(first, "report.pdf", undefined, false, 2); h.render(first, "report.pdf", undefined, true, 3);
  assert.equal(h.render().file, undefined); assert.equal(h.calls.length, 3); h.unmount();
});

test("重连不再信任旧结构化 download descriptor，但保持原首帧直接附件与手动重试边界", async () => {
  const h = fixture();
  assert.equal(h.render(first, "opaque", file(first), true, 1).file.downloadUrl, file(first).downloadUrl);
  const next = h.render(first, "opaque", file(first), true, 2);
  assert.equal(next.file, undefined); assert.equal(h.calls.length, 1);
  h.calls[0].reject(new Error("Mock revoked descriptor")); assert.match((await h.flush()).error, /revoked/);
  h.render(first, "opaque", file(first), true, 2); assert.equal(h.calls.length, 1);
  h.render().retry(); h.render(); assert.equal(h.calls.length, 2); h.unmount();
});

test("会话切换首帧不残留旧 href，失效、云端与坏元数据明确显示原因", async () => {
  const h = fixture();
  h.render(first, "a.pdf", undefined, true); h.calls[0].resolve({ file: file(first) }); await h.flush();
  assert.equal(h.render(second, "a.pdf", undefined, true).file, undefined);
  h.calls[1].reject(new Error("此文件已消失或无权限")); assert.match((await h.flush()).error, /消失或无权限/);
  h.render(second, "sandbox:/a.pdf", undefined, true); h.calls[2].resolve({ file: { name: "a.pdf", unavailableReason: "云端 sandbox 文件请在电脑端查看" } });
  assert.match((await h.flush()).error, /sandbox/);
  for (const invalid of [null, { file: { name: "a", size: -1 } }, { file: { ...file(first), downloadUrl: "https://evil.invalid/a" } }]) {
    h.render(second, `bad-${h.calls.length}`, undefined, true); h.calls.at(-1).resolve(invalid);
    assert.match((await h.flush()).error, /元数据无效|没有可用/);
  }
  h.unmount();
});

test("现有授权附件不重复解析，未连接和隐藏页面不发请求", () => {
  const h = fixture();
  assert.equal(h.render(first, "opaque", file(first), true).file.id, "opaque");
  h.render(first, "report.pdf", undefined, false); h.render("", "report.pdf", undefined, true);
  assert.equal(h.calls.length, 0); h.unmount();
});

test("繁忙或文件变化不自动重试，显式重新检查可获得新元数据", async () => {
  const h = fixture(); h.render(first, "report.pdf", undefined, true);
  h.calls[0].reject(new Error("文件服务繁忙，请稍后重新检查"));
  let result = await h.flush(); assert.match(result.error, /繁忙/); assert.equal(h.calls.length, 1);
  result.retry(); result = h.render(); assert.equal(result.loading, true); assert.equal(result.file, undefined);
  assert.equal(h.calls.length, 2); h.calls[1].resolve({ file: file(first) });
  assert.equal((await h.flush()).file.downloadUrl, file(first).downloadUrl); h.unmount();
});

test("一个已标记不可用的结构化附件也可以显式重新检查", async () => {
  const h = fixture(); let result = h.render(first, "opaque", { id: "opaque", name: "report.pdf", unavailableReason: "文件已变化" }, true);
  assert.equal(h.calls.length, 0); result.retry(); h.render(); assert.equal(h.calls.length, 1);
  h.calls[0].resolve({ file: file(first) }); assert.equal((await h.flush()).error, undefined); h.unmount();
});

test("元数据解析全局串行且等待队列有上限，取消排队项不占用下一次请求", async () => {
  const calls = [];
  const { resolveFile } = createModuleGraph(new URL("../src/remote/file-requests.ts", import.meta.url), { stubs: { "./transport": { remoteRequest(...args) { const call = { args, ...deferred() }; calls.push(call); return call.promise; } } } }).exports;
  const controllers = Array.from({ length: 19 }, () => new AbortController());
  const pending = controllers.map((controller, index) => resolveFile(first, `file-${index}`, controller.signal).then(value => value, error => error));
  assert.equal(calls.length, 1);
  assert.match((await pending[17]).message, /较多/); assert.match((await pending[18]).message, /较多/);
  controllers[1].abort(); await pending[1]; calls[0].resolve({ file: file(first) });
  for (let i = 0; i < 4; i++) await Promise.resolve();
  assert.equal(calls.length, 2); assert.equal(calls[1].args[1].reference, "file-2");
  for (const controller of controllers.slice(2)) controller.abort(); calls[1].resolve({ file: file(first) });
  await Promise.all(pending);
});

test("图片 GET 繁忙或变化后提供显式重新预览，不自动重试也不永久消失", () => {
  const h = createControllerHarness(new URL("../src/remote/files.tsx", import.meta.url), "FileImage", { "@tabler/icons-react": icons });
  let tree = h.render({ url: file(first).downloadUrl, alt: "Mock image" });
  let image = collectElements(tree, e => e.type === "img")[0]; assert.equal(image.props.src, file(first).downloadUrl);
  image.props.onError(); tree = h.render(); assert.equal(collectElements(tree, e => e.type === "img").length, 0);
  assert.match(textContent(tree), /文件已变化或服务繁忙/);
  h.render(); assert.equal(collectElements(h.render(), e => e.type === "img").length, 0);
  collectElements(tree, e => e.type === "button")[0].props.onClick(); tree = h.render();
  assert.equal(collectElements(tree, e => e.type === "img")[0].props.src, file(first).downloadUrl); h.unmount();
});

test("文件传输沿用同源 Cookie/CSRF/重定向拒绝，取消和旧登录代次的 401 不注销新登录", async t => {
  const { remoteRequest, clearRemoteSession } = createModuleGraph(new URL("../src/remote/transport.ts", import.meta.url)).exports;
  const previousFetch = globalThis.fetch, previousWindow = globalThis.window;
  let unauthorized = 0; const events = new EventTarget(); events.addEventListener("codey-remote-unauthorized", () => unauthorized++); globalThis.window = events;
  const calls = []; let pending;
  globalThis.fetch = (path, options) => { calls.push({ path, options });
    if (path === "/remote/files/resolve") { pending = deferred(); return pending.promise; }
    return Promise.resolve({ ok: true, status: 200, json: async () => ({ authenticated: true, csrfToken: path === "/remote/login" ? "new-Mock-token" : "old-Mock-token" }) });
  };
  t.after(() => { globalThis.fetch = previousFetch; if (previousWindow === undefined) delete globalThis.window; else globalThis.window = previousWindow; clearRemoteSession(); });
  await remoteRequest("/remote/session"); const controller = new AbortController();
  const old = remoteRequest("/remote/files/resolve", { threadId: first, reference: "report.pdf" }, controller.signal);
  assert.equal(calls.at(-1).options.credentials, "same-origin"); assert.equal(calls.at(-1).options.redirect, "error");
  assert.equal(calls.at(-1).options.headers["X-CSRF-Token"], "old-Mock-token");
  clearRemoteSession(); await remoteRequest("/remote/login", { username: "Mock", password: "Mock-not-secret" });
  pending.resolve({ ok: false, status: 401, json: async () => ({ message: "old denied" }) }); await assert.rejects(old, /old denied/);
  assert.equal(unauthorized, 0);
  const canceled = remoteRequest("/remote/files/resolve", { threadId: first, reference: "canceled.pdf" }, controller.signal);
  controller.abort(); assert.equal(calls.at(-1).options.signal.aborted, true);
  pending.resolve({ ok: false, status: 401, json: async () => ({}) }); await assert.rejects(canceled, error => error.name === "AbortError");
  assert.equal(unauthorized, 0);
});
