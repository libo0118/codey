# Agent Note: 远程控制、会话管理与配置备份的移植边界

Status: proposed

## Problem

现有 Codey 缺少控制台中的远程控制、会话批量管理及配置备份页面；CodeyX 与当前分支没有共同祖先，整体合并会覆盖本分支的模型档位、原生 Fast 资格、Linux、遥测和子代理生命周期定制。用户要求账号密码登录并记住登录、默认监听 `0.0.0.0:18787` 且端口可配置，允许局域网；不集成 HTTPS 代理、内网穿透或云配对。配置备份与现有单会话导入导出并非同一能力，恢复和清理必须避免误删、覆盖凭据或泄露配置。

## Proposal

以下为待评审设计，不代表实现或已经验证：

- 远程后台按能力选择性移植 `backend/src/remote_control/{server,store,projection,protocol,actions,desktop,create,images}.rs`，对接现有 `commands.rs`、`lib.rs`、`cdp.rs` 与 `public/codex-runtime-adapter.js`；不能直接替换这些共享文件。控制端 `src/features/remote-control/` 管启动、停止、端口、账号及设备撤销；浏览器 `src/remote/` 与 `src/features/remote-workspace/` 管登录、会话和操作，控制台页面按授权白名单接入。类型、请求、API 注册、页面路由、样式及测试必须同步。移除参考配对码、二维码、自动下载隧道和其设置，而非将其称作密码登录。
- 默认单管理员，首次启动前本地设置用户名和密码；禁止硬编码默认密码。用经维护的密码哈希实现（如 Argon2id，独立随机盐；现有依赖未发现该实现，需确认依赖及锁文件），绝不使用 SHA-256 代替密码哈希。凭据以私有限权文件持久化；仅保存随机 session token 摘要、过期时间与撤销信息，提供登出和管理员撤销。是否允许重启后记住登录应明确规定并测试；不得保存明文密码、写日志或通过 API 返回凭据。启动前未完成账号设置不得开放监听。
- 每个修改请求需要严格 Origin/Host 校验及 CSRF 防护，登录有界限速并避免用户名枚举；WebSocket 仅认证、Origin 和 upgrade 校验后允许连接，并在撤销或停服时中断。参考 Host 允许私网 IP 而域名依赖 `public_url`；用户自建入口不能据此假定可访问，需设计精确可配置的可信 Origin 和代理信任边界，不信任任意 forwarded 头。仅通过审计过的明确命令白名单调用现有 `invoke_api`；凭据、备份及批量删除默认不进入远程白名单，不从隐藏字段或错误回显密钥。现有白名单包括凭据导入和插件调用，不能原样假定低风险。
- 移植 `backend/src/session_management/`、`commands/sessions.rs` 中清理命令与 watcher，页面 `src/features/session-management/`，并对接 `runtime_operation`、现有 launcher 启动维护和关闭顺序。选中集合、revision 和确认标识由 UI 原样传后端；pending 文件先安全落盘，停止旧 Codex 写进程后重放，失败保留任务并报告部分失败。自动清理默认关闭，仅按已保存且确认的策略执行。现有 `session_transfer.rs` 已有分块单会话导出/导入，复用而不覆盖；导入与清理并发、活跃当前会话何时释放和崩溃中断的可恢复性须单独验证，不把“要求重启”说成立即释放。
- 参考 `backup.rs` 和 `commands/backup.rs` 及 `src/features/backup/` 按当前 `CodeyConfig` 映射适配：完整配置 JSON 与单会话包分开；导入先限制大小和 schema、预览、校验 revision，再在现有配置锁内创建权限受限恢复前快照并复用当前提交/应用流程。保留本机受管身份、插件身份及当前 fork 定制字段；不引入参考子代理权限模型。WebDAV 维持独立设置、限制大小、禁重定向和安全默认，不默认允许 HTTP；不将 WebDAV 凭据混入导出配置。配置可包含线路 API key 等秘密，导出文件、WebDAV 对象与恢复前快照均应明确警示并保护；参考 WebDAV 密码为本地明文 JSON，是否换现有系统安全存储需先确认平台能力。
- 构建同时接入 `vite.remote.config.ts`、`scripts/build-overlay.mjs`、构建缓存的产物列表及 `backend/build.rs` 的嵌入检查，覆盖 `codey-remote.js/css`；最小改动补齐必要 Rust HTTP server 特性/依赖与 Linux、Windows 构建，不改原有发行流程。测试按远程后台与 UI、会话管理与启动生命周期、配置备份与 UI 三块拆分，避免不同工作者同时改共享调度入口。

## 已核实的源码边界

