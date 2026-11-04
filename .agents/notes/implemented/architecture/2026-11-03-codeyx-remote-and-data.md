# Agent Note: 远程控制、会话管理与配置备份的移植边界

Status: implemented

预览分支保留哪些 custom 能力以[选定能力方案](../simplification/2026-11-04-selected-custom-preview.md)为准；速度、生命周期和会话选择回归上游，不改变本篇的账号、远程权限、备份或清理边界。

## Problem

用户需要账号密码远程会话、批量清理与配置备份，但 CodeyX 与本分支没有共同祖先，整体合并会覆盖模型档位、原生 Fast 资格、Linux、遥测和子代理生命周期定制。远程访问不能等同于本机管理权限；清理不能删除重新活跃的自动目标，配置恢复不能覆盖本机身份或丢失模型字段。

## Decision

- 在独立功能分支选择性适配 CodeyX `30e1daa4861762626628e6c2805366cd6519e6f2` 的远程、清理与备份模块，沿用现有桌面桥接、配置锁、启动维护及单会话传输，不整体覆盖共享文件，不改变 `custom` 分支或触发发布。[内置工具](../feature/2026-11-03-codeyx-managed-tools.md)采用同一移植边界。
- 远程服务默认关闭，首次必须在本机设置单管理员账号，没有默认密码；默认监听 `0.0.0.0:18787`，端口可配。密码复用现有 `ring` 的 PBKDF2-HMAC-SHA256、600000 轮及随机盐，不使用单次 SHA-256 密码摘要。随机 opaque 会话仅持久化摘要，记住登录有效期为 30 天；登出、改密及本机撤销使相应授权和 WebSocket 失效。账号与会话另存私有文件，不进入配置备份。
- 登录限速，Host、Origin、修改请求 CSRF 和 WebSocket upgrade 均受检查；外部 origin 必须明确配置，不信任任意 forwarded 头。不提供二维码配对、云隧道或 HTTPS 代理；HTTP 链路不加密，安全入口由用户自行保护。
- 账号写入在私有文件锁内重载最新文档，并复核已见设置代次、凭据和监听设置；只合并独立写者新增或撤销的会话。会话集合变化不推进设置 revision，配置无变化的请求也必须检查磁盘，不让旧代凭据或已撤销会话复活；只读授权的文件指纹变化仍拒绝授权。Windows 监听在 bind 前使用排他地址选项，避免特定地址与通配地址共享同一端口，Unix 保持既有重用语义。
- 远程工作区允许会话操作；远程控制台使用专用只读页面而非挂载完整本机控制台。仅允许 `runtime_status`、`load_codey_config`、`builtin_tools` 的 `status` 和 `list_codey_plugins` 四类 API，参数及返回字段均取白名单摘要，不回传密钥；备份、清理、安装和其他高权限管理不开放远程调用。
- 远程恢复使用独立能力判定，不以普通会话的 legacy signals 缓存冒充远程 manager；托管 RPC 通过现有请求生命周期桥接 resume，每次取最新连接并释放 pending。旧安装缺少远程代次时不整段热替换，以免销毁仍被桌面控件引用的闭包，而是提示用户本机重启后重连。首次安装与同代有效缓存正常使用，已知失败只返回白名单阶段消息，不能盲重放不确定的修改操作。
- 手机订阅的无效帧、明确断线和关闭统一使操作失效；保留未发送草稿，只有手动重连后的匹配有效状态才能恢复操作。旧连接帧、旧 render 操作及迟到的会话检查不能影响新代连接，无效数据不能只显示错误却继续宣称已连接。
- 会话清理要求本机 `confirmed` 与一致 revision，先将固定目标写入 pending，停止旧 Codex 后在 `finish_pending` 执行，再运行原有 `message_delete::reapply_persisted_deletions`。路径逐段拒绝链接、reparse point 和越界；失败保留任务与未删数据，可选清理异常不永久阻断 Codex 启动。
- pending v2 区分手动 ID 集合与自动项的排队活动时间、策略摘要及 `queued/started` 阶段；按天策略默认关闭。维护时重新检查活动和策略，未开始项若变活跃、策略改变或不再过期即取消；通过检查后先原子保存 `started` 再触碰文件。部分失败保留开始状态，文件失败不继续清数据库；重试不因元数据消失或合法策略变更丢弃已开始的清理，但新活动始终撤销自动意图，损坏策略仍拒绝处理。手动项保留明确意图。
- 旧 v1 只有 ID，无法追溯来源，保持待确认。库存用 `legacyPendingIds` 独立展示，`pendingCount` 仅统计可执行任务；本机以 `confirmed` 和当前 revision 逐项重新确认后，现存会话转为手动目标，已不存在的 ID 仅移除旧标记，不安排未来删除。结果分别报告 `scheduledCount`、`clearedLegacyCount`，界面不把旧标记说成重启即可完成；未知 schema 拒绝处理，不覆盖任务或误删数据。
- 配置备份为 `codey-config/v1` JSON，大小上限 4 MiB，与单会话包分开；包含线路 API key 等秘密，导出、上传与恢复要求明确确认。导入先检查格式、大小和配置并预览，恢复在配置锁内复核 revision、写恢复前快照，再以完整 `prepared` 配置调用共享校验与持久化尾段，普通 UI 保存仍按部分更新处理。准备阶段保留本机官方账号、插件托管身份及启动网络设置，完整模型档位等 fork 字段不被部分合并丢弃；持久化后的应用或状态查询失败仍报告已提交。持久化失败只保证不发布新内存配置，不保证主文件未被既有轮转移动；错误提示实际恢复前快照路径，要求核对磁盘状态后恢复，不假称文件完全未变。
- WebDAV 上传完整配置、下载后只预览，不自动恢复。凭据另存私有明文文件，不是加密 vault，也不回显密码或混入备份；默认 HTTPS，仅显式 `allowHttp` 才接受 HTTP，禁止重定向，限制大小和超时，以 revision 防止设置覆盖。远程页面独立构建并嵌入程序，沿用现有发行流程。

