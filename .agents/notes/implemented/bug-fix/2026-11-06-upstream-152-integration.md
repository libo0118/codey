# Agent Note: 合并上游 1.5.2 并保留 fork 的角色与发布边界

Status: implemented

## Problem

上游 v1.5.2 改进了子代理角色、调度与统计，同时将官方发布与设备管理依赖于独立服务。整份采用上游冲突文件会丢失 fork 自定义角色、失败派发恢复、直连计时或 Windows/Linux 公开发布；保留旧实现又会错过上游演进。

## Decision

- 合并上游正式标签 v1.5.2 指向的提交 `d345b82065b701cea35809af6661760be3522013`。内置角色采用上游 `codey_explorer`、`codey_worker`、`codey_reviewer` 和内部 `default`，旧角色设置按上游规则迁移；现有新角色设置优先，同一后继角色有多个旧设置时优先启用的设置。
- 保留自定义角色的受管注册、备份、显式删除和只读权限边界，以及可配置的 1..64 子代理并发上限。Explorer 使用原生只读证明，Reviewer 可写测试与构建产物并按潜在写入者调度；只有全部活动任务具备当前轮只读证明，根任务才可并行写入。
- 保留基于可信根 transcript 的精确失败派发恢复：只结算未绑定、未启动的 Pending，已 fenced 不排除明确失败，未知或不完整证据仍不放行。上游成功且未中断子代理的跨轮续用属于上游演进，不恢复已裁剪的其他 fork 生命周期补丁。
- 会话统计采用上游新增与累计用量、缓存口径和空闲轮询退避，同时保留 fork 的原生 OTLP 直连计时；运行期来源不与 rollout 累计混加，不跨来源拼接本轮计时。
- `build-desktop.yml` 完整保留 fork 的 Windows/Linux 打包、GitHub Release 与公开更新清单流程，固定 fork 的更新地址，不注入发布管理地址或构建标识，不接入上游 macOS、R2 和发布管理流水线。

## 活动笔记审计

本篇部分更新[自定义角色](../feature/2026-11-04-custom-subagents.md)和[可配并发](../feature/2026-11-04-subagent-concurrency.md)中的内置角色事实；其配置、显式删除和权限取舍继续生效。[失败派发恢复](2026-11-06-failed-spawn-recovery.md)保留原有决定，[选定 custom 范围](../simplification/2026-11-04-selected-custom-preview.md)仍限制恢复其他旧补丁；[上游 1.5.1 兼容](2026-11-04-upstream-151-compatibility.md)与[上游选择边界](2026-11-03-upstream-selection-boundary.md)保留各自理由及已选遥测范围。[内置工具管理](../feature/2026-11-03-codeyx-managed-tools.md)的安装与配置边界互补，远程投影、远程文件及 Windows 测试预算不涉及本次决定；没有完全吸收的活动笔记或需要处置的提案。

## Alternatives considered

- 整体采用上游冲突文件：最贴近官方发布流程，但要求 fork 接入现未使用的 macOS 构建、R2 与发布管理，且会丢失现有 fork 用户能力；保留 fork 工作流，只对重叠运行逻辑选择上游演进。
- 保留旧版五角色与全部旧生命周期机制：能维持原有角色分类，但会错过上游只读证明与后续轮次语义，并重新引入用户明确撤下的补丁；迁移角色设置，仅保留已单独确认的失败派发恢复。
- 把路由与原生 trace 或累计用量合并：显示可能更完整，但来源和时间范围不同，可能重复计数；按确认来源整组选取，缺少数据保持未知。

## Verification

角色与调度的检查入口包括 `backend/src/config/subagent/tests.rs`、`backend/src/subagent_orchestrator/scheduling_tests.rs` 和 `backend/src/subagent_gate/failed_spawn_recovery_tests.rs`；统计和公开清单检查入口包括 `tests/conversation-stats-inject.test.mjs` 与 `tests/update-manifest.test.mjs`。实际执行状态以主代理与 CI 的记录为准，真实 Codex 派发和设备更新仍需单独验证。

## Consequences

官方角色与调度随上游演进，自定义角色和既有公开发布渠道仍有独立维护成本；旧活动任务及未知、迟到事件继续受身份和权限约束。公开清单不提供上游发布管理的设备定向推送与回滚。最终 Rust、全量测试与跨平台打包由主代理和 CI 按实际执行结果验收；合成测试、静态检查或本笔记不代表真实 Codex、目标主机和客户端已验证。
