import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

const read = (path) => fs.readFileSync(new URL(`../backend/src/session_management/${path}`, import.meta.url), "utf8");

test("legacy cleanup source contract: separate inventory and marker-bound confirmation", () => {
  const source = read("mod.rs");
  assert.match(source, /legacy_pending_ids: Vec<String>/);
  assert.match(source, /pending_count: pending\.executable_count\(\)/);
  assert.match(source, /serde_json::to_vec\(&\(&scope, &pending\)\)/);
  assert.match(source, /known\.contains\(id\) \|\| legacy\.contains\(id\)/);
  assert.match(source, /scheduled_count: usize/);
  assert.match(source, /cleared_legacy_count: usize/);
  assert.match(source, /restart_required: pending\.executable_count\(\) > 0/);
});

test("automatic cleanup source contract: persist started phase before ordered file and database stages", () => {
  const source = read("mod.rs");
  assert.match(source, /enum AutomaticCleanupPhase[\s\S]*?Queued[\s\S]*?Started/);
  assert.match(source, /#\[serde\(default\)\]\s*phase: AutomaticCleanupPhase/);
  const finish = source.slice(source.indexOf("fn finish_pending_with("), source.indexOf("\nfn entry"));
  assert.match(finish, /entry\.updated_at <= intent\.updated_at/);
  assert.match(finish, /intent\.phase == AutomaticCleanupPhase::Started/);
  assert.match(finish, /intent\.policy_revision == revision/);
  assert.match(finish, /intent\.phase = AutomaticCleanupPhase::Started/);
  assert.ok(finish.indexOf("save_pending(home, &pending)?") > finish.indexOf("intent.phase = AutomaticCleanupPhase::Started"));
  assert.ok(finish.indexOf("save_pending(home, &pending)?") < finish.indexOf("clean_files(home, &ids, true)"));
  assert.match(finish, /clean_files\(home, &ids, true\)[\s\S]*?\?;\s*database::clean\(home, &ids, true\)/);
  assert.ok(finish.indexOf("let remaining = scan(home)?") < finish.indexOf("fs::remove_file"));
});

test("cleanup regression sources cover actual index replacement failure and new activity", () => {
  const files = read("files.rs");
  assert.match(files, /persist_index\(&temporary, path\)/);
  const tests = read("retention/tests.rs");
  for (const name of [
    "started_cleanup_retries_failed_index_replacement_after_database_metadata_changes",
    "failed_start_marker_write_cannot_touch_session_data",
    "started_cleanup_preserves_new_file_or_database_activity",
  ]) assert.ok(tests.includes(`fn ${name}(`), name);
  assert.match(tests, /files::clean_with/);
  assert.match(tests, /PermissionDenied/);
  assert.match(tests, /AutomaticCleanupPhase::Started/);
});
