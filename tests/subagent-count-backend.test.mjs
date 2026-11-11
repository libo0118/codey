import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = (path) => readFile(new URL(`../backend/src/${path}`, import.meta.url), "utf8");

test("subagent count source contract covers config, save presence, store and backup boundaries", async () => {
  const [types, validation, commands, store, backup] = await Promise.all([
    source("config/types.rs"), source("config/subagent.rs"), source("commands.rs"),
    source("config/store.rs"), source("backup.rs"),
  ]);
  assert.match(types, /#\[serde\(default = "default_subagent_max_threads"\)\]\s*pub subagent_max_threads: u32/);
  assert.match(types, /pub const fn default_subagent_max_threads\(\) -> u32\s*\{\s*2\s*\}/);
  assert.match(validation, /!\(1\.\.=64\)\.contains\(&limit\)/);
  assert.doesNotMatch(validation, /clamp\(/);
  assert.match(commands, /fields\.contains_key\("subagentMaxThreads"\)/);
  assert.match(commands, /if subagent_max_threads_present\s*\{\s*config\.subagent_max_threads = config_input\.subagent_max_threads;/);
  const save = commands.slice(commands.indexOf("async fn save_codey_config_locked("));
  assert.ok(save.indexOf("validate_subagent_max_threads(") < save.indexOf("config.remember_model_aliases()"));
  assert.equal(store.match(/validate_subagent_max_threads\(/g)?.length, 2);
  assert.match(backup, /validate_subagent_max_threads\(config\.subagent_max_threads\)/);
});

test("subagent count source contract reaches both launch paths, native overrides and dispatch gate", async () => {
  const [launcher, runtime, policy, orchestrator, wrapper] = await Promise.all([
    source("launcher.rs"), source("codex_config.rs"), source("subagent_gate/runtime_policy.rs"),
    source("subagent_orchestrator.rs"), source("codex_startup_patch.rs"),
  ]);
  assert.match(launcher, /let subagent_max_threads = runtime_subagent_config\.subagent_max_threads/);
  assert.match(launcher, /subagent_max_threads: native_subagent_config\.subagent_max_threads/);
  assert.ok(runtime.includes('agents["max_concurrent_threads_per_session"] = value(i64::from(limit))'));
  assert.ok(runtime.includes('"agents.max_concurrent_threads_per_session"'));
  assert.match(runtime, /agents\.remove\("max_threads"\)/);
  assert.doesNotMatch(runtime, /agents\["max_threads"\]\s*=|"agents\.max_threads"/);
  assert.match(runtime, /subagent_max_threads: options\.subagent_max_threads/);
  assert.match(policy, /pub\(crate\) max_threads: u32/);
  assert.match(policy, /validate_subagent_max_threads\(policy\.max_threads\)/);
  assert.match(orchestrator, /\|policy\| policy\.max_threads as usize/);
  assert.match(orchestrator, /concurrency_denial\(&ledger, active_agents, now_ms, limit\)/);
  assert.doesNotMatch(orchestrator, /const CONCURRENCY_LIMIT:/);
  assert.match(wrapper, /let configs = app_server_runtime_configs\(runtime_overrides\)/);
  assert.match(wrapper, /for config in runtime_overrides/);
});

test("runtime count prompt replaces the mode limit without hardcoding it in collaboration guidance", async () => {
  const [runtime, guidance] = await Promise.all([source("codex_config.rs"), source("codex_config_guidance.rs")]);
  const collaboration = guidance.match(/pub\(crate\) const ROOT_AGENT_COLLABORATION_USAGE_HINT: &str = r(#+)"([\s\S]*?)"\1;/)?.[2];
  assert.ok(collaboration);
  assert.doesNotMatch(collaboration, /concurrency limit of two|At most two sub-agents/);
  assert.match(runtime, /let collaboration_hint = root_agent_collaboration_usage_hint\(\)/);
  for (const [name, phrase, replacement] of [
    ["ROOT_AGENT_MULTI_AGENT_MODE_HINT", "At most two sub-agents run at once", (n) => `At most ${n} sub-agents run at once (excluding the root)`],
  ]) {
    const literal = guidance.match(new RegExp(`pub\\(crate\\) const ${name}: &str = ("(?:[^"\\\\]|\\\\[\\s\\S])*");`))?.[1];
    assert.ok(literal, name);
    const template = JSON.parse(literal.replace(/\\\r?\n\s*/g, ""));
    assert.equal(template.split(phrase).length - 1, 1);
    assert.match(runtime, new RegExp(`${name}\\.replace\\(`));
    for (const limit of [1, 4, 64]) {
      const rendered = template.replace(phrase, replacement(limit));
      assert.ok(!rendered.includes(phrase));
      if (name === "ROOT_AGENT_MULTI_AGENT_MODE_HINT") assert.ok(Buffer.byteLength(rendered) <= 1600);
    }
  }
  assert.match(runtime, /rebuild_constraint_file\([\s\S]*?CODEY_COLLABORATION_HINT_FILE\),\s*&collaboration_hint/);
});

test("count changes stay pending restart while role transactions retain the launch policy count", async () => {
  const [commands, transaction, apply] = await Promise.all([
    source("commands.rs"), source("codex_config/runtime_role_transaction.rs"), source("commands/config_apply.rs"),
  ]);
  assert.match(commands, /applied\.subagent_max_threads != current\.subagent_max_threads/);
  assert.match(transaction, /begin_runtime_subagent_policy_update\([\s\S]*?state\.subagent_max_threads/);
  assert.match(transaction, /commit_policy\([\s\S]*?state\.subagent_max_threads/);
  assert.doesNotMatch(transaction, /state\.subagent_max_threads\s*=|config\.subagent_max_threads/);
  assert.match(apply, /if subagents\.requires_restart\(\) \|\| runtime_config_requires_restart\(state, config\)\.await\s*\{\s*return json!\(\{"state":"restart_required"\}\);/);
});
