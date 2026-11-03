import { useEffect, useRef, useState } from "react";
import { errorText } from "../../appUtils";
import { useOperationScope } from "../../useOperationScope";
import { EMPTY_REMOTE, remoteDraft, remoteSettings } from "./state";
import { remoteControlRequests as requests } from "./requests";
import type { RemoteDraft, RemoteStatus } from "./types";

export function useRemoteControlController({ active, isBusy = false }: { active: boolean; isBusy?: boolean }) {
  const local = window.__codeyRemoteMode !== true;
  const scope = useOperationScope(active && local);
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [draft, setDraft] = useState<RemoteDraft>(EMPTY_REMOTE);
  const edited = useRef(false);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [confirmation, setConfirmation] = useState<"save" | "revoke-all" | null>(null);
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    setConfirmation(null);
    if (!active || !local || scope.pending) return;
    let cancelled = false;
    let timer: number | undefined;
    const valid = scope.read();
    const current = () => !cancelled && valid();
    async function read() {
      try {
        const next = await requests.status();
        if (!current()) return;
        setStatus(next); setError("");
        if (!edited.current) setDraft(remoteDraft(next));
        timer = window.setTimeout(() => void read(), 5000);
      } catch (cause) { if (current()) setError(errorText(cause)); }
    }
    void read();
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [active, local, busy, revision, scope]);

  const blocked = !active || !local || isBusy || !!busy || scope.pending || !status;
  const dirty = Boolean(status && (draft.username.trim() !== (status.username ?? "") || draft.port !== String(status.port)
    || draft.publicUrl.trim() !== status.publicUrl || draft.password));

  async function mutate(label: string, action: () => Promise<RemoteStatus>, saved = false) {
    if (blocked) return;
    const operation = scope.begin();
    if (!operation) return;
    setBusy(label); setError(""); setNotice("");
    let succeeded = false;
    try {
      const next = await action(); succeeded = true;
      if (operation.current()) {
        setStatus(next);
        if (saved) { edited.current = false; setDraft(remoteDraft(next)); }
        setNotice(label === "保存设置" ? "设置已保存。" : label === "启动服务" ? "远程服务已启动。" : label === "停止服务" ? "远程服务已停止。" : "登录会话已撤销。");
      }
    } catch (cause) { if (operation.current()) setError(errorText(cause)); }
    finally {
      if (operation.finish()) {
        if (saved && succeeded) setDraft(previous => ({ ...previous, password: "" }));
        setConfirmation(null); setBusy(""); setRevision(value => value + 1);
      }
    }
  }

  return {
    status, draft, busy, error, notice, dirty, blocked, confirmation, local,
    change<K extends keyof RemoteDraft>(key: K, value: RemoteDraft[K]) {
      if (blocked || status?.running) return;
      edited.current = true; setDraft(previous => ({ ...previous, [key]: value })); setConfirmation(null);
    },
    askSave() {
      if (blocked || status?.running || !dirty) return;
      try { remoteSettings(draft, Boolean(status?.accountConfigured)); setError(""); setConfirmation("save"); }
      catch (cause) { setError(errorText(cause)); }
    },
    askRevokeAll() { if (!blocked && status?.sessions.length) setConfirmation("revoke-all"); },
    cancelConfirmation() { if (!scope.pending) setConfirmation(null); },
    confirm() {
      if (confirmation === "save" && status) return mutate("保存设置", () => requests.save(remoteSettings(draft, status.accountConfigured), status.revision), true);
      if (confirmation === "revoke-all") return mutate("撤销登录", () => requests.revoke("*"));
      return Promise.resolve();
    },
    start: () => !dirty && !status?.running && status?.accountConfigured ? mutate("启动服务", requests.start) : Promise.resolve(),
    stop: () => status?.running ? mutate("停止服务", requests.stop) : Promise.resolve(),
    revoke: (id: string) => mutate("撤销登录", () => requests.revoke(id)),
    refresh() { if (!blocked) { scope.invalidateReads(); setRevision(value => value + 1); } },
  };
}
