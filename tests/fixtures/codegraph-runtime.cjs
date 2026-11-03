// Adapted from fraternity-z/CodeyX (30e1daa), AGPL-3.0-only.
const fs = require("node:fs");
const path = require("node:path");
const readline = require("node:readline");
const [action, ...args] = process.argv.slice(2);
const project = process.cwd();
fs.appendFileSync(path.join(project, "calls.jsonl"), JSON.stringify({ action, args, project }) + "\n");
const database = path.join(project, ".codegraph", "codegraph.db");
if (action === "status") {
  console.log(JSON.stringify({ initialized: fs.existsSync(database), projectPath: project, index: { state: "complete" } }));
} else if (action === "init" || action === "index") {
  setTimeout(() => {
    if (fs.existsSync(path.join(project, "fail"))) {
      console.error("private-source-and-token");
      process.exit(7);
    }
    fs.mkdirSync(path.dirname(database), { recursive: true });
    fs.writeFileSync(database, "mock index");
  }, 300);
} else if (action === "serve") {
  readline.createInterface({ input: process.stdin }).on("line", line => {
    const request = JSON.parse(line);
    if (request.id === undefined) return;
    const result = request.method === "initialize" ? { protocolVersion: "2024-11-05", capabilities: {}, serverInfo: { name: "fixture", version: "1" } }
      : { content: [{ type: "text", text: fs.readFileSync(path.join(project, "source.ts"), "utf8") }], project };
    console.log(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }));
  });
}