## 固定参考来源

- 参考默认 `0.0.0.0:43129`，每次启动新建内存 Auth；单次配对码、十二小时设备会话、撤销为既有机制，不满足持久账号密码需求：[remote_control/mod.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/mod.rs#L47-L63)、[auth.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/auth.rs#L8-L90)。
- 请求检查 Origin 后公开静态资源，登录外的操作要求授权；WebSocket 经有效升级、数量限制及循环复验撤销。远程浏览器同时打开 workspace 和 `<App />`，桌面创建/恢复路径经 CDP 调用注入的 `__codeyRemoteControl`：[server.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/server.rs#L122-L155)、[路由](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/server.rs#L270-L418)、[WebSocket](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/server.rs#L427-L488)、[main.tsx](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/src/remote/main.tsx#L78-L83)、[create.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/remote_control/create.rs#L7-L35)、[CDP](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/cdp.rs#L1436-L1472)、[adapter](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/public/codex-runtime-adapter.js#L841)。
- 参考删除要求确认、合法 UUID 和一致 revision，先记录固定 ID 后重放；对存储路径逐段拒绝 symlink、Windows reparse point 并校验 canonical root；完成后验证结果：[session_management/mod.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/session_management/mod.rs#L145-L226)、[files.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/session_management/files.rs#L20-L45)、[retention.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/session_management/retention.rs#L123-L142)。本分支保留既有轮次删除重放，并在它之前执行会话清理。
- 参考 JSON 备份上限为 4 MiB，直接序列化配置，测试入口覆盖 API key 恢复；账号、插件和本机网络等在 restore prepare 被保留。WebDAV 设置的密码被序列化到独立私有文件：[backup.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/backup.rs#L11-L46)、[测试](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/backup/tests.rs#L8-L23)、[prepare](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/backup.rs#L149-L207)、[webdav.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/backup/webdav.rs#L11-L21)、[保存](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/backup/webdav.rs#L78-L83)。参考恢复先检查 revision、存快照并提交，再进行后续应用，不保证配置和运行时构成单一原子事务：[commands/backup.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/commands/backup.rs#L28-L37)、[恢复](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/src/commands/backup.rs#L75-L95)。
- 参考实现的测试入口包括 `backend/src/remote_control/server/tests.rs`、`backend/src/session_management/{tests.rs,retention/tests.rs}`、`backend/src/backup/tests.rs`、`tests/remote-*.test.mjs`、`tests/session-management-*.test.mjs`、`tests/backup.test.mjs` 及对应 `tests/browser/*.spec.mjs`；构建另有 [vite.remote.config.ts](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/vite.remote.config.ts#L11)、[build.rs](https://github.com/fraternity-z/CodeyX/blob/30e1daa4861762626628e6c2805366cd6519e6f2/backend/build.rs#L30-L49)。

## Alternatives considered

- 直接移植参考配对与内置隧道：已有测试和完整移动体验，但不能满足用户明确的普通账号密码和不集成隧道要求，且不得依赖第三方下载。
- 整体合并或覆盖参考共享文件：可快速继承完整功能，但无共同基线且会覆盖当前 fork 的关键安全与生命周期定制；故选择以现有接口为中心按功能移植。
- 新增 Argon2id：是提案中的专用密码 KDF 选项，但需增加依赖；现有 `ring` 已提供 PBKDF2，因此复用其维护实现，使用随机盐与既定迭代次数，不用单次 SHA-256 冒充密码哈希。
- 沿用只存 ID 的 pending：格式简单且便于重放，但无法分辨手动意图与已失效的过期判断；因此新格式保存自动项依据，旧格式不能静默升级为手动授权。
- 直接复用普通 UI 保存合并：接线最少，但该入口只接受部分更新，会丢弃恢复准备结果中的模型字段；因此只共用校验、落盘和应用尾段，不改变 UI 保存语义。

## Verification

- 本地 Rust 回归入口包括 `remote_control/{auth,server}`、`session_management/{tests,retention/tests}`、`commands/backup.rs` 与 `backup/tests.rs`；重点包括记住登录及撤销、自动目标重新活跃与策略变化、旧 pending 和完整模型配置恢复。存在测试代码不等于用例已执行。
- 前端证据限于隔离 Chromium Mock，不代表真实 Codex 联动。真实 WebDAV 往返、默认 `18787` 监听、局域网和用户自建安全入口需单独实测；本机缺少 `link.exe`，Rust 构建、测试与 Clippy 以 Windows/Linux CI 的实际运行结果验收，不把源码合同或 metadata 检查当成编译证明。

## Consequences

- 远程会话、配置备份和可恢复清理共用现有核心流程，保留本分支边界；代价是配置提交与运行时应用、跨文件和 SQLite 清理均不是全系统原子事务。路径检查也不能单凭源码证明没有 TOCTOU。
- HTTP 下密码与会话可被窃听，记住登录延长 token 被盗窗口；私有权限与 WebDAV 传输不等于备份加密。只有用户提供可信网络或安全入口后才适合开放访问，不宣称端到端加密。
- 活跃自动目标可取消，旧来源不明任务宁可保留并要求重新确认；人工确认仍是永久删除意图，不能把延期执行说成立即释放。若扩展远程权限、改变持久格式或接管外部入口，需要重新审查这条边界。
- 活动 note 按远程、WebDAV、备份、清理、CodeGraph、Ponytail 和 FastCtx 关键词审计：本篇与[内置工具](../feature/2026-11-03-codeyx-managed-tools.md)交叉引用；[会话选择隔离](../bug-fix/2026-11-03-upstream-selection-boundary.md)及[Windows 测试预算](../testing/2026-11-03-windows-rust-test-parallelism.md)仅互补，不吸收、不覆盖，也不据此宣称 Rust CI 通过。
