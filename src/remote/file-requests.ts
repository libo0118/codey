import { remoteRequest } from "./transport";
import type { Attachment } from "./workspace-data";

const waiting: { start: () => void }[] = [];
let active = false;
function next() { if (!active) waiting.shift()?.start(); }

// Metadata only: a bounded serial queue protects the desktop bridge; GET is
// still a separate, genuine user click. Busy/errors are retried only explicitly.
export function resolveFile(threadId: string, reference: string, signal: AbortSignal) {
  return new Promise<{ file: Attachment }>((resolve, reject) => {
    if (signal.aborted) { reject(signal.reason); return; }
    if (waiting.length >= 16) { reject(new Error("待检查的文件较多，请稍后点击重新检查")); return; }
    const cancel = () => { const index = waiting.indexOf(job); if (index >= 0) waiting.splice(index, 1); reject(signal.reason); };
    const job = { start() {
      signal.removeEventListener("abort", cancel);
      active = true;
      remoteRequest<{ file: Attachment }>("/remote/files/resolve", { threadId, reference }, signal)
        .then(resolve, reject).finally(() => { active = false; next(); });
    } };
    signal.addEventListener("abort", cancel, { once: true });
    waiting.push(job); next();
  });
}
