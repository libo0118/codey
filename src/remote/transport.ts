export class RemoteError extends Error {
  constructor(message: string, readonly status: number, readonly activation?: string, readonly submission?: "not-started") { super(message); }
}

export type RemoteSession = { authenticated: boolean; username?: string; csrfToken?: string; expiresAt?: string };
let csrfToken = "";
export function clearRemoteSession() { csrfToken = ""; }

export async function remoteRequest<T>(path: string, args?: unknown, signal?: AbortSignal): Promise<T> {
  if (!path.startsWith("/remote/") && !path.startsWith("/api/")) throw new Error("不允许的远程请求地址");
  const mutation = args !== undefined;
  const login = path === "/remote/login";
  const requestSession = csrfToken;
  if (mutation && !login && !csrfToken) throw new RemoteError("请先登录后再操作", 401);
  const timeout = AbortSignal.timeout(90_000), controller = new AbortController();
  const abort = () => controller.abort(signal?.aborted ? signal.reason : timeout.reason);
  signal?.addEventListener("abort", abort, { once: true }); timeout.addEventListener("abort", abort, { once: true });
  if (signal?.aborted) abort();
  try {
    const response = await fetch(path, {
      method: mutation ? "POST" : "GET",
      credentials: "same-origin",
      redirect: "error",
      headers: mutation ? { "Content-Type": "application/json", ...(!login ? { "X-CSRF-Token": csrfToken } : {}) } : undefined,
      body: mutation ? JSON.stringify(args) : undefined,
      signal: controller.signal,
    });
    signal?.throwIfAborted();
    if (response.status === 401 && csrfToken === requestSession) {
      clearRemoteSession();
      window.dispatchEvent(new Event("codey-remote-unauthorized"));
    }
    let result: { status?: string; message?: string; activation?: unknown; submission?: unknown } & Partial<RemoteSession>;
    try { result = await response.json(); }
    catch { throw new RemoteError(`远程服务返回了无效响应 (${response.status})`, response.status); }
    signal?.throwIfAborted();
    if (!response.ok || result?.status === "failed") throw new RemoteError(result?.message || `请求失败 (${response.status})`, response.status, typeof result?.activation === "string" ? result.activation : undefined, result?.submission === "not-started" ? "not-started" : undefined);
    if (path === "/remote/session" || login) {
      if (typeof result?.authenticated !== "boolean" || result.authenticated && (typeof result.csrfToken !== "string" || !result.csrfToken)) { clearRemoteSession(); throw new RemoteError("登录响应缺少安全校验信息，请重新登录", 502); }
      csrfToken = result.authenticated ? result.csrfToken! : "";
    }
    return result as T;
  } finally { signal?.removeEventListener("abort", abort); timeout.removeEventListener("abort", abort); }
}

// randomUUID is unavailable on plain HTTP LAN origins; getRandomValues is
// available there and still provides cryptographically random request IDs.
export function requestId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 15) | 64;
  bytes[8] = (bytes[8] & 63) | 128;
  const hex = Array.from(bytes, byte => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

type Submission = { id: string; args: Record<string, unknown> };
const pending = new Map<string, Submission>();
const storageKey = (scope: string) => `codey-remote-pending:${scope}`;

export function readSubmission(scope: string): Submission | null {
  const current = pending.get(scope);
  if (current) return current;
  try {
    const value = JSON.parse(sessionStorage.getItem(storageKey(scope)) || "null");
    if (value && typeof value.id === "string" && /^[a-f0-9-]{36}$/.test(value.id) && value.args && typeof value.args === "object") return value;
  } catch { /* Private browsing may disable storage; keep the in-memory record. */ }
  return null;
}

// A page reload must not turn an unconfirmed mutation into a new request.
// This journal never resends anything itself and is scoped to the browser tab.
export function rememberSubmission(scope: string, args: Record<string, unknown>): Submission {
  const previous = readSubmission(scope);
  if (previous && JSON.stringify(previous.args) === JSON.stringify(args)) return previous;
  const record = { id: requestId(), args };
  pending.set(scope, record);
  const files = Array.isArray(args.files) ? args.files.map(file => ({ name: file.name, size: file.size, kind: "file", url: "" })) : undefined;
  try { sessionStorage.setItem(storageKey(scope), JSON.stringify({ ...record, args: { ...args, files } })); }
  catch {
    // Large photos can exceed the tab's storage quota. Persist a small marker
    // so a reload still prevents duplicate submission and asks for reselection.
    try {
      const images = Array.isArray(args.images) ? args.images.map(image => ({ name: image.name, size: image.size, url: "" })) : undefined;
      sessionStorage.setItem(storageKey(scope), JSON.stringify({ id: record.id, args: { ...args, images, files } }));
    } catch {
      // Ordinary-file content is never journaled, so refuse a new upload if even
      // its small anti-duplicate marker cannot survive a reload.
      if (files?.length || typeof args.action === "string" && args.action.startsWith("goal")) throw new Error(files?.length ? "无法保存附件提交记录，请清理浏览器存储后重试。" : "无法保存目标提交记录，请清理浏览器存储后重试。");
    }
  }
  return record;
}

export function forgetSubmission(scope: string): void {
  pending.delete(scope);
  try { sessionStorage.removeItem(storageKey(scope)); } catch { /* See readSubmission. */ }
}

export function clearSubmissions(): void {
  pending.clear();
  try {
    const keys = Array.from({ length: sessionStorage.length }, (_, index) => sessionStorage.key(index));
    for (const key of keys) if (key?.startsWith("codey-remote-pending:")) sessionStorage.removeItem(key);
  } catch { /* See readSubmission. */ }
}
