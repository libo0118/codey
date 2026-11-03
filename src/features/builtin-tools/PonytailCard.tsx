import { Badge, Button, Select, Switch } from "../../components/ui";
import { PONYTAIL_MODES } from "./ponytail";
import { usePonytailController } from "./usePonytailController";

export function PonytailCard(props: { active: boolean; isBusy: boolean }) {
  const c = usePonytailController(props);
  const status = c.status;
  return <article className={`feature-card ${status?.enabled ? "active" : ""}`} aria-label="Ponytail" aria-busy={Boolean(c.running)}>
    <div className="feature-card-header">
      <div className="feature-card-title"><strong>Ponytail 简洁开发</strong>
        <Badge variant={status?.installed ? "brand" : "secondary"}>{status ? status.installed ? `v${status.version}` : "待安装" : "检测中"}</Badge>
      </div>
      <Switch aria-label="启用 Ponytail" checked={Boolean(status?.enabled && status.installed)}
        disabled={c.blocked || Boolean(status?.conflict)} onCheckedChange={enabled => void c.setEnabled(enabled)} />
    </div>
    <div className="feature-card-body flex flex-col gap-3">
      <p className="m-0 text-xs leading-relaxed">优先复用已有能力，减少不必要的代码，并提供代码审查、仓库审计等技能。</p>
      <small>首次开启会自动安装官方 Codex 插件，需要 Node.js 和 Git。安装后重启 Codex，在 /hooks 中检查并信任官方钩子，再开始新聊天。</small>
      <div className="flex flex-col gap-2 min-w-0">
        <label htmlFor="ponytail-default-mode" className="text-xs font-medium">新聊天默认模式</label>
        <Select id="ponytail-default-mode" aria-label="Ponytail 默认模式" className="w-full min-w-0" filter={false}
          value={c.mode} disabled={c.blocked} optionList={PONYTAIL_MODES.map(mode => ({ ...mode }))} onChange={c.changeMode} />
        <small>Off 保留技能供手动调用；关闭上方开关会停用整个插件。当前聊天可用 /ponytail lite、full、ultra 或 off 切换。</small>
        {status?.settings.environmentOverride && <small role="status">环境变量 PONYTAIL_DEFAULT_MODE 优先：{status.settings.effectiveMode}。保存的默认模式会保留，移除环境覆盖后生效。</small>}
      </div>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" disabled={c.blocked || !c.dirty} onClick={() => void c.save()}>保存 Ponytail 参数</Button>
        {c.dirty && <Button size="sm" variant="outline" disabled={c.blocked} onClick={c.reset}>撤销参数修改</Button>}
        {status?.job?.phase === "failed" && <Button size="sm" disabled={c.blocked || Boolean(status.conflict)} onClick={() => void c.setEnabled(true)}>重试安装 Ponytail</Button>}
        <Button size="sm" variant="outline" disabled={c.busy || props.isBusy} onClick={c.refresh}>刷新 Ponytail</Button>
        <a href="https://github.com/DietrichGebert/ponytail/blob/main/i18n/README.zh-CN.md" target="_blank" rel="noreferrer" className="text-xs self-center underline">官方说明</a>
      </div>
      {status?.conflict && <p role="alert" className="m-0 text-xs text-danger">{status.conflict}</p>}
      {status?.job && <p role={status.job.phase === "failed" ? "alert" : "status"} className={`m-0 rounded-lg p-3 text-xs break-words ${status.job.phase === "failed" ? "bg-red-500/10 text-danger" : "bg-default/40"}`}>{status.job.message}</p>}
      {c.error && <p role="alert" className="m-0 text-xs text-danger break-words">{c.error}</p>}
      {c.notice && <p role="status" className="m-0 text-xs">{c.notice}</p>}
    </div>
  </article>;
}
