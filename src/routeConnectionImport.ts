/** 快捷导入所需的 API 密钥与线路地址。 */
export type NewApiConnectionInfo = {
  key: string;
  url: string;
};

const NEW_API_CONNECTION_TYPE = "newapi_channel_conn";

function parseJsonRecord(text: string): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(text);
    return value !== null && typeof value === "object" && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

function parseCcSwitchConnectionInfo(text: string): NewApiConnectionInfo | null {
  // 支持说明文字和 Markdown 包裹，但不从其它 URL 的路径或参数中提取链接。
  const links = [...text.matchAll(/(?:^|[\s"'`([{<:：])(ccswitch:\/\/[^\s"'`)\]}>（）]+)/gi)];
  if (links.length !== 1) return null;

  try {
    const link = links[0][1];
    // URLSearchParams 会容忍损坏的转义，先确认编码有效，避免密钥被悄悄改写。
    decodeURIComponent(link);
    const parsed = new URL(link);
    if (
      parsed.protocol !== "ccswitch:" || parsed.host !== "v1" ||
      parsed.pathname !== "/import" || parsed.username || parsed.password || parsed.hash
    ) return null;

    const params = parsed.searchParams;
    if (["resource", "endpoint", "apiKey"].some(name => params.getAll(name).length !== 1)) return null;
    if (params.get("resource") !== "provider") return null;
    const key = params.get("apiKey")!.trim();
    const url = params.get("endpoint")!.trim();
    // CC Switch 以逗号分隔多个地址；单条线路无法保留这些候选地址。
    if (!key || !url || url.includes(",") || /[\u0000-\u001f\u007f]/.test(key + url)) return null;
    if (!/^https?:\/\//i.test(url) || /\s/.test(url)) return null;
    const endpoint = new URL(url);
    if (!endpoint.hostname || !["http:", "https:"].includes(endpoint.protocol)) return null;
    // 只读取连接字段，不导入或执行 usageScript 等附加配置，也不重写地址。
    return { key, url };
  } catch {
    return null;
  }
}

/**
 * 解析 New API 连接 JSON 或 Sub2API/CC Switch 链接；无法识别时返回 null。
 * 容忍整体首尾空白，以及连接信息前后夹杂其它文本的粘贴内容。
 */
export function parseNewApiConnectionInfo(
  text: string | null | undefined,
): NewApiConnectionInfo | null {
  if (typeof text !== "string") return null;
  const trimmed = text.trim();
  if (!trimmed) return null;
  let parsed = parseJsonRecord(trimmed);
  if (!parsed) {
    const start = trimmed.indexOf("{");
    const end = trimmed.lastIndexOf("}");
    if (start >= 0 && end > start) {
      parsed = parseJsonRecord(trimmed.slice(start, end + 1));
    }
  }
  if (!parsed || parsed._type !== NEW_API_CONNECTION_TYPE) return parseCcSwitchConnectionInfo(trimmed);
  const { key, url } = parsed;
  if (typeof key !== "string" || typeof url !== "string") return null;
  const normalizedKey = key.trim();
  const normalizedUrl = url.trim();
  return normalizedKey && normalizedUrl
    ? { key: normalizedKey, url: normalizedUrl }
    : null;
}

/** 按连接地址推导线路名与短名称建议；推导不出时对应字段返回空串。 */
export function suggestRouteNamesFromUrl(
  url: string,
  maxNameLength: number,
  maxShortNameLength: number,
): { name: string; shortName: string } {
  let hostname = "";
  try {
    hostname = new URL(url).hostname;
  } catch {
    return { name: "", shortName: "" };
  }
  if (!hostname) return { name: "", shortName: "" };
  const labels = hostname.split(".");
  const base = labels.length > 1 && labels[0] === "api" ? labels[1] : labels[0];
  const shortName = Array.from(base.replace(/[^\p{L}\p{N}]/gu, ""))
    .slice(0, maxShortNameLength)
    .join("");
  const name = Array.from(hostname).length <= maxNameLength ? hostname : "";
  return { name, shortName };
}
