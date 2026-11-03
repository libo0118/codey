// Adapted from fraternity-z/CodeyX (30e1daa), AGPL-3.0-only.
import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { PassThrough } from "node:stream";
import { setTimeout as delay } from "node:timers/promises";
import launcher from "../backend/src/commands/builtin_tools/auto_mcp.cjs";

const sessions = new Map();

function fixture(t) {
  const temporary = fs.realpathSync(os.tmpdir());
  const root = fs.mkdtempSync(path.join(temporary, "codey-codegraph-test-"));
  sessions.set(root, []);
  t.after(async () => {
    for (const close of sessions.get(root)) close();
    await delay(250);
    sessions.delete(root);
    assert.equal(path.dirname(fs.realpathSync(root)), temporary);
    assert.match(path.basename(root), /^codey-codegraph-test-/);
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });
  const project = path.join(root, "中文 project with spaces");
  fs.mkdirSync(project);
  fs.writeFileSync(path.join(project, "source.ts"), "export function original() {}\n");
  const home = path.join(root, "home");
  const codexHome = path.join(home, ".codex");
  fs.mkdirSync(codexHome, { recursive: true });
  const statePath = path.join(codexHome, ".codex-global-state.json");
  const saveProjects = roots => fs.writeFileSync(statePath, JSON.stringify({ "local-projects": { saved: { rootPaths: roots } } }));
  saveProjects([project]);
  return { root, project, home, codexHome, statePath, saveProjects };
}

async function until(check) {
  for (let attempt = 0; attempt < 200; attempt++) {
    const value = check();
    if (value) return value;
    await delay(25);
  }
  throw new Error("Timed out waiting for automatic project indexing");
}

function session(t, root, project) {
  const input = new PassThrough();
  const output = new PassThrough();
  const messages = [];
  let buffer = "";
  output.on("data", chunk => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf("\n")) >= 0) {
      messages.push(JSON.parse(buffer.slice(0, end)));
      buffer = buffer.slice(end + 1);
    }
  });
  const server = launcher.start({ entry: fileURLToPath(new URL("./fixtures/codegraph-runtime.cjs", import.meta.url)),
    root, cwd: project, home: path.join(root, "home"), codexHome: path.join(root, "home", ".codex"),
    input, output, tools: [{ name: "codegraph_explore", inputSchema: { type: "object" } }] });
  sessions.get(root).push(() => { server.stop(); input.destroy(); output.destroy(); });
  const send = (id, method, params = {}) => input.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  const response = id => until(() => messages.find(message => message.id === id));
  return { send, response, messages, stop: server.stop };
}

function records(root) {
  const directory = path.join(root, "projects");
  return fs.existsSync(directory) ? fs.readdirSync(directory).filter(file => file.endsWith(".json")).map(file => JSON.parse(fs.readFileSync(path.join(directory, file)))) : [];
}

test("WSL workspace conversion preserves Linux paths and rejects failed conversions", () => {
  for (const root of ["C:/", "C:\\", "C:/project/..", "C:\\."]) assert.equal(launcher.mapWindowsPath(root, () => { throw new Error("must not map drive roots"); }), null);
  for (const original of ["/home/user/project", "relative", null]) {
    assert.equal(launcher.mapWindowsPath(original, () => { throw new Error("must not convert"); }), original);
  }
  for (const original of ["C:\\Project with spaces", "d:/project", "\\\\wsl.localhost\\Ubuntu\\home\\user"]) {
    assert.equal(launcher.mapWindowsPath(original, value => {
      assert.equal(value, original);
      return { status: 0, stdout: "/mapped/project\n" };
    }), "/mapped/project");
    for (const result of [{ status: 1, stdout: "/bad" }, { status: 0, stdout: "relative" }, { error: new Error("timeout") }]) {
      assert.equal(launcher.mapWindowsPath(original, () => result), null);
    }
  }
});

