import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import * as jsx from "react/jsx-runtime";
import * as icons from "@tabler/icons-react";
import MarkdownIt from "markdown-it";
import { renderToStaticMarkup } from "react-dom/server";
import { createControllerHarness } from "./helpers/controller-harness.mjs";
import { collectElements, createModuleGraph, textContent } from "./helpers/jsx-tree.mjs";

const stubs = { react: React, "react/jsx-runtime": jsx, "@tabler/icons-react": icons, "markdown-it": MarkdownIt };
const { ConversationTurn } = createModuleGraph(new URL("../src/remote/messages.tsx", import.meta.url), { stubs }).exports;
const render = messages => renderToStaticMarkup(React.createElement(ConversationTurn, { turn: { id: "turn", status: "completed", messages } }));
const user = text => ({ id: "user", kind: "userMessage", role: "user", text });

test("用户消息只直接显示实际提问，自动上下文保留在折叠区而不是正文", () => {
  const text = '<in-app-browser-context source="ambient-ui-state">\nContext sentinel\n</in-app-browser-context>\n\n## My request:\n真正的问题';
  const html = render([user(text)]);
  assert.match(html, /class="remote-user-text">真正的问题<\/div>/);
  assert.match(html, /<details class="remote-user-context"><summary>附加上下文<\/summary>/);
  assert.match(html, /Context sentinel/);
  assert.equal(render([user("普通正文\n\n## My request:\n仍是正文")]).includes("普通正文"), true);
});

test("提问回复呈现问题和答案，不显示协议标签或 JSON；未知格式保留原文", () => {
  const reply = { questionItemId: '["request_user_input_async","q",0]', question: "选哪个？", answer: "第二项" };
  for (const payload of [[reply], reply]) {
    const html = render([user(`<send_user_message_question_reply>\n${JSON.stringify(payload)}\n</send_user_message_question_reply>`)]);
    assert.match(html, /class="remote-question-reply"/);
    assert.match(html, /选哪个？/); assert.match(html, /第二项/);
    assert.doesNotMatch(html, /send_user_message_question_reply|questionItemId/);
  }
  assert.match(render([user('<send_user_message_question_reply>{"unknown":true}</send_user_message_question_reply>')]), /unknown/);
});

test("连续过程合为一个懒加载入口，回答穿插顺序不变，空摘要不生成空详情卡", () => {
  const messages = [
    { id: "a", role: "activity", kind: "reasoning", text: "" },
    { id: "b", role: "activity", kind: "commandExecution", text: "detail sentinel", status: "completed" },
    { id: "answer", role: "assistant", kind: "agentMessage", text: "回答" },
    { id: "c", role: "activity", kind: "reasoning", text: "公开摘要" },
  ];
  const html = render(messages);
  assert.equal((html.match(/class="remote-process"/g) || []).length, 2);
  assert.doesNotMatch(html, /detail sentinel|remote-activity-reasoning|remote-activity-commandExecution/);
  assert.ok(html.indexOf('data-message-id="a"') < html.indexOf('data-message-id="answer"'));
  assert.ok(html.indexOf('data-message-id="answer"') < html.indexOf('data-message-id="c"'));
});

test("过程展开才创建详情，嵌套详情关闭不折叠整个过程", () => {
  const h = createControllerHarness(new URL("../src/remote/messages.tsx", import.meta.url), (exports) => {
    const tree = exports.ConversationTurn({ turn: { id: "turn", status: "inProgress", messages: [
      { id: "empty", role: "activity", kind: "reasoning", text: "" },
      { id: "tool", role: "activity", kind: "mcpToolCall", text: "", tool: { name: "lookup", output: "body" } },
    ] } });
    const group = collectElements(tree, node => Array.isArray(node.props.messages))[0];
    return group.type(group.props);
  }, { "@tabler/icons-react": icons, "markdown-it": MarkdownIt });
  try {
    let tree = h.render(); assert.match(textContent(tree), /正在工作/);
    assert.equal(collectElements(tree, node => node.props.message).length, 0);
    const outer = { open: true }; tree.props.onToggle({ target: outer, currentTarget: outer }); tree = h.render();
    assert.equal(collectElements(tree, node => node.props.message).length, 1);
    tree.props.onToggle({ target: { open: false }, currentTarget: outer }); tree = h.render();
    assert.equal(collectElements(tree, node => node.props.message).length, 1);
  } finally { h.unmount(); }
});

