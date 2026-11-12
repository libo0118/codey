import assert from "node:assert/strict";
import test from "node:test";
import * as icons from "@tabler/icons-react";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
import { collectElements, createModuleGraph, textContent } from "./helpers/jsx-tree.mjs";

const media = await loadTypeScriptModule(new URL("../src/remote/images.ts", import.meta.url));
const state = await loadTypeScriptModule(new URL("../src/features/remote-workspace/state.ts", import.meta.url));
const transport = await loadTypeScriptModule(new URL("../src/remote/transport.ts", import.meta.url));

function globals(t, values) {
  for (const [name, value] of Object.entries(values)) {
    const original = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
    t.after(() => original ? Object.defineProperty(globalThis, name, original) : delete globalThis[name]);
  }
}

class MockReader {
  static LOADING = 1;
  readyState = 0;
  result = null;
  readAsDataURL(file) {
    this.readyState = MockReader.LOADING;
    file.arrayBuffer().then(bytes => {
      if (this.readyState !== MockReader.LOADING) return;
      this.readyState = 2;
      if (file.name === "broken.bin") return this.onerror?.();
      this.result = `data:${file.type || "application/octet-stream"};base64,${Buffer.from(bytes).toString("base64")}`;
      this.onload?.();
    });
  }
  abort() { this.readyState = 2; this.onabort?.(); }
}

const tick = () => new Promise(resolve => setImmediate(resolve));

// Mount actual Composer hooks and refs against native-like targets; requests never leave the process.
function composerHarness(t) {
  globals(t, { document: new EventTarget(), FileReader: MockReader });
  const picker = () => Object.assign(new EventTarget(), { value: "", files: [], shown: 0, showPicker() { this.shown++; } });
  const camera = picker(), photos = picker(), files = picker();
  const dialog = { open: false, showModal() { this.open = true; }, close() { this.open = false; } };
  const refs = [
    { style: {}, scrollHeight: 32 }, { contains: () => true, querySelector: () => null },
    { focus() {} }, { focus() {} }, dialog, camera, photos, files, null, null,
  ];
  let cursor = 0, nextRef = 0, effects = [];
  const hooks = [];
  const equal = (a, b) => a && b && a.length === b.length && a.every((item, index) => Object.is(item, b[index]));
  const react = {
    useId: () => ":test:",
    useRef(initial) { const i = cursor++; return hooks[i] ||= { current: initial === null ? refs[nextRef++] : initial }; },
    useState(initial) {
      const i = cursor++;
      if (!(i in hooks)) hooks[i] = { value: typeof initial === "function" ? initial() : initial };
      return [hooks[i].value, next => { hooks[i].value = typeof next === "function" ? next(hooks[i].value) : next; }];
    },
    useEffect(effect, deps) {
      const i = cursor++;
      if (!equal(hooks[i]?.deps, deps)) effects.push(() => { hooks[i]?.cleanup?.(); hooks[i] = { deps, cleanup: effect() }; });
    },
  };
  react.useLayoutEffect = react.useEffect;
  const { Composer } = createModuleGraph(new URL("../src/remote/composer.tsx", import.meta.url), { stubs: { react, "@tabler/icons-react": icons } }).exports;
  const props = { draft: "", onDraft() {}, images: [], onImages(next) { props.images = next; }, view: null, models: [], connected: true, busy: false, uncertain: false, onSend() {}, onStop() {}, onSettings() {}, catalogError: "", onReloadModels() {} };
  return {
    props, files, dialog,
    render() { cursor = 0; nextRef = 0; const tree = Composer(props); effects.splice(0).forEach(effect => effect()); return tree; },
    unmount() { hooks.forEach(hook => hook?.cleanup?.()); },
  };
}

const byLabel = (tree, label) => collectElements(tree, node => node.props["aria-label"] === label)[0];
const buttonText = (tree, label) => collectElements(tree, node => node.type === "button" && textContent(node) === label)[0];

test("ordinary files read bytes without image decoding, respect Unicode names, and report cancellation or read errors", async t => {
  globals(t, { FileReader: MockReader });
  const file = new File([Uint8Array.from([0, 1, 255])], `${"😀".repeat(190)}.pdf`, { type: "application/pdf" });
  assert.deepEqual(await media.readFiles([file], [], new AbortController().signal), [{ name: file.name, size: 3, kind: "file", url: "data:application/octet-stream;base64,AAH/" }]);
  await assert.rejects(media.readFiles([new File(["x"], "line\u2028break.txt")], [], new AbortController().signal), /文件名/);
  await assert.rejects(media.readFiles([new File(["x"], "broken.bin")], [], new AbortController().signal), /无法读取文件/);
  const controller = new AbortController();
  const work = media.readFiles([file], [], controller.signal); controller.abort();
  await assert.rejects(work, error => error.name === "AbortError");
});

