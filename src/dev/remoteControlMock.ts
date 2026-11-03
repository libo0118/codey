import type { RemoteStatus } from "../features/remote-control/types";

export function createRemoteControlPreview() {
  const state: RemoteStatus = { running: false, urls: [], port: 18787, publicUrl: "", accountConfigured: false, username: "", revision: "0", sessions: [] };
  let revision = 0;
  return (command: string, args: Record<string, unknown>) => {
    if (command === "remote_control_status") return structuredClone(state);
    if (command === "configure_remote_control") {
      if (state.running) throw new Error("请先停止远程服务");
      if (args.confirmed !== true || args.revision !== state.revision) throw new Error("请确认设置并刷新最新状态");
      const settings = args.settings as { username: string; password?: string; port: number; publicUrl?: string };
      if (!state.accountConfigured && !settings.password) throw new Error("首次启用请设置管理员密码");
      state.username = settings.username; state.port = settings.port; state.publicUrl = settings.publicUrl ?? "";
      state.accountConfigured = true; state.revision = String(++revision); state.sessions = [];
    } else if (command === "start_remote_control") {
      if (!state.accountConfigured) throw new Error("请先设置管理员账号密码");
      state.running = true; state.address = `0.0.0.0:${state.port}`;
      state.urls = [state.publicUrl || `http://127.0.0.1:${state.port}`];
    } else if (command === "stop_remote_control") {
      state.running = false; state.urls = [];
    } else if (command === "revoke_remote_session") {
      state.sessions = args.id === "*" ? [] : state.sessions.filter(session => session.id !== args.id);
    } else throw new Error("未知远程管理操作");
    return structuredClone(state);
  };
}
