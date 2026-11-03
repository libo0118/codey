import { useEffect, useState } from "react";
import { errorText } from "../../appUtils";
import { useOperationScope } from "../../useOperationScope";
import { isPonytailMode, type PonytailMode, type PonytailStatus } from "./ponytail";
import { ponytailRequests } from "./requests";

type Draft = { mode: PonytailMode; savedMode: PonytailMode; revision: string };
const draftFrom = (status: PonytailStatus): Draft => ({ mode: status.settings.defaultMode, savedMode: status.settings.defaultMode, revision: status.settings.revision });

export function usePonytailController({ active, isBusy }: { active: boolean; isBusy: boolean }) {
  const [status, setStatus] = useState<PonytailStatus | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [readError, setReadError] = useState("");
  const [notice, setNotice] = useState("");
  const [revision, setRevision] = useState(0);
  const scope = useOperationScope(active);
  const adopt = (next: PonytailStatus, saved = false) => {
    setStatus(next);
    setDraft(previous => saved || !previous || previous.mode === previous.savedMode ? draftFrom(next) : previous);
  };
  useEffect(() => {
    if (!active || scope.pending) return;
    let cancelled = false;
    let timer: number | undefined;
    const valid = scope.read();
    const current = () => !cancelled && valid();
    async function poll() {
      if (!current()) return;
      try {
        const next = await ponytailRequests.status();
        if (!current()) return;
        adopt(next); setReadError("");
        timer = window.setTimeout(() => void poll(), next.job?.phase === "running" ? 1500 : 5000);
      } catch (cause) { if (current()) setReadError(errorText(cause)); }
    }
    void poll();
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [active, busy, revision, scope]);

  const running = status?.job?.phase === "running";
  const blocked = !active || busy || isBusy || running || !status || Boolean(readError);
  const dirty = Boolean(draft && draft.mode !== draft.savedMode);
  async function mutate(request: () => Promise<PonytailStatus>, saved = false) {
    if (blocked) return;
    const operation = scope.begin();
    if (!operation) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const next = await request();
      if (operation.current()) { adopt(next, saved); setNotice(next.message ?? ""); }
    } catch (cause) { if (operation.current()) setError(errorText(cause)); }
    finally { if (operation.finish()) { setBusy(false); setRevision(value => value + 1); } }
  }
  function setEnabled(enabled: boolean) {
    if (status?.conflict) return Promise.resolve();
    return mutate(() => ponytailRequests.setEnabled(enabled));
  }
  function save() {
    if (!draft || !dirty) return Promise.resolve();
    return mutate(() => ponytailRequests.save(draft.mode, draft.revision), true);
  }
  function changeMode(value: unknown) {
    if (!blocked && !scope.pending && isPonytailMode(value)) setDraft(previous => previous && ({ ...previous, mode: value }));
  }
  function reset() { if (!blocked && !scope.pending && status) setDraft(draftFrom(status)); }
  function refresh() {
    if (!active || scope.pending || isBusy) return;
    scope.invalidateReads(); setReadError(""); setError(""); setRevision(value => value + 1);
  }
  return { status, mode: draft?.mode ?? "full", dirty, busy, running, blocked, error: error || readError,
    notice, setEnabled, save, changeMode, reset, refresh };
}
