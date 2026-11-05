import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";

assert(process.argv[2], "Pass the built Codey executable");
const executable = resolve(process.argv[2]);
const hash = value => createHash("sha256").update(value).digest("hex");
const temporaryRoot = resolve(tmpdir());
for (const shape of ["explicit", "missing", "null"]) {
  const home = mkdtempSync(join(temporaryRoot, "codey-hook-probe-"));
  try {
    const state = join(home, "codey-subagent-gate-v3"), workspace = join(home, "workspace");
    mkdirSync(workspace, { recursive: true }); mkdirSync(state, { recursive: true });
    writeFileSync(join(state, "runtime-subagent-policy.json"), JSON.stringify({ schemaVersion: 1, maxThreads: 2, runtimeAgentHashes: {}, roles: { default: { enabled: true, model: "synthetic-model", reasoningEffort: "low" } } }));
    const parent = "11111111-1111-4111-8111-111111111111", child = "22222222-2222-4222-8222-222222222222", runtime = "isolated-role-probe";
    const call = input => {
      const result = spawnSync(executable, ["--codey-subagent-gate-hook"], { input: JSON.stringify({ session_id: parent, cwd: workspace, turn_id: "probe-turn", ...input }), encoding: "utf8", windowsHide: true, timeout: 15000, env: { ...process.env, CODEX_HOME: home, CODEY_APP_STATE_DIR: join(home, "app"), CODEY_SUBAGENT_GATE_ACTIVE: "1", CODEY_SUBAGENT_GATE_RUNTIME_ID: runtime } });
      assert.equal(result.status, 0, result.stderr || result.error?.message);
      return JSON.parse(result.stdout);
    };
    const tool_input = { task_name: "default_probe", fork_turns: "none", message: "Read the synthetic fixture only." };
    const admitted = call({ hook_event_name: "PreToolUse", tool_name: "agents.spawn_agent", tool_input });
    assert.notEqual(admitted.hookSpecificOutput?.permissionDecision, "deny", JSON.stringify(admitted));
    call({ hook_event_name: "PostToolUse", tool_name: "agents.spawn_agent", tool_input, tool_response: JSON.stringify({ task_name: "/root/default_probe" }) });
    const sessions = join(home, "sessions", "2026", "11", "05"); mkdirSync(sessions, { recursive: true });
    const transcript = join(sessions, `rollout-probe-${child}.jsonl`);
    const payload = { id: child, parent_thread_id: parent, agent_path: "/root/default_probe", source: { subagent: { thread_spawn: { parent_thread_id: parent, agent_path: "/root/default_probe" } } } };
    if (shape !== "missing") payload.source.subagent.thread_spawn.agent_role = shape === "null" ? null : "default";
    writeFileSync(transcript, JSON.stringify({ type: "session_meta", payload }) + "\n");
    call({ hook_event_name: "SubagentStart", agent_id: child, transcript_path: transcript });
    const directory = join(state, hash(parent)); mkdirSync(directory, { recursive: true });
    writeFileSync(join(directory, `${hash(runtime)}-runtime-attestation-${hash(child)}.json`), JSON.stringify({ schemaVersion: 1, runtimeIdHash: hash(runtime), agentIdHash: hash(child), role: "default", model: "synthetic-model", reasoningEffort: "low" }));
    const output = call({ hook_event_name: "PreToolUse", agent_id: child, transcript_path: transcript, tool_name: "mcp__codey_fastctx__glob", tool_input: { path: workspace, pattern: ["*.rs"] } });
    assert.notEqual(output.hookSpecificOutput?.permissionDecision, "deny", `${shape}: ${JSON.stringify(output)}`);
    console.log(`Synthetic built-executable Hook probe passed: ${shape}`);
  } finally {
    assert.equal(dirname(home), temporaryRoot); assert(basename(home).startsWith("codey-hook-probe-"));
    rmSync(home, { recursive: true, force: true });
  }
}