test("Composer connects file picker, cancel, read, budget, preview, remove and reload marker guards", async t => {
  const h = composerHarness(t);
  try {
    let tree = h.render(); byLabel(tree, "添加附件").props.onClick(); tree = h.render();
    assert.ok(buttonText(tree, "相机")); assert.ok(buttonText(tree, "照片")); buttonText(tree, "文件").props.onClick();
    tree = h.render(); assert.equal(h.files.shown, 1); assert.match(textContent(tree), /最多 4 个、4 MB/);
    h.files.dispatchEvent(new Event("cancel")); tree = h.render(); assert.match(textContent(tree), /已取消选择文件/);
    h.files.files = []; byLabel(tree, "选择文件").props.onChange({ currentTarget: h.files }); tree = h.render();
    assert.match(textContent(tree), /已取消选择文件/);
    h.files.files = [new File([Uint8Array.from([0, 1, 255])], "report.pdf", { type: "application/pdf" })];
    byLabel(tree, "选择文件").props.onChange({ currentTarget: h.files }); tree = h.render();
    assert.match(textContent(tree), /正在读取附件/); await tick(); tree = h.render();
    assert.match(textContent(tree), /report\.pdf/); assert.match(textContent(tree), /刷新.*重新选择/);
    const preview = collectElements(tree, node => node.type === "figure")[0];
    assert.equal(collectElements(preview, node => node.type === "img").length, 0);
    assert.equal(collectElements(preview, node => node.type === icons.IconFile).length, 1);
    assert.equal(h.props.images[0].url, "data:application/octet-stream;base64,AAH/");
    byLabel(tree, "移除文件 report.pdf").props.onClick(); assert.equal(h.props.images.length, 0);
    tree = h.render(); h.files.files = Array.from({ length: 5 }, (_, index) => new File(["x"], `${index}.txt`));
    byLabel(tree, "选择文件").props.onChange({ currentTarget: h.files }); await tick(); tree = h.render();
    assert.match(textContent(tree), /最多添加 4 个附件/); assert.equal(h.props.images.length, 0);
    h.files.files = [new File(["x"], "broken.bin")]; byLabel(tree, "选择文件").props.onChange({ currentTarget: h.files }); await tick(); tree = h.render();
    assert.match(textContent(tree), /无法读取文件/); assert.equal(h.props.images.length, 0);
    h.props.images = [{ name: "old.png", size: 1, url: "data:image/png;base64,eA==" }]; tree = h.render();
    assert.equal(collectElements(tree, node => node.type === "img")[0].props.alt, "old.png");
    byLabel(tree, "移除图片 old.png").props.onClick();
    h.props.images = [{ name: "report.pdf", kind: "file", size: 3, url: "" }]; tree = h.render();
    assert.match(textContent(tree), /重新选择/); assert.equal(byLabel(tree, "发送消息").props.disabled, true);
    byLabel(tree, "移除文件 report.pdf").props.onClick(); assert.equal(h.props.images.length, 0);
  } finally { h.unmount(); }
});

test("mobile composer keeps input and task actions together and reuses settings actions in its advanced sheet", t => {
  const h = composerHarness(t), settings = [];
  h.props.view = { model: "model-a", effort: "high", permissionMode: "auto", serviceTier: "default", turns: [] };
  h.props.models = [{ id: "model-a", label: "Model A", route: "Mock", efforts: ["high"], defaultEffort: "high" }];
  h.props.onSettings = value => settings.push(value);
  try {
    let tree = h.render();
    const row = collectElements(tree, node => node.props.className === "remote-composer-input-row")[0];
    assert.ok(byLabel(row, "发送给 Codex 的指令"));
    assert.ok(byLabel(row, "添加附件"));
    assert.ok(byLabel(row, "发送消息"));
    assert.match(textContent(byLabel(tree, "调整模型与思考强度")), /Model A/);
    byLabel(tree, "调整模型与思考强度").props.onClick(); tree = h.render();
    collectElements(tree, node => node.type.name === "EffortControl")[0].props.onAdvanced(); tree = h.render();
    assert.equal(h.dialog.open, true);
    const permission = byLabel(tree, "高级访问权限");
    assert.equal(permission.props.value, "auto");
    permission.props.onChange({ target: { value: "read-only" } });
    byLabel(tree, "权限").props.onChange({ target: { value: "auto" } });
    assert.deepEqual(settings, [{ permissionMode: "read-only" }, { permissionMode: "auto" }]);
    assert.match(textContent(permission), /完全访问 · 无沙箱限制/);
    h.props.connected = false; tree = h.render();
    assert.equal(byLabel(tree, "高级访问权限").props.disabled, true);
    assert.equal(byLabel(tree, "权限").props.disabled, true);
    assert.equal(byLabel(tree, "模型").props.disabled, true);
    buttonText(tree, "完成").props.onClick(); h.render();
    assert.equal(h.dialog.open, false);
  } finally { h.unmount(); }
});

