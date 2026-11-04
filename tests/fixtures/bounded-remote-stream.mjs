// Synthetic public history only; never import a real transcript into this fixture.
export const mockThreadId = "12345678-1234-1234-1234-123456789abc";
export function largeRemoteView() {
  return {
    id: mockThreadId, title: "近期优先与按需详情（Mock）", cwd: "D:/Mock/workspace", status: "idle",
    model: "Mock-model", effort: "high", serviceTier: null, permissionMode: "auto", historyComplete: true,
    requests: [], turns: Array.from({ length: 14 }, (_, turn) => ({
      id: `turn-${turn}`, status: "completed", startedAt: 1000, completedAt: 2000,
      messages: Array.from({ length: 200 }, (_, item) => {
        const id = `message-${turn}-${item}`;
        if (item % 20 === 0) return { id, role: "assistant", kind: "agentMessage", text: `### 公开回答 ${turn}/${item}（Mock）\n\n只使用合成内容，未连接真实服务。` };
        if (item === 199) return { id, role: "activity", kind: "subAgentActivity", text: "", collaboration: { action: "completed", agents: [{ id: "Mock-child", name: "核验代理（Mock）", status: "completed", message: `**Mock lazy detail body**\n\n${"公开结果（Mock）。".repeat(100)}` }] } };
        return { id, role: "activity", kind: "mcpToolCall", status: "completed", text: "", tool: { name: "Mock.inspect", output: `**Mock lazy detail body**\n\n${"公开结果（Mock）。".repeat(100)}` } };
      }),
    })),
  };
}

export function remoteWindow(full, count = 20) {
  let remaining = count;
  const turns = full.turns.map(turn => ({ ...turn, messages: [] }));
  for (let index = turns.length - 1; index >= 0 && remaining > 0; index--) {
    turns[index].messages = full.turns[index].messages.slice(-remaining);
    remaining -= turns[index].messages.length;
  }
  return { ...full, turns, hasEarlierMessages: count < full.turns.reduce((total, turn) => total + turn.messages.length, 0) };
}