test("project selection only accepts saved workspace roots and resolves nested directories", t => {
  const { root, project, home, codexHome, saveProjects } = fixture(t);
  const nested = path.join(project, "nested");
  const sibling = `${project}-other`;
  fs.mkdirSync(nested);
  fs.mkdirSync(sibling);
  const select = candidate => launcher.projectPath(candidate, home, codexHome);
  assert.equal(select(project), fs.realpathSync(project));
  assert.equal(select(nested), fs.realpathSync(project));
  assert.equal(select(sibling), null);
  for (const candidate of ["relative", path.join(root, "missing"), path.join(project, "source.ts")]) assert.throws(() => select(candidate), /目录无效/);
  saveProjects([path.parse(root).root, home, codexHome, project, nested, null, "relative", path.join(root, "missing")]);
  for (const candidate of [path.parse(root).root, home, codexHome]) assert.equal(select(candidate), null);
  assert.equal(select(nested), fs.realpathSync(nested));
  if (process.platform === "win32") assert.equal(select(project.toUpperCase()), fs.realpathSync(project));
});

test("custom Codex home and directory aliases use the same saved project scope", t => {
  const { root, project, home, codexHome, statePath } = fixture(t);
  const customHome = path.join(root, "custom codex");
  const alias = path.join(root, "project alias");
  fs.mkdirSync(customHome);
  fs.symlinkSync(project, alias, process.platform === "win32" ? "junction" : "dir");
  fs.writeFileSync(path.join(customHome, ".codex-global-state.json"), JSON.stringify({ "local-projects": { p: { rootPaths: [alias, customHome, codexHome] } } }));
  fs.unlinkSync(statePath);
  assert.equal(launcher.projectPath(project, home, codexHome), null);
  assert.equal(launcher.projectPath(alias, home, customHome), fs.realpathSync(project));
  assert.equal(launcher.projectPath(customHome, home, customHome), null);
  assert.equal(launcher.projectPath(codexHome, home, customHome), null);
});

test("legacy workspace roots are used only when the current project list is absent", t => {
  const { project, home, codexHome, statePath } = fixture(t);
  fs.writeFileSync(statePath, JSON.stringify({ "electron-saved-workspace-roots": [project] }));
  assert.equal(launcher.projectPath(project, home, codexHome), fs.realpathSync(project));
  fs.writeFileSync(statePath, JSON.stringify({ "local-projects": {}, "electron-saved-workspace-roots": [project] }));
  assert.equal(launcher.projectPath(project, home, codexHome), null);
});

test("missing, malformed and oversized workspace state cannot authorize indexing", t => {
  const { project, home, codexHome, statePath } = fixture(t);
  fs.unlinkSync(statePath);
  assert.equal(launcher.projectPath(project, home, codexHome), null);
  for (const state of ["{private-state", "null", "[]", '{"local-projects":null}', '{"local-projects":[]}', '{"electron-saved-workspace-roots":{}}', '{"electron-saved-workspace-roots":null}', " ".repeat(16 * 1024 * 1024 + 1)]) {
    fs.writeFileSync(statePath, state);
    assert.throws(() => launcher.projectPath(project, home, codexHome), error => /工作区项目清单/.test(error.message) && !error.message.includes("private-state"));
  }
});

test("ordinary chats and Codex directories expose no tools, preserve old indexes and create no failure record", async t => {
  const { root, home, codexHome } = fixture(t);
  const documents = path.join(home, "Documents", "Codex", "2026-10-17", "chat");
  const installation = path.join(home, "AppData", "Local", "Programs", "Codex");
  for (const candidate of [documents, installation, codexHome]) {
    fs.mkdirSync(candidate, { recursive: true });
    if (candidate === documents) {
      fs.mkdirSync(path.join(candidate, ".codegraph"));
      fs.writeFileSync(path.join(candidate, ".codegraph", "codegraph.db"), "existing index");
    }
    const client = session(t, root, candidate);
    client.send(1, "initialize");
    client.send(2, "tools/list");
    client.send(3, "tools/call", { name: "codegraph_explore" });
    assert.deepEqual((await client.response(2)).result.tools, []);
    assert.match((await client.response(3)).result.content[0].text, /仅在工作区已添加/);
    assert.equal(fs.existsSync(path.join(candidate, ".codegraph")), candidate === documents);
    assert.equal(fs.existsSync(path.join(candidate, "calls.jsonl")), false);
  }
  assert.equal(fs.readFileSync(path.join(documents, ".codegraph", "codegraph.db"), "utf8"), "existing index");
  assert.deepEqual(records(root), []);
});

