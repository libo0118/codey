import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import * as React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "@tabler/icons-react";
import * as nativeWeb from "react-native-web";
import { renderToStaticMarkup } from "react-dom/server";
import { collectElements, createModuleGraph, textContent } from "./helpers/jsx-tree.mjs";

const stubs = { "react-native-web": nativeWeb, "@tabler/icons-react": icons };
const entry = new URL("../src/remote/cindy/HomeSurface.tsx", import.meta.url);
const { HomeSurface } = createModuleGraph(entry, { stubs: { ...stubs, react: React, "react/jsx-runtime": jsx } }).exports;
const props = {
  projects: [{ id: "project", name: "Mock project", cwd: "/project" }], busy: false, loading: false,
  archived: false, error: "", showHeaderChrome: true,
  onArchived() {}, onOpen() {}, onCreate() {}, onRefresh() {}, onPanel() {}, onLogout() {},
};
const thread = (index, running, cwd = "/project") => ({ id: `12345678-1234-1234-1234-123456789ab${index}`, title: `Mock thread ${index}`, updatedAt: 1, cwd, ...(running === undefined ? {} : { running }) });
const render = extra => renderToStaticMarkup(React.createElement(HomeSurface, { ...props, ...extra }));

test("Cindy RNWeb 控件独立持有按钮样式，旧通用按钮规则不能覆盖浮动新建入口", async () => {
  const { HomeNewTaskButton } = createModuleGraph(new URL("../src/remote/cindy/HomeNewTaskButton.tsx", import.meta.url), { stubs }).exports;
  const floating = HomeNewTaskButton({ onPress() {}, disabled: false });
  assert.deepEqual(floating.props.dataSet, { cindyControl: "" });
  const graph = createModuleGraph(entry, { stubs });
  const tree = graph.exports.HomeSurface({ ...props, threads: [thread(0, true)] });
  for (const control of collectElements(tree, node => node.type === nativeWeb.Pressable)) assert.deepEqual(control.props.dataSet, { cindyControl: "" });
  const css = await readFile(new URL("../src/remote/styles.css", import.meta.url), "utf8");
  assert.ok(css.includes("button:where(:not([data-cindy-control]))"));
});

// Approved v2 contract replaces legacy Sidebar DOM assertions, not its actions.
test("Cindy 项目与最近行仅严格 true 呈现运行图标及优先占用时间槽的 spinner", () => {
  const threads = [thread(0, true), thread(1, false), thread(2, null), thread(3, undefined), thread(4, "true"), thread(5, true, "/other")];
  const html = render({ threads });
  assert.equal((html.match(/class="[^"]*cindy-running-icon/g) || []).length, 2);
  assert.equal((html.match(/aria-label="正在进行"/g) || []).length, 2);
  assert.equal((html.match(/class="[^"]*cindy-running-spinner/g) || []).length, 2);
  assert.equal((html.match(/<time /g) || []).length, 4);
  assert.doesNotMatch(html, /未读|待处理|已配对|本地电脑/);
});

test("紧凑行保留完整标题、非运行时间、复制深链菜单，头尾采用显式 v2 合同", () => {
  const title = "非常长的标题".repeat(30), item = { ...thread(0, false), title };
  const html = render({ threads: [item] });
  assert.ok(html.includes(`title="${title}"`)); assert.ok(html.includes(`aria-label="${title}的会话操作"`));
  assert.match(html, /<time dateTime="1970-01-01T00:00:00.001Z">/);
  assert.equal((html.match(/class="cindy-session-menu"/g) || []).length, 1);
  const graph = createModuleGraph(entry, { stubs });
  const tree = graph.exports.HomeSessionRow({ thread: item, selected: false, disabled: false, onOpen() {} });
  const [copy] = collectElements(tree, node => node.type.name === "CopyButton");
  assert.equal(copy.props.text, "codex://threads/12345678-1234-1234-1234-123456789ab0");
  const titleNode = collectElements(tree, node => node.type === nativeWeb.Text && node.props.children === title)[0];
  assert.equal(titleNode.props.numberOfLines, 1);
  assert.match(html, /复制会话链接/);
  assert.equal((html.match(/data-codey-chrome="header"/g) || []).length, 1);
  assert.equal((html.match(/data-codey-chrome="footer"/g) || []).length, 1);
  for (const action of ["archive", "console", "logout"]) assert.equal((html.match(new RegExp(`data-codey-action="${action}"`, "g")) || []).length, 1);
  assert.match(html, /data-codey-action="archive" aria-pressed="false"/);
});

