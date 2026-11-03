import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

const read = (path) => fs.readFileSync(new URL(`../backend/src/${path}`, import.meta.url), "utf8");

test("backup source contract: sensitive actions require confirmation and use the existing commit queue", () => {
  const commands = read("commands/backup.rs");
  assert.match(commands, /"export" \| "restore" \| "webdav_upload"[\s\S]*?require_confirmation\(args\)/);
  assert.match(commands, /config_write_lock\.lock\(\)\.await/);
  assert.ok(commands.indexOf("backup::save_recovery") < commands.indexOf("let mut saved = persist_codey_config_locked"));
  assert.match(commands, /persist_codey_config_locked\(\s*state,\s*&previous,\s*config/);
  assert.match(commands, /config_apply::enqueue\(state, saved\.reconcile_subagent_config\)/);
  assert.match(commands, /result\["restored"\] = json!\(true\)/);
  assert.match(commands, /applyWarning/);
  const commitFailure = commands.slice(commands.indexOf("let mut saved = persist_codey_config_locked"), commands.indexOf("saved.config_apply ="));
  assert.match(commitFailure, /恢复未完成，内存未更新/);
  assert.match(commitFailure, /磁盘主文件可能已轮转/);
  assert.match(commitFailure, /recovery_path\.display\(\)/);
  assert.doesNotMatch(commitFailure, /原配置未更改/);
  assert.doesNotMatch(commands, /record_failure|commit_codey_config_locked/);
  const backup = read("backup.rs");
  assert.match(backup, /const FORMAT: &str = "codey-config"/);
  assert.match(backup, /upstream_model_reasoning_efforts_by_provider/);
  assert.match(backup, /next\.codex_launch_proxy\s*\.clone_from\(&previous\.codex_launch_proxy\)/);
  assert.doesNotMatch(backup, /network_proxy|conversation_git/);
});

test("WebDAV source contract: revision, bounded reads, private writes, and no credential redirects", () => {
  const source = read("backup/webdav.rs");
  assert.match(source, /self\.revision != previous\.revision/);
  assert.match(source, /self\.revision = uuid::Uuid::new_v4\(\)\.to_string\(\)/);
  assert.match(source, /try_lock_exclusive\(\)/);
  assert.match(source, /read_bounded\(path, 16 \* 1024\)/);
  assert.match(source, /atomic_write_private_with_parent/);
  assert.match(source, /redirect\(reqwest::redirect::Policy::none\(\)\)/);
  assert.match(source, /read_bounded_body\(response, MAX_BACKUP_BYTES/);
  const publicView = source.slice(source.indexOf("pub(crate) fn public("), source.indexOf("pub(crate) fn merge("));
  assert.match(publicView, /passwordConfigured/);
  assert.doesNotMatch(publicView, /"password"\s*:/);
});

test("session cleanup source contract: schedule first and mutate only during stopped-runtime maintenance", () => {
  const source = read("session_management/mod.rs");
  const schedule = source.slice(source.indexOf("fn remove_selected("), source.indexOf("\n/// 仅由"));
  assert.match(schedule, /pending\.ids\.extend/);
  assert.match(schedule, /atomic_write_private/);
  assert.match(schedule, /restart_required: pending\.executable_count\(\) > 0/);
  assert.doesNotMatch(schedule, /\bclean\(|remove_file|database::clean|files::clean/);
  const finish = source.slice(source.indexOf("pub(crate) fn finish_pending("), source.indexOf("\nfn entry"));
  assert.match(finish, /clean_files\(home, &ids, true\)/);
  assert.match(finish, /entry.updated_at <= intent.updated_at/);
  assert.match(finish, /intent.policy_revision == revision/);
  assert.match(finish, /clean_files\(home, &ids, true\)\.context\([^\n]+\)\?;/);
  assert.match(finish, /database::clean\(home, &ids, true\)\.context\([^\n]+\)\?;/);
  assert.ok(finish.indexOf("remaining") < finish.indexOf("fs::remove_file"));
  assert.match(read("commands/sessions.rs"), /start_cleanup_watcher/);
  assert.doesNotMatch(read("commands/sessions.rs"), /builtin_tools::session_status/);
});
