import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { autoStubModule, collectElements, createModuleGraph, elementProps, elementType, textContent } from "./helpers/jsx-tree.mjs";

const ui = autoStubModule("ui");
const { FastContextToolsCard } = createModuleGraph(new URL("../src/features/builtin-tools/FastContextToolsCard.tsx", import.meta.url), { stubs: { "../../components/ui": ui } }).exports;
const options = { active: true, isBusy: false, config: { fastContextTools: true, retained: "same-draft" }, fastContextToolsStatus: { userConfigured: false, detectionFailed: false }, runtimeStatus: { running: true, fastContextToolsActive: true } };
const control = tree => elementProps(collectElements(tree, element => elementType(element) === ui.Switch)[0]);

test("FastCtx retains its one shared config editor and actual runtime status", () => {
  const changes = [];
  const tree = FastContextToolsCard({ ...options, onConfigChange: next => changes.push(next) });
  assert.equal(control(tree).checked, true); control(tree).onCheckedChange(false);
  assert.deepEqual(changes, [{ fastContextTools: false, retained: "same-draft" }]);
  assert.match(textContent(tree), /当前运行实例已加载/);
  const oldPage = readFileSync(new URL("../src/FeaturePolicyCard.tsx", import.meta.url), "utf8");
  const toolsPage = readFileSync(new URL("../src/features/builtin-tools/BuiltinToolsPage.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(oldPage, /aria-label="启用 FastCtx 上下文工具"/);
  assert.equal((toolsPage.match(/<FastContextToolsCard /g) ?? []).length, 1);
});

test("user-managed, failed detection, busy and inactive FastCtx controls never overwrite settings", () => {
  const changes = [];
  for (const extra of [{ fastContextToolsStatus: { userConfigured: true, detectionFailed: false, serverId: "user-fastctx" } }, { fastContextToolsStatus: { userConfigured: false, detectionFailed: true } }, { isBusy: true }, { active: false }]) {
    const tree = FastContextToolsCard({ ...options, ...extra, onConfigChange: next => changes.push(next) });
    control(tree).onCheckedChange(false);
    if (extra.active !== false) assert.equal(control(tree).disabled, true);
    if (extra.fastContextToolsStatus) assert.equal(control(tree).checked, false);
  }
  assert.equal(changes.length, 0);
});
