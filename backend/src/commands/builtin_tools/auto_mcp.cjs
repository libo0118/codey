// Adapted from fraternity-z/CodeyX (30e1daa), AGPL-3.0-only.
// MCP 启动钩子：仅接入 Codex 工作区已添加的本地项目，cwd 只用于匹配项目根目录。
// 握手与建库分离；查询就绪后转发到官方 MCP，持续同步由其共享 daemon 负责。
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const { createHash, randomUUID } = require("node:crypto");
const { spawn, spawnSync } = require("node:child_process");
const { setTimeout: delay } = require("node:timers/promises");

const MAX_LINE = 8 * 1024 * 1024;
const FLAGS = ["--liftoff-only", "--disable-warning=ExperimentalWarning"];
class SetupError extends Error {}

function directory(candidate) {
  if (typeof candidate !== "string" || !path.isAbsolute(candidate)) return null;
  try { return fs.statSync(candidate).isDirectory() ? fs.realpathSync(candidate) : null; } catch { return null; }
}

function mapWindowsPath(candidate, convert = value => spawnSync("wslpath", ["-u", value], { encoding: "utf8", timeout: 5000 })) {
  if (typeof candidate !== "string" || !/^(?:[a-z]:[/\\]|\\\\)/i.test(candidate)) return candidate;
  const normalized = path.win32.normalize(candidate);
  if (path.win32.parse(normalized).root === normalized) return null;
  const result = convert(candidate);
  const mapped = result.stdout?.trimEnd();
  return result.status === 0 && mapped?.startsWith("/") ? mapped : null;
}

function workspaceRoots(home, codexHome) {
  let state;
  try {
    const location = path.join(codexHome, ".codex-global-state.json");
    const metadata = fs.lstatSync(location);
    if (!metadata.isFile() || metadata.size > 16 * 1024 * 1024) throw new Error();
    const bytes = fs.readFileSync(location);
    if (bytes.length > 16 * 1024 * 1024) throw new Error();
    state = JSON.parse(bytes.toString("utf8"));
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw new SetupError("无法读取 Codex 工作区项目清单，已暂停自动索引");
  }
  if (!state || typeof state !== "object" || Array.isArray(state)) throw new SetupError("Codex 工作区项目清单格式无效，已暂停自动索引");
  let candidates;
  // 新版项目清单优先，不能用旧缓存重新纳入已移除的项目。
  if (Object.hasOwn(state, "local-projects")) {
    const projects = state["local-projects"];
    if (!projects || typeof projects !== "object" || Array.isArray(projects)) throw new SetupError("Codex 工作区项目清单格式无效，已暂停自动索引");
    candidates = Object.values(projects).flatMap(project => Array.isArray(project?.rootPaths) ? project.rootPaths : []);
  } else {
    candidates = Object.hasOwn(state, "electron-saved-workspace-roots") ? state["electron-saved-workspace-roots"] : [];
    if (!Array.isArray(candidates)) throw new SetupError("Codex 工作区项目清单格式无效，已暂停自动索引");
  }
  if (process.platform === "linux" && process.env.CODEY_WSL_HELPER) candidates = candidates.map(candidate => mapWindowsPath(candidate));
  const excluded = [home, path.join(home, ".codex"), codexHome].map(directory).filter(Boolean);
  return [...new Set(candidates.map(directory).filter(candidate => candidate
    && candidate !== path.parse(candidate).root && !excluded.some(root => path.relative(root, candidate) === "")))];
}

function projectPath(cwd, home = os.homedir(), codexHome = process.env.CODEX_HOME || path.join(home, ".codex")) {
  const current = directory(cwd);
  if (!current) throw new SetupError("聊天项目目录无效");
  return workspaceRoots(home, codexHome).filter(root => {
    const relative = path.relative(root, current);
    return relative === "" || (relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative));
  }).sort((a, b) => b.length - a.length)[0] ?? null;
}

function needsIndex(status) {
  return status.initialized !== true || status.index?.reindexRecommended === true
    || ["failed", "partial", "indexing"].includes(status.index?.state);
}

