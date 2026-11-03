import type { RemoteDraft, RemoteStatus } from "./types";

export const EMPTY_REMOTE: RemoteDraft = { username: "", password: "", port: "18787", publicUrl: "" };

export function remoteDraft(status: RemoteStatus): RemoteDraft {
  return { username: status.username ?? "", password: "", port: String(status.port), publicUrl: status.publicUrl ?? "" };
}

export function remoteSettings(draft: RemoteDraft, configured: boolean) {
  const username = draft.username.trim();
  if (!username || username.length > 128) throw new Error("请输入不超过 128 个字符的管理员账号");
  if (!/^\d+$/.test(draft.port) || Number(draft.port) < 1 || Number(draft.port) > 65535) throw new Error("监听端口须为 1 至 65535 的整数");
  if (!configured && !draft.password) throw new Error("首次启用前请设置管理员密码");
  if (draft.password && ([...draft.password].length < 12 || new TextEncoder().encode(draft.password).length > 1024)) throw new Error("密码至少 12 个字符，且不超过 1024 字节");
  const publicUrl = draft.publicUrl.trim();
  if (publicUrl) {
    let url: URL;
    try { url = new URL(publicUrl); } catch { throw new Error("外部访问地址须为完整 HTTP(S) 地址"); }
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || url.pathname !== "/" || url.search || url.hash) throw new Error("外部访问地址只能包含 HTTP(S) 协议、主机与端口，不能包含路径或凭据");
  }
  return { username, port: Number(draft.port), publicUrl, ...(draft.password ? { password: draft.password } : {}) };
}
