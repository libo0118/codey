# Cindy 页面切片来源

本目录适配 [makecindy/cindy](https://github.com/makecindy/cindy/tree/deef17354321527fd857acf62662955139ef2491)，固定提交 `deef17354321527fd857acf62662955139ef2491`，Apache-2.0。原许可逐字保存在 `licenses/Cindy/LICENSE`。本目录是修改后的页面切片，不是完整 Cindy App。

| 本地文件 | 原始源码切片 | 实际保留与修改 |
| --- | --- | --- |
| `HomeSurface.tsx` | `apps/mobile/src/session/HomeSurface.tsx:4213-4289,4320-4344,4744-4782,4841-4868` | 保留 Pressable 项目头的 chevron、folder、标题、计数，嵌套项目子项，以及 View/Text 单行会话标题和右侧优先状态槽。数据改为 Codey Project/Thread；只支持真实 running，不移植未读、错误、等待、共享角色、预览或云状态。 |
| `HomeNewTaskButton.tsx` | `apps/mobile/src/session/HomeNewTaskButton.tsx:40-48` | 保留绝对定位圆形新建入口；移除 iOS morph、haptics 与原生玻璃依赖，改用 RNWeb Pressable。 |
| `NewSessionSurface.tsx` | `apps/mobile/app/sessions/new.tsx:5160-5187,5269-5287,5617-5671` | 保留 ScrollView 底部选择器堆栈、设备/工作区两层选择和底部首条消息区域；设备是只读当前连接地址，工作区选择现有项目或 Codey 接入的原生无项目模式。输入、附件、权限、模型/速度和实际提交复用 Codey Composer/控制器。 |
| `styles.ts` | `HomeSurface.tsx:5197-5204,5529-5535`；`new.tsx:6320-6343` | 保留 header、18×18 状态槽、bottomCluster、selectorStack/selectorRow/selectorText 的 StyleSheet 排列。主题 token 转为 Codey CSS 变量；未缓存的 HomeListVisuals 样式不声称复制，项目/会话行尺寸为 Web 适配。 |

`styles.css` 是 Web 布局、层级缩进、原生 details 菜单与响应式适配，不是原 Sidebar 的换肤层。图标沿用已安装的 Tabler；没有复制 lucide 代码。`sidebar.tsx` 已删除，无内部旧别名。

不移植 Expo、Cindy 网络/云账号、device pairing、harness/worktree 创建、虚拟历史、语音或原生动效。新建表单只有明确发送首条消息才使用 Codey `/remote/create`，选择项目和设置不创建空对话。
