import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import test from "node:test";
import vm from "node:vm";

const cdp = readFileSync(new URL("../backend/src/cdp.rs", import.meta.url), "utf8");
const template = cdp.match(/const REQUEST_GATE_VERIFY_SCRIPT_TEMPLATE: &str = r#"([\s\S]*?)"#;/)[1];
const script = template
  .replaceAll("__CODEY_GATE_BUDGET_MS__", "1000")
  .replaceAll("__CODEY_GATE_CALL_MS__", "2000")
  .replaceAll("__CODEY_GATE_STEP_MS__", "200")
  .replaceAll("__CODEY_GATE_NOTIFY__", "true");

test("the actual Fetch service narrows official scan candidates without the fork numeric wildcards", () => {
  const read = path => readFileSync(new URL(`../backend/src/${path}`, import.meta.url), "utf8");
  const patch = read("renderer_asset_patch.rs");
  const scan = read("renderer_asset_scan.rs");
  const launcher = read("launcher.rs");
  assert.match(launcher, /renderer_asset_patch::start_service\([\s\S]*?Some\(crate::renderer_asset_patch::CodexPaths\s*\{/);
  assert.match(patch, /renderer_asset_scan::load_or_scan\(&app_dir, &cache_file, &anchors\)/);
  assert.match(patch, /fetch_patterns\(self\.disable_pet, Some\(&outcome\.scan\)\)/);
  assert.match(patch, /context\.apply_asset_scan\(scan\.await, started\)[\s\S]*?refresh_fetch_patterns\(&connection, &context\)/);
  const patterns = patch.slice(patch.indexOf("fn fetch_patterns("), patch.indexOf("fn pet_sprite_id("));
  assert.match(patterns, /Some\(scan\) => \{\s*patterns\.extend\(scan\.candidates\.iter\(\)/);
  const candidates = patterns.slice(patterns.indexOf("Some(scan)"), patterns.indexOf("None =>"));
  assert.doesNotMatch(candidates, /NUMERIC_ASSET_URL_PATTERN|CSS_ASSET_URL_PATTERN/);
  assert.match(patterns, /None => \{[\s\S]*?"urlPattern": NUMERIC_ASSET_URL_PATTERN/);
  assert.doesNotMatch(patterns, /0\.\.=9|assets\/\*\{digit\}/);
  assert.match(scan, /numeric_asset_name/);
});

async function probe(patch, change) {
  let now = 0, sequence = 0, result, failure, settled = false;
  const timers = new Map();
  const context = vm.createContext({
    window: { __codeyModelWhitelistPatch: patch, __codexSessionDeleteBridge() {}, __codeyRequestGateWaitMs: 500 },
    Date: { now: () => now },
    setTimeout(callback, delay) { const id = ++sequence; timers.set(id, { at: now + delay, callback }); return id; },
    clearTimeout(id) { timers.delete(id); },
  });
  vm.runInContext(script, context).then(
    value => { result = JSON.parse(value); settled = true; },
    error => { failure = error; settled = true; },
  );
  change?.(context);
  while (!settled) {
    await new Promise(setImmediate);
    if (settled) break;
    const entry = [...timers.entries()].sort((left, right) => left[1].at - right[1].at)[0];
    assert.ok(entry, "the bounded production probe must not stall without a timer");
    timers.delete(entry[0]);
    now = entry[1].at;
    entry[1].callback();
  }
  if (failure) throw failure;
  assert.equal(timers.size, 0, "all race and retry timers must be cleared");
  return result;
}

test("the existing Rust request-gate harness executes against its actual embedded probe", () => {
  const harness = cdp.match(/fn request_gate_verify_script_marks_missing_patch_for_reinstall\(\)[\s\S]*?let harness = r#"([\s\S]*?)"#;/)[1];
  const output = spawnSync(process.execPath, ["-e", harness, JSON.stringify(script), "1000"], { encoding: "utf8" });
  assert.equal(output.status, 0, output.stderr);
});

test("background readiness requires both ensure success and the active complete request gate", async () => {
  for (const ready of [undefined, false, "true", true]) {
    let notices = 0;
    const patch = { ensureRequestGate: async () => true, notifyRequestGateUnavailable() { notices++; } };
    if (ready !== undefined) patch.requestGateReady = () => ready;
    const result = await probe(patch);
    assert.equal(result.ready, ready === true);
    assert.equal(notices, ready === true ? 0 : 1);
  }
});

test("background probes abandon replaced bridges and patches without publishing stale readiness or alerts", async () => {
  for (const replacement of ["patch", "bridge", "disposed"]) {
    let notices = 0;
    const patch = { ensureRequestGate: async () => {
      if (replacement === "disposed") throw Error("模型请求校验已重新加载");
      return true;
    }, requestGateReady: () => true, notifyRequestGateUnavailable() { notices++; } };
    const result = await probe(patch, context => {
      if (replacement === "patch") context.window.__codeyModelWhitelistPatch = {};
      if (replacement === "bridge") context.window.__codexSessionDeleteBridge = () => {};
    });
    assert.equal(result.ready, false);
    assert.equal(result.reinstall, true);
    assert.equal(notices, 0);
  }
});

test("failed or hanging probes stay bounded and never expose native exception text", async () => {
  for (const ensureRequestGate of [() => false, () => Promise.reject(Error("PRIVATE_NATIVE_PROMPT")), () => new Promise(() => {})]) {
    let notices = 0;
    const result = await probe({ ensureRequestGate, requestGateReady: () => false, notifyRequestGateUnavailable() { notices++; } });
    assert.equal(result.ready, false);
    assert.equal(result.budgetExhausted, true);
    assert.ok(result.waitedMs <= 500);
    assert.ok(!JSON.stringify(result).includes("PRIVATE_NATIVE_PROMPT"));
    assert.equal(notices, 1);
  }
});