test("an unreadable workspace state keeps MCP responsive without starting a runtime", async t => {
  const { root, project, statePath } = fixture(t);
  fs.writeFileSync(statePath, "{private-state");
  const client = session(t, root, project);
  client.send(1, "initialize");
  client.send(2, "tools/list");
  client.send(3, "tools/call", { name: "codegraph_explore" });
  assert.deepEqual((await client.response(2)).result.tools, []);
  const result = (await client.response(3)).result;
  assert.match(result.content[0].text, /已暂停自动索引/);
  assert.doesNotMatch(JSON.stringify(result), /private-state/);
  assert.equal(fs.existsSync(path.join(project, ".codegraph")), false);
  assert.deepEqual(records(root), []);
});

test("managed environment does not inherit routing or watcher overrides", () => {
  const env = launcher.cleanEnv({ KEEP: "yes", NODE_OPTIONS: "private", NODE_PATH: "custom", CODEGRAPH_DIR: "other", CODEGRAPH_NO_WATCH: "1", CODEGRAPH_DAEMON_INTERNAL: "1" });
  assert.equal(env.KEEP, "yes");
  assert.equal(env.CODEGRAPH_TELEMETRY, "0");
  for (const key of ["NODE_OPTIONS", "NODE_PATH", "CODEGRAPH_DIR", "CODEGRAPH_NO_WATCH", "CODEGRAPH_DAEMON_INTERNAL"]) assert.equal(env[key], undefined);
});

test("missing, partial, failed and outdated indexes require preparation", () => {
  assert.equal(launcher.needsIndex({ initialized: false }), true);
  for (const state of ["partial", "failed", "indexing"]) assert.equal(launcher.needsIndex({ initialized: true, index: { state } }), true);
  assert.equal(launcher.needsIndex({ initialized: true, index: { reindexRecommended: true } }), true);
  assert.equal(launcher.needsIndex({ initialized: true, index: { state: "complete" } }), false);
});

test("handshake and tool discovery stay responsive during automatic initialization", async t => {
  const { root, project } = fixture(t);
  const client = session(t, root, project);
  client.send(1, "initialize");
  client.send(2, "tools/list");
  client.send(3, "tools/call", { name: "codegraph_explore", arguments: { query: "original" } });
  assert.equal(client.messages.find(message => message.id === 1).result.serverInfo.name, "codey-codegraph");
  assert.equal((await client.response(2)).result.tools[0].name, "codegraph_explore");
  assert.match((await client.response(3)).result.content[0].text, /后台/);
  await until(() => records(root).some(record => record.phase === "watching"));
  client.send(4, "tools/call", { name: "codegraph_explore", arguments: { query: "original" } });
  assert.equal((await client.response(4)).result.project, fs.realpathSync(project));
  assert.match((await client.response(4)).result.content[0].text, /original/);
  const calls = fs.readFileSync(path.join(project, "calls.jsonl"), "utf8").trim().split("\n").map(JSON.parse);
  assert.equal(calls.filter(call => call.action === "init").length, 1);
  assert.deepEqual(calls.find(call => call.action === "serve").args, ["--mcp", "--path", fs.realpathSync(project)]);
  assert.equal(fs.existsSync(path.join(root, ".codegraph")), false);
});

test("existing project launches retain the index and use the official watcher", async t => {
  const { root, project } = fixture(t);
  fs.mkdirSync(path.join(project, ".codegraph"));
  fs.writeFileSync(path.join(project, ".codegraph", "codegraph.db"), "existing");
  const first = session(t, root, project);
  const second = session(t, root, project);
  first.send(1, "initialize");
  second.send(1, "initialize");
  await until(() => records(root).some(record => record.phase === "watching"));
  await delay(150);
  const calls = fs.readFileSync(path.join(project, "calls.jsonl"), "utf8").trim().split("\n").map(JSON.parse);
  assert.equal(calls.some(call => ["init", "index", "sync"].includes(call.action)), false);
  assert.equal(fs.readFileSync(path.join(project, ".codegraph", "codegraph.db"), "utf8"), "existing");
});

