// Remote-only native-manager adapter. Existing session/model hooks retain ownership.
(() => {
  const version = 3;
  if (window.__codeyRemoteControl?.version === version) return;
  window.__codeyRemoteControl?.dispose?.();
  let disposed = false;
  const appServerManagerResolverFromModule = (...args) => window.__codeyAppServerManagerResolverFromModule?.(...args);
  const appServerManagerRegistryResolverFromModule = (...args) => window.__codeyAppServerManagerRegistryResolverFromModule?.(...args);
  const appServerManagerFromReact = (...args) => window.__codeyAppServerManagerFromReact?.(...args);
  const remoteError = (code, message) => Object.assign(new Error(message), { code });
  const getCodexSessionController = async () => {
    if (typeof window.__codeyLoadCodexSessionController !== "function") {
      throw remoteError("codey_remote_controller_not_ready", "桌面会话接口尚未就绪");
    }
    if (window.__codeyLoadCodexSessionController.remoteVersion !== 1) {
      throw remoteError("codey_remote_restart_required", "桌面会话工具版本已更新，请在本机通过 Codey 重启 Codex 后重新连接");
    }
    try {
      return await window.__codeyLoadCodexSessionController({ feature: "remote" });
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
        if (factory) factories.add(factory);
        if (resolver) resolvers.add(resolver);
        if (registryResolver) registryResolvers.add(registryResolver);
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
    if (args.action !== "create" && args.action !== "defaults") throw new Error("不支持的远程会话操作");
    const { manager, reader, readServiceTier } = await remoteCreationContext();
    assertCurrent();
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
