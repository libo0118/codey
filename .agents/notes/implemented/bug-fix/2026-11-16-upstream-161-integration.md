# Agent Note: 合并上游 1.6.1 并保留独立发布边界

Status: implemented

## Problem

本机新版 Codex 会话流升级为 v13，旧远程端拒绝连接；用户同时要求合并上游 1.6.1 并发布 Latest。上游发布工作流依赖其托管发布服务，不能直接覆盖 fork 的 Windows/Linux、公开清单和独立质量门禁。

## Decision

- 合并 `v1.6.1`（`2bb3967c1cd44df9cf34cb9c8461ac7642f1ab09`），保留 fork 的 Linux、子代理原生只读门禁、原生速率资格和本地插件边界。远程协议仅增加已核验的 v13，依据及拒绝未知版本的规则见[有界远程流](../architecture/2026-11-04-bounded-remote-stream.md)。
- 接纳上游 Windows 打包路径校验、渲染补丁幂等性、额度禁用原因适配、个人结账页预热优化及会话级警告关闭；Crashpad 保留 Unix 文件身份验证，渲染匹配保留 fork 的多种引号与长属性测试。
- 前端检查和嵌入资源只生成一次，Windows/Linux 依赖该成功产物，各自继续执行 Rust 格式、测试、Clippy、构建及打包检查；发布依赖两个平台成功。不接入上游 R2、Copilot 发布说明权限或设备托管凭据。

## Existing-note audit

[1.6.0 集成](2026-11-12-upstream-160-integration.md)保留原模型、子代理及统计决定；[公开发布边界](../architecture/2026-11-03-codeyx-remote-and-data.md)与本篇互补。检索上游、renderer_asset_patch、Crashpad 相关活跃记录，无需归档或删除。

## Alternatives considered

- 整份采用上游工作流能直接复用其发布平台，但本 fork 无对应服务凭据且需 Linux 包；只复用共享前端，保留现有独立发布流程。
- 继续在每个平台重复前端检查与构建可少改工作流，但无法采用上游的共享产物优化；将相同检查移入两个平台共同依赖的任务，不减少门禁。

## Consequences

兼容修复与上游运行时改进一并交付，发布仍需双平台 CI 成功。工作流断言随检查位置迁移，保持发布前检查语义不变；Windows 原生执行由 CI 验证，不把本机格式检查或只读 IPC 订阅当成真实发送、停止、审批或安装验收。发布不会自行重启当前聊天或部署远端 AGV/HAP 服务。
