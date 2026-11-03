import type { Config } from "../App.types";
import type { ConfigCommandResult } from "../configCommandResult";
import type { WebDavDraft } from "../features/backup";

export function createBackupPreview(read: () => ConfigCommandResult, write: (config: Config) => void) {
  let settings = { url: "", username: "", passwordConfigured: false, allowHttp: false, revision: "" };
  let cloud = "";
  let revision = 0;
  const exportConfig = () => JSON.stringify({ format: "codey-config", version: 1, createdAt: new Date().toISOString(), config: read().config }, null, 2);
  function parse(content: unknown) {
    let backup;
    try { backup = JSON.parse(String(content)); } catch { throw new Error("备份不是有效的 JSON 文件"); }
    if (backup.format !== "codey-config" || backup.version !== 1 || !Array.isArray(backup.config?.profiles)) throw new Error("不支持的备份格式或版本");
    return backup as { createdAt: string; config: Config };
  }
  return (args: Record<string, unknown>) => {
    try {
      if (["export", "restore", "webdav_upload"].includes(String(args.action)) && args.confirmed !== true) throw new Error("请先确认敏感配置操作");
      switch (args.action) {
        case "webdav_get": return { ...settings };
        case "webdav_save": {
          const input = args.settings as WebDavDraft;
          if (input.revision !== settings.revision) throw new Error("WebDAV 设置已变化，请刷新后重试");
          settings = { revision: String(++revision), url: input.url.trim(), username: input.username, allowHttp: input.allowHttp,
            passwordConfigured: !input.clearPassword && (!!input.password || settings.passwordConfigured && settings.url === input.url.trim() && settings.username === input.username) };
          return { ...settings };
        }
        case "export": return { content: exportConfig(), containsSecrets: true };
        case "inspect": {
          const backup = parse(args.content);
          return { createdAt: backup.createdAt, routeCount: backup.config.profiles.filter(p => p.authMode !== "officialAccount" && !p.pluginOwnerId).length,
            channelCount: backup.config.webhook.channels.length, settingsRevision: read().config.settingsRevision };
        }
        case "restore": {
          const previous = read().config;
          if (args.settingsRevision !== previous.settingsRevision) throw new Error("配置已被其他操作更新，请重新预览备份后再恢复");
          const { config } = parse(args.content);
          write({ ...config, settingsRevision: previous.settingsRevision + 1, codexAppPath: previous.codexAppPath,
            userScripts: previous.userScripts, codexLaunchProxy: previous.codexLaunchProxy });
          return { ...read(), recoveryPath: "预览/before-restore.json", restartRequired: true };
        }
        case "webdav_upload": cloud = exportConfig(); return { status: "ok" };
        case "webdav_download":
          if (!cloud) throw new Error("WebDAV 文件或父目录不存在，请先上传备份");
          return { content: cloud };
        default: throw new Error("未知备份操作");
      }
    } catch (error) { return { status: "failed", message: error instanceof Error ? error.message : "备份失败" }; }
  };
}
