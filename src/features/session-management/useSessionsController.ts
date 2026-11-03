import { useEffect, useRef, useState } from "react";
import { errorText } from "../../appUtils";
import { useOperationScope } from "../../useOperationScope";
import { sessionRequests } from "./requests";
import { filterSessions, PAGE_SIZE, parseRetentionDays } from "./state";
import type { CleanupConfirmation, CleanupPolicy, CleanupPolicyDraft, CleanupResult, SessionFilter, SessionInventory } from "./types";

export function useSessionsController({ active, isBusy, onRestart }: {
  active: boolean;
  isBusy: boolean;
  onRestart: () => void;
}) {
  const [inventory, setInventory] = useState<SessionInventory | null>(null);
  const [busy, setBusy] = useState<"" | "list" | "delete" | "policy">("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [result, setResult] = useState<CleanupResult | null>(null);
  const [policyDraft, setPolicyDraft] = useState<CleanupPolicyDraft>({ enabled: false, retentionDays: "30" });
  const draftEdited = useRef(false);
  const [policyConfirmation, setPolicyConfirmation] = useState<{ policy: CleanupPolicy; revision: string } | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<SessionFilter>("all");
  const [olderOnly, setOlderOnly] = useState(false);
  const [now, setNow] = useState(Date.now);
  const [page, setPage] = useState(0);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [legacySelected, setLegacySelected] = useState<Set<string>>(new Set());
  const [confirmation, setConfirmation] = useState<CleanupConfirmation | null>(null);
  const [revision, setRevision] = useState(0);
  const scope = useOperationScope(active);

  useEffect(() => {
    setConfirmation(null);
    setPolicyConfirmation(null);
    setSelected(new Set());
    setLegacySelected(new Set());
    setInventory(null);
    setError("");
    setNotice("");
    setResult(null);
  }, [active]);

  useEffect(() => {
    if (!active || scope.pending) return;
    scope.invalidateReads();
    const valid = scope.read();
    let cancelled = false;
    const current = () => !cancelled && valid();
    setBusy("list");
    void (async () => {
      try {
        await sessionRequests.waitForPending();
        if (!current()) return;
        const next = await sessionRequests.list();
        if (current()) {
          setInventory(next); setPage(0); setNow(Date.now());
          setPolicyDraft(previous => {
            if (draftEdited.current && (previous.enabled !== next.cleanupPolicy.enabled
              || parseRetentionDays(previous.retentionDays) !== next.cleanupPolicy.retentionDays)) return previous;
            draftEdited.current = false;
            return { enabled: next.cleanupPolicy.enabled, retentionDays: String(next.cleanupPolicy.retentionDays) };
          });
        }
      } catch (cause) { if (current()) setError(errorText(cause)); }
      finally { if (current()) setBusy(""); }
    })();
    return () => { cancelled = true; };
  }, [active, revision, scope]);

  const blocked = !active || isBusy || !!busy || scope.pending || !inventory;
  const retentionDays = parseRetentionDays(policyDraft.retentionDays);
  const policyProblem = retentionDays === null ? "保留天数须为 1 至 36500 的整数。" : "";
  const policyDirty = !!inventory && (policyDraft.enabled !== inventory.cleanupPolicy.enabled
    || retentionDays !== inventory.cleanupPolicy.retentionDays);
  const filtered = olderOnly && retentionDays === null ? []
    : filterSessions(inventory?.sessions ?? [], query, filter, olderOnly ? retentionDays : null, now);
  const pages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const visible = filtered.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);
  const legacyPendingIds = inventory?.legacyPendingIds ?? [];

  function refresh() {
    if (!active || scope.pending || isBusy) return;
    scope.invalidateReads();
    setInventory(null); setSelected(new Set()); setLegacySelected(new Set()); setConfirmation(null); setPolicyConfirmation(null); setError("");
    setRevision(value => value + 1);
  }
  function changeQuery(value: string) { if (!scope.pending) { setQuery(value); setPage(0); setSelected(new Set()); setConfirmation(null); } }
  function changeFilter(value: SessionFilter) { if (!scope.pending) { setFilter(value); setPage(0); setSelected(new Set()); setConfirmation(null); } }
  function changeOlderOnly(value: boolean) { if (!scope.pending) { setOlderOnly(value); setPage(0); setSelected(new Set()); setConfirmation(null); } }
  function changePolicy<K extends keyof CleanupPolicyDraft>(key: K, value: CleanupPolicyDraft[K]) {
    if (blocked) return;
    draftEdited.current = true;
    setPolicyDraft(previous => ({ ...previous, [key]: value }));
    setPolicyConfirmation(null); setConfirmation(null); setSelected(new Set()); setPage(0); setNotice("");
  }
  function toggle(id: string) {
    if (blocked || !inventory?.sessions.some(session => session.id === id)) return;
    setSelected(previous => { const next = new Set(previous); if (next.has(id)) next.delete(id); else next.add(id); return next; });
  }
  function toggleVisible() {
    if (blocked) return;
    const allSelected = visible.every(session => selected.has(session.id));
    setSelected(previous => {
      const next = new Set(previous);
      visible.forEach(session => { if (allSelected) next.delete(session.id); else next.add(session.id); });
      return next;
    });
  }
  function askRemove(all: boolean) {
    if (blocked || !inventory || scope.pending) return;
    const entries = inventory.sessions.filter(session => all || selected.has(session.id));
    if (!entries.length) return;
    setConfirmation({ ids: entries.map(session => session.id), revision: inventory.revision,
      archivedCount: entries.filter(session => session.archived).length, all });
  }
  function toggleLegacy(id: string) {
    if (blocked || !legacyPendingIds.includes(id)) return;
    setLegacySelected(previous => { const next = new Set(previous); if (next.has(id)) next.delete(id); else next.add(id); return next; });
  }
  function reconfirmLegacy(ids: string[]) {
    if (blocked || !inventory) return;
    const requested = new Set(ids);
    const pendingIds = legacyPendingIds.filter(id => requested.has(id));
    if (!pendingIds.length || pendingIds.length !== requested.size) return;
    const entries = inventory.sessions.filter(session => requested.has(session.id));
    setPolicyConfirmation(null);
    setConfirmation({ ids: pendingIds, revision: inventory.revision, all: false,
      archivedCount: entries.filter(session => session.archived).length, legacyMissingCount: pendingIds.length - entries.length });
  }
  async function remove() {
    if (blocked || !confirmation) return;
    const operation = scope.begin();
    if (!operation) return;
    setBusy("delete"); setError(""); setResult(null);
    try {
      const next = await sessionRequests.remove(confirmation.ids, confirmation.revision);
      if (operation.current()) setResult(next);
    } catch (cause) { if (operation.current()) setError(errorText(cause)); }
    finally {
      if (operation.finish()) {
        setBusy(""); setConfirmation(null); setInventory(null); setSelected(new Set()); setLegacySelected(new Set());
        setRevision(value => value + 1);
      }
    }
  }
  async function savePolicy(policy: CleanupPolicy, policyRevision: string) {
    if (blocked) return;
    const operation = scope.begin();
    if (!operation) return;
    setBusy("policy"); setError(""); setNotice(""); setResult(null);
    try {
      const next = await sessionRequests.savePolicy(policy, policyRevision);
      if (operation.current()) {
        draftEdited.current = false;
        setPolicyDraft({ enabled: next.policy.enabled, retentionDays: String(next.policy.retentionDays) });
        setNotice(next.policy.enabled ? "自动清理设置已保存，已安排检查；实际删除在重启 Codex 后完成，之后每小时检查。" : "自动清理已关闭。");
        setResult(next.cleanup);
        if (next.cleanupError) setError(`设置已保存，但本次自动清理失败：${next.cleanupError}`);
      }
    } catch (cause) { if (operation.current()) setError(errorText(cause)); }
    finally {
      if (operation.finish()) {
        setBusy(""); setPolicyConfirmation(null); setConfirmation(null); setInventory(null); setSelected(new Set()); setLegacySelected(new Set());
        setRevision(value => value + 1);
      }
    }
  }
  function askSavePolicy() {
    if (blocked || !inventory || !policyDirty) return;
    if (retentionDays === null) { setError(policyProblem); return; }
    const policy = { enabled: policyDraft.enabled, retentionDays };
    if (policy.enabled && (!inventory.cleanupPolicy.enabled || retentionDays < inventory.cleanupPolicy.retentionDays)) {
      setConfirmation(null);
      setPolicyConfirmation({ policy, revision: inventory.cleanupPolicyRevision });
    } else void savePolicy(policy, inventory.cleanupPolicyRevision);
  }
  return { inventory, busy, error, notice, result, query, filter, olderOnly, page, pages, visible, filteredCount: filtered.length,
    policyDraft, policyProblem, policyDirty, policyConfirmation, changePolicy, askSavePolicy,
    confirmSavePolicy: () => policyConfirmation ? savePolicy(policyConfirmation.policy, policyConfirmation.revision) : Promise.resolve(),
    cancelSavePolicy: () => { if (!scope.pending) setPolicyConfirmation(null); },
    selected, confirmation, blocked, refresh, changeQuery, changeFilter, toggle, toggleVisible, askRemove, remove,
    legacyPendingIds, legacySelected, toggleLegacy, reconfirmLegacy,
    changeOlderOnly,
    changePage: (value: number) => { if (!blocked) setPage(Math.max(0, Math.min(value, pages - 1))); },
    cancelRemove: () => { if (!scope.pending) setConfirmation(null); },
    restart: () => { if (active && !isBusy && !scope.pending && busy !== "list") onRestart(); },
  };
}
