# Agent Note: 远程消息流、会话文件与原生深链

Status: implemented

## Problem

远程消息只展示纯文本和代码围栏，并把整轮工具活动提前到第一项活动，破坏回答与工具之间的顺序；生成文件没有经过会话授权的下载入口，HTTP 页面复制也没有接通已发布手机壳的剪贴板接口。

## Decision

- 使用锁定的 `markdown-it@15.0.3` 将 token 转为 React 安全节点，禁用原始 HTML、可执行 URL 与未知 schema 的 JSON 展示。按消息 identity 保留原顺序和原生 details 状态；公开摘要、计划、命令、工具和 Unified Diff 分别呈现，长代码与表格局部横滚，保留单栏消息与原 composer。已有审批只展示白名单详情，不为未知或缺少可核对摘要的请求提供盲批。
- `collab*`、`subAgentActivity` 只展示原生公开活动与结果；未知结构使用安全占位，不输出 hidden prompt、私有 arguments 或原始 RPC，也不凭工具名把未公开参数拼成说明。
- 只通过带 CSRF 的 `/remote/files/resolve` 解析文件元数据，最多一个请求活跃、十六个等待，超额、繁忙、变化或无权限均提供明确原因与显式重新检查，不无限排队或自动重试。会话切换取消旧请求，迟到结果不回写；旧登录的 401 不能清除新的 CSRF 会话。
- 下载只使用服务端给出的、绑定同一 UUID 的 `/api/sessions/{UUID}/files/{64hex}` 同源链接，准备好 href 后由用户直接点击；不在解析时预下载文件、不异步模拟点击。Markdown 文件目的字符串仅是候选，包括 parser 正常化的 Windows 路径，不能在浏览器把路径拼成下载地址。受支持同源图片按需懒预览，暂时繁忙或源变化后提供显式重新预览，不永久隐藏或自动循环请求；外部图片是普通链接，不偷偷发背景请求。
- 会话列表提供显式复制 `codex://threads/{UUID}` 操作，不恢复或激活会话，不复制当前网页登录地址。当前库存只证明本地来源，非本地身份不臆造。
- bootstrap 提供与 Bridge 网页兼容的 `BridgeClipboard.copy(getText, button)`；按钮渲染即带 `.message-copy` 或 `.code-copy`，允许旧 Harmony 壳捕获首击；普通浏览器依次使用安全 Clipboard、HTTP `execCommand` 和可选择的手动复制。

## Alternatives considered

- 继续手写正则 Markdown：无需依赖，但不能完整处理列表、表格、嵌套与 URL 边界，放弃。
- 整套迁移 Bridge 时间线：成熟，但会替换当前工作区与壳识别契约，且未知 item 原始 JSON 不符合公开投影边界，放弃；只适配 token→安全节点与公开剪贴板接口。
- 点击后异步解析再模拟导航：接线少，但会失去手机原生下载的真实手势，改为预先解析元数据后渲染直接链接。

## Verification

- 真实生产组件 SSR 先复现 Markdown 缺失与交错活动提前，再验证语法、危险 URL、Windows 文件目的字符串、结构化消息、审批与未知 schema 占位。Hook 与真实 transport 回归覆盖文件成功/失败/跨会话、有限队列、显式重试、取消及旧登录 401。
- 隔离浏览器 Mock 读取真实生产组件：320/390 px 文档宽度均不溢出、composer 单排；流式更新前后命令 details 的 DOM nodeId 均为 25，保持展开和交错顺序。新会话没有旧 UUID 的文件 href。
- 已发布 Harmony 剪贴板脚本作为 MIT 测试 fixture，浏览器真实首击与键盘操作只记录 Mock native prompt；复制深链时 socket 数为零，不激活会话。单测另覆盖一次消费、非可信事件、iframe、HTTP execCommand、手动复制和 native 失败反馈，不读取系统剪贴板。
- 多图 Mock 限两个活跃 GET，出现两个临时繁忙后保留重新预览；显式重试后成功图片从两张变三张，繁忙计数未自动增长，暂忙文件经重新检查得到直接 href。截图与证据只写外层 `outputs/remote-rich-*-Mock-*` 和 `remote-rich-Mock-evidence.json`，不连接真实服务或修改同级 Mobile。
- `tsc --noEmit` 与独立 remote Vite 构建通过；新依赖使用自带类型，无高亮或数学扩展依赖，完整与生产依赖 audit 均为零 advisory， bundled 依赖许可写入 `THIRD_PARTY_NOTICES.md`。

## Consequences

消息更可读、文件与复制入口不破坏现有原生壳契约；代价是 parser 进入本地 bundle，超额元数据和暂时繁忙的图片需要用户显式重试。HTTP 传输本身不加密；前端不代替后端会话、原生 refs 与真实工作区授权。真实 Harmony 设备、最终原生下载与系统剪贴板仍需主代理独立联测，Mock 不能证明真机成功；全量测试与 Windows/Linux CI 由主代理汇合执行。

## Note audit

[远程及数据边界](../architecture/2026-11-03-codeyx-remote-and-data.md)部分重叠，继续沿用其授权、会话代次与只读控制台限制，不修改其决定；同批后端文件与公开投影笔记互补，服务端授权不归本篇实现。[会话库存](../bug-fix/2026-11-04-remote-thread-inventory.md)与[选定能力](../simplification/2026-11-04-selected-custom-preview.md)互补，不恢复已撤回的 custom 补丁。命中的内置工具、自定义角色不属于本次消息、文件或复制实现，无过时提案需要删除或归档。
