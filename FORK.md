# 自定义分支维护

本仓库的 `master` 跟随 [SuperGness/codey](https://github.com/SuperGness/codey) 上游主分支；`custom` 在上游基础上保留模型思考档位同步、生命周期修复与 fork 更新配置。日常使用和构建选择 `custom`，不要把自定义提交合并回 `master`。

同步模型时保留上游声明的思考档位。Responses 兼容服务缺少普通模型元数据时，会补查 Codex 模型目录；手动档位设置优先，但不能超出插件声明的支持范围，上游请求失败时保留上次成功结果。不同线路的同名模型互不覆盖。该功能同时作用于 Codex 模型列表与 Codey 子代理角色选择。

子代理权限与调度沿用上游，生命周期补足已完成代理的身份保留与派生失败结算：各角色使用一致的工具能力，操作受任务范围和 Codex 原生权限约束；并发上限为两个，写入由调用方协调。角色仅用于任务分工和模型选择，不再额外维护分析角色只读限制或命令白名单。

## 合并上游更新

保持工作区干净后，在 PowerShell 运行：

```powershell
./scripts/sync-upstream.ps1
```

脚本只在 `master` 快进更新，随后合并到 `custom`。冲突时停下，解决冲突并测试后再推送；不会重置本地修改或强制推送。也可以手动执行 `git fetch upstream --tags`、`git merge upstream/master`，但应在 `custom` 上保留修复提交。

## 发布

`Build desktop packages` 工作流分别构建 Windows x64 安装包，以及 Linux x64 的 `.tar.gz`、Debian 系 `.deb`、红帽系 `.rpm`；暂不构建 ARM。对 `custom` 手动运行只上传构建产物，不发布 Release。版本号使用 `上游版本-libo.序号` 区分，例如 `1.1.1-libo.2`；版本标签触发构建，两平台检查和打包都成功后，统一发布安装包与更新清单。发布前运行 `pnpm run check`、`pnpm run test:js`、`cargo test --workspace --locked`、`cargo fmt --all -- --check` 和 `cargo clippy --workspace --all-targets --locked -- -D warnings`。

构建时将更新地址设为本 fork 的 `https://github.com/libo0118/codey/releases/latest/download`，不会自动替换为上游包。指定自定义更新地址后，仅在同时显式配置发布管理服务时使用该服务。工作流使用 `scripts/generate-update-manifest.mjs` 生成统一 `latest.json`，包含两平台各格式包的地址、大小及 SHA-256；无需 R2 密钥或存储。Linux 安装包将 `codey` 与 `codey-fastctx` 放在 `/usr/lib/codey`，提供 `codey` 命令和桌面入口；使用 `apt install ./Codey-版本-linux-x64.deb` 或 `dnf install ./Codey-版本-linux-x64.rpm` 安装，压缩包也可解压后运行 `./codey`。需系统已安装官方 Codex 桌面端；Linux 自动安装更新暂不支持。卸载仅删除包文件，不删除用户数据，不配置开机自启。

Linux ELF 在 UBI 8.10（glibc 2.28）x64 容器中完成 Rust 检查和构建，使用独立的 `target/linux-el8`，避免引入 Ubuntu 构建机较新的 glibc 依赖。同一组 ELF 在 Debian 11 容器中生成三个 Linux 包，`dpkg-shlibdeps` 使用该基线的符号元数据计算 Debian 依赖，避免打包主机误抬高 glibc 要求；RPM 保留自动 ELF 依赖。CI 读回元数据、解包比对内容，并在 Debian 11 与 UBI 8 容器中验证安装和卸载。运行时需 `ps`（Debian 的 `procps`／红帽系的 `procps-ng`），安装包会声明该依赖，压缩包需自行准备。系统 ABI 基线为 Debian 11／Ubuntu 20.04／RHEL 8 或更新版本；这些容器检查不替代 Codex 桌面 GUI 的实机验收。

供日常更新的自定义 Release 应标记为 Latest，不能标记为 GitHub Pre-release，否则 `releases/latest` 不会选中它。初次从官方版切换需手动安装自定义包；版本后缀仅用于区分分支，不代表已经安装。

原有许可证和第三方声明保持不变。Release 只包含构建产物和校验清单，不包含本机配置、API Key 或账号文件。
