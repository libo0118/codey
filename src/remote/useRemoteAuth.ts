import { useEffect, useRef, useState } from "react";
import { clearRemoteSession, clearSubmissions, remoteRequest, RemoteError, type RemoteSession } from "./transport";

export function useRemoteAuth() {
  const [session, setSession] = useState<RemoteSession | null>(null);
  const [checking, setChecking] = useState(true);
  const [busy, setBusy] = useState(false);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [remember, setRemember] = useState(true);
  const [error, setError] = useState("");
  const mounted = useRef(false);
  const pending = useRef(false);
  const epoch = useRef(0);

  useEffect(() => {
    mounted.current = true;
    const current = ++epoch.current;
    const unauthorized = () => {
      epoch.current++; clearRemoteSession(); clearSubmissions();
      setSession(null); setPassword(""); setError("登录已过期或被撤销，请重新登录。");
    };
    window.addEventListener("codey-remote-unauthorized", unauthorized);
    void remoteRequest<RemoteSession>("/remote/session").then(next => {
      if (mounted.current && epoch.current === current) setSession(next.authenticated ? next : null);
    }).catch(() => undefined).finally(() => { if (mounted.current) setChecking(false); });
    return () => { mounted.current = false; epoch.current++; window.removeEventListener("codey-remote-unauthorized", unauthorized); };
  }, []);

  return {
    session, checking, busy, username, password, remember, error,
    changeUsername: (value: string) => { if (!pending.current) setUsername(value); },
    changePassword: (value: string) => { if (!pending.current) setPassword(value); },
    changeRemember: (value: boolean) => { if (!pending.current) setRemember(value); },
    unauthorized() { window.dispatchEvent(new Event("codey-remote-unauthorized")); },
    async login() {
      if (pending.current || checking || !username.trim() || !password) return;
      pending.current = true; const current = ++epoch.current; setBusy(true); setError("");
      try {
        const next = await remoteRequest<RemoteSession>("/remote/login", { username: username.trim(), password, remember });
        if (!next.authenticated) throw new Error("账号或密码错误");
        if (mounted.current && epoch.current === current) setSession(next);
      } catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : "登录失败，请重试"); }
      finally { pending.current = false; if (mounted.current) { setPassword(""); setBusy(false); } }
    },
    async logout() {
      if (pending.current) return;
      pending.current = true; const current = ++epoch.current; setBusy(true); setError("");
      try {
        await remoteRequest("/remote/logout", {});
        clearRemoteSession(); clearSubmissions();
        if (mounted.current && epoch.current === current) setSession(null);
      } catch (cause) { if (mounted.current) setError(cause instanceof RemoteError && cause.status === 401 ? "" : "注销结果尚未确认，请重试或在本机撤销此登录。"); }
      finally { pending.current = false; if (mounted.current) { setPassword(""); setBusy(false); } }
    },
  };
}
