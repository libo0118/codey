import type { MouseEvent } from "react";

type ClipboardBridge = { copy: (getText: () => string | Promise<string>, button: HTMLButtonElement) => Promise<unknown> };
declare global { interface Window { BridgeClipboard?: ClipboardBridge } }

function selectionCopy(text: string, button: HTMLButtonElement): boolean {
  const active = document.activeElement as HTMLElement | null;
  const field = active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement ? active : null;
  const start = field?.selectionStart, end = field?.selectionEnd, direction = field?.selectionDirection;
  const selection = document.getSelection();
  const ranges = selection ? Array.from({ length: selection.rangeCount }, (_, i) => selection.getRangeAt(i).cloneRange()) : [];
  const input = document.createElement("textarea");
  input.value = text; input.readOnly = true; input.tabIndex = -1; input.className = "remote-clipboard-selection";
  (button.closest("dialog[open]") || document.body).append(input);
  try { input.focus({ preventScroll: true }); input.select(); return document.execCommand?.("copy") === true; }
  catch { return false; }
  finally {
    input.remove(); active?.focus({ preventScroll: true });
    if (field && typeof start === "number") field.setSelectionRange(start, end ?? start, direction || undefined);
    if (selection) { selection.removeAllRanges(); ranges.forEach(range => selection.addRange(range)); }
  }
}

function manualCopy(text: string, button: HTMLButtonElement) {
  const dialog = document.createElement("dialog"); dialog.className = "remote-copy-dialog";
  const title = document.createElement("h2"); title.textContent = "手动复制";
  const help = document.createElement("p"); help.textContent = "浏览器限制自动复制，请长按文本或使用系统复制快捷键。";
  const input = document.createElement("textarea"); input.value = text; input.readOnly = true; input.setAttribute("aria-label", "复制内容");
  const close = document.createElement("button"); close.type = "button"; close.textContent = "完成"; close.onclick = () => dialog.close();
  dialog.append(title, help, input, close); document.body.append(dialog);
  dialog.onclose = () => { dialog.remove(); button.focus({ preventScroll: true }); };
  dialog.showModal(); input.focus(); input.select();
}

// Public Bridge interface only. Published mobile shells replace copy before the
// first click; no native prompt, credential or privileged bridge is created here.
export function installRemoteClipboard() {
  if (window.BridgeClipboard) return;
  window.BridgeClipboard = { async copy(getText, button) {
    if (button.disabled) return;
    const label = button.textContent; button.disabled = true;
    try {
      const value = getText();
      if (typeof value === "string" && value.length > 262144) throw new Error("内容过长，请分段复制");
      let pending: Promise<boolean> | null = null;
      try { if (typeof value === "string" && navigator.clipboard?.writeText) pending = navigator.clipboard.writeText(value).then(() => true, () => false); }
      catch { /* A denied Clipboard API still permits the HTTP/manual fallback. */ }
      const selectionTried = pending === null && typeof value === "string";
      const selected = selectionTried && selectionCopy(value as string, button);
      const text = await value;
      if (typeof text !== "string" || text.length > 262144) throw new Error("内容过长或无效，请分段复制");
      const copied = await pending || selected || !selectionTried && selectionCopy(text, button);
      if (copied) { button.textContent = "已复制"; window.setTimeout(() => { if (button.isConnected) button.textContent = label; }, 1500); return true; }
      manualCopy(text, button);
      return false;
    } catch (error) {
      document.dispatchEvent(new CustomEvent("bridge-message-error", { detail: error instanceof Error ? error.message : "复制失败，请重试" }));
      button.textContent = "复制失败，请重试";
      return false;
    } finally { button.disabled = false; }
  } };
}

export function CopyButton({ text, label = "复制", code = false }: { text: string; label?: string; code?: boolean }) {
  function copy(event: MouseEvent<HTMLButtonElement>) {
    installRemoteClipboard();
    const button = event.currentTarget;
    button.title = "";
    const failed = (event: Event) => { button.textContent = "复制失败，请重试"; button.title = typeof (event as CustomEvent).detail === "string" ? (event as CustomEvent).detail : "复制失败，请重试"; };
    document.addEventListener("bridge-message-error", failed);
    void window.BridgeClipboard!.copy(() => text, button).finally(() => document.removeEventListener("bridge-message-error", failed));
  }
  return <button type="button" className={code ? "code-copy" : "message-copy"} aria-live="polite" onClick={copy}>{label}</button>;
}

export function sessionDeepLink(id: string, host = "local", runtime?: string): string | null {
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id) || host !== "local" || runtime === "wsl") return null;
  return `codex://threads/${id}`;
}
