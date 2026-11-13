import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "@tabler/icons-react";
import * as nativeWeb from "react-native-web";
import MarkdownIt from "markdown-it";
import { renderToStaticMarkup } from "react-dom/server";
import { collectElements, createModuleGraph, textContent } from "./helpers/jsx-tree.mjs";
import { createControllerHarness } from "./helpers/controller-harness.mjs";

const stubs = { "@tabler/icons-react": icons, "react-native-web": nativeWeb, "markdown-it": MarkdownIt };
const projects = [{ id: "p", name: "Project", cwd: "/synthetic/project" }, { id: "q", name: "Other", cwd: "/synthetic/other" }];
const entry = new URL("../src/remote/cindy/NewSessionSurface.tsx", import.meta.url);

test("Cindy 新建页只选择现有工作区，展示项目路径并透传原项目与返回动作", () => {
  const calls = [], graph = createModuleGraph(entry, { stubs });
  const props = { projects, project: projects[0], busy: false, defaultsReady: true, onProject: p => calls.push(p), onBack: () => calls.push("back"), children: "composer-slot" };
  let tree = graph.exports.NewSessionSurface(props);
  assert.match(textContent(tree), /Project/); assert.match(textContent(tree), /\/synthetic\/project/);
  assert.doesNotMatch(textContent(tree), /云同伴|worktree|已配对|已连接/);
  const [back] = collectElements(tree, node => node.props["data-codey-action"] === "back");
  back.props.onClick(); assert.deepEqual(calls, ["back"]);
  collectElements(tree, node => node.props["aria-label"] === "选择工作区")[0].props.onPress();
  graph.restart(); tree = graph.exports.NewSessionSurface(props);
  assert.equal(collectElements(tree, node => node.props["aria-label"] === "工作区").length, 1);
  collectElements(tree, node => node.props["aria-label"] === "选择项目 Other")[0].props.onPress();
  assert.deepEqual(calls, ["back", projects[1]]);
  graph.restart(); tree = graph.exports.NewSessionSurface({ ...props, project: projects[1] });
  assert.equal(collectElements(tree, node => node.props["aria-label"] === "工作区").length, 0);
});

test("没有项目仍可明确选择不指定工作目录，忙碌和默认值未就绪保持禁发", () => {
  const graph = createModuleGraph(entry, { stubs });
  let tree = graph.exports.NewSessionSurface({ projects: [], busy: false, defaultsReady: false });
  assert.equal(collectElements(tree, node => node.props["aria-label"] === "选择工作区")[0].props.disabled, false);
  assert.match(textContent(tree), /没有可用项目|保存本地项目/);
  graph.restart(); tree = graph.exports.NewSessionSurface({ projects, project: projects[0], busy: true, defaultsReady: false });
  assert.equal(collectElements(tree, node => node.props["aria-label"] === "选择工作区")[0].props.disabled, true);
  assert.equal(collectElements(tree, node => node.props["data-codey-action"] === "back")[0].props.disabled, true);
  assert.match(textContent(tree), /默认设置尚未就绪/);
});

test("不指定工作目录是独立的 null 选择，不伪造主目录或已保存项目", () => {
  const calls = [], graph = createModuleGraph(entry, { stubs });
  const props = { projects, project: projects[0], projectless: false, busy: false, defaultsReady: true, onProject: p => calls.push(p) };
  let tree = graph.exports.NewSessionSurface(props);
  collectElements(tree, node => node.props["aria-label"] === "选择工作区")[0].props.onPress();
  graph.restart(); tree = graph.exports.NewSessionSurface(props);
  collectElements(tree, node => node.props["aria-label"] === "不指定工作目录")[0].props.onPress();
  assert.deepEqual(calls, [null]);
  graph.restart(); tree = graph.exports.NewSessionSurface({ ...props, project: undefined, projectless: true });
  assert.match(textContent(tree), /不指定工作目录/); assert.match(textContent(tree), /Codex 管理实际目录/);
  assert.doesNotMatch(textContent(tree), /\/home|没有可用项目/);
});

function controller(scene) {
  const view = { id: "synthetic", title: "合成会话", cwd: projects[0].cwd, model: "model", effort: "high", permissionMode: "auto", serviceTier: "default", turns: [], requests: [], historyComplete: true };
  return { threads: [], projects, selected: scene === "conversation" ? { id: view.id, title: view.title, cwd: view.cwd } : undefined,
    selectedId: scene === "conversation" ? view.id : undefined, selectedProject: projects[0], creating: scene === "create", draftProject: projects[0],
    draftScope: "synthetic-draft", defaultsReady: false, composerView: view, view: scene === "conversation" ? view : null,
    models: [{ id: "model", label: "Mock model", route: "Mock", efforts: ["high"], defaultEffort: "high" }], busy: false, loading: false,
    search: "", archived: false, error: "", draft: "合成首条消息", images: [], uncertain: false, catalogError: "", connectionError: "", connected: false,
    changeSearch() {}, changeArchived() {}, open() {}, startCreating() {}, refresh() {}, back() {}, changeDraft() {}, changeImages() {}, changeSettings() {}, reloadModels() {}, stop() {}, reconnect() {},
  };
}

