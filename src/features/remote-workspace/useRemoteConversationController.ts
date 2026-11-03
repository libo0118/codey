import { useEffect, useRef, useState } from "react";
import { useStableEvent } from "../../useStableEvent";
import type { View } from "../../remote/workspace-data";
import { remoteWorkspaceRequests as requests } from "./requests";

export function useRemoteConversationController(active: boolean, threadId: string | undefined, onUnauthorized: () => void) {
  const [view, setView] = useState<View | null>(null);
  const [connected, setConnected] = useState(false);
  const [connectionError, setConnectionError] = useState("");
  const [revision, setRevision] = useState(0);
  // 断线时同步拦住旧 render 的操作回调，不等待 React 提交。
  const connectedThread = useRef<string | undefined>(undefined);
  const unauthorized = useStableEvent(onUnauthorized);

  useEffect(() => {
    connectedThread.current = undefined;
    setConnected(false); setConnectionError("");
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
      const current = () => !disposed && connection === token;
      const closed = (message = "", stop = false) => {
        if (!current()) return;
        const closedToken = connection = {};
        unavailable = stop; connectedThread.current = undefined;
        setConnected(false); setConnectionError(previous => message || previous || "连接已断开，正在重新连接…");
        socket?.close(); socket = undefined; checkSession(closedToken);
        if (!unavailable) { timer = window.setTimeout(connect, delay); delay = Math.min(delay * 2, 30000); }
      };
      try {
        socket = requests.connect(threadId, {
          message(event) {
            if (!current() || unavailable) return;
            if (event.type === "disconnected") {
              closed(event.message, true);
            } else if (event.state.id === threadId) {
              connectedThread.current = threadId;
              setView(event.state); setConnected(true); setConnectionError(""); delay = 2000;
            }
          },
          invalid: () => closed("会话数据无效，请重新连接", true),
          close: () => closed(),
        });
      } catch { closed(); }
    };
    connect();
    return () => { disposed = true; connectedThread.current = undefined; window.clearTimeout(timer); socket?.close(); };
  }, [active, threadId, revision, unauthorized]);

  return { view: view?.id === threadId ? view : null, connected: active && connected && view?.id === threadId,
    isConnected: () => active && threadId !== undefined && connectedThread.current === threadId,
    connectionError, reconnect: () => { if (active) setRevision(value => value + 1); } };
}
