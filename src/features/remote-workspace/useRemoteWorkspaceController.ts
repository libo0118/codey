import { useEffect, useRef, useState } from "react";
import { errorText } from "../../appUtils";
import { useOperationScope } from "../../useOperationScope";
import { useStableEvent } from "../../useStableEvent";
import { defaultThreadSettings, modelOptions, projectForThread } from "../../remote/workspace-data";
import type { ModelCatalog, Project, Thread, ThreadSettings, View } from "../../remote/workspace-data";
import type { DraftImage } from "../../remote/images";
import { remoteWorkspaceRequests as requests } from "./requests";
import { canSendDraft, firstTurnDraft, restoreWorkspaceDraft } from "./state";
import type { WorkspaceDraft } from "./types";
import { useRemoteConversationController } from "./useRemoteConversationController";

export function useRemoteWorkspaceController({ active, onUnauthorized }: { active: boolean; onUnauthorized: () => void }) {
  const [threads, setThreads] = useState<Thread[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [selected, setSelected] = useState<Thread | null>(null);
  const [search, setSearch] = useState("");
  const [archived, setArchived] = useState(false);
  const [project, setProject] = useState("");
  const [creating, setCreating] = useState(false);
  const [editor, setEditor] = useState<WorkspaceDraft>(() => restoreWorkspaceDraft(undefined, null));
  const [creationDefaults, setCreationDefaults] = useState<{ project: string; settings: ThreadSettings } | null>(null);
  const [defaultsError, setDefaultsError] = useState("");
  const [defaultsRevision, setDefaultsRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [catalog, setCatalog] = useState<ModelCatalog>({});
  const [catalogError, setCatalogError] = useState("");
  const [loading, setLoading] = useState(true);
  const [listRevision, setListRevision] = useState(0);
  const scope = useOperationScope(active);
  const session = useRef<object | null>(null);
  const listVersion = useRef(0);
  const modelVersion = useRef(0);
  const drafts = useRef(new Map<string, WorkspaceDraft>());
  const selectedId = selected?.id;
  const draftScope = creating ? `create:${project}` : selectedId;
  const draftProject = projects.find(item => item.id === project);
  const defaultsReady = creationDefaults?.project === project;
  const composerSettings = { ...defaultThreadSettings(catalog, defaultsReady ? creationDefaults.settings : {}), ...editor.settings };
  const { view, connected, connectionError, reconnect } = useRemoteConversationController(active, selectedId, onUnauthorized);
  const composerView: View | null = creating ? { id: "", turns: [], requests: [], historyComplete: true, ...composerSettings } : view;
  const activeTurn = view ? [...view.turns].reverse().find(turn => turn.status === "inProgress") : undefined;

  useEffect(() => {
    session.current = active ? {} : null;
    return () => { session.current = null; listVersion.current++; modelVersion.current++; };
  }, [active]);
  useEffect(() => { if (draftScope) drafts.current.set(draftScope, editor); }, [draftScope, editor]);

  const fail = useStableEvent((cause: unknown) => {
    if (requests.unauthorized(cause)) onUnauthorized();
    setError(errorText(cause));
  });
  const refreshList = useStableEvent(async () => {
    const token = session.current;
    if (!token) return;
    const version = ++listVersion.current;
    const current = () => session.current === token && version === listVersion.current;
    setLoading(true);
    try {
      const [nextThreads, nextProjects] = await Promise.all([requests.threads(search, archived), requests.projects()]);
      if (!current()) return;
      setThreads(nextThreads); setProjects(nextProjects);
      // 刷新目录不能把正在编辑的新会话草稿改绑到另一个项目。
      if (!creating) setProject(previous => nextProjects.some(item => item.id === previous) ? previous : nextProjects[0]?.id || "");
    } catch (cause) { if (current()) fail(cause); }
    finally { if (current()) setLoading(false); }
  });
  useEffect(() => {
    if (!active) return;
    const timer = window.setTimeout(() => void refreshList(), search ? 180 : 0);
    return () => { window.clearTimeout(timer); listVersion.current++; };
  }, [active, search, archived, listRevision, refreshList]);

  const loadModels = useStableEvent(async () => {
    const token = session.current;
    if (!token) return;
    const version = ++modelVersion.current;
    const current = () => session.current === token && version === modelVersion.current;
    try {
      const next = await requests.models();
      if (current()) { setCatalog(next); setCatalogError(""); }
    } catch (cause) {
      if (!current()) return;
      if (requests.unauthorized(cause)) onUnauthorized();
      setCatalogError("模型列表暂时无法加载，仍沿用桌面设置。");
    }
  });
  useEffect(() => { if (active) void loadModels(); }, [active, loadModels]);

  useEffect(() => {
    if (!active || !creating || !project) return;
    let disposed = false;
    setCreationDefaults(null); setDefaultsError("");
    void requests.defaults(project).then(settings => {
      if (!disposed) setCreationDefaults({ project, settings });
    }).catch(cause => {
      if (disposed) return;
      if (requests.unauthorized(cause)) onUnauthorized();
      setDefaultsError("桌面默认设置暂时无法加载，请重试。");
    });
    return () => { disposed = true; };
  }, [active, creating, project, defaultsRevision]);

  function restoreDraft(key: string) { setEditor(restoreWorkspaceDraft(drafts.current.get(key), requests.readSubmission(key))); }
  function selectThread(thread: Thread) {
    setCreating(false); setSelected(thread); setError(""); restoreDraft(thread.id);
  }
  const open = (thread: Thread) => {
    if (!session.current || scope.pending || selectedId === thread.id) return;
    selectThread(thread);
  };
  function startCreating(target?: Project) {
    if (!session.current || scope.pending) return;
    const next = target?.id || project;
    setProject(next); setCreating(true); setCreationDefaults(null); setDefaultsRevision(value => value + 1);
    setSelected(null); setError(""); restoreDraft(`create:${next}`);
  }

  async function run(operation: (current: () => boolean) => Promise<void>) {
    const pending = scope.begin();
    if (!pending) return false;
    listVersion.current++;
    setBusy(true); setError("");
    try { await operation(pending.current); }
    catch (cause) { if (pending.current()) fail(cause); }
    finally {
      if (pending.finish()) { setBusy(false); setListRevision(value => value + 1); }
    }
    return true;
  }

  async function send() {
    if (!session.current || scope.pending || !canSendDraft(editor) || !draftScope || requests.readSubmission(draftScope)) return false;
    if (creating) {
      if (!draftProject || !defaultsReady) return false;
      const key = draftScope;
      return run(async current => {
        const sent = { ...editor, settings: composerSettings, uncertain: true };
        setEditor(sent);
        const submission = requests.rememberSubmission(key, { projectId: project, text: sent.text, images: sent.images, ...composerSettings });
        const result = await requests.create(submission);
        if (!result.id) throw new Error("新建结果尚未确认，请先检查会话列表");
        if (!current()) return;
        requests.forgetSubmission(key); drafts.current.delete(key);
        selectThread({ id: result.id, title: "新会话", cwd: draftProject.cwd, updatedAt: Date.now() });
        const next = firstTurnDraft(sent, result.firstTurn);
        setEditor(next);
        if (next.uncertain) requests.rememberSubmission(result.id, { action: "send", text: sent.text, images: sent.images });
        if (result.firstTurn !== "accepted") setError("会话已创建，请检查第一条消息是否已发送。");
      });
    }
    if (!selectedId || !connected) return false;
    return run(async current => {
      setEditor(previous => ({ ...previous, uncertain: true }));
      const submission = requests.rememberSubmission(selectedId, { action: activeTurn ? "steer" : "send", text: editor.text, images: editor.images });
      await requests.submit(selectedId, submission);
      if (!current()) return;
      requests.forgetSubmission(selectedId);
      setEditor(previous => ({ ...previous, text: "", images: [], uncertain: false }));
    });
  }

  function perform(action: string, args: Record<string, unknown> = {}, reload = false) {
    if (!session.current || !selectedId || !connected) return Promise.resolve(false);
    return run(async current => {
      await requests.action(selectedId, action, args);
      if (current() && reload) reconnect();
    });
  }
  function changeSettings(settings: ThreadSettings) {
    if (!session.current || scope.pending) return;
    if (creating) setEditor(previous => ({ ...previous, settings: { ...composerSettings, ...settings } }));
    else void perform("settings", settings, true);
  }
  function confirmSubmission() {
    if (!session.current || scope.pending || !draftScope) return;
    requests.forgetSubmission(draftScope); setEditor(previous => ({ ...previous, uncertain: false })); setError("");
  }

  return {
    threads, projects, selected, selectedId, selectedProject: selected ? projectForThread(selected, projects) : undefined,
    search, archived, project, creating, draftScope, draftProject, defaultsReady, composerView,
    view, connected, connectionError, activeTurn, models: modelOptions(catalog), busy, loading, error,
    draft: editor.text, images: editor.images, uncertain: editor.uncertain,
    catalogError: [catalogError, creating ? defaultsError : ""].filter(Boolean).join(" "),
    open, startCreating, send, changeSettings, confirmSubmission,
    changeDraft: (text: string) => { if (session.current && !scope.pending) setEditor(previous => ({ ...previous, text })); },
    changeImages: (images: DraftImage[]) => { if (session.current && !scope.pending && !editor.uncertain) setEditor(previous => ({ ...previous, images })); },
    changeSearch: (value: string) => { if (session.current) { listVersion.current++; setSearch(value); } },
    changeArchived: (value: boolean) => { if (session.current) { listVersion.current++; setArchived(value); } },
    refresh: () => { void refreshList(); void loadModels(); },
    back: () => { if (session.current && !scope.pending) { setSelected(null); setCreating(false); void refreshList(); } },
    reconnect: () => { if (session.current && !scope.pending) reconnect(); },
    loadHistory: () => perform("history", {}, true),
    respond: (approvalId: string | number, args: Record<string, unknown>) => perform("respond", { ...args, approvalId }),
    stop: () => activeTurn ? perform("interrupt", { turnId: activeTurn.id }) : Promise.resolve(false),
    reloadModels: () => { if (session.current) { void loadModels(); setDefaultsRevision(value => value + 1); } },
  };
}