function cleanEnv(env) {
  const next = { ...env, CODEGRAPH_TELEMETRY: "0", DO_NOT_TRACK: "1", NO_COLOR: "1" };
  for (const key of ["NODE_OPTIONS", "NODE_PATH", "CODEGRAPH_DIR", "CODEGRAPH_NO_WATCH", "CODEGRAPH_NO_DAEMON", "CODEGRAPH_DAEMON_INTERNAL", "CODEGRAPH_DAEMON_HANDOVER"]) delete next[key];
  return next;
}

function writeRecord(root, project, phase, message) {
  const directory = path.join(root, "projects");
  fs.mkdirSync(directory, { recursive: true });
  const key = createHash("sha256").update(process.platform === "win32" ? project.toLowerCase() : project).digest("hex");
  const destination = path.join(directory, `${key}.json`);
  const temporary = `${destination}.${randomUUID()}.tmp`;
  try {
    fs.writeFileSync(temporary, JSON.stringify({ projectPath: project, phase, message, updatedAt: Date.now() }), { flag: "wx", mode: 0o600 });
    fs.renameSync(temporary, destination);
  } finally {
    try { fs.unlinkSync(temporary); } catch {}
  }
}

function activeWriter(project) {
  return ["codegraph.lock", "writer.pid", "daemon.pid"].some(file => {
    try {
      const location = path.join(project, ".codegraph", file);
      if (fs.statSync(location).size > 4096) return true;
      const text = fs.readFileSync(location, "utf8").trim();
      let pid;
      try { const value = JSON.parse(text); pid = typeof value === "number" ? value : value.pid; } catch { return true; }
      if (!Number.isSafeInteger(pid) || pid <= 0) return true;
      try { process.kill(pid, 0); return true; } catch (error) { return error.code !== "ESRCH"; }
    } catch (error) { return error.code !== "ENOENT"; }
  });
}

// 仅协调 Codey 的初始化者；官方数据库锁和 daemon 锁保持原样。
async function acquireBootstrap(project, signal) {
  const directory = path.join(project, ".codegraph");
  for (const location of [directory, path.join(directory, "codegraph.db"), path.join(directory, "codey-bootstrap.lock")]) {
    try {
      if (fs.lstatSync(location).isSymbolicLink()) throw new SetupError("项目索引指向外部链接，已保留现有配置并停止自动建库");
    } catch (error) { if (error.code !== "ENOENT") throw error; }
  }
  fs.mkdirSync(directory, { recursive: true });
  const location = path.join(directory, "codey-bootstrap.lock");
  const owner = JSON.stringify({ pid: process.pid, token: randomUUID() });
  const deadline = Date.now() + 1_800_000;
  while (Date.now() < deadline) {
    signal.throwIfAborted();
    try {
      fs.writeFileSync(location, owner, { flag: "wx", mode: 0o600 });
      return () => { try { if (fs.readFileSync(location, "utf8") === owner) fs.unlinkSync(location); } catch {} };
    } catch (error) {
      if (error.code !== "EEXIST") throw error;
      try {
        if (fs.statSync(location).size > 4096) throw new SetupError("项目自动索引锁格式无效");
        const existing = fs.readFileSync(location, "utf8");
        const { pid } = JSON.parse(existing);
        if (Number.isSafeInteger(pid) && pid > 0) {
          try { process.kill(pid, 0); } catch (error) {
            if (error.code === "ESRCH" && fs.readFileSync(location, "utf8") === existing) fs.unlinkSync(location);
          }
        }
      } catch (error) {
        if (error instanceof SetupError) throw error;
        // 正在写入锁标识时只等待，不能删除另一个会话的锁。
      }
      await delay(250, undefined, { signal });
    }
  }
  throw new SetupError("另一个聊天仍在准备项目索引，下次进入项目时自动重试");
}

async function prepareProject(options) {
  const release = await acquireBootstrap(options.project, options.signal);
  try { await prepareLocked(options); } finally { release(); }
}

