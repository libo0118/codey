export type DraftImage = { name: string; url: string; size: number; kind?: "file" };

// Keep in sync with remote_control/images.rs. Steer carries two copies of input
// in the desktop's 16 MiB frame, so the budget includes base64 expansion.
export const MAX_IMAGES = 4;
export const MAX_IMAGE_BYTES = 4 * 1024 * 1024;
export const IMAGE_ACCEPT = "image/png,image/jpeg,image/gif,image/webp";
const types: Record<string, string> = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp" };

export async function readImages(files: readonly File[], existing: readonly DraftImage[], signal: AbortSignal): Promise<DraftImage[]> {
  checkBudget(files, existing);
  const selected = files.map(file => {
    const type = file.type.toLowerCase() || types[file.name.split(".").pop()?.toLowerCase() || ""];
    if (!Object.values(types).includes(type)) throw new Error("请选择 PNG、JPEG、GIF 或 WebP 图片；其他格式请先转换。");
    if (!file.size) throw new Error("图片为空，请重新选择。");
    return { file, type };
  });
  const images: DraftImage[] = [];
  for (const { file, type } of selected) {
    signal.throwIfAborted();
    const url = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      const preview = new Image();
      const finish = (error?: Error) => {
        signal.removeEventListener("abort", abort);
        reader.onload = reader.onerror = reader.onabort = null;
        preview.onload = preview.onerror = null;
        if (reader.readyState === FileReader.LOADING) reader.abort();
        if (error) { preview.src = ""; reject(error); }
        else resolve(preview.src);
      };
      const abort = () => finish(new DOMException("图片读取已取消", "AbortError"));
      signal.addEventListener("abort", abort, { once: true });
      reader.onerror = () => finish(new Error("无法读取图片，请检查照片访问权限后重新选择。"));
      reader.onabort = abort;
      preview.onerror = () => finish(new Error("图片无法解码，请选择有效的 PNG、JPEG、GIF 或 WebP 图片。"));
      preview.onload = () => finish();
      reader.onload = () => {
        const data = typeof reader.result === "string" ? reader.result.split(",")[1] : "";
        if (!data) { finish(new Error("图片读取失败，请重新选择。")); return; }
        preview.src = `data:${type};base64,${data}`;
      };
      try { reader.readAsDataURL(file); }
      catch { finish(new Error("无法读取图片，请检查照片访问权限后重新选择。")); }
    });
    images.push({ name: file.name.slice(0, 200) || "图片", url, size: file.size });
  }
  return images;
}

function checkBudget(files: readonly File[], existing: readonly DraftImage[]) {
  if (files.length + existing.length > MAX_IMAGES) throw new Error("每条消息最多添加 4 个附件。");
  if (files.reduce((size, file) => size + file.size, existing.reduce((size, image) => size + image.size, 0)) > MAX_IMAGE_BYTES) throw new Error("附件总大小不能超过 4 MB。");
}

export async function readFiles(files: readonly File[], existing: readonly DraftImage[], signal: AbortSignal): Promise<DraftImage[]> {
  checkBudget(files, existing);
  if (files.some(file => !file.name.trim() || [...file.name].length > 200 || /^[.]{1,2}$/.test(file.name) || /[\\/:\u0000-\u001f\u007f-\u009f\u2028\u2029]/.test(file.name))) throw new Error("文件名无效，请重命名后重新选择。");
  const result: DraftImage[] = [];
  for (const file of files) {
    signal.throwIfAborted();
    const url = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      const finish = (error?: Error) => {
        signal.removeEventListener("abort", abort);
        reader.onload = reader.onerror = reader.onabort = null;
        if (reader.readyState === FileReader.LOADING) reader.abort();
        if (error) reject(error);
        else resolve(`data:application/octet-stream;base64,${String(reader.result).split(",")[1]}`);
      };
      const abort = () => finish(new DOMException("文件读取已取消", "AbortError"));
      signal.addEventListener("abort", abort, { once: true });
      reader.onerror = () => finish(new Error("无法读取文件，请重新选择。"));
      reader.onabort = abort;
      reader.onload = () => { if (typeof reader.result !== "string" || !reader.result.includes(",")) finish(new Error("文件读取失败，请重新选择。")); else finish(); };
      try { reader.readAsDataURL(file); } catch { finish(new Error("无法读取文件，请重新选择。")); }
    });
    result.push({ name: file.name, url, size: file.size, kind: "file" });
  }
  return result;
}

export function imagePickerError(error: unknown): string {
  if (error instanceof DOMException && ["NotAllowedError", "SecurityError"].includes(error.name)) return "无法访问相机或照片，请在浏览器及系统设置中允许访问后重试。";
  return "当前浏览器无法打开相机或照片，请使用系统浏览器重试，或通过照片入口选择已有图片。";
}
