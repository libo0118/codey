# Agent Note: 远程会话目标与 Cindy 管理面板

Status: implemented

## Problem

远程页尚未投影或管理 Codex 会话 Goal；用户需要查看、修改、暂停/继续和新建，并要求先看样式。原生目标与普通执行轮次、定时自动化不是同一对象，不能以“停止本轮”代替“暂停目标”，也不能把目标 active 状态当作启动消息已确认。

## Decision

- 外观优先沿用 Cindy 的轻量摘要与 Context Sheet，仅未完成目标或未知写入结果在 composer 上方保留 44px 以上状态条；无目标、未读和已完成目标不占普通聊天空间。已有会话通过加号菜单的“会话目标”打开面板；完成目标先展示详情，再明确新建下一项。编辑与新建复用同一底部面板，切会话关闭面板，新建对话未取得 thread 时不提供目标入口；不做全局仪表盘、删除入口或不支持的进度百分比。
- 状态和操作参考 codex-mobile-bridge 的六态、expected 身份检查、写入防重及错误语义；修改须先暂停，保存重置目标用量并保持暂停，暂停不停止当前回复。新建与继续采用明确的启动动作，不在读取或打开面板时发模型请求。
- Cindy 固定来源为 `deef17354321527fd857acf62662955139ef2491` 的 `ContextSheetGoalView.tsx`、`goalStatusLabel.ts` 与会话 Goal 调用点；实际参考源码已补全。仅借鉴布局与交互，不搬 Cindy 自建 GoalController、maker 指令或其轮数/无进展限制。
- 原生实现复用当前 Desktop manager 的固定 Goal RPC/方法，等待真实 owner 角色回执，不把 `thread/goal/set` 当作不存在的 follower IPC，也不增加第二个 app-server writer。新建和继续仅在空闲时写入，随后由既有 owner follower 通道发送带稳定消息标识的启动消息；目标 active 与启动 accepted 分开确认。RPC 句柄用完释放。
- 公共数据只保留目标正文、已知状态、稳定身份/时间及真实 Token/时长字段，不回传原始 RPC 或私有配置。默认值未知不等于无目标，读取失败时不得开放覆盖已有目标的新建。
- 写入复核目标身份和状态，并按会话串行；创建和恢复必须校验模型门禁。私有持久回执仅保存请求摘要和固定结果，不含目标正文，跨重启与超时后不自动重放；浏览器提交标记保留未知结果，用户核对目标和启动消息后明确解锁。
- 保留 Cookie/Origin/CSRF、权限、消息草稿和 HAP DOM v2 的唯一 header/footer/action 合同。加号菜单继续提供相机、照片和文件；打开目标不解锁未知附件提交、不自动重发，关闭后焦点回到入口。发布和更新主机须分别授权，不对现有真实目标作写入验证。

## Existing-note audit

按 Goal、目标模式、会话目标搜索活跃 proposed/implemented/rejected，尚无对应旧决定。[Cindy 页面切片](../../implemented/architecture/2026-11-13-cindy-page-slices-and-dom-v2.md)、[移动层级](../../implemented/feature/2026-11-12-remote-mobile-layout.md)提供视觉上下文；[远程与数据边界](../../implemented/architecture/2026-11-03-codeyx-remote-and-data.md)提供授权约束，均互补、不取代。自动清理中的“目标”与此无关，无需归档或删除。

## Alternatives considered

- 直接照搬 Bridge 的独立 Goal sidecar 接线明确，但会引入第二写入者及 owner 激活协调；优先验证当前 Desktop owner 通道。
- 原生桌面横向长目标条信息密度高，常驻“设置目标”也容易发现，但都会挤压普通手机聊天；只保留未完成摘要与未知写入警示，其余按需从加号进入。
- 同时复制 Cindy 的轮数、无进展和用量限制可更完整复刻，但原生 Goal 合同不同；首版只展示或编辑已证实支持的字段。

## Testing

- 合成 Native manager 测试覆盖六态、能力失败、模型门禁、身份变化、owner 角色、当前回复及 RPC 句柄释放；控制器测试覆盖双击、启动未确认、旧登录和人工核对。
- Rust 测试覆盖公开投影、请求边界、持久防重和取消后的未知结果；完整原生编译与平台打包由 CI 校验，本机缺少 Windows target 标准库。
- UI 合成测试覆盖六态、空目标无占位、加号入口、完成详情后明确新建、切会话关闭、只读打开与未知附件防重；Goal 与 Composer 的 React key 分开，HAP chrome 合同保持唯一。production bundle 可构建；当前 CUA 没有可用浏览器，新菜单的 390px 浅色、320px 深色实际渲染与焦点恢复仍待截图核验，不沿用先前目标条的截图结论，也不连接真实账号或模型。

## Consequences

远程页可管理同一原生目标，不维护第二套调度器。Goal API 具有原生版本依赖，非 owner 窗口拒绝写入；新建或继续可能产生模型费用，暂停不撤销已执行操作或停止当前轮次。持久回执有界，满额后拒绝新写入而非丢弃防重证据。真实模型请求、用户目标及鸿蒙真机未作写入验收，构建与合成测试不等同于这些验收。
