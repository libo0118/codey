# Agent Note: 新版 Codex 适配优先使用上游实现

Status: implemented

## Problem

CodeyX 适配与官方数字 chunk 支持有重叠；叠加宽泛拦截会绕过离线候选扫描，整份覆盖又会丢失 fork 的远程请求门禁及已确认入口。

## Decision

- 合并官方 v1.5.1 的准确提交 `34ab9a0b33721a6d839c60a7d712cd1dba10a0e0`，重叠功能以此基线为准。数字 chunk 使用官方 `renderer_asset_scan`、严格 URL 与正文锚点、候选缓存和实际 Fetch 规则刷新；移除本地末位数字通配与仅识别请求客户端的预筛。
- `renderer_asset_patch.js` 使用上游实现与 1500 字符匹配预算，仅保留已覆盖的单引号入口；双引号、反引号同样要求引号配对。严格候选不再接纳任意嵌套 assets 路径。
- 保留 fork 的 `requestGateReady`、原型回滚、后台 settle、远程 ready 校验及 owner follow；本地 manager 探测继续使用 declaredMethod、有界 BFS 并跳过 getter，不用上游整份注入脚本替换。用量响应原样保留，只对已确认第三方模型调整发送条件，不引入全局 JSON/WebSocket 改写。
- 官方账号导入导出、配置应用与重启判定、协议工具转换、原有子代理修复照常合并；未选的 custom 速度、额外生命周期与 observer 补丁不恢复，也不引入 CodeyX Inspector/WSL 补丁。源树候选版本统一为 `1.5.1-libo.1-preview.1`，不创建标签或修改既有发布。

## 活动笔记审计

[选定 custom 范围](../simplification/2026-11-04-selected-custom-preview.md)继续决定保留能力，此处仅将重叠实现交回官方；[可配并发](../feature/2026-11-04-subagent-concurrency.md)、[远程文件](../feature/2026-11-04-remote-files-and-projection.md)与[公开消息](../feature/2026-11-04-remote-rich-timeline-and-links.md)互补，保存、权限及数据边界不变。[上游 v1.5.2 整合](2026-11-06-upstream-152-integration.md)延续上游优先与 fork 独有能力的边界。无过时提案需要删除或归档。

## Alternatives considered

- 保留两套数字 chunk 拦截：能继续命中旧测试，但重复通配会使候选扫描失效；采用官方扫描并修正测试候选路径。
- 整份覆盖注入与远程代码：冲突最少，但会丢失已完成的请求就绪、只读探测和远程授权功能；只解决真正重叠的代码块。

## Consequences

上游成为真实 Git 祖先，重复适配被删除，fork 的独立能力仍在同一分支。Node 合成与源码契约检查覆盖数字 chunk、三种引号、Fetch 扫描接线、门禁及用量；它们不等于 Rust 执行或真实 Codex/手机验证。本机缺少 `link.exe`，原生测试及最终全量验收由主代理和 CI 执行；不读取真实会话、移动存储或修改全局 `CODEX_HOME`。