test("journal keeps full payload in memory but stores file markers, including quota fallback and legacy images", t => {
  const values = new Map(); let quota = Infinity;
  const storage = { getItem: key => values.get(key) ?? null, setItem(key, value) { if (value.length > quota) throw new Error("quota"); values.set(key, value); }, removeItem: key => values.delete(key) };
  globals(t, { sessionStorage: storage });
  const image = { name: "old.png", size: 1, url: `data:image/png;base64,${"A".repeat(3000)}` };
  const file = { name: "report.pdf", size: 3, kind: "file", url: "data:application/octet-stream;base64,AAH/" };
  for (const fallback of [false, true]) {
    const scope = `test-${fallback}`; quota = fallback ? 500 : Infinity;
    const record = transport.rememberSubmission(scope, { text: "hello", images: [image], files: [file] });
    assert.deepEqual(record.args.files, [file]);
    const stored = values.get(`codey-remote-pending:${scope}`);
    assert.ok(stored); assert.doesNotMatch(stored, /AAH\//);
    transport.forgetSubmission(scope); values.set(`codey-remote-pending:${scope}`, stored);
    const restored = state.restoreWorkspaceDraft(undefined, transport.readSubmission(scope));
    assert.equal(restored.uncertain, true); assert.equal(state.canSendDraft(restored), false);
    assert.deepEqual(restored.images, [fallback ? { ...image, url: "" } : image, { ...file, url: "" }]);
    transport.forgetSubmission(scope);
  }
  const legacy = state.restoreWorkspaceDraft(undefined, { id: "legacy", args: { text: "old", images: [image] } });
  assert.deepEqual(legacy.images, [image]); assert.equal(legacy.uncertain, true);
});

test("file submissions fail visibly before the request if even a journal marker cannot be saved", t => {
  globals(t, { sessionStorage: { getItem: () => null, setItem() { throw new Error("quota"); }, removeItem() {} } });
  const file = { name: "a.txt", size: 1, kind: "file", url: "data:application/octet-stream;base64,eA==" };
  assert.throws(() => transport.rememberSubmission("full-quota", { text: "x", images: [], files: [file] }), /保存.*记录/);
  transport.forgetSubmission("full-quota");
});

test("request adapter forwards create and send/steer file arrays unchanged", async () => {
  const calls = [];
  const stub = { remoteRequest: (path, args) => { calls.push({ path, args }); return Promise.resolve({}); }, requestId: () => "new-id" };
  const { remoteWorkspaceRequests: requests } = createModuleGraph(new URL("../src/features/remote-workspace/requests.ts", import.meta.url), { stubs: { "../../remote/transport": stub } }).exports;
  const image = { name: "old.png", size: 1, url: "data:image/png;base64,eA==" };
  const file = { name: "report.pdf", size: 3, kind: "file", url: "data:application/octet-stream;base64,AAH/" };
  const create = { id: "create-id", args: { projectId: "p", text: "create", images: [image], files: [file] } };
  await requests.create(create);
  for (const action of ["send", "steer"]) await requests.submit("thread", { id: `${action}-id`, args: { action, text: action, images: [image], files: [file] } });
  assert.deepEqual(calls, [
    { path: "/remote/create", args: { ...create.args, requestId: "create-id" } },
    ...["send", "steer"].map(action => ({ path: "/remote/action", args: { action, text: action, images: [image], files: [file], threadId: "thread", requestId: `${action}-id` } })),
  ]);
});
