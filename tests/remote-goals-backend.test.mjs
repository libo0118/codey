import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";

const source = fs.readFileSync(new URL("../public/codey-remote-control.js", import.meta.url), "utf8");
const threadId = "67a5aa54-3eac-4d13-b885-b7fe95819d5f";
const initial = { objective: "完成隔离测试", status: "paused", createdAt: 10, tokenBudget: 1000, tokensUsed: 20, timeUsedSeconds: 3 };
const expected = goal => goal && Object.fromEntries(["objective", "status", "goalId", "createdAt", "createdAtMs"].filter(key => goal[key] !== undefined).map(key => [key, goal[key]]));
const plain = value => JSON.parse(JSON.stringify(value));

function fixture(value = initial) {
  let goal = value && { ...value }, completed = null;
  const writes = [], reads = [];
  const manager = {
    startConversation() { assert.fail("goal must not create a conversation"); },
    async getStreamRole(id) { assert.equal(id, threadId); return { role: "owner" }; },
    async getConversation(id) { return { id, turns: [], completedThreadGoal: completed }; },
    async sendRequest(method, params) {
      reads.push([method, plain(params)]);
      assert.equal(params.threadId, threadId);
      if (method === "thread/goal/get") return { goal };
      assert.equal(method, "thread/goal/set");
      writes.push([method, plain(params)]);
      goal = { ...goal, ...params, createdAt: 11, tokensUsed: 0, timeUsedSeconds: 0 };
      return { goal };
    },
    async setThreadGoal(id, options) {
      writes.push(["setThreadGoal", id, plain(options)]);
      goal = { objective: options.objective, status: "active", createdAt: 11 };
      return goal;
    },
    async setThreadGoalStatus(id, options) {
      writes.push(["setThreadGoalStatus", id, plain(options)]);
      goal = { ...goal, status: options.status };
      return goal;
    },
  };
  const window = {
    setTimeout, clearTimeout,
    __codeyModelWhitelistPatch: { ensureRequestGate: async () => true, requestGateReady: () => true },
    __codeyAppServerManagerResolverFromModule: () => () => manager,
    __codeyAppServerManagerFromReact: resolve => resolve({ get: () => null }, "local"),
    __codeyImportCodexAsset: async () => ({ factory: function (scope, hostId) {
      return { scope: scope, hostId: hostId, requestClient: scope.get("client"), dynamicTools: new Map(), readCreationInputs: async () => ({}) };
    } }),
  };
  vm.runInNewContext(source, { window, document: { scripts: [{ src: "app://codex/assets/app-initial-goal-fixture.js" }], querySelectorAll: () => [] }, performance: { getEntriesByType: () => [] } });
  const run = (action, patch = {}) => window.__codeyRemoteControl({ action, threadId, expected: expected(goal), ...patch }, Date.now() + 10_000);
  return { manager, window, run, writes, reads, setGoal: value => { goal = value; }, setCompleted: value => { completed = value; } };
}

test("goal reads normalize six states, timestamps and allowlist all public values without model requests", async () => {
  for (const [native, status] of [["active", "active"], ["paused", "paused"], ["blocked", "blocked"], ["usage_limited", "usageLimited"], ["budget_limited", "budgetLimited"], ["complete", "complete"]]) {
    const app = fixture({ ...initial, status: native, createdAt: undefined, createdAtMs: 10500, privateConfig: "SECRET" });
    delete app.window.__codeyModelWhitelistPatch;
    const result = plain(await app.run("goalRead"));
    assert.equal(result.status, "ok"); assert.equal(result.availability, "ready");
    assert.equal(result.goal.status, status); assert.equal(result.goal.createdAt, 10);
    assert.equal(result.activation, "not-requested"); assert.ok(!JSON.stringify(result).includes("SECRET"));
    assert.deepEqual(app.writes, []);
  }
});

test("unreadable, unsupported and malformed goal responses never mean absent", async () => {
  for (const invalid of ["unsupported", "offline", "missing", "badstatus", "badnumber", "coercion"]) {
    const app = fixture();
    if (["unsupported", "offline"].includes(invalid)) app.manager.sendRequest = async () => { throw Object.assign(Error("PRIVATE"), { code: invalid === "unsupported" ? -32601 : 500 }); };
    if (invalid === "missing") app.manager.sendRequest = async () => ({});
    if (invalid === "badstatus") app.setGoal({ ...initial, status: "future" });
    if (invalid === "badnumber") app.setGoal({ ...initial, tokensUsed: Number.MAX_SAFE_INTEGER + 1 });
    if (invalid === "coercion") app.setGoal({ ...initial, createdAtMs: "10000" });
    const result = await app.run("goalCreate", { expected: null, objective: "new" });
    assert.equal(result.status, "failed"); assert.notEqual(result.availability, "ready");
    assert.equal(result.activation, "not-requested"); assert.equal(Object.hasOwn(result, "goal"), false);
    assert.ok(!JSON.stringify(result).includes("PRIVATE")); assert.deepEqual(app.writes, []);
  }
});

test("goal creation only replaces none or the same completed goal, and never claims activation", async () => {
  for (const before of [null, { ...initial, status: "complete" }]) {
    const app = fixture(before);
    const result = await app.run("goalCreate", { objective: "new objective" });
    assert.equal(result.status, "ok"); assert.equal(result.goal.status, "active");
    assert.equal(result.activation, "not-requested"); assert.equal(app.writes.length, 1);
    assert.equal(app.writes[0][0], "setThreadGoal");
  }
  const app = fixture({ ...initial, status: "active" });
  assert.equal((await app.run("goalCreate", { objective: initial.objective })).code, "goal_conflict");
  assert.deepEqual(app.writes, []);
  const cached = fixture(null); cached.setCompleted({ ...initial, status: "complete" });
  const read = await cached.run("goalRead"); assert.equal(read.goal.status, "complete");
  assert.equal((await cached.run("goalCreate", { expected: expected(read.goal), objective: "new" })).status, "ok");
});