test("explicit tool paths cannot escape saved projects", async t => {
  const { root, project } = fixture(t);
  const nested = path.join(project, "nested");
  fs.mkdirSync(nested);
  const client = session(t, root, nested);
  client.send(1, "initialize");
  await until(() => records(root).some(record => record.phase === "watching"));
  client.send(2, "tools/call", { name: "codegraph_explore", arguments: { projectPath: root } });
  assert.equal((await client.response(2)).result.isError, true);
  client.send(3, "tools/call", { name: "codegraph_explore", arguments: { projectPath: nested } });
  assert.equal((await client.response(3)).result.project, fs.realpathSync(project));
  assert.equal(fs.existsSync(path.join(nested, ".codegraph")), false);
});

test("two new chats initialize a project only once", async t => {
  const { root, project } = fixture(t);
  const first = session(t, root, project);
  const second = session(t, root, project);
  first.send(1, "initialize");
  second.send(1, "initialize");
  const calls = () => fs.existsSync(path.join(project, "calls.jsonl")) ? fs.readFileSync(path.join(project, "calls.jsonl"), "utf8").trim().split("\n").map(JSON.parse) : [];
  await until(() => calls().filter(call => call.action === "serve").length === 2);
  assert.equal(calls().filter(call => call.action === "init").length, 1);
  assert.equal(fs.existsSync(path.join(project, ".codegraph", "codey-bootstrap.lock")), false);
});

test("index failure is bounded, visible and redacts child output", async t => {
  const { root, project } = fixture(t);
  fs.writeFileSync(path.join(project, "fail"), "");
  const client = session(t, root, project);
  client.send(1, "initialize");
  await until(() => records(root).some(record => record.phase === "failed"));
  client.send(2, "tools/call", { name: "codegraph_explore" });
  const reply = await client.response(2);
  assert.equal(reply.result.isError, true);
  assert.doesNotMatch(JSON.stringify([reply, records(root)]), /private-source-and-token/);
  assert.match(reply.result.content[0].text, /自动重试/);
});

test("active writer is respected and cancellation stops preparation", async t => {
  const { project } = fixture(t);
  fs.mkdirSync(path.join(project, ".codegraph"));
  fs.writeFileSync(path.join(project, ".codegraph", "codegraph.lock"), String(process.pid));
  assert.equal(launcher.activeWriter(project), true);
  const controller = new AbortController();
  const work = launcher.prepareProject({ project, signal: controller.signal, record() {}, run() { throw new Error("must not index while another writer is active"); } });
  controller.abort();
  await assert.rejects(work, { name: "AbortError" });
  assert.equal(fs.readFileSync(path.join(project, ".codegraph", "codegraph.lock"), "utf8"), String(process.pid));
});

test("linked index directories are preserved instead of indexing outside the chat project", async t => {
  const { root, project } = fixture(t);
  const external = path.join(root, "existing-index");
  fs.mkdirSync(external);
  fs.writeFileSync(path.join(external, "keep"), "user data");
  fs.symlinkSync(external, path.join(project, ".codegraph"), process.platform === "win32" ? "junction" : "dir");
  await assert.rejects(launcher.acquireBootstrap(project, new AbortController().signal), /外部链接/);
  assert.deepEqual(fs.readdirSync(external), ["keep"]);
});

test("separate project chats remain isolated when one initialization fails", async t => {
  const { root, project, saveProjects } = fixture(t);
  const other = path.join(root, "another project");
  fs.mkdirSync(other);
  fs.writeFileSync(path.join(other, "source.ts"), "export const separateProject = true;");
  saveProjects([project, other]);
  fs.writeFileSync(path.join(project, "fail"), "");
  const first = session(t, root, project);
  const second = session(t, root, other);
  first.send(1, "initialize");
  second.send(1, "initialize");
  await until(() => records(root).some(record => record.phase === "failed") && records(root).some(record => record.phase === "watching"));
  second.send(2, "tools/call", { name: "codegraph_explore" });
  assert.match((await second.response(2)).result.content[0].text, /separateProject/);
  assert.equal(fs.existsSync(path.join(root, ".codegraph")), false);
});
