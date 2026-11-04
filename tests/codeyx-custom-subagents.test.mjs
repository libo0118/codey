import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
import { createControllerHarness } from "./helpers/controller-harness.mjs";
import { autoStubModule, collectElements, elementProps, elementType, textContent } from "./helpers/jsx-tree.mjs";

const state = await loadTypeScriptModule(new URL("../src/features/subagents/state.ts", import.meta.url));
const { reconcileConfigDraft } = await loadTypeScriptModule(new URL("../src/configDraft.ts", import.meta.url));
const model = { value: "route/model", modelId: "model", label: "模型", providerId: "route", supportedReasoningEfforts: ["low", "ultra"], defaultReasoningEffort: "ultra" };
const fixture = () => ({
  settingsRevision: 1, profiles: [], activeProfileId: "", subagentOptimization: true,
  subagentModel: "route/model", subagentReasoningEffort: "low",
  subagentRoles: Object.fromEntries([...state.BUILTIN_SUBAGENT_ROLES, { id: "default" }]
    .map(({ id }) => [id, { enabled: true, model: "route/model", reasoningEffort: "low" }])),
});

test("custom roles default to read-only, use free IDs and stop at 32 without altering builtins", () => {
  const original = fixture();
  let config = state.addCustomSubagent(original, model);
  assert.deepEqual(config.subagentRoles.custom_agent_1, {
    enabled: true, model: model.value, reasoningEffort: "ultra", access: "readOnly",
    name: "自定义子代理 1", description: "用于主代理明确指定的专项调查与核验。",
    developerInstructions: "根据主代理交付的目标进行调查，返回结论、证据与未完成项。",
  });
  assert.equal(state.validateSubagents(config), null);
  for (const { id } of state.BUILTIN_SUBAGENT_ROLES) assert.equal(config.subagentRoles[id], original.subagentRoles[id]);
  for (let i = 1; i < 32; i++) config = state.addCustomSubagent(config, model);
  assert.equal(Object.keys(config.subagentRoles).filter(state.isCustomSubagent).length, 32);
  assert.equal(state.addCustomSubagent(config, model), config);
  assert.equal(Object.keys(original.subagentRoles).length, 6);
});

test("custom editing validates UTF-8 bytes without trimming or discarding invalid drafts", () => {
  const config = state.addCustomSubagent(fixture(), model);
  for (const { key, limit } of state.CUSTOM_SUBAGENT_FIELDS) {
    const valid = state.updateSubagent(config, "custom_agent_1", { [key]: "界".repeat(Math.floor(limit / 3)) });
    assert.equal(state.validateSubagents(valid), null);
    const tooLong = "界".repeat(Math.floor(limit / 3) + 1);
    const invalid = state.updateSubagent(config, "custom_agent_1", { [key]: tooLong });
    assert.match(state.validateSubagents(invalid), new RegExp(`${limit} 字节`));
    assert.equal(invalid.subagentRoles.custom_agent_1[key], tooLong);
    assert.notEqual(state.validateSubagents(state.updateSubagent(config, "custom_agent_1", { [key]: "  " })), null);
  }
  const configured = state.updateSubagent(config, "custom_agent_1", { access: "readWrite", reasoningEffort: "ultra" });
  assert.equal(configured.subagentRoles.custom_agent_1.access, "readWrite");
  assert.equal(state.selectSubagentModel(configured, "custom_agent_1", model).subagentRoles.custom_agent_1.reasoningEffort, "ultra");
  assert.equal(state.selectSubagentModel(configured, "custom_agent_1", { ...model, supportedReasoningEfforts: ["low"], defaultReasoningEffort: "low" }).subagentRoles.custom_agent_1.reasoningEffort, "low");
  const defaultChanged = state.selectSubagentModel(configured, "default", model);
  assert.equal(defaultChanged.subagentReasoningEffort, defaultChanged.subagentRoles.default.reasoningEffort);
});

