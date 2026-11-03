// Adapted from fraternity-z/CodeyX (30e1daa), AGPL-3.0-only.
// 由 Rust ignored 集成测试传入已校验的官方运行包；仅操作测试临时目录。
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { setTimeout: delay } = require("node:timers/promises");
const root = process.argv[2];
const project = path.join(path.dirname(root), "中文 automatic project");
const other = path.join(path.dirname(root), "second project");
const codexHome = path.join(path.dirname(root), "codex-home");
const clients = [];

function connect(cwd) {
  const child = spawn(process.execPath, ["--liftoff-only", "--disable-warning=ExperimentalWarning", path.join(root, "auto-mcp-v1.cjs"), path.join(root, "v1.6.2/lib/dist/bin/codegraph.js")], {
    cwd, windowsHide: true, env: { ...process.env, CODEX_HOME: codexHome, CODEGRAPH_DAEMON_IDLE_TIMEOUT_MS: "300" }, stdio: ["pipe", "pipe", "pipe"],
  });
  clients.push(child);
  child.stderr.resume();
  child.stdin.on("error", () => {});
  const messages = new Map();
  let buffer = "";
  let id = 0;
  child.stdout.on("data", chunk => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const message = JSON.parse(buffer.slice(0, end));
      buffer = buffer.slice(end + 1);
      messages.set(message.id, message);
    }
  });
  return async (method, params = {}, timeout = 30_000) => {
    const requestId = ++id;
    child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: requestId, method, params }) + "\n");
    const deadline = Date.now() + timeout;
    while (!messages.has(requestId) && Date.now() < deadline) {
      assert.equal(child.exitCode, null, "MCP exited prematurely");
      await delay(25);
    }
    assert.ok(messages.has(requestId), `Timed out: ${method}`);
    const message = messages.get(requestId);
    messages.delete(requestId);
    assert.equal(message.error, undefined, JSON.stringify(message.error));
    return message.result;
  };
}

async function ready(project) {
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    const directory = path.join(root, "projects");
    const records = fs.existsSync(directory) ? fs.readdirSync(directory).filter(file => file.endsWith(".json")).map(file => JSON.parse(fs.readFileSync(path.join(directory, file)))) : [];
    const record = records.find(record => record.projectPath === fs.realpathSync(project));
    assert.notEqual(record?.phase, "failed", record?.message);
    if (record?.phase === "watching") return;
    await delay(100);
  }
  throw new Error("Automatic indexing did not finish");
}

(async () => {
  try {
    for (const directory of [project, other]) fs.mkdirSync(directory);
    fs.mkdirSync(codexHome);
    fs.writeFileSync(path.join(codexHome, ".codex-global-state.json"), JSON.stringify({ "local-projects": { p: { rootPaths: [project] }, q: { rootPaths: [other] } } }));
    fs.writeFileSync(path.join(project, "main.ts"), "export function greet(name: string) { return name; }\nexport function main() { return greet('world'); }\n");
    fs.writeFileSync(path.join(other, "other.ts"), "export function otherProjectOnly() { return 42; }\n");
    const first = connect(project);
    const second = connect(project);
    const third = connect(other);
    const init = { protocolVersion: "2024-11-05", capabilities: {}, clientInfo: { name: "CodeyX-smoke", version: "1" } };
    const started = Date.now();
    await Promise.all([first("initialize", init), second("initialize", init), third("initialize", init)]);
    assert.ok(Date.now() - started < 15_000, "MCP handshake must not wait for indexing");
    assert.ok((await first("tools/list")).tools.some(tool => tool.name === "codegraph_explore"));
    await Promise.all([ready(project), ready(other)]);
    const query = (client, query) => client("tools/call", { name: "codegraph_explore", arguments: { query } });
    assert.match(JSON.stringify(await query(first, "greet")), /greet/);
    assert.match(JSON.stringify(await query(third, "otherProjectOnly")), /otherProjectOnly/);
    fs.writeFileSync(path.join(project, "main.ts"), "export function newlyAddedSymbol() { return 7; }\nexport function main() { return newlyAddedSymbol(); }\n");
    let updated = false;
    for (let attempt = 0; attempt < 30; attempt++) {
      await delay(500);
      const result = await query(second, "newlyAddedSymbol");
      if (!result.isError && JSON.stringify(result).includes("function newlyAddedSymbol")) { updated = true; break; }
    }
    assert.ok(updated, "Official watcher must index edits without a manual sync command");
    assert.equal(fs.existsSync(path.join(path.dirname(root), ".codegraph")), false);
    console.log("PASS: automatic initialization, concurrent chats, project isolation and live file synchronization");
  } finally {
    for (const child of clients) child.stdin.end();
    await delay(3000);
    for (const child of clients) if (child.exitCode === null) child.kill();
  }
})().catch(error => { console.error(error.message); process.exitCode = 1; });
