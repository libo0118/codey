import { useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { errorText } from "../../appUtils";
import { useOperationScope } from "../../useOperationScope";
import { useStableEvent } from "../../useStableEvent";
import { backupRequests, isCodeyApiError } from "./requests";
import { backupFilename, EMPTY_WEBDAV, validateWebdav, webdavDraft } from "./state";
import type { BackupPreview, RestoreResult, WebDavDraft, WebDavSettings } from "./types";

export type BackupOptions = {
  active: boolean; isBusy: boolean; dirty: boolean; remote?: boolean;
  setBusy: Dispatch<SetStateAction<string | null>>;
  onSaved: (result: RestoreResult) => boolean;
  onReload: () => Promise<void>;
};

export function useBackupController({ active, isBusy, dirty, remote = false, setBusy: setAppBusy, onSaved, onReload }: BackupOptions) {
  const scope = useOperationScope(active && !remote);
  const [settings, setSettings] = useState<WebDavSettings | null>(null);
  const [draft, setDraft] = useState(() => webdavDraft(EMPTY_WEBDAV));
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [preview, setPreview] = useState<BackupPreview | null>(null);
  const [uploadConfirmation, setUploadConfirmation] = useState(false);
  const [exportConfirmation, setExportConfirmation] = useState(false);
  const [revision, setRevision] = useState(0);
  const refreshConfig = useRef(false);
  const reload = useStableEvent(onReload);

  useEffect(() => {
    setPreview(null); setUploadConfirmation(false); setExportConfirmation(false);
    if (!active || remote || scope.pending) return;
    const valid = scope.read();
    let cancelled = false;
    const current = () => !cancelled && valid();
    setBusy("读取 WebDAV 设置");
    void backupRequests.settings().then(value => {
      if (!current()) return;
      setSettings(value);
      // 已开始编辑的连接草稿在切页后仍然保留。
      setDraft(previous => previous.url || previous.username || previous.password ? previous : webdavDraft(value));
    }).catch(cause => { if (current()) setError(errorText(cause)); })
      .finally(() => { if (current()) setBusy(""); });
    if (refreshConfig.current) { refreshConfig.current = false; void reload(); }
    return () => { cancelled = true; };
  }, [active, remote, revision, scope, reload]);

  const blocked = !active || remote || isBusy || !!busy || scope.pending;
  const connectionDirty = !settings || draft.url.trim() !== settings.url || draft.username !== settings.username
    || draft.allowHttp !== settings.allowHttp || !!draft.password || draft.clearPassword;
  const cloudBlocked = blocked || connectionDirty || !settings?.url;

  async function run(label: string, action: (current: () => boolean) => Promise<void>) {
    if (blocked) return;
    const operation = scope.begin();
    if (!operation) return;
    setBusy(label); setAppBusy("backup"); setError(""); setNotice("");
    try { await action(operation.current); }
    catch (cause) { if (operation.current()) setError(errorText(cause)); }
    finally {
      const current = operation.current();
      setAppBusy(value => value === "backup" ? null : value);
      if (operation.finish()) {
        setBusy("");
        if (!current) setRevision(value => value + 1);
      }
    }
  }

  async function inspect(content: string, source: string, current: () => boolean) {
    const summary = await backupRequests.inspect(content);
    if (current()) setPreview({ ...summary, content, source });
  }

  return {
    draft, settings, busy, error, notice, preview, uploadConfirmation, exportConfirmation, blocked, cloudBlocked, connectionDirty,
    change<K extends keyof WebDavDraft>(key: K, value: WebDavDraft[K]) {
      if (!blocked) { setDraft(previous => ({ ...previous, [key]: value })); setUploadConfirmation(false); }
    },
    saveSettings: () => run("保存 WebDAV 设置", async current => {
      const problem = validateWebdav(draft);
      if (problem) throw new Error(problem);
      const value = await backupRequests.saveSettings(draft);
      if (current()) { setSettings(value); setDraft(webdavDraft(value)); setNotice("WebDAV 设置已保存"); }
    }),
    askExport: () => { if (!blocked && !dirty) setExportConfirmation(true); },
    cancelExport: () => { if (!scope.pending) setExportConfirmation(false); },
    exportBackup: () => {
      if (!exportConfirmation || dirty) return Promise.resolve();
      return run("导出 JSON", async current => {
        setExportConfirmation(false);
        if (dirty) throw new Error("请先保存当前配置更改，再导出备份");
        const { content } = await backupRequests.export();
        if (!current()) return;
        const url = URL.createObjectURL(new Blob([content], { type: "application/json" }));
        const link = document.createElement("a");
        link.href = url; link.download = backupFilename(new Date()); link.click();
        window.setTimeout(() => URL.revokeObjectURL(url), 1000);
        setNotice("已发起 JSON 下载，请妥善保管其中的密钥");
      });
    },
    importFile: (file: File) => run("校验备份", async current => {
      setPreview(null);
      if (dirty) throw new Error("请先保存或放弃当前配置更改，再导入备份");
      const content = await backupRequests.readFile(file);
      if (current()) await inspect(content, file.name, current);
    }),
    download: () => {
      if (cloudBlocked) return Promise.resolve();
      return run("下载云端备份", async current => {
        setPreview(null);
        if (dirty) throw new Error("请先保存或放弃当前配置更改，再恢复备份");
        const { content } = await backupRequests.download();
        if (current()) await inspect(content, "WebDAV 云端备份", current);
      });
    },
    askUpload: () => { if (!cloudBlocked && !dirty) setUploadConfirmation(true); },
    cancelUpload: () => { if (!scope.pending) setUploadConfirmation(false); },
    upload: () => {
      if (!uploadConfirmation || cloudBlocked || dirty) return Promise.resolve();
      return run("上传云端备份", async current => {
        setUploadConfirmation(false);
        await backupRequests.upload();
        if (current()) setNotice("已备份到 WebDAV");
      });
    },
    cancelRestore: () => { if (!scope.pending) setPreview(null); },
    restore: () => {
      if (!preview || dirty) return Promise.resolve();
      return run("恢复配置", async current => {
        try {
          const result = await backupRequests.restore(preview.content, preview.settingsRevision);
          if (!current()) { refreshConfig.current = true; return; }
          setPreview(null);
          if (!onSaved(result)) { refreshConfig.current = true; await reload(); }
          const warnings = [result.applyWarning, result.warning, result.modelHotReloadError, result.subagentConfigHotReloadError, result.routeRequestLogHotReloadError].filter(Boolean);
          setNotice(`配置已恢复${result.configApply ? "，正在后台应用" : result.restartRequired ? "，请重启 Codex 使启动设置生效" : ""}。恢复前备份：${result.recoveryPath}${warnings.length ? `；${warnings.join("；")}` : ""}`);
        } catch (cause) {
          refreshConfig.current = true;
          if (current()) setPreview(null);
          if (!isCodeyApiError(cause)) throw new Error("恢复结果未确认，请重新打开设置检查当前配置后再操作");
          throw cause;
        }
      });
    },
  };
}
