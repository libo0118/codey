# Agent Note: 远程原生权限与无项目创建

Status: implemented

## Problem

已有会话切到完全访问失败；原远程设置同时发送 `permissions` 与 `sandboxPolicy`，新 Codex 明确拒绝两者并用。新建页仅接受已保存项目，也无法表达用户要求的“不指定工作目录”。不能用假项目或主目录冒充无项目，更不能通过重试未知写入或自动提升权限绕过问题。

## Decision

- 已有会话只在明确选择时发送原生 profile ID、approval policy 和 reviewer；不再并用旧 sandbox 描述或重复发送 active profile。沙箱和实际根目录由原生 profile 解析，模型或速度单独更新仍不改变权限。保留原生后续轮次设置语义，不追溯改写正在执行的任务；确认回执后重连读取真实状态，不乐观伪造成功。
- 新建与 defaults 以显式 `projectId: null` 表达无项目，缺字段及字符串/null 以外的类型仍拒绝；已有字符串 ID 继续从已保存项目解析。无项目发送 `workspaceKind: projectless` 和空 workspace roots，不接受客户端指定目录，也不伪造 project assignment；实际目录由原生 manager 管理。
- 新建权限复用原生 `permissionSelection`。标准工作区使用 auto，无项目使用其原生 granular 选择；只读和完全访问均保留显式选择。完整 granular policy 才投影为默认权限，未知或不同的 policy 仍为 custom。未选择权限沿用原生默认值。
- 无项目草稿使用独立 `create-no-project` 范围；已有 `create:{projectId}` 草稿与未知提交记录不改名。读取对应默认值成功后才允许首条发送，选项目、权限或模型本身不创建会话；失败、旧登录和迟到结果仍受原防重与代次约束。
- 沿用已发布 adapter 6 和原创建回执，不新增代次或 RPC；无项目创建后先不假定 cwd，由原生会话流快照提供实际目录，前端不猜测 home。模型门禁、原生 creation-input tools、Cookie/Origin/CSRF、附件校验和未知结果不重放均不变。

## Source evidence

从 121 当前安装的 Codex `app.asar` 只读核对 `app-initial-c0c47a7e860a.js` 的创建/设置处理器，以及 `app-shared-c5bf1a371bd8.js` 的权限目录：原生支持 projectless 和 permissionSelection，projectless 的默认选择为 granular。未把第三方完整资产复制入仓库。

同一内置 CLI 在临时 CODEX_HOME、临时工作区和无认证的回环假线路中创建空本地线程，不发送 turn/start。旧参数返回 `permissions cannot be combined with sandboxPolicy`；原生 profile 参数成功。用户现有对话、账号与配置未用于写入探测。

## Existing-note audit

- [Cindy 页面切片](2026-11-13-cindy-page-slices-and-dom-v2.md)部分重叠：本篇扩展其原有“仅保存项目”范围；页面主体、首条发送和宿主 DOM v2 合同保留。
- [远程与数据边界](2026-11-03-codeyx-remote-and-data.md)、[文件与公开投影](../feature/2026-11-04-remote-files-and-projection.md)互补：权限仍需明确操作，不扩大只读控制台或客户端路径能力。
- [远程消息与图片](../feature/2026-11-04-remote-rich-timeline-and-links.md)同批相关，图片局部缩放不改变本篇的任务权限和创建协议。
- 按权限、工作目录、无项目、Cindy 检索活跃笔记；子代理沙箱和上游合并记录无关，无需归档或删除。

## Alternatives considered

- 保留手写 sandbox、删除 profile 可避开互斥错误，但会继续复制原生根目录与权限语义，放弃，改用桌面自身的 profile。
- 将 home 当作保存项目改动较少，但不是真实无项目语义且会误导文件访问范围，放弃。
- 无项目也复用第一个项目草稿可少一个状态分支，但会混淆工作区与未确认提交，保留独立且不冲突的范围。
- 把 cwd 加入创建回执可立即显示目录，但宿主管理器可能是 RPC 存根，额外读取会增加延迟并让已提交创建受后续读取失败牵连；沿用现有会话流。存根任意属性都可调用，不以 typeof 方法存在猜测新能力。

## Verification

定向前端回归覆盖无项目选项、默认值与草稿隔离、零项目、首发一次、未知结果防重，以及已有会话完全访问只由显式设置提交并在确认后重连。原生 adapter 测试覆盖全局默认值、原生参数/creation-input tools 透传；目录由原生会话快照提供。Rust 测试覆盖三种权限不混合旧 sandbox、无项目输入路径不可信和 granular/custom 区分。本机全量 Node 为 1635 通过、3 跳过，后续按改动范围回归，TypeScript 与两套前端构建通过。实际浏览器合成会话验证首发前零创建、首发一次、已有会话只读/完全访问切换及原始尺寸仅放大图片；双指和滚轮为 DOM 事件模拟，浏览器不能注入硬件触摸，不计作真机验收。原生编译与打包由 CI 验收，未发送真实模型消息。

## Consequences

远程设置与原生 profile 保持一致，新建可选择不指定目录，但这不代表没有实际目录或没有文件权限。原生创建/权限接口仍具有版本依赖，不提供将主目录代替无项目的静默降级。未发送真实模型消息，未自动更改已有会话权限，也未在此决定中授权发布、安装或重启。