test("实际 Workspace 三场景满足 v2 唯一header/footer/action合同，默认值未就绪时发送禁用", () => {
  for (const scene of ["list", "create", "conversation"]) {
    const c = controller(scene);
    if (scene === "conversation") { c.selected.cwd = ""; c.selectedProject = undefined; c.view.cwd = "/synthetic/codex-managed"; }
    const { Workspace } = createModuleGraph(new URL("../src/features/remote-workspace/Workspace.tsx", import.meta.url), {
      stubs: { ...stubs, react: React, "react/jsx-runtime": jsx, "./useRemoteWorkspaceController": { useRemoteWorkspaceController: () => c } },
    }).exports;
    const html = renderToStaticMarkup(React.createElement(Workspace, { active: true, onUnauthorized() {}, onPanel() {}, onLogout() {} }));
    assert.match(html, new RegExp(`data-codey-ui="2" data-codey-scene="${scene}"`));
    assert.equal((html.match(/data-codey-chrome="header"/g) || []).length, 1);
    assert.equal((html.match(/data-codey-chrome="footer"/g) || []).length, 1);
    assert.equal((html.match(/data-codey-action=/g) || []).length, scene === "list" ? 3 : 4);
    assert.equal((html.match(/data-codey-action="back"/g) || []).length, scene === "list" ? 0 : 1);
    assert.match(html, /data-codey-action="archive" aria-pressed="false"/);
    if (scene !== "list") assert.match(html, /aria-label="发送消息" disabled=""/);
    if (scene === "conversation") assert.match(html, /codex-managed/);
  }
});

test("新建页沿用 Composer 的草稿/附件/模型权限动作，只有显式发送调用 c.send", async () => {
  const calls = [], c = controller("create");
  c.send = async () => { calls.push("send"); return false; };
  c.changeDraft = value => calls.push(["draft", value]); c.changeImages = value => calls.push(["images", value]);
  c.changeSettings = value => calls.push(["settings", value]); c.startCreating = value => calls.push(["project", value]);
  const graph = createModuleGraph(new URL("../src/features/remote-workspace/Workspace.tsx", import.meta.url), {
    stubs: { ...stubs, "./useRemoteWorkspaceController": { useRemoteWorkspaceController: () => c } },
  });
  const tree = graph.exports.Workspace({ active: true });
  const surface = collectElements(tree, node => node.type.name === "NewSessionSurface")[0];
  const composer = collectElements(tree, node => node.type.name === "Composer")[0];
  assert.equal(composer.props.onGoal, undefined);
  assert.deepEqual(calls, []); assert.equal(surface.props.onBack, c.back);
  surface.props.onProject(projects[1]); composer.props.onDraft("保留草稿"); composer.props.onImages([]); composer.props.onSettings({ permissionMode: "read-only" });
  assert.deepEqual(calls, [["project", projects[1]], ["draft", "保留草稿"], ["images", []], ["settings", { permissionMode: "read-only" }]]);
  assert.equal(composer.props.connected, false);
  composer.props.onSend({ preventDefault() { calls.push("preventDefault"); } }); await Promise.resolve();
  assert.deepEqual(calls.slice(-2), ["preventDefault", "send"]);
});

test("目标条与输入区使用不同会话 key，不因实时列表重渲染积累重复目标条", () => {
  const c = controller("conversation"); c.draftScope = c.selectedId;
  const graph = createModuleGraph(new URL("../src/features/remote-workspace/Workspace.tsx", import.meta.url), { stubs: { ...stubs, "./useRemoteWorkspaceController": { useRemoteWorkspaceController: () => c } } });
  const tree = graph.exports.Workspace({ active: true });
  const [goal] = collectElements(tree, node => node.type.name === "GoalSurface");
  const [composer] = collectElements(tree, node => node.type.name === "Composer");
  assert.notEqual(goal.props.key, composer.props.key);
});

test("会话加号只打开受控目标面板，切换会话或进入新建页关闭，不发送或创建目标", () => {
  const calls = [], c = controller("conversation"); c.goal = null;
  c.readGoal = () => calls.push("read"); c.commandGoal = () => calls.push("command"); c.send = () => calls.push("send");
  const h = createControllerHarness(new URL("../src/features/remote-workspace/Workspace.tsx", import.meta.url), "Workspace", {
    ...stubs, "./useRemoteWorkspaceController": { useRemoteWorkspaceController: () => c },
  });
  const child = (tree, name) => collectElements(tree, node => node.type?.name === name)[0];
  try {
    let tree = h.render({ active: true });
    assert.equal(child(tree, "GoalSurface").props.open, false);
    child(tree, "Composer").props.onGoal(); tree = h.render();
    assert.equal(child(tree, "GoalSurface").props.open, true); assert.deepEqual(calls, []);
    child(tree, "GoalSurface").props.onOpenChange(false); tree = h.render();
    assert.equal(child(tree, "GoalSurface").props.open, false);
    child(tree, "Composer").props.onGoal(); tree = h.render();
    c.selectedId = "other"; c.draftScope = "other"; tree = h.render();
    assert.equal(child(tree, "GoalSurface").props.open, false);
    c.selectedId = "synthetic"; c.draftScope = "synthetic-draft"; tree = h.render();
    assert.equal(child(tree, "GoalSurface").props.open, false);
    child(tree, "Composer").props.onGoal(); tree = h.render();
    c.creating = true; tree = h.render();
    assert.equal(child(tree, "Composer").props.onGoal, undefined); assert.equal(child(tree, "GoalSurface"), undefined);
    c.creating = false; tree = h.render();
    assert.equal(child(tree, "GoalSurface").props.open, false); assert.deepEqual(calls, []);
  } finally { h.unmount(); }
});
