# Agent Note: 远程会话库存的身份与正式标题

Status: implemented

## Problem

CodeyX 新增的远程列表直接读取 `threads`，不识别派生身份，也遗漏 `local_thread_catalog.display_title`。空白 `name` 还会遮住正式标题。未命名不能证明是子代理：旧 schema 和合法无标题主会话必须保留，不能通过手机端隐藏标题或清理真实数据修复。

## Decision

- 范围限定为远程库存读取、所需候选发现接口及合成测试，不恢复已撤回的生命周期或会话选择补丁，不改变接口、操作端点或桌面行为。
- 远程库存使用未做 SQLite schema 预筛的原生候选路径，避免锁定库在身份判断前消失；既有缓存发现 API 的行为不改。所有数据库只读打开，保留 250 ms busy timeout、参数化搜索、归档筛选、UUID 边界、跨库最新记录去重和最终 200 条上限。
- 已知子代理身份在所有已发现数据库之间生效，旧库同 ID 的未知身份不能将其重新放进列表。只接受明确的 `source` / `thread_source`、合法父 ID 或 `thread_spawn_edges` 关系；损坏和未知元数据不推断成子代理，`forked_from_id` 不代表子代理。
- 在 SQLite 的 `LIMIT 200` 之前执行身份与搜索谓词。正式标题直接复用 `SessionMetadataCache`：`name` → 本地 catalog 标题 → legacy `title`，沿用 host、排序、空白清理和 80 字规则，不读取消息或 preview。搜索对最终标题进行判断，不先无界收集全部会话到 Rust。
- 身份和标题只在一次库存请求内按 ID 记忆，搜索、投影与跨库重复记录不重复读取同一 ID 的元数据，下次请求重新获取。读取可变外部元数据的 SQL 函数不声明 `SQLITE_DETERMINISTIC`；身份库打开、schema 或查询错误传回列表失败，不静默放行子代理；正常不存在的可选路径不当作读取失败。只有纯解析函数声明确定性。

## 活动笔记审计

按 `remote_control`、标题、库存、会话列表、会话选择、生命周期检索现有活动笔记。[远程及数据边界](../../implemented/architecture/2026-11-03-codeyx-remote-and-data.md)与本篇互补；[选定 custom 能力](../../implemented/simplification/2026-11-04-selected-custom-preview.md)及[上游选择边界](../../implemented/bug-fix/2026-11-03-upstream-selection-boundary.md)约束本次范围，不吸收旧决定。[内置工具](../../implemented/feature/2026-11-03-codeyx-managed-tools.md)与[自定义角色](../../implemented/feature/2026-11-04-custom-subagents.md)不涉及本次库存读取，不修改。无过时提案需要删除或归档。

## Alternatives considered

- 隐藏全部未命名项：代码最少，但会隐藏合法主会话，且不能修复丢失正式标题的根因。
- 先取 200 条再在 Rust 过滤和搜索：复用原查询最简单，但较新的子代理会挤掉主会话，跨库 catalog 标题也无法正确命中搜索。
- 附加所有数据库后用一条联合 SQL：标题和身份能直接关联，但会引入 SQLite 附加数据库数量限制及新连接管理；复用已有只读命名器和按 ID 查询的 SQL 函数，不复制命名规则。

## Verification

- 先增加直接调用真实 `threads` 的 Rust 合成回归，再修改实现。修改前从真实查询字符串抽取 SQL 的两个合成断言均失败：228 个较新子代理挤走主会话，空白 name 下的 catalog 标题无法搜索。这不是现场数据分类证据。
- `store/tests.rs` 新增八组回归：227 个较新子代理与两个主会话；字符串和 JSON 来源、合法父关系及损坏元数据；224 个主会话中旧条目的跨库正式标题搜索；跨库身份不复活及 200 条上限；归档与最新记录去重；只读连接及数据库字节不变；最小 schema；请求内缓存与身份 SQL 错误传播。其中缓存、错误传播分别独立成组，覆盖现有命名规则的测试不改写。另补初始化时身份库被 `EXCLUSIVE` 锁定不得从旧库复活 child，以及可选路径正常不存在的 Rust 回归；本机仍未执行原生测试。
- 外层 `work/mobile-unnamed-diagnosis/inventory_regression_probe.py` 从当前 Rust 代码抽取库存、关系和命名 SQL，只在临时合成 SQLite 上运行；八组通过，实测 0.830 秒。其 Python UDF 镜像不等于 Rust UDF 已执行，不能当成原生测试通过。
- 定向 `rustfmt --edition 2024 --check backend/src/remote_control/store.rs`、`cargo metadata --locked --offline --no-deps --format-version 1` 和 note 校验通过。`cargo test --locked --offline -p codey --lib remote_control::store::` 在依赖 build script 阶段因缺少 `link.exe` 失败，尚未编译或执行本次 Rust 测试，交由 CI 复验。

## Consequences

- 子代理不再挤占 200 条主会话名额，正式标题和搜索共用既有命名器，无新依赖、磁盘格式或远程权限。代价是跨库身份和非空子串搜索仍可能检查全部 SQLite 候选，按 ID 的请求级缓存也随访问过的 ID 数增长；200 限制的是返回结果，不是扫描工作量或缓存内存。不宣称性能提升，也不为此新增搜索架构。
- 未提供足够身份元数据的旧子代理仍可能作为未知会话保留；不能将所有现场未命名项归因于子代理。真实服务与手机联动由主代理独立验收。
- 未读取、清理或修改真实会话、transcript、消息、账号及服务；不恢复已撤回的生命周期与会话选择补丁。Rust 编译、Clippy、原生测试与真实端到端行为仍需 CI 和主代理验收。
