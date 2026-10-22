import assert from "node:assert/strict";
import test from "node:test";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
import { readSource } from "./helpers/read-source.mjs";

const { parseNewApiConnectionInfo, suggestRouteNamesFromUrl } =
  await loadTypeScriptModule(new URL("../src/routeConnectionImport.ts", import.meta.url));

const ccSwitchLink = (params = {}) => `ccswitch://v1/import?${new URLSearchParams({
  resource: "provider",
  app: "codex",
  name: "Sub2API",
  endpoint: "https://api.example.test/v1",
  apiKey: "sk-test",
  ...params,
})}`;

test("parses a newapi channel connection payload", () => {
  const info = parseNewApiConnectionInfo(
    '{"_type":"newapi_channel_conn","key":"sk-abc123","url":"https://api.pei.ee"}',
  );
  assert.deepEqual(info, { key: "sk-abc123", url: "https://api.pei.ee" });
});

test("tolerates surrounding whitespace and wrapped text", () => {
  const payload = '{"_type":"newapi_channel_conn","key":"sk-abc","url":"https://a.dev"}';
  assert.deepEqual(parseNewApiConnectionInfo(`\n ${payload} \n`), {
    key: "sk-abc",
    url: "https://a.dev",
  });
  assert.deepEqual(parseNewApiConnectionInfo(`连接信息：${payload}（完）`), {
    key: "sk-abc",
    url: "https://a.dev",
  });
  assert.deepEqual(
    parseNewApiConnectionInfo(' { "_type" : "newapi_channel_conn", "key" : " sk-k ", "url" : " https://a.dev " } '),
    { key: "sk-k", url: "https://a.dev" },
  );
});

test("rejects non-connection payloads and malformed input", () => {
  for (const text of [
    "",
    "   ",
    null,
    undefined,
    "sk-abc123",
    "not json",
    "[]",
    "null",
    '{"_type":"other","key":"sk-a","url":"https://a.dev"}',
    '{"key":"sk-a","url":"https://a.dev"}',
    '{"_type":"newapi_channel_conn","key":"sk-a"}',
    '{"_type":"newapi_channel_conn","key":1,"url":"https://a.dev"}',
    '{"_type":"newapi_channel_conn","key":"  ","url":"https://a.dev"}',
  ]) {
    assert.equal(parseNewApiConnectionInfo(text), null, JSON.stringify(text));
  }
});

test("parses Sub2API links and ignores supplementary provider configuration", () => {
  const link = ccSwitchLink({
    homepage: "https://sub2api.example.test",
    endpoint: "https://relay.example.test/custom/v1/",
    apiKey: "sk-sub2api-test",
    name: "线路名称不参与导入",
    model: "gpt-5.5",
    configFormat: "json",
    usageEnabled: "true",
    usageScript: Buffer.from('(() => { throw new Error("must not execute"); })()').toString("base64"),
    usageAutoInterval: "30",
  });
  assert.deepEqual(parseNewApiConnectionInfo(link), {
    key: "sk-sub2api-test",
    url: "https://relay.example.test/custom/v1/",
  });
});

test("decodes link parameters once and preserves endpoint paths and query endings", () => {
  for (const endpoint of [
    "https://relay.example.test/prefix/%2F/v1/",
    "https://relay.example.test/v1/?tenant=a%2Bb&values=a%2Cb&empty=",
    "http://127.0.0.1:8080/custom?flag&",
  ]) {
    assert.deepEqual(parseNewApiConnectionInfo(ccSwitchLink({
      endpoint: ` ${endpoint} `,
      apiKey: " sk-测试+/%&=?= ",
    })), { key: "sk-测试+/%&=?=", url: endpoint });
  }
});

test("accepts Sub2API app variants and surrounding clipboard text", () => {
  for (const app of ["codex", "claude", "gemini", "grokbuild"]) {
    const link = ccSwitchLink({ app });
    for (const text of [
      `\n ${link} \n`,
      `连接信息：${link}（完）`,
      `[导入链接](${link})`,
      `<${link}>`,
      `链接：\n${link}\n复制完成`,
    ]) {
      assert.deepEqual(parseNewApiConnectionInfo(text), {
        key: "sk-test",
        url: "https://api.example.test/v1",
      });
    }
  }
});

