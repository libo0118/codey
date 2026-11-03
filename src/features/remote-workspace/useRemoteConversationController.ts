import { useEffect, useState } from "react";
import { useStableEvent } from "../../useStableEvent";
import type { View } from "../../remote/workspace-data";
import { remoteWorkspaceRequests as requests } from "./requests";

export function useRemoteConversationController(active: boolean, threadId: string | undefined, onUnauthorized: () => void) {
  const [view, setView] = useState<View | null>(null);
  const [connected, setConnected] = useState(false);
  const [connectionError, setConnectionError] = useState("");
  const [revision, setRevision] = useState(0);
  const unauthorized = useStableEvent(onUnauthorized);

  useEffect(() => {
    setConnected(false); setConnectionError("");
    if (!active || !threadId) return;
    let disposed = false;
    let unavailable = false;
    let timer: number | undefined;
    let socket: ReturnType<typeof requests.connect> | undefined;
    let connection: object;
    let delay = 2000;
    const checkSession = () => {
      void requests.session().catch(error => {
        if (!disposed && requests.unauthorized(error)) {
          disposed = true; window.clearTimeout(timer); socket?.close(); unauthorized();
        }
      });
    };
    const connect = () => {
      if (disposed || unavailable) return;
      const token = connection = {};
      const current = () => !disposed && connection === token;
      const closed = () => {
        if (!current()) return;
        connection = {};
        setConnected(false); setConnectionError(previous => previous || "连接已断开，正在重新连接…");
        checkSession();
        if (!unavailable) { timer = window.setTimeout(connect, delay); delay = Math.min(delay * 2, 30000); }
      };
      try {
        socket = requests.connect(threadId, {
          message(event) {
            if (!current() || unavailable) return;
            if (event.type === "disconnected") {
              unavailable = true; setConnectionError(event.message); socket?.close(); closed();
            } else if (event.state.id === threadId) {
              setView(event.state); setConnected(true); setConnectionError(""); delay = 2000;
            }
          },
          invalid() { if (current() && !unavailable) setConnectionError("会话数据无效，请重新连接"); },
          close: closed,
        });
      } catch { closed(); }
    };
    connect();
    return () => { disposed = true; window.clearTimeout(timer); socket?.close(); };
  }, [active, threadId, revision, unauthorized]);

  return { view: view?.id === threadId ? view : null, connected: active && connected && view?.id === threadId,
    connectionError, reconnect: () => { if (active) setRevision(value => value + 1); } };
}