// 尊重官方跨进程写入锁；竞争者只等待，不清锁、不停止其他聊天的 daemon。
async function prepareLocked({ project, run, signal, record }) {
  record("indexing", "正在后台检查并建立项目索引…");
  const deadline = Date.now() + 1_800_000;
  while (Date.now() < deadline) {
    signal.throwIfAborted();
    let status = fs.existsSync(path.join(project, ".codegraph", "codegraph.db"))
      ? await run("status") : { initialized: false };
    if (!needsIndex(status)) return;
    if (activeWriter(project)) {
      // 已有运行实例负责恢复和同步；不停止其他聊天的 daemon。
      if (status.initialized && status.index?.state !== "indexing") return;
      await delay(500, undefined, { signal });
      continue;
    }
    try {
      await run(status.initialized ? "index" : "init");
      status = await run("status");
      if (status.initialized && !needsIndex(status)) return;
    } catch (error) {
      if (signal.aborted || !activeWriter(project)) throw error;
      // 另一个会话可能先拿到初始化锁；下轮重新检查实际索引。
    }
    await delay(500, undefined, { signal });
  }
  throw new SetupError("项目索引暂时未就绪，下次项目聊天会自动重试");
}

function lines(stream, consume, fail) {
  let buffer = "";
  stream.setEncoding("utf8");
  stream.on("data", chunk => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, end);
      buffer = buffer.slice(end + 1);
      if (line.length > MAX_LINE) { fail(); return; }
      if (line.trim()) {
        try { consume(JSON.parse(line)); } catch { fail(); return; }
      }
    }
    if (buffer.length > MAX_LINE) fail();
  });
}