test("rejects unrelated links, unsupported resources, and ambiguous connection fields", () => {
  const link = ccSwitchLink();
  for (const text of [
    link.replace("ccswitch:", "https:"),
    link.replace("ccswitch:", "sub2api:"),
    link.replace("//v1/", "//v2/"),
    link.replace("//v1/", "//v1.example.test/"),
    link.replace("//v1/", "//user@v1/"),
    link.replace("//v1/", "//v1:1234/"),
    link.replace("/import?", "/other?"),
    link.replace("/import?", "/import/?"),
    `${link}#fragment`,
    ccSwitchLink({ resource: "mcp" }),
    ccSwitchLink({ resource: "" }),
    link.replace("resource=provider&", ""),
    `${link}&resource=provider`,
    `${link}&apiKey=another-key`,
    `${link}&endpoint=https%3A%2F%2Fother.test`,
    `${link}\n${ccSwitchLink({ endpoint: "https://other.test" })}`,
    `https://unrelated.test/?link=${link}`,
    `https://unrelated.test/${link}`,
  ]) {
    assert.equal(parseNewApiConnectionInfo(text), null, text);
  }
});

test("requires an explicit single endpoint and key without using homepage or config fallbacks", () => {
  const link = ccSwitchLink();
  for (const text of [
    link.replace(/&endpoint=[^&]+/, "") + "&homepage=https%3A%2F%2Fhome.test",
    link.replace(/&apiKey=[^&]+/, "") + "&config=eyJhcGlLZXkiOiJzay1mYWxsYmFjayJ9",
    ccSwitchLink({ apiKey: "" }),
    ccSwitchLink({ apiKey: " \n " }),
    ccSwitchLink({ apiKey: "sk-abc\nheader" }),
    ccSwitchLink({ endpoint: "" }),
    ccSwitchLink({ endpoint: "   " }),
    ccSwitchLink({ endpoint: "not a url" }),
    ccSwitchLink({ endpoint: "javascript:alert(1)" }),
    ccSwitchLink({ endpoint: "ftp://relay.example.test" }),
    ccSwitchLink({ endpoint: "https://relay.example.test/line\nbreak" }),
    ccSwitchLink({ endpoint: "https://first.test/v1,https://second.test/v1" }),
    ccSwitchLink({ endpoint: "https://first.test/v1," }),
    link.replace("apiKey=sk-test", "apiKey=%ZZ"),
    link.replace("apiKey=sk-test", "apiKey=%FF"),
  ]) {
    assert.equal(parseNewApiConnectionInfo(text), null, text);
  }
});

test("suggests route names from the connection url", () => {
  assert.deepEqual(suggestRouteNamesFromUrl("https://api.pei.ee", 15, 4), {
    name: "api.pei.ee",
    shortName: "pei",
  });
  assert.deepEqual(suggestRouteNamesFromUrl("https://api.newapi.io/v1", 15, 4), {
    name: "api.newapi.io",
    shortName: "newa",
  });
  assert.deepEqual(suggestRouteNamesFromUrl("https://relay.example.com/v1", 15, 4), {
    name: "",
    shortName: "rela",
  });
  assert.deepEqual(suggestRouteNamesFromUrl("https://this-hostname-is-way-too-long.example.com", 15, 4), {
    name: "",
    shortName: "this",
  });
  assert.deepEqual(suggestRouteNamesFromUrl("not a url", 15, 4), {
    name: "",
    shortName: "",
  });
});

test("the new-route dialog wires the quick import affordance", async () => {
  const source = await readSource("src/ModelSection.tsx");
  assert.match(source, /parseNewApiConnectionInfo/);
  assert.match(source, /快速导入/);
  assert.match(source, /navigator\.clipboard\.readText\(\)/);
  assert.match(source, /route-import-panel/);
  assert.match(source, /isNewRouteDraft && routeImportOpen &&/);
});
