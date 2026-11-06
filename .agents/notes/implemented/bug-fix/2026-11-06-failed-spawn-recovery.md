# Agent Note: 按可信根记录恢复失败的子代理派发

Status: implemented

## Problem

派发前账本已创建 Pending reservation，但 provider 可能返回明确的创建失败而未交付 `PostToolUse`。同轮仍有其他活动子代理时，混合名单缺席无法证明该任务没有创建，根任务会继续被未结算 reservation 阻塞。现场是否漏发 Hook 未经证实。

## Decision

选择性复用 `origin/custom` 的 `d566cd3707867923b20bdc0c0a4bbdcc009e885d` 失败回执扫描及 `93d224a06b9b403199dcd200a2f0088467b1a53f` 时间修正，只从会话自己的可信 root transcript 增量扫描完整、有界记录。只结算 Pending、未绑定且未启动的 reservation；即使曾因中断请求 fence，明确的创建失败仍可证明该任务未产生 provider 工作。缓存损坏、记录不完整、身份或调用歧义均不结算。

有 `tool_use_id` 时精确匹配调用 ID；无 ID 时仅在完整可信 transcript 中任务唯一且 reservation 创建时间落在调用与返回之间才匹配。通过派发 admission 检查后、写 reservation 前保存绑定，避免拒绝的同名派发覆盖已有绑定，也避免缓存写失败留下 provider 未执行的活动 reservation。

## 活动笔记审计

本篇部分取代[选定 custom 范围](../simplification/2026-11-04-selected-custom-preview.md)中不恢复失败派发的决定；不恢复其他终态身份保留、turn 结算或会话选择补丁。[可配并发](../feature/2026-11-04-subagent-concurrency.md)、[自定义角色](../feature/2026-11-04-custom-subagents.md)和[上游适配](2026-11-04-upstream-151-compatibility.md)的角色、权限及上游优先边界继续生效；远程、工具、会话选择相关记录与本次互补，无过时提案需要删除或归档。

## Alternatives considered

- 用混合 `list_agents` 中缺席作为失败依据：无需读取 transcript，但无身份条目、未知 pending 与并发在途请求会造成误判，不能据此释放门禁。
- 整体恢复旧生命周期提交：已有配套测试，但会同时改变终态身份与跨 turn 语义，超出本次明确恢复的范围。

## Consequences

明确失败能在活动 sibling 存在时单独结算，未知或不完整证据继续阻塞；扫描和缓存有大小上限，异常或过大记录不会自动放行。仍需通过 Rust 原生测试、真实 Codex Hook 交付和跨平台构建验证，不以合成 transcript 推断现场漏发原因。
