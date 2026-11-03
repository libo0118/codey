import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8").replaceAll("\r\n", "\n");

test("optional backend features are registered on the actual application entry points", () => {
  const lib = read("backend/src/lib.rs");
  const commands = read("backend/src/commands.rs");
  for (const name of ["backup", "codegraph_runtime", "ponytail", "session_management", "remote_control"]) {
    assert.match(lib, new RegExp(`mod ${name};`));
  }
  assert.match(commands, /"builtin_tools" => builtin_tools::invoke/);
  assert.match(commands, /"config_backup" => backup::invoke/);
  assert.match(commands, /"save_codex_session_cleanup_policy" =>\s*\{?\s*sessions::invoke/);
  assert.match(commands, /"revoke_remote_session" => crate::remote_control::invoke/);
  const stats = commands.slice(commands.indexOf('"/session/stats"'), commands.indexOf('"/session/model"'));
  assert.match(stats, /snapshot_with_history/);
  assert.match(stats, /builtin_tools::session_status/);
  assert.match(stats, /unavailable/);
  assert.doesNotMatch(lib, /codegraph_runtime::install|ponytail::install/);
});

test("pending automatic deletions are revalidated and complete restores use the shared persistence seam", () => {
  const sessions = read("backend/src/session_management/mod.rs");
  assert.match(sessions, /struct AutomaticCleanup/);
  assert.match(sessions, /policy_revision/);
  assert.match(sessions, /updated_at <= intent.updated_at/);
  assert.match(sessions, /retention::eligible/);
  assert.match(sessions, /legacy_ids/);
  const backup = read("backend/src/commands/backup.rs");
  assert.match(backup, /persist_codey_config_locked\(\s*state,\s*&previous,\s*config/);
  assert.doesNotMatch(backup.slice(0, backup.indexOf("#[cfg(test)]")), /codey_config_save_input|save_codey_config_locked/);
  assert.match(backup, /backup_restore_commits_complete_model_preferences_to_memory_and_disk/);
});

test("cleanup and remote service lifecycle preserve existing replay protections", () => {
  const runtime = read("backend/src/commands/runtime.rs");
  assert.match(runtime, /sessions::start_cleanup_watcher\(state\)\.await/);
  const shutdown = runtime.slice(runtime.indexOf("pub async fn begin_shutdown("));
  assert.match(shutdown, /sessions::stop_cleanup_watcher\(state\)\.await/);
  assert.match(shutdown, /remote_control.shutdown\(\)\.await/);
  const launcher = read("backend/src/launcher.rs");
  assert.ok(launcher.indexOf("session_management::finish_pending") < launcher.indexOf("message_delete::reapply_persisted_deletions"));
  assert.match(read("backend/src/lib.rs"), /remote_control.restore_startup\(&state\)\.await/);
});

test("both real Vite bundles and the CDP remote helper are embedded and validated", () => {
  assert.match(read("scripts/build-overlay.mjs"), /vite.remote.config.ts/);
  for (const path of ["scripts/build.mjs", "backend/build.rs"]) {
    const source = read(path);
    assert.match(source, /codey-remote.js/);
    assert.match(source, /codey-remote.css/);
    assert.match(source, /inject\/codey-remote-control.js/);
  }
  assert.match(read("backend/src/cdp.rs"), /dist-overlay\/inject\/codey-remote-control.js/);
  const license = read("third_party/codex-mobile-bridge.LICENSE");
  assert.match(license, /Copyright \(c\) 2026 try2love/);
  assert.ok(read("THIRD_PARTY_NOTICES.md").includes(license.trim()));
  assert.match(read("scripts/installer/windows/Codey.nsi"), /File "\$\{PROJECT_ROOT\}\\THIRD_PARTY_NOTICES.md"/);
  assert.match(read("scripts/package-linux.mjs"), /THIRD_PARTY_NOTICES.md/);
});
