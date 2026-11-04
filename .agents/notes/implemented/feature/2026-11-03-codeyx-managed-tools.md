# Agent Note: FastCtx、CodeGraph 与 Ponytail 内置工具管理

Status: implemented

预览分支的 custom 保留范围以[选定能力方案](../simplification/2026-11-04-selected-custom-preview.md)为准；原速度与生命周期定制不再保留，本篇的工具安装、索引范围与配置保护仍适用。

## Problem

CodeGraph 项目索引与 Ponytail 安装会产生网络、用户配置和项目写入副作用，需要显式管理而非成为启动依赖。FastCtx 已有配置及重复加载保护，新增入口不能再维护第二个开关，也不能接管用户自管 MCP 或插件来源。

## Decision

- 从 CodeyX `30e1daa4861762626628e6c2805366cd6519e6f2` 选择性适配后端命令、项目代理和 Ponytail 管理；与[远程及数据功能](../architecture/2026-11-03-codeyx-remote-and-data.md)共享独立分支边界，不覆盖现有启动补丁或整套扩展服务。内置工具页只设 FastCtx、CodeGraph、Ponytail 三卡；FastCtx 沿用 `fastContextTools`、已有用户配置检测及重复加载保护，从原入口迁移而不新增第二个开关。
- CodeGraph 仅在显式开启时下载 `colbymchenry/codegraph` 的 [v1.6.2 release](https://github.com/colbymchenry/codegraph/releases/tag/v1.6.2) 并配置受管 MCP；运行包为 MIT，多平台资产与 `SHA256SUMS` 来自同一 release，**不是独立签名验证**。下载、解压和校验有大小、路径、链接及原子落盘保护；来源冲突和 revision 变化拒绝覆盖用户配置，关闭保留索引和运行包。固定参考实现见[运行包与校验](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/codegraph_runtime.rs#L14-L76)、[下载解压](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/codegraph_runtime.rs#L79-L247)、[配置与关闭](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/commands/builtin_tools.rs#L281-L396)。
- `auto_mcp.cjs` 只为 Codex 已登记的本地项目聊天初始化并监听索引，普通聊天和非项目目录不建库；按已登记根的最长前缀选项目，排除系统根、home 与 Codex home，异常配置拒绝建库。建库锁、异步代理和所属进程回收适配现有生命周期，隔离失败并避免暴露子进程敏感输出。固定参考见[项目范围](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/commands/builtin_tools/auto_mcp.cjs#L14-L75)、[启动和协议](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/commands/builtin_tools/auto_mcp.cjs#L195-L327)。
- Ponytail 状态读取不安装插件；仅用户主动启用未安装插件时调用 Codex 官方 CLI 注册官方市场并安装，不自动信任 hooks，用户在 `/hooks` 自行确认。启停与新会话默认模式 off/lite/full/ultra 分别管理，环境变量覆盖时明示，当前会话以 `/ponytail` 切换；禁用保留文件及参数。配置通过锁、revision、未知字段保留和原子写处理，来源冲突或竞态拒绝覆盖，安装部分成功或超时先刷新状态而非假定回滚。固定参考见[配置和状态](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/ponytail.rs#L30-L53)、[启停](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/ponytail.rs#L145-L250)、[官方安装](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/ponytail.rs#L275-L377)、[界面语义](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/src/features/builtin-tools/PonytailCard.tsx#L8-L31)。
- Ponytail 按官方市场提供版本安装并读取实际缓存版本，不固定截图中的 5.3.3。CodeyX 适配源码保留 AGPL 来源，CodeGraph 运行包与依赖保留各自许可，不将用户动态下载写成随应用内置；声明入口为 `THIRD_PARTY_NOTICES.md`，固定参考见[第三方声明](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/THIRD_PARTY_NOTICES.md#L3-L13)。

## Alternatives considered

- 整体合并或覆盖参考项目：省去逐文件接线，但无共同 merge-base 且会覆盖本仓模型思考档位、原生 Fast/Ultrafast 资格、Linux、遥测和子代理生命周期定制；不采用。
- 启动时自动安装或接管已有配置：减少首次点击，但会在未经用户操作时执行下载和更改配置，破坏已有插件与 MCP 状态；不采用。
- 保留原 FastCtx 入口再新增工具页开关：改动较少，但同一能力出现两个控制入口，容易让保护和状态分叉；因此只迁移卡片，继续使用同一配置及检测结果。

## Verification

- 本地验证入口为 `tests/builtin-tools-auto-mcp.test.mjs`、`tests/codeyx-builtin-tools-controller.test.mjs`、`tests/codeyx-ponytail-controller.test.mjs` 及 `codegraph_runtime.rs`、`commands/builtin_tools.rs`、`ponytail.rs` 中的 Rust 用例；存在用例不代表完成真实安装、索引或平台验收。
- 前端证据限于隔离 Chromium Mock；真实 Codex、CodeGraph/Ponytail 下载安装、索引监听及 hooks 授权需单独实测。本机缺少 `link.exe`，Windows/Linux Rust 构建、测试及打包许可分发以 CI 的实际结果验收，不把 Mock 或源码合同当成编译与真实安装证明。

## Consequences

- 参考 README 声明 Windows 为主要维护平台；虽然代码提供 Linux 运行包路径且有 WSL 辅助，不能据此断言本仓 Linux 桌面端已验证。[平台范围](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/README.md#L26-L30)。
- 同站点哈希不能抵御发布源被篡改；下载包版本和其许可证需按实际资产复核。Ponytail 官方 CLI 可能先完成市场注册再安装失败，无法承诺事务回滚；明确状态并让用户手动重试。
- 三卡管理复用现有配置与扩展服务，启动和状态读取不增加安装副作用；代价是首次启用依赖网络、官方 CLI 与用户钩子授权，关闭不自动回收磁盘数据。若扩大索引范围或接管自管配置，需要重新审查授权边界。
- 活动 note 按 CodeGraph、Ponytail、FastCtx、MCP 和索引关键词审计：与[远程及数据边界](../architecture/2026-11-03-codeyx-remote-and-data.md)交叉引用；[会话选择隔离](../bug-fix/2026-11-03-upstream-selection-boundary.md)及[Windows 测试预算](../testing/2026-11-03-windows-rust-test-parallelism.md)仅互补，不改写原决定，也未审计无关笔记。