test("last-enabled protection includes custom roles and deletion keeps an explicit pending tombstone", () => {
  let config = state.addCustomSubagent(fixture(), model);
  for (const { id } of state.BUILTIN_SUBAGENT_ROLES) config = state.updateSubagent(config, id, { enabled: false });
  assert.equal(state.enabledSubagentCount(config), 1);
  assert.equal(state.updateSubagent(config, "custom_agent_1", { enabled: false }), config);
  assert.equal(state.removeCustomSubagent(config, "custom_agent_1"), config);
  config = state.updateSubagent(config, "codey_worker", { enabled: true });
  const removed = state.removeCustomSubagent(config, "custom_agent_1");
  assert.equal(removed.subagentRoles.custom_agent_1, undefined);
  assert.deepEqual(removed.deletedSubagentRoles, ["custom_agent_1"]);
  assert.ok(state.addCustomSubagent(removed, model).subagentRoles.custom_agent_2);
  assert.equal(state.removeCustomSubagent(config, "default"), config);
  assert.equal(state.removeCustomSubagent(config, "codey_worker"), config);
  const { deletedSubagentRoles: _, ...saved } = removed;
  const result = reconcileConfigDraft(removed, removed, { ...saved, settingsRevision: 2 });
  assert.equal(result.config.deletedSubagentRoles, undefined);
  assert.equal(result.dirty, false);
});

test("real role controls keep drafts local, require deletion confirmation and disable while busy", () => {
  const ui = autoStubModule("ui");
  const combobox = autoStubModule("combobox");
  const confirmations = [];
  const h = createControllerHarness(new URL("../src/FeaturePolicyCard.tsx", import.meta.url), "SubagentPolicyCardComponent", {
    "./components/ui": ui, "./components/ModelCombobox": combobox,
    "./SettingsPageHeader": autoStubModule("header"), "@tabler/icons-react": autoStubModule("icons"),
    "./notifications/NotificationChannelsCard": autoStubModule("notifications"),
  });
  let config = fixture();
  const render = (isBusy = false) => h.render({ config, isBusy, subagentModelOptions: [model],
    onConfigChange: next => { config = next; }, onSubagentOptimizationChange() {},
    onRequestConfirmation: confirmation => confirmations.push(confirmation) });
  const controls = (tree, type) => collectElements(tree, node => elementType(node) === type).map(elementProps);
  let tree = render();
  controls(tree, ui.Button).find(props => textContent(props.children).includes("添加子代理")).onClick();
  tree = render();
  const name = controls(tree, ui.Input).find(props => props.id === "custom_agent_1-name");
  name.onChange({ target: { value: "" } });
  tree = render();
  assert.equal(config.subagentRoles.custom_agent_1.name, "");
  assert.match(textContent(tree), /名称不能为空/);
  assert.equal(elementProps(collectElements(tree, node => elementProps(node)?.id === "custom_agent_1-name-hint")[0]).className, "subagent-field-error");
  controls(tree, ui.Input).find(props => props.id === "custom_agent_1-name").onChange({ target: { value: "专项检查" } });
  tree = render();
  const access = controls(tree, ui.Select).find(props => props["aria-label"] === "专项检查读写能力");
  assert.equal(access.value, "readOnly"); access.onChange("readWrite");
  tree = render();
  controls(tree, ui.Button).find(props => props["aria-label"] === "删除专项检查").onClick();
  assert.equal(confirmations.length, 1);
  assert.ok(config.subagentRoles.custom_agent_1, "requesting confirmation must not delete");
  assert.equal(confirmations[0].confirmLabel, "删除子代理");
  confirmations[0].run();
  assert.equal(config.subagentRoles.custom_agent_1, undefined);
  tree = render(true);
  const add = controls(tree, ui.Button).find(props => textContent(props.children).includes("添加子代理"));
  assert.equal(add.disabled, true);
  const unchanged = config; add.onClick(); assert.equal(config, unchanged);
});

