import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const adapter = readFileSync(new URL("../public/codey-remote-control.js", import.meta.url), "utf8");
const file = (name, bytes) => ({ name, url: `data:application/octet-stream;base64,${Buffer.from(bytes).toString("base64")}` });
const plain = value => JSON.parse(JSON.stringify(value));

function runtime(overrides = {}) {
  const calls = { created: [], removed: [], started: [] };
  const manager = {
    async createFileAttachment({ contents, label }) {
      assert.ok(contents instanceof Blob);
      calls.created.push({ label, bytes: [...new Uint8Array(await contents.arrayBuffer())] });
      return { label, path: `/managed/${label}`, fsPath: `/managed/${label}` };
    },
    async removePastedTextAttachment(path) { calls.removed.push(path); },
    async startConversation(params, options) {
      calls.started.push({ params, inputs: await options.readThreadCreationInputs() });
      return { status: "created", conversationId: "thread", firstTurn: { status: "started" } };
    },
    ...overrides,
  };
  const window = {
    setTimeout, clearTimeout,
    __codeyModelWhitelistPatch: { ensureRequestGate: async () => true, requestGateReady: () => true },
    __codeyAppServerManagerResolverFromModule: () => () => manager,
    __codeyAppServerManagerRegistryResolverFromModule: () => null,
    __codeyAppServerManagerFromReact: resolver => resolver({ get: () => null }, "local"),
    __codeyImportCodexAsset: async () => ({
      factory: function (scope, hostId) {
        return { scope: scope, hostId: hostId, requestClient: scope.get("client"), dynamicTools: new Map(), readCreationInputs: async () => "native inputs" };
      },
    }),
  };
  const context = vm.createContext({ window, Blob, Uint8Array, atob, document: { scripts: [{ src: "app://codex/assets/app-initial-fixture.js" }], querySelectorAll: () => [] }, performance: { getEntriesByType: () => [] } });
  vm.runInContext(adapter, context);
  const run = args => window.__codeyRemoteControl(args, Date.now() + 10_000);
  return { window, calls, run };
}

test("submitted files are registered as real Blobs and create preserves native metadata", async () => {
  const app = runtime();
  assert.deepEqual(app.calls.created, []);
  const result = await app.run({ action: "attachments", files: [file("报告.pdf", [0, 1, 255]), file("empty.txt", [])] });
  assert.deepEqual(app.calls.created, [{ label: "报告.pdf", bytes: [0, 1, 255] }, { label: "empty.txt", bytes: [] }]);
  const attachments = plain(result.files);
  assert.deepEqual(attachments, [
    { label: "报告.pdf", path: "/managed/报告.pdf", fsPath: "/managed/报告.pdf" },
    { label: "empty.txt", path: "/managed/empty.txt", fsPath: "/managed/empty.txt" },
  ]);
  const params = { input: [{ type: "text", text: "# Files mentioned by the user:\n\n## 报告.pdf: /managed/报告.pdf\n" }], attachments };
  assert.equal((await app.run({ action: "create", params })).id, "thread");
  assert.equal(app.calls.started.length, 1);
  assert.equal(app.calls.started[0].params, params);
  assert.deepEqual(app.calls.started[0].params.attachments, attachments);
  assert.equal(app.calls.started[0].inputs, "native inputs");
  assert.deepEqual(app.calls.removed, []);
});

test("partial registration and gate revocation clean up, without retrying writes", async () => {
  const first = runtime();
  let calls = 0;
  first.window.__codeyAppServerManagerResolverFromModule = () => () => ({
    startConversation() {},
    async createFileAttachment({ label }) {
      if (++calls === 2) throw Error("native failure");
      return { label, path: `/managed/${label}`, fsPath: `/managed/${label}` };
    },
    async removePastedTextAttachment(path) { first.calls.removed.push(path); },
  });
  await assert.rejects(first.run({ action: "attachments", files: [file("a", [1]), file("b", [2])] }), /native failure/);
  assert.equal(calls, 2);
  assert.deepEqual(first.calls.removed, ["/managed/a"]);

  const second = runtime();
  let release;
  const pending = new Promise(resolve => { release = resolve; });
  let created = 0;
  second.window.__codeyAppServerManagerResolverFromModule = () => () => ({
    startConversation() {},
    async createFileAttachment({ label }) { created++; await pending; return { label, path: `/managed/${label}`, fsPath: `/managed/${label}` }; },
    async removePastedTextAttachment(path) { second.calls.removed.push(path); },
  });
  const operation = second.run({ action: "attachments", files: [file("a", [1])] });
  while (!created) await new Promise(setImmediate);
  second.window.__codeyModelWhitelistPatch = { requestGateReady: () => true };
  release();
  await assert.rejects(operation, /模型线路尚未就绪/);
  assert.equal(created, 1);
  assert.deepEqual(second.calls.removed, ["/managed/a"]);
});

test("rejected gates never register; explicit discard attempts every cleanup and reports failures", async () => {
  const app = runtime();
  app.window.__codeyModelWhitelistPatch = { ensureRequestGate: async () => false, requestGateReady: () => false };
  await assert.rejects(app.run({ action: "attachments", files: [file("a", [1])] }), /模型线路尚未就绪/);
  assert.deepEqual(app.calls.created, []);

  const removed = [];
  const failing = runtime({ removePastedTextAttachment(path) { removed.push(path); if (path.endsWith("a")) throw Error("offline"); return Promise.resolve(); } });
  await assert.rejects(failing.run({ action: "discard-attachments", files: [{ path: "/managed/a" }, { path: "/managed/b" }] }), /清理未确认/);
  assert.deepEqual(removed, ["/managed/a", "/managed/b"]);
});

test("invalid native metadata is rejected and its known managed path is discarded", async () => {
  const removed = [];
  const app = runtime({
    async createFileAttachment({ label }) { return { label, path: "/managed/a", fsPath: "/different" }; },
    async removePastedTextAttachment(path) { removed.push(path); },
  });
  await assert.rejects(app.run({ action: "attachments", files: [file("a", [1])] }), /附件创建结果无效/);
  assert.deepEqual(removed, ["/managed/a"]);
  assert.deepEqual(app.calls.started, []);
});
