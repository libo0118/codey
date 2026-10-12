import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(new URL("../src/remoteCompactionStatus.ts", import.meta.url), "utf8");
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
const { remoteCompactionStatusText, getRouteCompactionStatus } = await import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);

test("saving remote compaction does not claim it is already active", () => {
  const local = remoteCompactionStatusText({ running: true, remoteCompaction: {
    configured: true, active: false, restartRequired: true, blockingRoutes: [],
  } });
  assert.equal(local.title, "当前使用本地压缩");
  assert.match(local.detail, /已保存配置支持远程压缩.*需重启 Codex/);
  const remote = remoteCompactionStatusText({ running: true, remoteCompaction: {
    configured: false, active: true, restartRequired: true, blockingRoutes: [],
  } });
  assert.equal(remote.title, "当前已启用远程压缩");
  assert.match(remote.detail, /未启用远程压缩.*需重启 Codex/);
});

test("missing status, stopped Codex and unmanaged compression remain distinct", () => {
  assert.equal(remoteCompactionStatusText({ running: true }).title, "压缩方式待确认");
  const remoteCompaction = { configured: true, active: null, restartRequired: false, blockingRoutes: [] };
  assert.equal(remoteCompactionStatusText({ running: false, remoteCompaction }).title, "Codex 未运行");
  assert.equal(remoteCompactionStatusText({ running: true, remoteCompaction }).title, "压缩由 Codex 自行管理");
  assert.equal(remoteCompactionStatusText(undefined).title, "压缩方式待确认");
});

test("mixed routes advertise independent compaction and capability changes require restart", () => {
  const status = { running: true, remoteCompaction: {
    configured: true, active: true, configuredMode: "mixed", activeMode: "mixed",
    restartRequired: true, blockingRoutes: [{ routeName: "Chat", reason: "上游协议不支持原生压缩" }],
  } };
  const text = remoteCompactionStatusText(status);
  assert.equal(text.title, "当前按线路选择压缩方式");
  assert.match(text.detail, /支持的线路独立使用远程压缩/);
  assert.match(text.detail, /需重启 Codex/);
});

test("getRouteCompactionStatus correctly identifies official, remote, and local routes", () => {
  const officialRoute = {
    id: "route-official",
    name: "plus",
    authMode: "officialAccount",
    officialAccount: true,
  };
  const offStatus = getRouteCompactionStatus(officialRoute, { running: true });
  assert.equal(offStatus.mode, "remote");
  assert.equal(offStatus.badgeText, "远程压缩");
  assert.equal(offStatus.badgeVariant, "success");
  assert.match(offStatus.tooltip, /官方账号原生支持远程压缩/);

  const unavailableOfficial = getRouteCompactionStatus(officialRoute, { running: true }, false);
  assert.equal(unavailableOfficial.mode, "local");
  assert.equal(unavailableOfficial.badgeText, "本地压缩");
  assert.equal(unavailableOfficial.badgeVariant, "secondary");
  assert.equal(unavailableOfficial.reason, "官方账号未就绪");

  const customRemoteRoute = {
    id: "route-custom-remote",
    name: "自建-开启",
    authMode: "apiKey",
    upstreamProtocol: "openaiResponses",
    supportsRemoteCompaction: true,
    remoteCompactionProtocol: "compactEndpoint",
  };
  const customRemoteStatus = getRouteCompactionStatus(customRemoteRoute, { running: true });
  assert.equal(customRemoteStatus.mode, "remote");
  assert.equal(customRemoteStatus.badgeText, "远程压缩");
  assert.equal(customRemoteStatus.badgeVariant, "success");
  assert.match(customRemoteStatus.tooltip, /独立压缩接口/);

  const customDisabledRoute = {
    id: "route-custom-disabled",
    name: "自建-未开启",
    authMode: "apiKey",
    upstreamProtocol: "openaiResponses",
    supportsRemoteCompaction: false,
  };
  const customDisabledStatus = getRouteCompactionStatus(customDisabledRoute, { running: true });
  assert.equal(customDisabledStatus.mode, "local");
  assert.equal(customDisabledStatus.badgeText, "本地压缩");
  assert.equal(customDisabledStatus.badgeVariant, "secondary");
  assert.equal(customDisabledStatus.reason, "未开启远程压缩");

  const chatRoute = {
    id: "route-chat",
    name: "obu-gpt",
    authMode: "apiKey",
    upstreamProtocol: "openaiChatCompletions",
    supportsRemoteCompaction: true,
  };
  const chatStatus = getRouteCompactionStatus(chatRoute, { running: true });
  assert.equal(chatStatus.mode, "local");
  assert.equal(chatStatus.badgeText, "本地压缩");
  assert.equal(chatStatus.badgeVariant, "secondary");
  assert.equal(chatStatus.reason, "上游协议不支持原生压缩");

  const runtimeBlockedRoute = {
    id: "route-wa",
    name: "wa",
    authMode: "apiKey",
    upstreamProtocol: "openaiResponses",
    supportsRemoteCompaction: false,
  };
  const runtimeStatus = {
    running: true,
    remoteCompaction: {
      configured: true,
      active: true,
      restartRequired: true,
      blockingRoutes: [{ routeId: "route-wa", routeName: "wa", reason: "未开启远程压缩" }],
    },
  };
  const waStatus = getRouteCompactionStatus(runtimeBlockedRoute, runtimeStatus);
  assert.equal(waStatus.mode, "local");
  assert.equal(waStatus.badgeText, "本地压缩");
  assert.equal(waStatus.reason, "未开启远程压缩");

  const restartRouteStatus = getRouteCompactionStatus(customRemoteRoute, runtimeStatus);
  assert.equal(restartRouteStatus.mode, "remote");
  assert.match(restartRouteStatus.tooltip, /需重启 Codex 生效/);
});
