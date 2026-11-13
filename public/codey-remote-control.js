// Remote-only native-manager adapter. Existing session/model hooks retain ownership.
(() => {
  const version = 7;
  if (window.__codeyRemoteControl?.version === version) return;
  window.__codeyRemoteControl?.dispose?.();
  let disposed = false;
  const appServerManagerResolverFromModule = (...args) => window.__codeyAppServerManagerResolverFromModule?.(...args);
  const appServerManagerRegistryResolverFromModule = (...args) => window.__codeyAppServerManagerRegistryResolverFromModule?.(...args);
  const appServerManagerFromReact = (...args) => window.__codeyAppServerManagerFromReact?.(...args);
  const remoteError = (code, message) => Object.assign(new Error(message), { code });
  const getCodexSessionController = async (feature = "remote") => {
    if (typeof window.__codeyLoadCodexSessionController !== "function") {
      throw remoteError("codey_remote_controller_not_ready", "桌面会话接口尚未就绪");
    }
    if (window.__codeyLoadCodexSessionController.remoteVersion !== 1) {
      throw remoteError("codey_remote_restart_required", "桌面会话工具版本已更新，请在本机通过 Codey 重启 Codex 后重新连接");
    }
    if (feature === "activity" && window.__codeyLoadCodexSessionController.activityVersion !== 1) {
      throw remoteError("codey_remote_unsupported", "桌面会话活动接口尚未就绪");
    }
    try {
      return await window.__codeyLoadCodexSessionController({ feature });
    } catch (error) {
      if (error?.code === "codey_capability_unavailable") {
        throw remoteError("codey_remote_unsupported", "当前 Codex 暂不支持远程读取会话，请更新 Codey 后重试");
      }
      throw error;
    }
  };
  const discoverCodexAppAssetUrls = async () => {
    const loaded = [...new Set([
      ...Array.from(document.scripts || []).map(script => script.src),
      ...Array.from(document.querySelectorAll("link[href]") || []).map(link => link.href),
      ...(typeof performance !== "undefined" && typeof performance.getEntriesByType === "function" ? performance.getEntriesByType("resource").map(entry => entry.name) : []),
    ].filter(url => url && url.includes("/assets/") && url.split("?")[0].endsWith(".js")))];
    const urls = loaded.filter(url => /app-(?:initial|shared|server-manager-signals)-/.test(url));
    for (const url of loaded.filter(url => !urls.includes(url)).slice(0, 6)) {
      if (disposed) break;
      try {
        const response = await window.fetch(url);
        if (!response?.ok) continue;
        for (const found of window.__codeyCodexAssetReferencesFromSource?.(await response.text(), url) || []) {
          if (!urls.includes(found)) urls.push(found);
        }
      } catch { continue; }
    }
    return urls;
  };
  const remoteCreationInputsFactoryFromModule = (module) => {
    const matches = [...new Set(Object.values(module || {}))].filter((value) => {
      if (typeof value !== "function" || value.length !== 2) return false;
      const source = Function.prototype.toString.call(value);
      return /\bscope\s*:/.test(source) && /\bhostId\s*:/.test(source)
        && /\brequestClient\s*:\s*[\w$]+\.get\(/.test(source)
        && /\bdynamicTools\s*:\s*new\b/.test(source);
    });
    return matches.length === 1 ? matches[0] : null;
  };

  const remoteModelSettingsOverride = (scope, cwd) => {
    if (typeof scope?.get !== "function") return null;
    // When startup overrides win config/batchWrite, the native composer keeps
    // its last selection in a signal family keyed by host and working directory.
    // Read only already-instantiated entries; do not mount unrelated signals.
    const nodes = new Set([scope.node, ...scope.chain?.values?.() || []]);
    for (const directory of new Set([cwd ?? null, null])) {
      const matches = new Map();
      for (const node of nodes) {
        for (const [family, bindings] of node?.familyBindings?.entries?.() || []) {
          if (family?.kind !== "signal-family" || !bindings?.has?.(`local:${directory ?? ""}`)) continue;
          try {
            const value = scope.get(family, { hostId: "local", cwd: directory });
            if (!value || typeof value.model !== "string" || !value.model.trim()
              || typeof value.reasoningEffort !== "string"
              || !(value.profile === null || typeof value.profile === "string")
              || Object.keys(value).some(key => !["model", "reasoningEffort", "profile"].includes(key))) continue;
            const settings = { model: value.model, effort: value.reasoningEffort };
            matches.set(JSON.stringify(settings), settings);
          } catch { continue; }
        }
      }
      if (matches.size > 1) throw new Error("桌面默认模型设置不明确，请在电脑端重新选择模型");
      if (matches.size === 1) return [...matches.values()][0];
    }
    return null;
  };

  const remoteCreationContext = async () => {
    const factories = new Set();
    const resolvers = new Set();
    const registryResolvers = new Set();
    const serviceTierReaders = new Set();
    for (const url of await discoverCodexAppAssetUrls()) {
      if (!url.includes("app-initial-") && !url.includes("app-shared-")) continue;
      try {
        const module = typeof window.__codeyImportCodexAsset === "function"
          ? await window.__codeyImportCodexAsset(url) : await import(url);
        const factory = remoteCreationInputsFactoryFromModule(module);
        const resolver = appServerManagerResolverFromModule(module);
        const registryResolver = appServerManagerRegistryResolverFromModule(module);
        const sourceResolver = await window.__codeyLoadAppServerManagerSourceResolver?.(url, module);
        if (factory) factories.add(factory);
        if (resolver) resolvers.add(resolver);
        if (registryResolver) registryResolvers.add(registryResolver);
        if (sourceResolver) registryResolvers.add(sourceResolver);
        for (const value of Object.values(module || {})) {
          if (typeof value !== "function") continue;
          const source = Function.prototype.toString.call(value);
          if (source.includes("Failed to read service tier for request") && source.includes("service_tier")) {
            serviceTierReaders.add(value);
          }
        }
      } catch {
        continue;
      }
    }
    // Newer Codex exports the creation reader from app-initial and the manager
    // resolver from app-shared. Resolve both against the same local scope.
    for (const resolver of [...resolvers, ...registryResolvers]) {
      for (const factory of factories) {
        let reader = null;
        const manager = appServerManagerFromReact((scope, hostId) => {
          const candidate = resolver(scope, hostId);
          if (typeof candidate?.startConversation !== "function") return null;
          const inputs = factory(scope, hostId);
          if (typeof inputs?.readCreationInputs !== "function") return null;
          reader = inputs;
          return candidate;
        });
        if (manager && reader) return { manager, reader, readServiceTier: serviceTierReaders.size === 1 ? [...serviceTierReaders][0] : null };
      }
    }
    throw new Error("当前 Codex 暂不支持远程新建会话，请更新 Codey 后重试");
  };

  const goalStatuses = new Set(["active", "paused", "blocked", "usageLimited", "budgetLimited", "complete"]);
  const goalView = value => {
    if (value === null) return null;
    const status = { usage_limited: "usageLimited", budget_limited: "budgetLimited" }[value?.status] || value?.status;
    if (typeof value?.objective !== "string" || !value.objective.trim() || value.objective.length > 20_000
      || !goalStatuses.has(status)) throw new Error("invalid goal");
    const goal = { objective: value.objective, status };
    if (value.goalId != null) {
      if (typeof value.goalId !== "string" || !value.goalId || value.goalId.length > 512) throw new Error("invalid goal identity");
      goal.goalId = value.goalId;
    }
    for (const key of ["createdAt", "updatedAt", "tokenBudget", "tokensUsed", "timeUsedSeconds"]) {
      const millis = key.endsWith("At") ? value[`${key}Ms`] : undefined;
      if (millis != null && (!Number.isSafeInteger(millis) || millis < 0)) throw new Error("invalid goal time");
      const number = value[key] ?? (millis != null ? Math.floor(millis / 1000) : undefined);
      if (number == null) continue;
      if (!Number.isSafeInteger(number) || number < 0) throw new Error("invalid goal number");
      goal[key] = number;
    }
    return goal;
  };
  const sameGoal = (current, expected) => current === null ? expected === null
    : expected !== null && current.objective === expected.objective && current.status === expected.status
      && (current.goalId ? current.goalId === expected.goalId : current.createdAt != null && current.createdAt === expected.createdAt);
  const goalFailure = (code, message, availability = "unavailable", activation = "not-requested") =>
    ({ status: "failed", code, message, availability, activation });
  const goalCall = async call => {
    const pending = call();
    try { return await pending; }
    finally { try { pending?.[Symbol.dispose]?.(); } catch {} }
  };
  const remoteGoal = async (args, assertCurrent, ensureRequestGate) => {
    let manager, current;
    const readGoal = async () => {
      const result = await goalCall(() => manager.sendRequest("thread/goal/get", { threadId: args.threadId }));
      assertCurrent();
      if (!result || !Object.hasOwn(result, "goal")) throw new Error("missing goal result");
      if (result.goal !== null) return goalView(result.goal);
      const conversation = await goalCall(() => manager.getConversation(args.threadId));
      assertCurrent();
      if (conversation?.id !== args.threadId) throw new Error("conversation unavailable");
      return conversation.completedThreadGoal?.status === "complete" ? goalView(conversation.completedThreadGoal) : null;
    };
    try {
      ({ manager } = await remoteCreationContext());
      assertCurrent();
      current = await readGoal();
    } catch (error) {
      const unsupported = error?.code === -32601 || error?.code === "method_not_found";
      return goalFailure(unsupported ? "goal_unsupported" : "goal_unavailable", "原生目标暂不可读取，请在电脑端检查", unsupported ? "unsupported" : "unavailable");
    }
    if (args.action === "goalRead") return { status: "ok", availability: "ready", goal: current, activation: "not-requested" };
    let expected;
    try { expected = goalView(args.expected); } catch { return goalFailure("goal_invalid", "目标身份无效"); }
    if (!sameGoal(current, expected)) return goalFailure("goal_conflict", "目标已变化，请刷新后重试", "ready");
    const allowed = args.action === "goalCreate" ? current === null || current.status === "complete"
      : args.action === "goalPause" ? current && ["active", "blocked", "usageLimited", "budgetLimited"].includes(current.status)
        : ["goalResume", "goalEdit"].includes(args.action) && current?.status === "paused";
    if (!allowed) return goalFailure("goal_conflict", "目标当前状态不支持此操作", "ready");
    const activating = ["goalCreate", "goalResume"].includes(args.action);
    try {
      // Hosted managers expose async RPC stubs for every property; await real owner evidence.
      const role = await goalCall(() => manager.getStreamRole(args.threadId));
      assertCurrent();
      if (role?.role !== "owner") return goalFailure("goal_unavailable", "请在会话所属的 Codex 窗口重新连接");
      if (activating) await ensureRequestGate();
      assertCurrent();
      if (activating) {
        const conversation = await goalCall(() => manager.getConversation(args.threadId));
        assertCurrent();
        const history = conversation?.turnHistory?.kind === "canonical" ? conversation.turnHistory.history : conversation?.turns;
        const turns = Array.isArray(history) ? history : Array.isArray(history?.islands) && history?.entitiesByKey
          ? history.islands.flatMap(island => island.entries || []).map(entry => history.entitiesByKey[entry.value]) : history?.items;
        if (conversation?.id !== args.threadId || !Array.isArray(turns)) return goalFailure("goal_unavailable", "尚未确认当前回复状态，请重新连接");
        if (turns.some(turn => turn?.status === "inProgress")) return goalFailure("goal_busy", "当前回复仍在执行，请等待完成后新建或继续目标", "ready");
      }
      current = await readGoal();
      if (!sameGoal(current, expected)) return goalFailure("goal_conflict", "目标已变化，请刷新后重试", "ready");
    } catch { return goalFailure("goal_unavailable", "目标操作校验未就绪，未发送操作"); }
    try {
      let result;
      if (args.action === "goalCreate" && args.tokenBudget === undefined) {
        result = await goalCall(() => manager.setThreadGoal(args.threadId, { objective: args.objective, appendTranscriptItem: false }));
      } else if (["goalPause", "goalResume"].includes(args.action)) {
        result = await goalCall(() => manager.setThreadGoalStatus(args.threadId, { status: activating ? "active" : "paused" }));
      } else {
        const params = { threadId: args.threadId, objective: args.objective, status: activating ? "active" : "paused" };
        if (Object.hasOwn(args, "tokenBudget")) params.tokenBudget = args.tokenBudget;
        else if (current?.tokenBudget != null) params.tokenBudget = current.tokenBudget;
        const response = await goalCall(() => manager.sendRequest("thread/goal/set", params));
        result = response?.goal;
      }
      assertCurrent();
      const goal = goalView(result);
      if (!goal || goal.status !== (activating ? "active" : "paused")
        || goal.objective !== (args.objective ?? current.objective)) throw new Error("unconfirmed goal");
      return { status: "ok", availability: "ready", goal, activation: "not-requested" };
    } catch { return goalFailure("goal_outcome_unknown", "目标操作结果未确认，请刷新查看，勿重复提交", "unavailable", "unknown"); }
  };

  // Keep creation and resumption inside the desktop's native manager. In
  // particular, its creation-input reader installs the same tools, project
  // instructions and permission defaults as the desktop composer.
  const remoteControl = async (args, expiresAt) => {
    const requestGateMessage = "模型线路尚未就绪，远程操作未发送，请稍后重试";
    let requestPatch = null;
    const assertCurrent = () => {
      if (disposed || Date.now() >= expiresAt) throw new Error("远程操作已过期，请检查会话状态");
      if (requestPatch && (window.__codeyModelWhitelistPatch !== requestPatch
        || requestPatch.requestGateReady?.() !== true)) {
        throw remoteError("codey_remote_request_gate_not_ready", requestGateMessage);
      }
    };
    const ensureRequestGate = async () => {
      const patch = window.__codeyModelWhitelistPatch;
      let ready = false;
      let timer;
      try {
        ready = typeof patch?.ensureRequestGate === "function"
          && await Promise.race([
            Promise.resolve().then(() => patch.ensureRequestGate()),
            new Promise(resolve => { timer = window.setTimeout(() => resolve(false), 2_000); }),
          ]) === true
          && patch.requestGateReady?.() === true
          && window.__codeyModelWhitelistPatch === patch;
      } catch {} finally { window.clearTimeout(timer); }
      assertCurrent();
      if (!ready) {
        if (window.__codeyModelWhitelistPatch === patch) patch?.notifyRequestGateUnavailable?.();
        throw remoteError("codey_remote_request_gate_not_ready", requestGateMessage);
      }
      requestPatch = patch;
    };
    assertCurrent();
    if (["goalRead", "goalCreate", "goalPause", "goalResume", "goalEdit"].includes(args.action)) {
      return remoteGoal(args, assertCurrent, ensureRequestGate);
    }
    if (args.action === "activity") {
      if (!Array.isArray(args.threadIds) || args.threadIds.length > 200
        || args.threadIds.some(id => typeof id !== "string" || !id)) throw new Error("会话活动请求无效");
      const threadIds = [...new Set(args.threadIds)];
      const unknown = { running: Object.fromEntries(threadIds.map(id => [id, null])) };
      try {
        const controller = await getCodexSessionController("activity");
        assertCurrent();
        return await controller.readThreadActivity(threadIds, expiresAt);
      } catch (error) {
        const reason = error?.code === "codey_remote_disconnected" ? "disconnected"
          : error?.code === "codey_remote_unsupported" || error?.code === "codey_remote_controller_not_ready"
            || error?.code === "codey_remote_restart_required" ? "unavailable" : "request_failed";
        return { ...unknown, reason };
      }
    }
    if (args.action === "resume") {
      const controller = await getCodexSessionController();
      const manager = controller.manager;
      if (typeof manager?.resumeConversation !== "function") {
        throw remoteError("codey_remote_unsupported", "当前 Codex 暂不支持远程读取会话，请更新 Codey 后重试");
      }
      assertCurrent();
      // resumeConversation also discovers an owner in another window and
      // follows it. Never steal ownership with a second app-server process.
      await ensureRequestGate();
      assertCurrent();
      const result = await manager.resumeConversation({
        conversationId: args.threadId, model: null, serviceTier: null,
        reasoningEffort: null, workspaceRoots: [], collaborationMode: null,
      });
      if (result?.status !== "ready") {
        throw remoteError("codey_remote_not_ready", "桌面尚未就绪，暂时无法读取此会话");
      }
      return { status: "ok" };
    }
    if (!["create", "defaults", "attachments", "discard-attachments"].includes(args.action)) throw new Error("不支持的远程会话操作");
    const { manager, reader, readServiceTier } = await remoteCreationContext();
    assertCurrent();
    const discardAttachments = async (files) => {
      const valid = files.filter(file => typeof file?.path === "string" && file.path);
      const results = await Promise.allSettled(valid.map(file => Promise.resolve().then(() => manager.removePastedTextAttachment(file.path))));
      if (valid.length !== files.length || results.some(result => result.status === "rejected")) throw new Error("附件清理未确认");
    };
    if (args.action === "discard-attachments") {
      if (!Array.isArray(args.files) || !args.files.length || args.files.length > 4
        || typeof manager.removePastedTextAttachment !== "function") throw new Error("桌面附件清理接口不可用");
      await discardAttachments(args.files);
      return { status: "ok" };
    }
    if (args.action === "attachments") {
      if (!Array.isArray(args.files) || !args.files.length || args.files.length > 4
        || typeof manager.createFileAttachment !== "function" || typeof manager.removePastedTextAttachment !== "function") throw new Error("桌面附件接口不可用");
      await ensureRequestGate();
      const created = [];
      try {
        for (const file of args.files) {
          assertCurrent();
          if (typeof file?.name !== "string" || typeof file?.url !== "string" || !file.url.startsWith("data:application/octet-stream;base64,")) throw new Error("文件数据无效");
          const bytes = Uint8Array.from(atob(file.url.slice("data:application/octet-stream;base64,".length)), char => char.charCodeAt(0));
          const attachment = await manager.createFileAttachment({ contents: new Blob([bytes]), label: file.name });
          created.push(attachment);
          if (attachment?.label !== file.name || typeof attachment?.path !== "string" || !attachment.path
            || attachment.fsPath !== attachment.path) throw new Error("桌面附件创建结果无效");
          assertCurrent();
        }
        return { files: created };
      } catch (error) {
        await discardAttachments(created);
        throw error;
      }
    }
    if (args.action === "defaults") {
      if (typeof manager.sendRequest !== "function") throw new Error("当前 Codex 暂不支持读取默认设置");
      const { config } = await manager.sendRequest("config/read", { includeLayers: false, cwd: args.cwd });
      assertCurrent();
      if (!config || typeof config !== "object") throw new Error("桌面默认设置格式不兼容");
      const override = remoteModelSettingsOverride(reader.params?.scope, args.cwd);
      const model = override?.model ?? config.model;
      const effort = override?.effort ?? config.model_reasoning_effort;
      const tier = readServiceTier && reader.params?.scope
        ? await readServiceTier(reader.params.scope, "local", model) : config.service_tier;
      assertCurrent();
      // Only expose composer settings: config/read may also contain credentials.
      return {
        ...(typeof model === "string" ? { model } : {}),
        ...(typeof effort === "string" ? { effort } : {}),
        serviceTier: tier === "priority" ? "priority" : "default",
      };
    }
    await ensureRequestGate();
    assertCurrent();
    const result = await manager.startConversation(args.params, {
      readThreadCreationInputs: async (...values) => {
        assertCurrent();
        const inputs = await reader.readCreationInputs(...values);
        assertCurrent();
        return inputs;
      },
    });
    if (result?.status !== "created" || !result.conversationId) {
      throw new Error("新建结果尚未确认，请先检查会话列表，勿重复发送");
    }
    return {
      id: result.conversationId,
      firstTurn: result.firstTurn?.status || "outcome-unknown",
    };
  };

  remoteControl.version = version;
  remoteControl.dispose = () => {
    disposed = true;
    if (window.__codeyRemoteControl === remoteControl) delete window.__codeyRemoteControl;
  };
  window.__codeyRemoteControl = remoteControl;
})();