- 参考默认 `0.0.0.0:43129`，每次启动新建内存 Auth；单次配对码、十二小时设备会话、撤销为既有机制，不满足持久账号密码需求：[remote_control/mod.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/mod.rs#L47-L63)、[auth.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/auth.rs#L8-L90)。
- 请求检查 Origin 后公开静态资源，登录外的操作要求授权；WebSocket 经有效升级、数量限制及循环复验撤销。远程浏览器同时打开 workspace 和 `<App />`，桌面创建/恢复路径经 CDP 调用注入的 `__codeyRemoteControl`：[server.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/server.rs#L122-L155)、[路由](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/server.rs#L270-L418)、[WebSocket](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/server.rs#L427-L488)、[main.tsx](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/src/remote/main.tsx#L78-L83)、[create.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/create.rs#L7-L35)、[CDP](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/cdp.rs#L1436-L1472)、[adapter](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/public/codex-runtime-adapter.js#L841)。
- 删除要求确认、合法 UUID 和一致 revision，先记录固定 ID 后重放；对存储路径逐段拒绝 symlink、Windows reparse point 并校验 canonical root；完成后验证结果：[session_management/mod.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/session_management/mod.rs#L145-L226)、[files.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/session_management/files.rs#L20-L45)、[retention.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/session_management/retention.rs#L123-L142)。当前分支 `backend/src/launcher.rs:434-447` 另有 `message_delete::reapply_persisted_deletions`，不可被参考启动流程覆盖。
- JSON 备份 4 MiB 上限，直接序列化配置，测试证明 API key 可恢复；账号、插件和本机网络等在 restore prepare 被保留。WebDAV 设置的密码被序列化到独立私有文件：[backup.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/backup.rs#L11-L46)、[测试](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/backup/tests.rs#L8-L23)、[prepare](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/backup.rs#L149-L207)、[webdav.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/backup/webdav.rs#L11-L21)、[保存](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/backup/webdav.rs#L78-L83)。恢复先检查 revision、存快照并提交，再进行后续应用；不保证配置和运行时构成单一原子事务：[commands/backup.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/commands/backup.rs#L28-L37)、[恢复](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/commands/backup.rs#L75-L95)。
- 参考实现的测试入口包括 `backend/src/remote_control/server/tests.rs`、`backend/src/session_management/{tests.rs,retention/tests.rs}`、`backend/src/backup/tests.rs`、`tests/remote-*.test.mjs`、`tests/session-management-*.test.mjs`、`tests/backup.test.mjs` 及对应 `tests/browser/*.spec.mjs`；构建另有 [vite.remote.config.ts](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/vite.remote.config.ts#L11)、[build.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/build.rs#L30-L49)。

## Alternatives considered

- 直接移植参考配对与内置隧道：已有测试和完整移动体验，但不能满足用户明确的普通账号密码和不集成隧道要求，且不得依赖第三方下载。
- 整体合并或覆盖参考共享文件：可快速继承完整功能，但无共同基线且会覆盖当前 fork 的关键安全与生命周期定制；故选择以现有接口为中心按功能移植。

## Acceptance criteria

- 首次设置、错误密码限速、记住登录、登出及撤销、服务重启、Origin/CSRF/Host 和 WebSocket 未授权拒绝均有自动化测试；默认 `0.0.0.0:18787`、自定义端口、占用错误及局域网访问在允许环境验证。控制台与浏览器 UI 均验证；密钥不出现在响应、日志和错误中。
- 会话清理覆盖选择确认、并发修改、当前活跃会话、自动策略、异常中断与重启重放、符号链接/Windows reparse point、跨平台路径、SQLite 残留和单会话导入导出的交互；失败不误报完成。
- 备份/恢复覆盖凭据提示、JSON 格式大小版本、恶意路径及本机身份保护、revision 冲突、快照或提交失败、重启恢复和 WebDAV 授权与错误；明确验证成功提交后的运行时应用失败如何呈现。
- 保持原有模型档位、Fast、Linux、遥测和子代理测试；执行相关 Rust、Node、UI 测试和两平台可用构建；未执行的平台明确列为缺口。保留 AGPL 和第三方声明。

## Risks

- 明文 HTTP 即使使用强哈希和 HttpOnly cookie，登录密码与会话仍可被链路窃听；只应在可信网络使用，互联网入口由用户自行提供安全传输，应用不能声称端到端加密。跨重启会话持久化扩大 token 被盗窗口，具体期限和会话持久化方案待拍板。
- 参考恢复在配置提交之后仍有运行时应用，无法宣称全系统原子；清理跨文件和 SQLite 不是单一事务，崩溃、正在写入和失败重试必须明确展示。路径校验不能凭静态检查就证明无 TOCTOU，需测试并审阅并发写与链接替换。
- 参考保留策略仅按时间判断，未证明当前活跃会话豁免；实现前必须确认用户预期并设计安全边界。原备份为明文含秘密，WebDAV 与本地文件权限不等于加密。
- 本 note 为研究草案；远程白名单、外部反代 Origin、账号初始化与凭据持久化的具体用户体验、当前配置字段映射、许可证逐项归属、目标平台构建尚未验证。活动笔记检索仅命中 `implemented/bug-fix/2026-11-03-upstream-selection-boundary.md`：其会话选择隔离与本提案清理生命周期互补，不吸收也不覆盖。