test("Cindy 列表不提供搜索控件，折叠仅聚合已加载真实运行状态，空项目可新建", () => {
  const graph = createModuleGraph(entry, { stubs });
  let tree = graph.exports.HomeSurface({ ...props, threads: [thread(0, true)] });
  assert.equal(collectElements(tree, node => node.type === "input" || /搜索/.test(node.props["aria-label"] || "")).length, 0);
  assert.doesNotMatch(render({ threads: [thread(0, true)] }), /<input\b|搜索会话|清除搜索|cindy-search/);
  collectElements(tree, node => node.props["aria-label"] === "项目 Mock project")[0].props.onPress();
  graph.restart(); tree = graph.exports.HomeSurface({ ...props, threads: [thread(0, true)] });
  assert.equal(collectElements(tree, node => node.props["aria-expanded"] === false).length, 1);
  assert.equal(collectElements(tree, node => node.props["aria-label"] === "已加载会话中有任务正在进行").length, 1);
  assert.equal(collectElements(tree, node => node.props["aria-label"] === "当前已加载 1 个会话").length, 1);
  assert.equal(collectElements(tree, node => node.type.name === "HomeSessionRow").length, 0);
  const empty = render({ threads: [] });
  assert.match(empty, /此项目暂无会话/); assert.match(empty, /在 Mock project 中新建会话/);
});

test("归档、刷新、控制台、退出和新建保留真实事件及忙碌禁用态", () => {
  const calls = [], graph = createModuleGraph(entry, { stubs });
  const p = { ...props, threads: [], onArchived: value => calls.push(["archive", value]), onPanel: () => calls.push("console"), onLogout: () => calls.push("logout"), onCreate: value => calls.push(["create", value]), onRefresh: () => calls.push("refresh") };
  let tree = graph.exports.HomeSurface(p);
  for (const action of ["archive", "console", "logout"]) collectElements(tree, node => node.props["data-codey-action"] === action)[0].props.onClick();
  collectElements(tree, node => node.props["aria-label"] === "刷新会话")[0].props.onClick();
  collectElements(tree, node => node.props["aria-label"] === "在 Mock project 中新建会话")[0].props.onClick();
  assert.deepEqual(calls, [["archive", true], "console", "logout", "refresh", ["create", props.projects[0]]]);
  graph.restart(); tree = graph.exports.HomeSurface({ ...p, busy: true, error: "真实读取失败" });
  for (const node of collectElements(tree, node => node.props["data-codey-action"])) assert.equal(node.props.disabled, true);
  assert.match(textContent(tree), /真实读取失败/);
});

test("列表与项目请求透传取消 signal，保留原 POST 参数及 CSRF 传输边界", async () => {
  const calls = [];
  const { remoteWorkspaceRequests: requests } = createModuleGraph(new URL("../src/features/remote-workspace/requests.ts", import.meta.url), {
    stubs: { "../../remote/transport": { remoteRequest: (...args) => { calls.push(args); return Promise.resolve([]); } } },
  }).exports;
  const controller = new AbortController();
  await requests.threads("Mock query", true, controller.signal); await requests.projects(controller.signal);
  assert.deepEqual(calls, [["/remote/threads", { search: "Mock query", archived: true }, controller.signal], ["/remote/projects", {}, controller.signal]]);
});
