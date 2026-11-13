# Agent Note: Cindy 页面切片与显式宿主 DOM v2

Status: implemented

## Problem

用户明确要求第一阶段以 Cindy 前端为主体迁移项目/会话列表与新建对话，而不是为旧 Sidebar 换肤。后端与控制器已经处理默认值、草稿、附件与未知写入结果；迁移不应改变这些语义，也不应为不存在的云功能伪造状态。鸿蒙宿主原先依赖旧 DOM 层级，需要明确而可验证的导航合同。

## Decision

- `src/remote/cindy/` 从 Cindy `deef17354321527fd857acf62662955139ef2491` 的 HomeSurface、HomeNewTaskButton 与 sessions/new 页面提取 RN render/StyleSheet 切片，经 `react-native-web@0.21.4` 渲染。原始行段、实际改动及未复制部分写在 `SOURCE.md`，Apache-2.0 原许可与新 Web 运行依赖许可均保留。不搬 Expo、云服务、语音、原生 morph、虚拟历史或业务状态模型。
- 列表由 Cindy 的 chevron/folder/标题/计数项目头、缩进子项、行首状态图标、单行标题和右侧优先状态槽构成。项目子项桌面缩进 25px、手机 27px；标题省略但 DOM `title` 保留全名。只有 `running === true` 呈现橙色运行图标与右侧中性 spinner，未知状态只显示已有时间，不伪造错误、等待或未读。折叠汇总仅观察当前已加载条目，计数明确标注该范围。保留空项目、新建、归档、刷新与深链复制；按用户要求移除列表搜索入口和输入栏，后端既有查询协议不变。
- 新建页保留底部设备/工作区选择器堆栈和首条消息区域；设备只读显示 `location.host`，工作区仅选择真实已保存项目。草稿、附件、模型、速度、权限和发送仍由原 Composer 与 `c.startCreating/changeSettings/changeDraft/send` 处理。选择项目不创建假会话；默认值成功加载前禁发、附件读取/丢失禁发、项目/会话草稿隔离以及未知写入防重均不变。消息和审批页只接入显式导航标记，不重写认证、CSRF、transport、DTO 或历史增量。
- 原 `sidebar.tsx` 删除，全部调用方迁到 `HomeSurface`，不保留内部旧别名。当前活动页只有一个标记顶栏；驻留列表仍保留真实 footer 动作，因此未接入 HAP 的网页也能自行导航。
- RNWeb Pressable 用 `data-cindy-control` 声明样式所有权，原 Web 通用按钮规则通过零特异性的 `:where(:not(...))` 排除这些控件，不覆盖 Cindy 的圆形新建入口和透明列表行。图标、菜单及选择器仍保留至少 44px 点击区，不用缩小热区换取紧凑外观。

## 宿主合同

- Workspace 根保留 `.remote-codex-workspace` 供未迁移消息样式使用，并带 `data-codey-ui="2"`、`data-codey-scene="list|create|conversation"`。
- 当前活动页唯一顶栏为 `data-codey-chrome="header"`；列表唯一 footer 为 `data-codey-chrome="footer"`。footer 内三个唯一真实按钮分别标记 `data-codey-action="archive|console|logout"`，archive 的 `aria-pressed` 始终为 true/false，忙碌时真实 disabled。
- create/conversation 另有唯一 `data-codey-action="back"` 按钮，回调原 `c.back`。list 总动作标记为 3，create/conversation 为 4。新建等按钮不复用此标记。控制台原返回按钮标记 `data-codey-console-back=""`，调用原 tab 切换。没有新增 window 全局、token 或高权限 API。

## 依赖边界

现有 React/React DOM 锁定版本不变。RNWeb 的 peer 支持 React 18/19。`@types/react-native-web@0.19.2` 的 `react-native:*` 原本解析为要求 React ^19.3 的 0.88.1；仅在 `pnpm-workspace.yaml` 覆盖该父子依赖到 0.80.3，不做全局覆盖，也不新增直接原生运行依赖。pnpm 11 不读取 package.json 的旧 overrides 字段，因此使用现有 workspace 配置且保留原 allowBuilds/patch。