test("App save failures keep custom drafts and never emit a saved notice", async () => {
  const source = ts.createSourceFile("App.tsx", readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const declarations = new Map();
  const visit = node => {
    if (ts.isFunctionDeclaration(node) && ["persist", "saveCurrent"].includes(node.name?.text)) declarations.set(node.name.text, node.getText(source));
    if (ts.isVariableDeclaration(node) && node.name.getText(source) === "runOperation") declarations.set("runOperation", `const ${node.getText(source)};`);
    ts.forEachChild(node, visit);
  };
  visit(source);
  assert.equal(declarations.size, 3);
  const compiled = ts.transpileModule([...declarations.values()].join("\n"), { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;
  const createSave = new Function("dependencies", `const { config, validateSubagents, invoke, setNotice, setBusy, persistedConfigRef, applyCommandResult } = dependencies;
    const useCallback = callback => callback, isBusy = false, errorText = error => error.message;
    ${compiled}\nreturn saveCurrent;`);
  for (const invalid of [false, true]) {
    let config = state.addCustomSubagent(fixture(), model);
    config = state.removeCustomSubagent(state.addCustomSubagent(config, model), "custom_agent_2");
    if (invalid) config = state.updateSubagent(config, "custom_agent_1", { name: "" });
    const snapshot = structuredClone(config), notices = [], calls = [];
    const save = createSave({ config, validateSubagents: state.validateSubagents,
      persistedConfigRef: { current: fixture() }, setBusy() {}, setNotice: notice => notices.push(notice),
      applyCommandResult() { assert.fail("failed save must not replace the draft"); },
      async invoke(command, args) { calls.push({ command, args }); throw new Error("模拟保存失败"); },
    });
    await save();
    assert.equal(calls.length, invalid ? 0 : 1);
    if (!invalid) {
      assert.equal(calls[0].args.config.subagentRoles.custom_agent_1.name, snapshot.subagentRoles.custom_agent_1.name);
      assert.deepEqual(calls[0].args.config.deletedSubagentRoles, ["custom_agent_2"]);
      assert.equal(calls[0].args.deletedSubagentRoles, undefined);
    }
    assert.deepEqual(config, snapshot);
    assert.deepEqual(notices.map(notice => notice.tone), ["error"]);
    assert.doesNotMatch(notices[0].text, /已保存/);
  }
});

test("unrelated notification saves cannot commit pending role drafts, and restart warns before saving", async () => {
  const source = ts.createSourceFile("App.tsx", readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const declarations = [];
  const visit = node => {
    if (ts.isFunctionDeclaration(node) && ["persistNotificationChannels", "askRestartCodex"].includes(node.name?.text)) declarations.push(node.getText(source));
    ts.forEachChild(node, visit);
  };
  visit(source);
  assert.equal(declarations.length, 2);
  const compiled = ts.transpileModule(declarations.join("\n"), { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText;
  const create = new Function("dependencies", `const { dirty, persist, setNotice, setConfirmation } = dependencies;
    const isBusy = false, restartStatusError = false, setBusy = () => {}, errorText = error => error.message;
    ${compiled}\nreturn { persistNotificationChannels, askRestartCodex };`);
  for (const dirty of [false, true]) {
    const notices = [], confirmations = [], calls = [];
    const controller = create({ dirty, setNotice: notice => notices.push(notice), setConfirmation: confirmation => confirmations.push(confirmation),
      async persist(config) { calls.push(config); return {}; } });
    const config = state.removeCustomSubagent(state.addCustomSubagent(fixture(), model), "custom_agent_1");
    assert.equal(await controller.persistNotificationChannels(config, [], "通知已保存"), !dirty);
    assert.equal(calls.length, dirty ? 0 : 1);
    assert.equal(notices[0].tone, dirty ? "error" : "success");
    if (dirty) assert.match(notices[0].text, /先保存或放弃/);
    controller.askRestartCodex();
    assert.equal(confirmations.length, 1);
    assert.equal(confirmations[0].description.includes("保存当前所有未保存"), dirty);
  }
});
