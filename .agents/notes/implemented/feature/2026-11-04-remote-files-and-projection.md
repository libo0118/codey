# Agent Note: 会话文件下载与公开消息投影

Status: implemented

## Problem

远程投影将未知 item 和受支持审批的整个 params 序列化，会把未来协议的私有配置带出桌面；本地附件只有名称，模型在公开消息内交付的工作区文件也不能下载。下载不能变成任意文件代理、安装包中心或另一个模型运行入口。

## Decision

- 保留既有 JSON 字段，按已知原生 item 白名单投影公开文本、命令、工具结果、计划、Diff、图片和审批。未知 item 固定占位；隐藏推理、原始 RPC、配置和凭据不参与投影。审批摘要不足的结构在显示与 respond 校验中一致拒绝，不盲批；异步问题按同一请求尚未回答的完整集合验证，非法问题或选项使整组 unsupported，不过滤后部分答题，已回答项仍使用原始索引身份。
- 手机首次发送、普通发送和补充指令支持普通文件：图片与普通文件共享最多 4 个、解码后合计 4 MiB 的边界；服务端只接受文件名和内联字节，不接受客户端路径。仅实际提交时调用 Native 的 `createFileAttachment` 创建托管文件，并按 Native 的文件上下文及 `attachments` 元数据传递；准备前后复验授权和模型门禁。尚未发送时准备失败或授权撤销会清理已知托管文件；一旦写入结果不明，不删除也不自动重试，以免破坏已提交的消息。
- `POST /remote/files/resolve` 接受 `threadId/reference`，使用现有 cookie、Origin、Host、CSRF；只查询已绑定原生会话的私有引用，不启动或切换桌面。解析只读取文件版本、大小及最多 12 字节格式头；公开内嵌图片只校验编码、长度和格式头，不解析本机路径或预先复制全部数据。Markdown 候选复用手机实际可见的截断公开投影，包括工具结果、公开摘要和协作结果，不从隐藏 prompt 或未知事件取链接；候选必须另满足原生 cwd 内的安全工作区边界；仅明确用户输入的原生结构化附件允许 cwd 外的精确引用，普通原生 path 不按 Markdown URL 百分号解码。
- `GET /api/sessions/{UUID}/files/{64hex}` 使用同源 cookie 重新授权，检查会话、引用、源文件身份和边界。只下载普通文件，不跟随 symlink/reparse、不访问 UNC 或外部 URL；不映射 sandbox、cloud、SSH 路径，不浏览目录。
- 显式 GET 在响应头前重验解析时记录的版本，使用现有 `tempfile`、`sha2`、`http-body-util` 和 Tokio 创建私有磁盘快照，再分块发送；变化返回 409，不暗中换版本。单文件 200 MiB、并行下载 2 个、快照总额 400 MiB；元数据引用最多 64 项并保留 10 分钟，快照在发送完成、取消、撤销或停止后释放，不占满元数据 TTL。生产仅解析受保护目录的路径元数据和真实目标，父工作区中的配置目录同样不能下载；保护根存在却无法确认时失败，不静默放行。
- 同一登录和会话的窗口各持独立 binding，依据 Native owner、数值 revision 与历史覆盖范围协调授权；已观察到的较新撤回不能被旧窗口维持或复活，正常未加载范围不当作删除。未知版本、同源回退或 reset 使文件授权需重新建立，但不连带关闭有效聊天流；最后 lease 清除整组授权。配置保护根按真实目标核验，不能借父工作区或符号链接绕过。
- Unix 用逐段 `openat/O_NOFOLLOW`，Windows 打开 reparse point 本身检查属性，并持有禁止删除/改名的父目录及源文件句柄。Linux 从 `/proc/self/fd` 核验实际位置，受限制时明确失败；不以路径字符串检查替代句柄边界。仅魔数确认的 PNG/JPEG/GIF/WebP 可同源图片预览，HTML/SVG 始终下载。传输队列最多两个 64 KiB 块，等待消费 30 秒超时；取消只清理自己拥有的匿名临时文件，不删除源文件。

## 活动笔记审计

[远程与数据边界](../../implemented/architecture/2026-11-03-codeyx-remote-and-data.md)、[会话库存](../../implemented/bug-fix/2026-11-04-remote-thread-inventory.md)与本篇互补，不改变原有账号、审批动作、库存和只读控制台能力；检索命中的内置工具、自定义角色与选定 custom 方案不属于此次实现。无须修改或归档其他笔记。

## Alternatives considered

- 直接输出原始 RPC：兼容未来 item 最简单，但会泄漏私有字段，选择明确白名单与安全占位。
- 将 Markdown 链接视为读取授权：可下载更多路径，但模型可以构造任意文件路径，选择候选与授权分离。
- 直接读取源文件响应：无需快照磁盘，但编辑器替换和原地写入会产生混合内容，选择有界私有快照。
- 引入通用文件浏览、Range 或服务端 URL 代理：可扩展更多工作流，但不属于会话附件下载最小需求，本次不实现。

## Verification

先新增调用真实 `protocol::view`、`Projection::frame` 的秘密 canary、已知多块和 patch 一致性回归，再替换全量序列化。文件与 HTTP 回归使用临时合成目录、保护根及 symlink 目标，覆盖真实字节与 SHA、授权、跨会话、引用替换、越界、链接、变化、大小、取消、资源上限和恶意文件名；不读取真实配置、会话或凭据。操作 ACK 只保留固定的 `ok/accepted` status，不回传原生 message 或 RPC 扩展字段，未知状态明确报结果不明。

本机已执行定向 rustfmt、`cargo metadata --locked --offline --no-deps --format-version 1` 和 `git diff --check -- backend/src/remote_control`。缺少 `link.exe`，本次不重复已知无法启动的原生编译，尚未获得可执行 red/green 证据；测试代码存在不等于通过，Rust 编译、测试及 Clippy 由 Windows/Linux CI 负责。

附件的 Native 契约已只读核对本机与 121 的静态 ASAR：`createFileAttachment({contents: Blob,label})` 返回 `label/path/fsPath`，`removePastedTextAttachment(path)` 清理同一托管集合，steer 接受附件元数据；未启动或调用 121 的模型。Node 隔离 Mock 覆盖真实适配器的 Blob、创建参数、部分失败、门禁变更和清理失败；它不能证明真实桌面或模型端到端效果。Rust 新增聚焦回归，执行结果仍须 CI 验证。

## Consequences

下载能力依赖当前在线绑定投影，不读取 transcript 或凭据补齐历史；未映射远程路径明确不支持。安全工作区不能是 home、系统根或 Codex 配置目录。文件级快照保证响应字节一致，不声称对整台主机提供原子快照；取消在块边界检查，不能承诺中断已经进入内核的磁盘调用。附件准备中若桌面响应中断，无法保证已创建文件已清理；提交结果不明时需先检查会话。超额下载明确繁忙，由前端提供手动重试，不增加无限队列、Range、URL 代理、目录浏览或写权限。
