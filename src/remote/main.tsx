import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { IconArrowLeft, IconDeviceLaptop, IconLogout } from "@tabler/icons-react";
import { ReadOnlyConsole } from "./ReadOnlyConsole";
import { codeyApiPath } from "../api";
import { Workspace } from "./workspace";
import { remoteRequest } from "./transport";
import { useRemoteAuth } from "./useRemoteAuth";
import { installRemoteClipboard } from "./clipboard";
import "../tailwind.css";
import "../styles.css";
import "../styles.integrations.css";
import "./styles.css";
import "./cindy/styles.css";

window.__codeyRemoteMode = true;
installRemoteClipboard();
window.__codeyInvokeApi = (command, args) => remoteRequest(codeyApiPath(command), args);

function RemoteApp() {
  const c = useRemoteAuth();
  const [tab, setTab] = useState<"workspace" | "panel">("workspace");
  useEffect(() => {
    const viewport = window.visualViewport;
    const resize = () => {
      document.documentElement.style.setProperty("--remote-viewport-height", `${viewport?.height || window.innerHeight}px`);
      document.documentElement.style.setProperty("--remote-viewport-top", `${viewport?.offsetTop || 0}px`);
    };
    resize(); viewport?.addEventListener("resize", resize); viewport?.addEventListener("scroll", resize);
    return () => { viewport?.removeEventListener("resize", resize); viewport?.removeEventListener("scroll", resize); };
  }, []);

  if (c.checking) return <main className="remote-login"><p role="status">正在检查登录状态…</p></main>;
  if (!c.session?.authenticated) return <main className="remote-login"><form className="remote-login-card" onSubmit={event => { event.preventDefault(); void c.login(); }}>
    <IconDeviceLaptop size={32} aria-hidden="true" /><h1>登录 Codey</h1>
    <p>使用在电脑端“远程控制”中设置的管理员账号，继续 Codex 会话或打开控制台。</p>
    <label>管理员账号<input name="username" autoComplete="username" maxLength={128} required disabled={c.busy} value={c.username} onChange={event => c.changeUsername(event.target.value)} /></label>
    <label>密码<input name="password" type="password" autoComplete="current-password" required disabled={c.busy} value={c.password} onChange={event => c.changePassword(event.target.value)} /></label>
    <label className="remote-remember"><input name="remember" type="checkbox" checked={c.remember} disabled={c.busy} onChange={event => c.changeRemember(event.target.checked)} />记住登录 30 天</label>
    <button className="remote-primary" disabled={c.busy || !c.username.trim() || !c.password}>{c.busy ? "正在登录…" : "登录"}</button>
    {location.protocol !== "https:" && <p className="remote-transport-warning">当前为 HTTP 连接，仅用于可信网络；内网穿透入口的传输保护由你自行提供。</p>}
    {c.error && <p className="remote-client-error" role="alert">{c.error}</p>}
  </form></main>;

  return <div className="remote-shell" aria-busy={c.busy}>
    {tab === "panel" && <header className="remote-topbar"><button data-codey-console-back="" aria-label="返回 Codex" onClick={() => setTab("workspace")}><IconArrowLeft size={18} /></button><strong>Codey 控制台</strong><button disabled={c.busy} aria-label="注销登录" onClick={() => void c.logout()}><IconLogout size={18} /></button></header>}
    <div className="remote-codex-workspace-container" hidden={tab !== "workspace"}><Workspace active={tab === "workspace" && !c.busy} onUnauthorized={c.unauthorized} onPanel={() => setTab("panel")} onLogout={() => void c.logout()} /></div>
    {tab === "panel" && <div className="remote-panel"><ReadOnlyConsole active={tab === "panel"} /></div>}
    {c.error && <p className="remote-client-error" role="alert">{c.error}</p>}
  </div>;
}

createRoot(document.getElementById("root")!).render(<RemoteApp />);