test("过程摘要只用最新公开动作，不提前创建工具输出或思考正文", () => {
  const messages = [
    { id: "old", role: "activity", kind: "commandExecution", text: "", command: { command: "old-command", output: "OLD_OUTPUT" } },
    { id: "tool", role: "activity", kind: "mcpToolCall", status: "failed", text: "PRIVATE_TEXT", tool: { name: "inspect_local_file", input: "PRIVATE_INPUT", output: "HIDDEN_OUTPUT" } },
    { id: "reason", role: "activity", kind: "reasoning", text: "HIDDEN_REASONING" },
  ];
  const html = render(messages);
  assert.match(html, /remote-process-preview">MCP 工具 · inspect_local_file/);
  assert.doesNotMatch(html, /old-command|OLD_OUTPUT|PRIVATE_TEXT|PRIVATE_INPUT|HIDDEN_OUTPUT|HIDDEN_REASONING|任务失败|已完成/);
  assert.equal((html.match(/<details /g) || []).length, 1);
  for (const [message, expected] of [
    [{ kind: "commandExecution", command: { command: "npm run check\nDO_NOT_PREVIEW", output: "HIDDEN_OUTPUT" } }, /执行命令 · npm run check/],
    [{ kind: "fileChange", changes: [{ path: "src/App.tsx", diff: "HIDDEN_DIFF" }] }, /文件变更 · 1 个文件 · src\/App.tsx/],
    [{ kind: "plan", plan: [{ step: "已做的步骤", status: "completed" }, { step: "验证页面", status: "in_progress" }] }, /工作计划 · 验证页面/],
  ]) {
    const preview = render([{ id: "action", role: "activity", text: "", ...message }]);
    assert.match(preview, expected);
    assert.doesNotMatch(preview, /DO_NOT_PREVIEW|HIDDEN_OUTPUT|HIDDEN_DIFF|已做的步骤/);
  }
});

test("问题卡默认可折叠，选项和手写答案仍只在明确提交后发送", async () => {
  const graph = createModuleGraph(new URL("../src/features/remote-workspace/Workspace.tsx", import.meta.url), { stubs: { "@tabler/icons-react": icons, "markdown-it": MarkdownIt, "./useRemoteWorkspaceController": {} } });
  const calls = [], request = { id: "async:q", method: "codey/requestUserInputAsync", supported: true, params: { questions: [{ id: "q", question: "选哪个？", options: [{ label: "第一项" }, { label: "第二项" }] }] } };
  const props = { request, disabled: false, onRespond: async args => calls.push(args) };
  let tree = graph.exports.ApprovalCard(props);
  assert.equal(tree.type, "details"); assert.notEqual(tree.props.open, true);
  const option = collectElements(tree, node => node.type === "button" && textContent(node) === "第二项")[0];
  option.props.onClick(); graph.restart(); tree = graph.exports.ApprovalCard(props); assert.equal(calls.length, 0);
  const input = collectElements(tree, node => node.type === "input")[0]; assert.equal(input.props.value, "第二项");
  input.props.onChange({ target: { value: "自填答案" } }); graph.restart(); tree = graph.exports.ApprovalCard(props);
  const form = collectElements(tree, node => node.type === "form")[0]; form.props.onSubmit({ preventDefault() {} });
  assert.deepEqual(calls, [{ answers: { q: "自填答案" } }]);
});
