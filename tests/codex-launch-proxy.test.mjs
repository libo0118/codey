import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
import { autoStubModule, collectElements, createModuleGraph, elementProps, elementType } from "./helpers/jsx-tree.mjs";

const root = new URL("../", import.meta.url);

test("codex launch proxy validator matches the launch-time constraints", async () => {
  const { validateCodexLaunchProxy } = await loadTypeScriptModule(
    new URL("../src/urlValidation.ts", import.meta.url),
  );

  for (const accepted of [
    "",
    "http://127.0.0.1:7890",
    "https://proxy.example.com:8443",
    "socks5://127.0.0.1:1080",
    "socks5h://proxy.example.com:1080",
    "http://[::1]:7890",
    // 凭据交给本地认证中继，不交给 Chromium 命令行。
    "http://user:pass@proxy.example.com:8080",
    "socks5://user@127.0.0.1:1080",
  ]) {
    assert.equal(validateCodexLaunchProxy(accepted), "", accepted);
  }

  for (const rejected of [
    // 只允许协议、地址、端口和认证信息。
    "http://127.0.0.1:7890/v1",
    "socks5://127.0.0.1:1080/v1",
    "http://127.0.0.1:7890?x=1",
    "http://127.0.0.1:7890#frag",
    // SOCKS 必须显式端口。
    "socks5://127.0.0.1",
    "socks5h://proxy.example.com",
    // RFC 1929 的用户名和密码各限 255 字节。
    `socks5://${"u".repeat(256)}@127.0.0.1:1080`,
    // 协议与格式。
    "ftp://proxy.example.com:21",
    "127.0.0.1:7890",
    "not a url",
  ]) {
    assert.notEqual(validateCodexLaunchProxy(rejected), "", rejected);
  }
});

test("launch proxy has a single independent settings page and shares the existing draft save", async () => {
  const [cardSource, typesSource, mockSource, oldSource, appSource, layoutSource] = await Promise.all([
    readFile(new URL("src/NetworkProxyPage.tsx", root), "utf8"),
    readFile(new URL("src/App.types.ts", root), "utf8"),
    readFile(new URL("src/dev/mockApi.ts", root), "utf8"),
    readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/SettingsLayout.tsx", root), "utf8"),
  ]);

  assert.match(typesSource, /codexLaunchProxy: string/);
  assert.match(mockSource, /codexLaunchProxy: ""/);
  assert.match(cardSource, /title="网络代理"/);
  assert.doesNotMatch(oldSource, /codexLaunchProxy|codex-launch-proxy-input/);
  assert.match(layoutSource, /id: "networkProxy", title: "网络代理"/);
  assert.match(appSource, /networkProxy: <NetworkProxyPage config=\{config\} isBusy=\{isBusy\} onConfigChange=\{handleConfigChange\}/);
  assert.match(cardSource, /codexLaunchProxy: event\.target\.value/);
  assert.match(cardSource, /aria-invalid=\{Boolean\(launchProxyError\)\}/);
  assert.match(cardSource, /validateCodexLaunchProxy\(launchProxy\)/);
  assert.match(cardSource, /重启 Codex 后生效/);
});

test("network proxy edits and clear preserve the draft while validation and busy state remain intact", () => {
  const ui = autoStubModule("ui");
  const graph = createModuleGraph(new URL("../src/NetworkProxyPage.tsx", import.meta.url), { stubs: {
    "./components/ui": ui, "./SettingsPageHeader": autoStubModule("header"), "@tabler/icons-react": autoStubModule("icons"),
  } });
  const changes = [];
  const config = { codexLaunchProxy: "http://127.0.0.1:7890", retainedDraft: "same" };
  const render = (current = config, isBusy = false) => graph.exports.NetworkProxyPage({ config: current, isBusy, onConfigChange: next => changes.push(next) });
  const input = tree => elementProps(collectElements(tree, node => elementType(node) === ui.Input)[0]);
  const field = input(render());
  field.onChange({ target: { value: "socks5://127.0.0.1:1080" } });
  assert.deepEqual(changes[0], { ...config, codexLaunchProxy: "socks5://127.0.0.1:1080" });
  elementProps(field.rightSection).onClick();
  assert.deepEqual(changes[1], { ...config, codexLaunchProxy: "" });
  assert.equal(input(render({ ...config, codexLaunchProxy: "bad" }))["aria-invalid"], true);
  const busy = input(render(config, true));
  assert.equal(busy.disabled, true);
  assert.equal(elementProps(busy.rightSection).disabled, true);
});

test("launch proxy reaches Chromium arguments, spawn environment and restart detection", async () => {
  const [configSource, commandsSource, processSource, proxySource, appServerSource] =
    await Promise.all([
      Promise.all([
        readFile(new URL("backend/src/config/types.rs", root), "utf8"),
        readFile(new URL("backend/src/config/normalize.rs", root), "utf8"),
      ]).then((parts) => parts.join("\n")),
      readFile(new URL("backend/src/commands.rs", root), "utf8"),
      readFile(new URL("backend/src/launcher/process.rs", root), "utf8"),
      readFile(new URL("backend/src/launch_proxy.rs", root), "utf8"),
      readFile(new URL("backend/src/appserver_call.rs", root), "utf8"),
    ]);

  assert.match(configSource, /pub codex_launch_proxy: String/);
  assert.match(configSource, /fn validate_codex_launch_proxy/);
  assert.match(commandsSource, /validate_codex_launch_proxy\(codex_launch_proxy\)/);
  assert.match(
    commandsSource,
    /applied\.codex_launch_proxy != current\.codex_launch_proxy/,
  );
  assert.match(processSource, /proxy\.chromium_argument\(\)/);
  assert.match(proxySource, /--proxy-server=/);
  assert.match(proxySource, /"socks5"[\s\S]{0,40}else/);
  assert.match(proxySource, /"HTTP_PROXY"/);
  assert.match(proxySource, /"NO_PROXY"/);
  assert.match(
    processSource,
    /cli_runtime_environment[\s\S]*launch_proxy_environment/,
  );
  assert.match(
    appServerSource,
    /shared_app_server_environment\(\)/,
  );
});

test("credentialed launch proxy goes through the local auth relay", async () => {
  const [proxySource, relaySource, launcherSource] = await Promise.all([
    readFile(new URL("backend/src/launch_proxy.rs", root), "utf8"),
    readFile(new URL("backend/src/launch_proxy_relay.rs", root), "utf8"),
    readFile(new URL("backend/src/launcher.rs", root), "utf8"),
  ]);

  // 带凭据的上游解析为中继计划，启动时落成回环地址，客户端与子进程都
  // 拿不到凭据。
  assert.match(proxySource, /LaunchProxyPlan::Relayed/);
  assert.match(proxySource, /fn relayed\(port: u16\)/);
  assert.match(proxySource, /http:\/\/127\.0\.0\.1:\{port\}/);
  assert.match(launcherSource, /plan\.start\(\)\.await/);
  // 中继向上游注入认证：http(s) 走 Proxy-Authorization，socks5 走 RFC 1929。
  assert.match(relaySource, /Proxy-Authorization/);
  assert.match(relaySource, /0x02|method.*auth|socks5_negotiate/);
  assert.match(relaySource, /pub\(crate\) async fn ensure_running/);
});