function start({ entry, root, cwd = process.cwd(), home = os.homedir(), codexHome = process.env.CODEX_HOME || path.join(home, ".codex"), input = process.stdin, output = process.stdout, tools, protocolVersion = "2024-11-05" }) {
  const controller = new AbortController();
  const children = new Set();
  const pending = new Set();
  let backend = null;
  let ready = false;
  let initialized = false;
  let stopped = false;
  let project;
  let message = "CodeGraph 正在后台建立当前项目的索引。请先使用其他文件工具，稍后可直接再次查询，无需执行初始化命令。";
  const send = value => { if (!stopped && !output.destroyed) output.write(`${JSON.stringify(value)}\n`); };
  const result = (id, value) => send({ jsonrpc: "2.0", id, result: value });
  const failure = (id, text) => result(id, { isError: true, content: [{ type: "text", text }] });
  const record = (phase, text) => { if (project) { try { writeRecord(root, project, phase, text); } catch {} } };
  const fail = error => {
    ready = false;
    message = `${error instanceof SetupError ? error.message : "CodeGraph 无法接入项目，请检查目录权限和运行包是否可用"}；下次项目聊天会自动重试。`;
    record("failed", message);
    for (const id of pending) failure(id, message);
    pending.clear();
  };
  const stop = () => {
    if (stopped) return;
    stopped = true;
    controller.abort();
    for (const child of children) child.kill();
    input.destroy();
    clearInterval(watchParent);
    clearTimeout(initialTimeout);
  };
  const launch = args => {
    const child = spawn(process.execPath, [...FLAGS, entry, ...args], {
      cwd: project, env: cleanEnv(process.env), windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
    });
    children.add(child);
    child.once("close", () => children.delete(child));
    // 官方输出可能含源代码或配置，不转发 stderr。
    child.stderr.resume();
    child.stdin.on("error", () => {});
    return child;
  };
  const run = action => new Promise((resolve, reject) => {
    const args = [action, project];
    if (action === "status") args.push("--json");
    if (action === "init") args.push("--yes");
    const child = launch(args);
    child.stdin.end();
    let output = "";
    let timedOut = false;
    const timer = setTimeout(() => { timedOut = true; child.kill(); }, action === "status" ? 30_000 : 1_800_000);
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", chunk => { if (action === "status") output = (output + chunk).slice(-128 * 1024); });
    child.once("error", () => { clearTimeout(timer); reject(new SetupError("无法启动 CodeGraph 运行包")); });
    child.once("close", code => {
      clearTimeout(timer);
      if (code !== 0 || timedOut || controller.signal.aborted) { reject(new SetupError(timedOut ? "CodeGraph 索引超时" : `CodeGraph 索引执行失败（退出码 ${code}），请检查目录权限和磁盘空间`)); return; }
      try {
        const value = action === "status" ? JSON.parse(output) : null;
        if (action === "status" && (typeof value.initialized !== "boolean" || fs.realpathSync(value.projectPath) !== project)) throw new Error();
        resolve(value);
      } catch { reject(new SetupError("CodeGraph 返回的项目状态无效")); }
    });
  });
  async function activate(params) {
    try {
      project = projectPath(cwd, home, codexHome);
      if (!project) {
        message = "CodeGraph 仅在工作区已添加的本地项目中启用；当前聊天目录未接入自动索引。";
        return;
      }
      await prepareProject({ project, run, signal: controller.signal, record });
      if (stopped) return;
      backend = launch(["serve", "--mcp", "--path", project]);
      const initId = `codey-init-${randomUUID()}`;
      const timer = setTimeout(() => { fail(); backend.kill(); }, 60_000);
      backend.once("close", () => { clearTimeout(timer); if (!stopped) fail(); });
      backend.once("error", () => { clearTimeout(timer); fail(); });
      lines(backend.stdout, value => {
        if (value.id === initId) {
          clearTimeout(timer);
          if (value.error) { fail(); backend.kill(); return; }
          ready = true;
          backend.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
          record("watching", "项目索引已接入，代码变化会自动同步。");
        } else if (value.id !== undefined && pending.delete(value.id)) send(value);
        else if (value.method && value.id === undefined) send(value);
      }, () => { fail(); backend.kill(); });
      backend.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: initId, method: "initialize", params: { ...params, capabilities: {} } })}\n`);
    } catch (error) {
      if (!stopped) fail(error);
    }
  }
  const parent = process.ppid;
  const watchParent = setInterval(() => {
    try { process.kill(parent, 0); } catch (error) { if (error.code === "ESRCH") stop(); }
  }, 2000);
  watchParent.unref();
  const initialTimeout = setTimeout(stop, 60_000);
  initialTimeout.unref();
  lines(input, request => {
    const { id, method, params } = request;
    if (method === "initialize") {
      clearTimeout(initialTimeout);
      result(id, { protocolVersion, serverInfo: { name: "codey-codegraph", version: "1" }, capabilities: { tools: {} },
        instructions: "CodeGraph 仅自动管理工作区已添加的本地项目索引，普通聊天目录不启用。首次建库在后台进行；若工具提示索引准备中，先使用其他文件工具，稍后重试，不要要求用户输入目录或运行命令。" });
      if (!initialized) { initialized = true; void activate(params); }
    } else if (method === "tools/list") result(id, { tools: project ? tools : [] });
    else if (method === "ping") result(id, {});
    else if (method === "resources/list") result(id, { resources: [] });
    else if (method === "resources/templates/list") result(id, { resourceTemplates: [] });
    else if (method === "prompts/list") result(id, { prompts: [] });
    else if (method === "tools/call") {
      if (!ready) failure(id, message);
      else if (pending.size >= 32) failure(id, "CodeGraph 查询较多，请稍后重试。");
      else {
        if (params?.arguments?.projectPath !== undefined) {
          try {
            const target = projectPath(params.arguments.projectPath, home, codexHome);
            if (!target) throw new SetupError("CodeGraph 仅支持工作区已添加的本地项目");
            params.arguments.projectPath = target;
          } catch (error) { failure(id, error.message); return; }
        }
        pending.add(id); backend.stdin.write(`${JSON.stringify(request)}\n`);
      }
    } else if (method === "notifications/cancelled") {
      pending.delete(params?.requestId);
      if (ready) backend.stdin.write(`${JSON.stringify(request)}\n`);
    } else if (id !== undefined) send({ jsonrpc: "2.0", id, error: { code: -32601, message: "Method not found" } });
  }, stop);
  input.once("end", stop);
  input.once("error", stop);
  output.once("error", stop);
  return { stop };
}

module.exports = { projectPath, mapWindowsPath, needsIndex, cleanEnv, writeRecord, activeWriter, acquireBootstrap, prepareProject, start };
if (require.main === module) {
  const entry = process.argv[2];
  try {
    const { getStaticTools } = require(path.join(path.dirname(entry), "../mcp/tools.js"));
    const server = start({ entry, root: __dirname, tools: getStaticTools() });
    process.once("SIGTERM", server.stop);
    process.once("SIGINT", server.stop);
  } catch {
    process.stderr.write("CodeGraph 自动接入启动失败，请在内置工具中重新启用。\n");
    process.exitCode = 1;
  }
}
