# Agent Note: 可选 CodeGraph 与 Ponytail 管理

Status: proposed

## Problem

当前仓库已有 Codex MCP、Skill 和插件管理，但没有 CodeGraph 自动项目索引或 Ponytail 专用状态、启停与默认模式界面。两者是有网络、用户配置和项目写入副作用的可选能力，不能成为应用启动依赖，也不能改变既有插件状态。

## Proposal

- 从 CodeyX 固定版本按功能挑取管理状态、后端命令、`auto_mcp.cjs`、Ponytail 配置及对应 UI/测试；适配本仓现有设置路由与扩展服务，不覆盖 launcher、启动补丁或整套 `codex_extensions`。需要触及后端模块注册、`AppState` 作业互斥、命令分发、前端 `invoke` 白名单、设置页/组件、类型、mock、测试、依赖清单与第三方声明。主仓 `src/App.tsx:1721-1726` 已接插件、MCP、Skill 页面；`backend/src/codex_extensions/mod.rs:13-53` 已实现扩展分发和受确认及 revision 保护的测试。
- CodeGraph 的显式开启才下载官方 1.6.2 运行包并配置受管 MCP；用现有 MCP 配置管理做来源、冲突和 revision 检查。只为 Codex 已登记的本地项目聊天初始化与监听，普通聊天和非项目目录不建库；关闭只停用接入，不删除索引和运行包。运行失败、下载/解压/校验失败、用户自管 MCP 冲突均应可见且不覆盖用户配置。参考实现使用同源 `SHA256SUMS` 和运行包的哈希校验，**不是独立签名验证**；应保留有限大小、路径与链接校验、临时目录和原子落盘，不能宣称已经验证发布者签名。[运行包与校验](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/codegraph_runtime.rs#L14-L76)、[下载解压](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/codegraph_runtime.rs#L79-L247)、[配置与关闭](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/commands/builtin_tools.rs#L281-L396)。
- 可复用参考项目匹配、建库锁与 MCP 异步代理，但必须适配本仓本地聊天生命周期、路径、进程回收及 Linux 原生实现；失败时其他聊天及应用仍可用。项目选择遵循已登记根路径及最长前缀，排除系统根、home 和 Codex home，异常配置拒绝建库；清理环境中会影响路径、守护或遥测的变量，索引过程不暴露子进程敏感输出。[项目范围](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/commands/builtin_tools/auto_mcp.cjs#L14-L75)、[启动和协议](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/commands/builtin_tools/auto_mcp.cjs#L195-L327)。
- Ponytail 读取官方插件与配置状态；用户明确启用未安装插件时才调用 Codex 官方 CLI 注册官方市场并安装，用户需自行在 `/hooks` 检查并信任钩子。启停与默认参数分别操作；配置保存使用锁、revision、原字段保留和原子写，来源冲突、配置失效或竞态须停止并提示刷新。新聊天默认值提供 off/lite/full/ultra；环境变量覆盖时明示；当前聊天以 `/ponytail` 切换。禁用保留插件文件及参数。安装部分成功或超时须提示先刷新状态，不能假定回滚或静默重试。[配置和状态](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/ponytail.rs#L30-L53)、[启停](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/ponytail.rs#L145-L250)、[官方安装](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/ponytail.rs#L275-L377)、[界面语义](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/src/features/builtin-tools/PonytailCard.tsx#L8-L31)。
- 参考 Ponytail 安装流程未固定 5.3.3，而按官方市场当前提供版本安装并从缓存读取版本；截图版本不能当作 pin 或保证。参考项目声明 CodeGraph 为 MIT、运行包及其依赖保留各自许可；移植 CodeyX 源码需保留 AGPL 来源和第三方声明，检查 Windows 与 Linux 安装包中的 LICENSE/NOTICE 分发，不将用户动态下载的运行包误写成随应用内置。[第三方声明](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/THIRD_PARTY_NOTICES.md#L3-L13)。

## Alternatives considered

- 整体合并或覆盖参考项目：省去逐文件接线，但无共同 merge-base 且会覆盖本仓模型思考档位、原生 Fast/Ultrafast 资格、Linux、遥测和子代理生命周期定制；不采用。
- 启动时自动安装或接管已有配置：减少首次点击，但会在未经用户操作时执行下载和更改配置，破坏已有插件与 MCP 状态；不采用。

## Acceptance criteria

- Windows 与 Linux 均可构建；应用不联网、不安装、不写用户 Codex 配置也能启动。用户明确操作前不执行安装器、不启用插件、不信任 hooks、不删除索引。
- 仅登记本地项目聊天建库并持续同步；普通聊天、无效根、配置冲突、嵌套项目、并发启动、失效索引、退出或失败均有隔离测试；关闭/刷新保留已有数据。
- 覆盖运行包损坏、路径穿越/链接/超限、下载失败及未完成安装，Ponytail 官方来源冲突、配置损坏/并发变更、部分安装与超时，前端读取/保存/刷新/重试及环境变量覆盖。不得触碰现有 FastCtx 和模型选择行为；复用参考 `tests/builtin-tools-auto-mcp.test.mjs`、`tests/builtin-tools-controller.test.mjs`、`tests/ponytail-controller.test.mjs`、Rust 单测和浏览器测试并适配本仓。
- 保留当前项目依赖和第三方授权声明，核对两平台打包清单。真实官方安装及钩子授权只在单独隔离环境并经用户授权后验证，不针对现有用户目录执行。

## Risks

- 参考 README 声明 Windows 为主要维护平台；虽然代码提供 Linux 运行包路径且有 WSL 辅助，不能据此断言本仓 Linux 桌面端已验证。[平台范围](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/README.md#L26-L30)。
- 同站点哈希不能抵御发布源被篡改；下载包版本和其许可证需按实际资产复核。Ponytail 官方 CLI 可能先完成市场注册再安装失败，无法承诺事务回滚；明确状态并让用户手动重试。
- 已检索当前活动 notes 中 CodeGraph、Ponytail、插件、MCP、索引关键词，未找到重叠决策；未审计与本功能无关的笔记。未执行安装或运行时烟测。