第三方类型同时漏声明运行时 StyleSheet 值与 Pressable 的 accessibilityRole；局部精确模块补充只恢复这两个实际支持的接口，不使用 any 逃避类型检查，不补运行时兼容层。其他 Web ARIA 参数直接使用已有类型。构建模块清单确认没有把 react-native/Metro/Expo 引入远程 bundle。

三平台分发清单同时包含 Cindy 原许可、页面切片 SOURCE 和 RNWeb 运行依赖许可；只把文件放进源码仓库不足以让安装包保留声明。Linux tar/安装树使用真实打包夹具检查字节与白名单，macOS/NSIS 检查配方来源与 Windows 明确卸载清单，不扩大为递归删除用户文件。

## Existing-note audit

- [移动页面视觉](../feature/2026-11-12-remote-mobile-layout.md)部分重叠：本篇取代其列表/新建页纯 Web 与旧固定 DOM 决定，原有消息、输入、审批及滚动理由保留，双方互链。
- [可信运行状态](../feature/2026-11-04-remote-running-indicators.md)部分重叠：真实状态来源、失联清除与不激活会话仍有效，只更新橙色图标/spinner 的呈现事实。
- [远程数据边界](2026-11-03-codeyx-remote-and-data.md)、[富消息与深链](../feature/2026-11-04-remote-rich-timeline-and-links.md)、[有界增量](2026-11-04-bounded-remote-stream.md)部分重叠，其独立安全与公开投影决定不变。
- 按 remote、远程、Cindy、手机、HAP 检索 proposed/implemented/rejected（排除 archived）；命中的上游集成、自定义角色和内置工具与本次页面切片无关，没有需删除或归档的过时提案。

## Alternatives considered

- 仅重排 Sidebar/CSS 的改动与依赖成本最小，但不满足用户明确的 Cindy 页面主体迁移要求，因此删除旧页面而不是换肤。
- 搬完整 Expo/Cindy App 可以保留更多原生细节，但会引入无关云状态、认证、harness 和运行时，且可能替换现有业务控制器；只适配已确认的 RN 页面切片。
- 整体重写输入为 Cindy Composer 可以更完整复刻其动效，但会重复已验证的附件、权限和防重状态机；保留 Codey Composer，作为新页面底部真实业务插槽。

## Verification

10 个定向前端测试文件共 105 项通过，覆盖三场景唯一 header/footer/action、返回接线、项目路径和选择、空项目/忙碌态、默认值未就绪禁发、单行标题、严格 running、折叠汇总、深链及原 footer 事件。旧 DOM 测试迁到用户批准的 v2 合同，未删除业务行为断言；原控制器与附件/未知结果测试保持执行。

类型检查与独立远程 production 构建通过，pnpm peer 检查无冲突；锁文件断言确认原有 18 个直接依赖版本不变，React/React DOM 仍为 19.2.7。笔记 tree/format/archived 三门禁和 `git diff --check` 通过。合成 QA 夹具单独位于外层 `work/cindy-migration-qa`，拒绝外部网络，只提供 production bundle 与合成 fetch/WebSocket。

主代理真实浏览器检查先发现旧按钮 CSS 将新建入口覆盖成白底 12px 圆角；新增样式所有权反例先失败，修复后通过，实际计算样式恢复为深色背景／28px 圆角。production UI 与实际 HAP 脚本联测确认列表接管、三场景状态、控制台返回、归档和注销撤回；390×844、320×500 无横向溢出，320×500 输入区底部约 460px。项目、模型、权限和草稿返回保留；选择期间零创建，显式发送后仅一次合成 `/remote/create`，工作区、模型及只读权限透传。随后按用户要求移除列表搜索，反例先失败、定向 10 项测试及类型检查／远程构建再通过；390×844 实际浏览器确认零搜索输入／按钮且无横向溢出，HAP 仍识别 list 并隐藏重复 footer。以上不是 ArkWeb 真机或真实模型调用验收。

## Consequences

页面主体可追溯到 Cindy，而后端和写入语义留在 Codey；代价是加入有限 RNWeb 运行时及开发类型依赖，原生动效和完整 Cindy 功能并不随迁移提供。宿主须使用明确 v2 合同，新版 DOM 不保留旧 Sidebar 层级；普通网页仍保留全部导航。合成测试不代表真实模型、实机软键盘、ArkWeb 或生产服务验收。本次不发布、部署、安装或发送真实模型消息。
