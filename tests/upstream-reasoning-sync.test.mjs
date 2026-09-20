import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const sourceRoot = new URL("../src/", import.meta.url);
function load(name) {
  const source = readFileSync(new URL(`${name}.ts`, sourceRoot), "utf8");
  const exports = {};
  new Function("require", "exports", ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  }).outputText)((dependency) => load(dependency), exports);
  return exports;
}
const { buildSubagentModelOptions } = load("subagentModels");
const declaration = (levels) => levels.map((level) => ({ level, value: level }));

test("native and routed subagents use synced route-specific efforts and preserve manual overrides", () => {
  const model = "workbuddy/DeepSeek-v4.1-flash";
  const state = { officialModels: [], officialModelIds: [], thirdPartyModels: [model], thirdPartyModelMetadata: [] };
  for (const localRouterEnabled of [false, true]) {
    const config = {
      localRouterEnabled, activeProfileId: "route-a",
      profiles: [
        { id: "route-a", sourceProviderId: "a", name: "A", enabled: true },
        { id: "route-b", sourceProviderId: "b", name: "B", enabled: true },
      ],
      selectedModelsByProvider: { a: [model], b: [model] }, declaredOfficialModelsByProvider: {},
      upstreamModelReasoningEffortsByProvider: { a: { [model]: declaration(["low", "high", "max"]) }, b: { [model]: declaration(["low", "high", "xhigh"]) } },
    };
    let options = buildSubagentModelOptions(config, state, false, { id: "a", name: "A", official: false });
    assert.deepEqual(options.find((item) => item.providerId === "a").supportedReasoningEfforts, ["low", "high", "max"]);
    if (localRouterEnabled) assert.deepEqual(options.find((item) => item.providerId === "b").supportedReasoningEfforts, ["low", "high", "xhigh"]);
    config.modelReasoningEffortsByProvider = { a: { [model]: declaration(["high"]) } };
    options = buildSubagentModelOptions(config, state, false, { id: "a", name: "A", official: false });
    assert.deepEqual(options.find((item) => item.providerId === "a").supportedReasoningEfforts, ["high"]);
  }
});
