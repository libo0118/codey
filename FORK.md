# 自定义分支维护

本仓库的 `master` 跟随 [SuperGness/codey](https://github.com/SuperGness/codey) 上游主分支；`custom` 保留模型思考档位同步修复。日常使用和构建选择 `custom`，不要把自定义提交合并回 `master`。

同步模型时保留上游声明的思考档位。Responses 兼容服务缺少普通模型元数据时，会补查 Codex 模型目录；手动档位设置优先，上游请求失败时保留上次成功结果。不同线路的同名模型互不覆盖。该功能同时作用于 Codex 模型列表与 Codey 子代理角色选择。

## 合并上游更新

保持工作区干净后，在 PowerShell 运行：

```powershell
./scripts/sync-upstream.ps1
```

脚本只在 `master` 快进更新，随后合并到 `custom`。冲突时停下，解决冲突并测试后再推送；不会重置本地修改或强制推送。也可以手动执行 `git fetch upstream --tags`、`git merge upstream/master`，但应在 `custom` 上保留修复提交。

## 发布

沿用仓库的 `Build desktop packages` 工作流，对 `custom` 手动运行可生成安装包；版本号使用 `上游版本-libo.序号` 区分，例如 `1.1.1-libo.1`。发布前运行 `pnpm run check`、`pnpm run test:js`、`cargo test --workspace --locked`、`cargo fmt --all -- --check` 和 `cargo clippy --workspace --all-targets --locked -- -D warnings`。

GitHub 仓库变量 `CLOUDFLARE_R2_PUBLIC_BASE_URL` 指向 `https://github.com/libo0118/codey/releases/latest/download`，因此自定义安装包查询本 fork 的更新清单，不会自动替换为上游包。每次发布应使用 `scripts/generate-update-manifest.mjs` 生成并上传 `latest.json`，下载地址设为该版本的 GitHub Release URL，清单包含安装包大小及 SHA-256。未配置上游的 R2 密钥，无需上传到上游存储。

供日常更新的自定义 Release 应标记为 Latest，不能标记为 GitHub Pre-release，否则 `releases/latest` 不会选中它。初次从官方版切换需手动安装自定义包；版本后缀仅用于区分分支，不代表已经安装。

原有许可证和第三方声明保持不变。Release 只包含构建产物和校验清单，不包含本机配置、API Key 或账号文件。