test("pause never interrupts a reply and accepts blocked and native limited states", async () => {
  for (const status of ["active", "blocked", "usageLimited", "budgetLimited"]) {
    const app = fixture({ ...initial, status }); delete app.window.__codeyModelWhitelistPatch;
    const result = await app.run("goalPause");
    assert.equal(result.goal.status, "paused"); assert.equal(result.activation, "not-requested");
    assert.deepEqual(app.writes, [["setThreadGoalStatus", threadId, { status: "paused" }]]);
  }
  const app = fixture({ ...initial, status: "active" });
  assert.equal((await app.run("goalResume")).code, "goal_conflict");
  assert.equal((await app.run("goalEdit", { objective: "new" })).code, "goal_conflict");
  assert.deepEqual(app.writes, []);
});

test("editing is a paused write, resets native usage, preserves or explicitly changes budget", async () => {
  for (const patch of [{}, { tokenBudget: 2000 }, { tokenBudget: null }]) {
    const app = fixture(); delete app.window.__codeyModelWhitelistPatch;
    const result = await app.run("goalEdit", { objective: "updated", ...patch });
    assert.equal(result.status, "ok"); assert.equal(result.goal.status, "paused");
    assert.equal(result.goal.tokensUsed, 0); assert.equal(result.goal.timeUsedSeconds, 0);
    assert.deepEqual(app.writes, [["thread/goal/set", { threadId, objective: "updated", status: "paused", tokenBudget: Object.hasOwn(patch, "tokenBudget") ? patch.tokenBudget : 1000 }]]);
  }
});

test("real awaited owner capability and stable expected identity are required before every write", async () => {
  for (const change of ["identity", "status", "role", "stub", "missing-identity", "none"]) {
    const app = fixture();
    const stale = expected(initial);
    if (change === "identity") app.setGoal({ ...initial, createdAt: 99 });
    if (change === "status") app.setGoal({ ...initial, status: "active" });
    if (change === "role") app.manager.getStreamRole = async () => ({ role: "follower" });
    if (change === "stub") app.manager.getStreamRole = async () => undefined;
    if (change === "missing-identity") { app.setGoal({ objective: initial.objective, status: "paused" }); delete stale.createdAt; }
    if (change === "none") app.setGoal(null);
    const result = await app.run("goalResume", { expected: stale });
    assert.equal(result.status, "failed", change); assert.deepEqual(app.writes, []);
  }
  const app = fixture();
  app.window.__codeyModelWhitelistPatch.ensureRequestGate = async () => { app.setGoal({ ...initial, createdAt: 99 }); return true; };
  assert.equal((await app.run("goalResume")).code, "goal_conflict"); assert.deepEqual(app.writes, []);
});

test("create and resume fail closed at model gate, expiration or lost mutation acknowledgements", async () => {
  for (const action of ["goalCreate", "goalResume"]) {
    for (const failure of ["gate", "expired", "ack"]) {
      const app = fixture(action === "goalCreate" ? null : initial);
      if (failure === "gate") delete app.window.__codeyModelWhitelistPatch;
      if (failure === "expired") app.manager.getStreamRole = async () => { app.window.__codeyRemoteControl.dispose(); return { role: "owner" }; };
      if (failure === "ack") {
        const name = action === "goalCreate" ? "setThreadGoal" : "setThreadGoalStatus";
        app.manager[name] = async () => { app.writes.push("submitted"); throw Error("PRIVATE_ACK"); };
      }
      const result = await app.run(action, { objective: action === "goalCreate" ? "new" : undefined });
      assert.equal(result.status, "failed");
      assert.equal(result.activation, failure === "ack" ? "unknown" : "not-requested");
      assert.equal(app.writes.length, failure === "ack" ? 1 : 0);
      assert.ok(!JSON.stringify(result).includes("PRIVATE_ACK"));
    }
  }
});

test("active or unreadable native replies reject create/resume before Goal mutations, including canonical history", async () => {
  for (const action of ["goalCreate", "goalResume"]) {
    for (const turns of [undefined, [{ status: "inProgress" }], { turnHistory: { kind: "canonical", history: { entitiesByKey: { active: { status: "inProgress" } }, islands: [{ entries: [{ value: "active" }] }] } } }]) {
      const app = fixture(action === "goalCreate" ? null : initial);
      app.manager.getConversation = async id => ({ id, ...(turns && !Array.isArray(turns) ? turns : { turns }) });
      const result = await app.run(action, { objective: "new" });
      assert.equal(result.status, "failed"); assert.equal(result.activation, "not-requested");
      assert.deepEqual(app.writes, []);
    }
  }
  const app = fixture({ ...initial, status: "active" });
  app.manager.getConversation = async id => ({ id, turns: [{ status: "inProgress" }] });
  assert.equal((await app.run("goalPause")).status, "ok", "pausing is not interrupting");
});

test("hosted Goal RPC handles release after both successful reads and writes", async () => {
  const app = fixture(); let pending = 0, disposed = 0;
  for (const name of ["sendRequest", "getConversation", "getStreamRole", "setThreadGoalStatus"]) {
    const original = app.manager[name].bind(app.manager);
    app.manager[name] = (...args) => {
      pending++;
      const result = original(...args);
      result[Symbol.dispose] = () => disposed++;
      return result;
    };
  }
  assert.equal((await app.run("goalResume")).status, "ok");
  assert.ok(pending >= 4); assert.equal(disposed, pending);
});
