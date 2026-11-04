import { useEffect, useRef, useState } from "react";
import { useStableEvent } from "../../useStableEvent";
import type { View } from "../../remote/workspace-data";
import { remoteWorkspaceRequests as requests } from "./requests";
import { applyConversationPatches } from "./state";

function historyBoundary(view: View): string {
  const turn = view.turns.find(turn => turn.messages.length);
  const first = turn?.messages[0];
  return first ? JSON.stringify([turn!.id, first.id ?? [first.role, first.kind, first.text]]) : "";
}

export function useRemoteConversationController(active: boolean, threadId: string | undefined, onUnauthorized: () => void) {
  const [view, setView] = useState<View | null>(null);
  const [connected, setConnected] = useState(false);
  const [connectionError, setConnectionError] = useState("");
  const [revision, setRevision] = useState(0);
  const [fileGeneration, setFileGeneration] = useState(0);
  const [loadingEarlier, setLoadingEarlier] = useState(false);
  const historyRequest = useRef<(() => boolean) | undefined>(undefined);
  // 断线时同步拦住旧 render 的操作回调，不等待 React 提交。
  const connectedThread = useRef<string | undefined>(undefined);
  const unauthorized = useStableEvent(onUnauthorized);

  useEffect(() => {
    connectedThread.current = undefined; historyRequest.current = undefined;
    setConnected(false); setConnectionError(""); setLoadingEarlier(false);
    if (!active || !threadId) return;
    let disposed = false;
    let unavailable = false;
    let timer: number | undefined;
    let socket: ReturnType<typeof requests.connect> | undefined;
    let connection: object;
    let delay = 2000;
    const checkSession = (token: object) => {
      void requests.session().catch(error => {
        if (!disposed && connection === token && requests.unauthorized(error)) {
          disposed = true; window.clearTimeout(timer); socket?.close(); unauthorized();
        }
      });
    };
    const connect = () => {
      if (disposed || unavailable) return;
      const token = connection = {};
      let snapshot: View | null = null;
      let streamRevision: number | undefined;
      let earlierBoundary: string | undefined;
      const current = () => !disposed && connection === token;
      const closed = (message = "", stop = false) => {
        if (!current()) return;
        const closedToken = connection = {};
        unavailable = stop; connectedThread.current = undefined; historyRequest.current = undefined;
        setConnected(false); setLoadingEarlier(false); setConnectionError(previous => message || previous || "连接已断开，正在重新连接…");
        socket?.close(); socket = undefined; checkSession(closedToken);
        if (!unavailable) { timer = window.setTimeout(connect, delay); delay = Math.min(delay * 2, 30000); }
      };
      try {
        socket = requests.connect(threadId, {
          message(event) {
            if (!current() || unavailable) return;
            if (event.type === "disconnected") {
              closed(event.message, true);
              return;
            }
            try {
              let next: View;
              if (event.type === "state") {
                if (streamRevision !== undefined || event.streamVersion === 2 && (snapshot !== null || event.revision !== 1)) throw new Error();
                next = event.state;
                streamRevision = event.streamVersion === 2 ? event.revision : undefined;
              } else {
                if (!snapshot || streamRevision === undefined || event.baseRevision !== streamRevision || event.revision !== streamRevision + 1) throw new Error();
                next = applyConversationPatches(snapshot, event.patches);
                streamRevision = event.revision;
              }
              if (next.id !== threadId) throw new Error();
              if (!snapshot) setFileGeneration(value => value + 1);
              snapshot = next;
              if (earlierBoundary !== undefined && (!next.hasEarlierMessages || historyBoundary(next) !== earlierBoundary)) {
                earlierBoundary = undefined; setLoadingEarlier(false);
              }
              connectedThread.current = threadId;
              setView(next); setConnected(true); setConnectionError(""); delay = 2000;
            } catch { closed("会话数据无效，请重新连接", true); }
          },
          invalid: () => closed("会话数据无效，请重新连接", true),
          close: () => closed(),
        });
        historyRequest.current = () => {
          if (!current() || unavailable || connectedThread.current !== threadId || streamRevision === undefined || !snapshot?.hasEarlierMessages || earlierBoundary !== undefined) return false;
          earlierBoundary = historyBoundary(snapshot); setLoadingEarlier(true);
          try { socket!.history(); return true; }
          catch { closed(); return false; }
        };
      } catch { closed(); }
    };
    connect();
    return () => { disposed = true; connectedThread.current = undefined; historyRequest.current = undefined; window.clearTimeout(timer); socket?.close(); };
  }, [active, threadId, revision, unauthorized]);

  return { view: view?.id === threadId ? view : null, connected: active && connected && view?.id === threadId,
    isConnected: () => active && threadId !== undefined && connectedThread.current === threadId,
    connectionError, fileGeneration, loadingEarlier, loadEarlier: () => active && Boolean(historyRequest.current?.()),
    reconnect: () => { if (active) setRevision(value => value + 1); } };
}
