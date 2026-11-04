import { memo } from "react";
import { IconHelpCircle, IconNetwork, IconX } from "@tabler/icons-react";
import type { Config } from "./App.types";
import { Input, Tooltip } from "./components/ui";
import { SettingsPageHeader } from "./SettingsPageHeader";
import { validateCodexLaunchProxy } from "./urlValidation";

const LAUNCH_PROXY_TOOLTIP_CONTENT = (
  <div className="space-y-1.5 text-xs text-left leading-relaxed">
    <div className="font-semibold">Codex 启动代理格式与说明</div>
    <div>支持协议：<code>http://</code>、<code>https://</code>、<code>socks5://</code>、<code>socks5h://</code></div>
    <div>认证可用 http://用户名:密码@主机:端口 形式；SOCKS 代理必须指定端口；https:// 表示用 TLS 连接代理本身。</div>
    <div className="space-y-0.5 font-mono text-[11px]">
      <div>http://127.0.0.1:7890</div>
      <div>http://user:pass@127.0.0.1:7890</div>
      <div>socks5://127.0.0.1:1080</div>
    </div>
    <div>客户端与 app-server 经此代理访问网络，重启 Codex 后生效；本地路由与额度查询不受影响。</div>
  </div>
);

type NetworkProxyPageProps = {
  config: Config;
  isBusy: boolean;
  onConfigChange: (config: Config) => void;
};

function NetworkProxyPageComponent({ config, isBusy, onConfigChange }: NetworkProxyPageProps) {
  const launchProxy = config.codexLaunchProxy ?? "";
  const launchProxyError = validateCodexLaunchProxy(launchProxy);

  return (
    <section className="secondary-section launch-proxy-section" aria-labelledby="network-proxy-title">
      <SettingsPageHeader
        id="network-proxy-title"
        title="网络代理"
        icon={<IconNetwork size={15} />}
        description="设置由 Codey 启动的 Codex 客户端与 app-server 使用的网络代理。"
      />
      <div className="feature-policy-heading">
        <div className="flex items-center gap-2">
          <h3 id="launch-proxy-title" className="settings-section-heading">Codex 启动代理</h3>
          <Tooltip content={LAUNCH_PROXY_TOOLTIP_CONTENT} position="top" asChild>
            <button type="button" className="subagent-role-help" aria-label="Codex 启动代理格式与说明">
              <IconHelpCircle size={14} aria-hidden="true" />
            </button>
          </Tooltip>
        </div>
        <p>保存并重启 Codex 后生效；留空为直连，本地路由与额度查询不受影响。</p>
      </div>
      <div className={`feature-card launch-proxy-card ${launchProxy.trim() ? "active" : ""}`}>
        <div className="feature-card-body launch-proxy-row">
          <div className="launch-proxy-label">
            <label htmlFor="codex-launch-proxy-input">代理地址</label>
            <span className={`launch-proxy-state ${launchProxyError ? "is-error" : launchProxy.trim() ? "is-active" : ""}`}>
              {launchProxyError ? "格式错误" : launchProxy.trim() ? "已设置代理" : "直连"}
            </span>
          </div>
          <div className="launch-proxy-field">
            <Input
              id="codex-launch-proxy-input"
              aria-label="Codex 启动代理"
              aria-invalid={Boolean(launchProxyError)}
              aria-describedby={launchProxyError ? "codex-launch-proxy-error" : undefined}
              value={launchProxy}
              disabled={isBusy}
              placeholder="留空不走代理，例如 http://127.0.0.1:7890"
              rightSection={launchProxy ? (
                <button
                  type="button"
                  className="flex h-5 w-5 cursor-pointer items-center justify-center rounded-full text-[var(--codey-muted,#8e8e93)] hover:bg-[rgb(var(--codey-ink-rgb,0,0,0))]/5 hover:text-[var(--codey-text,#1d1d1f)] transition-colors"
                  disabled={isBusy}
                  onClick={() => onConfigChange({ ...config, codexLaunchProxy: "" })}
                  aria-label="清空代理设置"
                  title="清空代理设置"
                >
                  <IconX size={12} aria-hidden="true" />
                </button>
              ) : undefined}
              onChange={(event) => onConfigChange({ ...config, codexLaunchProxy: event.target.value })}
            />
            {launchProxyError && (
              <small id="codex-launch-proxy-error" className="text-[var(--codey-red,#d70015)]" role="alert">{launchProxyError}</small>
            )}
          </div>
        </div>
      </div>
    </section>
  );
}

export const NetworkProxyPage = memo(NetworkProxyPageComponent);
