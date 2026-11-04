import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "@tabler/icons-react";
import { renderToStaticMarkup } from "react-dom/server";
import { collectElements, createModuleGraph } from "./helpers/jsx-tree.mjs";

const { Sidebar } = createModuleGraph(new URL("../src/remote/sidebar.tsx", import.meta.url), {
  stubs: { react: React, "react/jsx-runtime": jsx, "@tabler/icons-react": icons },
}).exports;
const props = {
  projects: [{ id: "project", name: "Mock project", cwd: "/project" }], busy: false, loading: false,
  search: "", archived: false, error: "", onSearch() {}, onArchived() {}, onOpen() {}, onCreate() {}, onRefresh() {}, onPanel() {}, onLogout() {},
};
const thread = (index, running, cwd = "/project") => ({ id: `12345678-1234-1234-1234-123456789ab${index}`, title: `Mock thread ${index}`, updatedAt: 1, cwd, ...(running === undefined ? {} : { running }) });

test("真实 Sidebar 的项目与最近行仅严格 true 呈现标题前的可访问绿点", () => {
  const threads = [thread(0, true), thread(1, false), thread(2, null), thread(3, undefined), thread(4, "true"), thread(5, true, "/other")];
  const html = renderToStaticMarkup(React.createElement(Sidebar, { ...props, threads }));
  assert.equal((html.match(/class="remote-thread-running"/g) || []).length, 2);
  assert.equal((html.match(/role="img" aria-label="正在进行" title="正在进行"/g) || []).length, 2);
  assert.match(html, /remote-thread-running[^>]*><\/span>Mock thread 0<\/span><time/);
  assert.match(html, /remote-thread-running[^>]*><\/span>Mock thread 5<\/span><time/);
  for (const index of [1, 2, 3, 4]) assert.match(html, new RegExp(`<span>Mock thread ${index}</span><time`));
});

test("绿点不增设操作列，保留完整标题、时间、复制深链菜单与 Harmony 识别的原头尾动作", () => {
  const title = "非常长的标题".repeat(30), item = { ...thread(0, true), title };
  const html = renderToStaticMarkup(React.createElement(Sidebar, { ...props, threads: [item] }));
  assert.ok(html.includes(`title="${title}"`)); assert.ok(html.includes(`aria-label="${title}的会话操作"`));
  assert.match(html, /<time dateTime="1970-01-01T00:00:00.001Z">/);
  assert.equal((html.match(/class="remote-thread-menu"/g) || []).length, 1);
  const tree = createModuleGraph(new URL("../src/remote/sidebar.tsx", import.meta.url), { stubs: { "@tabler/icons-react": icons } }).exports.Sidebar({ ...props, threads: [item] });
  const [copy] = collectElements(tree, node => node.type.name === "CopyButton");
  assert.equal(copy.props.text, "codex://threads/12345678-1234-1234-1234-123456789ab0");
  assert.match(html, /复制会话链接/);
  const heading = html.match(/<div class="remote-list-heading">(.*?)<\/div>/)[1];
  assert.equal((heading.match(/<button /g) || []).length, 2); assert.match(heading, /<h1>Codex<\/h1>/);
  const footer = html.match(/<footer class="remote-sidebar-footer">(.*?)<\/footer>/)[1];
  assert.equal((footer.match(/<button\b/g) || []).length, 3);
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
