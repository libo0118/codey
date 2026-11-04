import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = (path) => readFile(new URL(`../backend/src/${path}`, import.meta.url), "utf8");

test("custom role save validates the raw matrix and uses an explicit non-persistent delete request", async () => {
  const [commands, types, validation, store, backup] = await Promise.all([
    source("commands.rs"), source("config/types.rs"), source("config/subagent.rs"),
    source("config/store.rs"), source("backup.rs"),
  ]);
  for (const field of ["name", "description", "developer_instructions", "access"]) {
    assert.match(types, new RegExp(`pub ${field}:`));
  }
  assert.match(commands, /fields\s*\.get\("deletedSubagentRoles"\)/);
  assert.doesNotMatch(types, /deleted_subagent_roles/);
  const save = commands.slice(commands.indexOf("async fn save_codey_config_locked("));
  assert.ok(save.indexOf("merge_subagent_role_updates(") < save.indexOf("config.remember_model_aliases()"));
  assert.match(validation, /merged\.extend\(incoming\.clone\(\)\)/);
  assert.match(validation, /for role in deleted\s*\{\s*merged\.remove\(role\)/);
  assert.match(validation, /MAX_CUSTOM_SUBAGENT_ROLES: usize = 32/);
  for (const limit of [80, 1000, 4000]) assert.match(validation, new RegExp(`, ${limit}\\)`));
  assert.match(store, /validate_subagent_roles/);
  assert.match(backup, /validate_subagent_roles/);
});

test("custom roles are registered through the existing lease without sweeping unrelated runtime files", async () => {
  const [runtime, transaction, conflicts] = await Promise.all([
    source("codex_config.rs"), source("codex_config/runtime_role_transaction.rs"),
    source("codex_config/custom_subagents.rs"),
  ]);
  assert.match(runtime, /\.chain\(subagent_roles/);
  assert.match(runtime, /for \(role, selection\) in roles/);
  assert.match(runtime, /selection\.developer_instructions\.trim\(\)/);
  assert.match(transaction, /snapshot_runtime_agent_files\(&constraints_dir, &registration_roles\)/);
  assert.match(transaction, /restore_runtime_agent_files_and_lease/);
  assert.doesNotMatch(runtime + transaction, /fs::read_dir\(/);
  assert.match(conflicts, /home\.join\("agents"\)/);
  assert.match(conflicts, /read_bounded\(&path, 256 \* 1024\)/);
  assert.match(conflicts, /\.get\("name"\)/);
  assert.match(conflicts, /metadata_is_link/);
});

test("read-only roles have a native sandbox and immutable attempt capability gates", async () => {
  const [runtime, orchestrator, gateTests] = await Promise.all([
    source("codex_config.rs"), source("subagent_orchestrator.rs"),
    source("subagent_orchestrator/custom_subagent_tests.rs"),
  ]);
  assert.match(runtime, /document\["sandbox_mode"\] = value\("read-only"\)/);
  assert.match(orchestrator, /capability != "command\.execute" && capability != "workspace\.write"/);
  assert.match(orchestrator, /!reservation_declares_write\(reservation\)/);
  assert.match(orchestrator, /register_runtime_role\(role\)/);
  assert.match(gateTests, /authorize_child_tool\(/);
  assert.match(gateTests, /mcp__cua_repl__js/);
});
