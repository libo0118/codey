import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { IconDownload, IconFile } from "@tabler/icons-react";
import { isAttachment, type Attachment } from "./workspace-data";
import { resolveFile } from "./file-requests";

export const MessageFilesContext = createContext({ threadId: "", enabled: false, generation: 0 });
const uuid = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
const downloadPath = new RegExp(`^/api/sessions/(${uuid})/files/[0-9a-f]{64}$`, "i");

export function downloadUrl(url: string | undefined, threadId: string): string {
  const match = url && downloadPath.exec(url);
  return match && match[1].toLowerCase() === threadId.toLowerCase() ? url! : "";
}

export function fileReference(reference: string): boolean {
  if (!reference || /[\u0000-\u001f\u007f]/.test(reference) || /^(?:\/\/|\\\\|#)/.test(reference)) return false;
  return /^(?:file:\/\/|sandbox:\/|[a-z]:(?:[\\/]|%5c|%2f))/i.test(reference) || !/^[a-z][a-z0-9+.-]*:/i.test(reference);
}

type ResolvedFile = { scope: string; file?: Attachment; error?: string };
export function useResolvedFile(threadId: string, reference: string, initial: Attachment | undefined, enabled: boolean, generation = 0) {
  const [attempt, setAttempt] = useState(0);
  const firstGeneration = useRef(generation);
  const scope = `${threadId}\0${reference}\0${generation}\0${attempt}`;
  const [result, setResult] = useState<ResolvedFile>();
  const direct = attempt === 0 && generation === firstGeneration.current && (initial?.unavailableReason || downloadUrl(initial?.downloadUrl, threadId)) ? initial : undefined;
  useEffect(() => {
    if (!enabled || !threadId || !reference || direct) return;
    const controller = new AbortController();
    resolveFile(threadId, reference, controller.signal).then(response => {
      if (controller.signal.aborted) return;
      if (!response || !isAttachment(response.file)) throw new Error("文件元数据无效，请在电脑端查看");
      if (!response.file.unavailableReason && !downloadUrl(response.file.downloadUrl, threadId)) throw new Error("此文件没有可用的会话下载地址");
      setResult({ scope, file: response.file });
    }).catch(error => {
      if (!controller.signal.aborted) setResult({ scope, error: error instanceof Error ? error.message : "文件暂不可下载，请在电脑端查看" });
    });
    return () => controller.abort();
  }, [threadId, reference, enabled, scope, direct]);
  const current = result?.scope === scope ? result : undefined;
  return {
    file: direct || current?.file,
    error: direct?.unavailableReason || current?.file?.unavailableReason || current?.error,
    loading: enabled && !!threadId && !!reference && !direct && !current,
    retry: () => setAttempt(current => current + 1),
  };
}

export function FileLink({ reference, attachment, children, image = false }: { reference: string; attachment?: Attachment; children?: ReactNode; image?: boolean }) {
  const { threadId, enabled, generation } = useContext(MessageFilesContext);
  const { file, error, loading, retry } = useResolvedFile(threadId, reference, attachment, enabled, generation);
  const url = enabled ? downloadUrl(file?.downloadUrl, threadId) : "";
  const label = children || file?.name || attachment?.name || "会话文件";
  const size = file?.size === undefined ? "" : file.size < 1024 ? `${file.size} B` : file.size < 1048576 ? `${(file.size / 1024).toFixed(1)} KB` : `${(file.size / 1048576).toFixed(1)} MB`;
  const preview = url && file?.mime && /^image\/(png|jpeg|gif|webp)$/i.test(file.mime) && (image || !!attachment);
  return <span className="remote-file">
    {url && !error ? <a className="remote-file-link" href={url} download={file?.name || attachment?.name} title="下载会话文件"><IconDownload size={14} aria-hidden="true" />{label}{size && <small>{size}</small>}</a>
      : <span className="remote-file-unavailable"><IconFile size={14} aria-hidden="true" />{label}<small role="status">{error || (loading ? "正在检查文件…" : !reference ? "此附件没有可下载引用，请在电脑端查看" : "文件需在已连接的本地会话中下载")}</small>{error && enabled && reference && <button className="remote-file-retry" type="button" onClick={retry}>重新检查</button>}</span>}
    {preview && !error && <FileImage key={`${generation}:${url}`} url={url} alt={file?.name || "会话图片"} />}
  </span>;
}

export function FileImage({ url, alt }: { url: string; alt: string }) {
  const [failed, setFailed] = useState(false);
  return <span className="remote-file-preview">{failed ? <><small role="status">图片暂不可预览，可能文件已变化或服务繁忙。</small><button className="remote-file-retry" type="button" onClick={() => setFailed(false)}>重新预览</button></>
    : <img className="remote-artifact-image" src={url} alt={alt} loading="lazy" onError={() => setFailed(true)} />}</span>;
}
