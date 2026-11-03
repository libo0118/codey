// Adapted from fraternity-z/CodeyX (30e1daa), AGPL-3.0-only.
pub(crate) mod process;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
#[cfg(test)]
use std::{
    io::Read,
    process::{Command, Stdio},
    time::Instant,
};
use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};

pub(super) const VERSION: &str = "1.6.2";
const MAX_DOWNLOAD: usize = 200 * 1024 * 1024;
const MAX_UNPACKED: u64 = 800 * 1024 * 1024;

pub(super) fn bundle(root: &Path) -> PathBuf {
    root.join(format!("v{VERSION}"))
}
pub(super) fn executable(root: &Path) -> PathBuf {
    bundle(root).join(if cfg!(windows) { "node.exe" } else { "node" })
}
fn entry(root: &Path) -> PathBuf {
    bundle(root).join("lib/dist/bin/codegraph.js")
}
pub(super) fn installed(root: &Path) -> bool {
    executable(root).is_file() && entry(root).is_file()
}

pub(super) fn mcp_config(root: &Path) -> Value {
    json!({
        "command": executable(root),
        "args": ["--liftoff-only", "--disable-warning=ExperimentalWarning", root.join("auto-mcp-v1.cjs"), entry(root)],
        "env": {"CODEX_HOME":crate::codex_config::codex_home(), "CODEGRAPH_TELEMETRY":"0", "DO_NOT_TRACK":"1", "NODE_OPTIONS":"", "NODE_PATH":""},
        "startup_timeout_sec": 60, "tool_timeout_sec": 120, "enabled": true
    })
}

pub(super) fn legacy_mcp_args(root: &Path) -> Value {
    json!([
        "--liftoff-only",
        "--disable-warning=ExperimentalWarning",
        entry(root),
        "serve",
        "--mcp"
    ])
}

pub(crate) fn is_managed_mcp(config: &Value, root: &Path) -> bool {
    let expected = mcp_config(root);
    config["command"] == expected["command"]
        && config.get("cwd").is_none_or(Value::is_null)
        && (config["args"] == expected["args"] || config["args"] == legacy_mcp_args(root))
}

pub(super) fn prepare_launcher(root: &Path) -> Result<()> {
    crate::fs_util::atomic_write_private_with_parent(
        &root.join("auto-mcp-v1.cjs"),
        include_bytes!("commands/builtin_tools/auto_mcp.cjs"),
    )
}

pub(super) fn platform(os: &str, arch: &str) -> Result<String> {
    let os = match os {
        "windows" => "win32",
        "macos" => "darwin",
        "linux" => "linux",
        _ => anyhow::bail!("当前系统没有 CodeGraph 官方运行包"),
    };
    let arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => anyhow::bail!("当前架构没有 CodeGraph 官方运行包"),
    };
    Ok(format!("{os}-{arch}"))
}

async fn download(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .context("无法下载 CodeGraph，请检查网络后重试")?
        .error_for_status()
        .context("CodeGraph 下载服务器返回错误")?;
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= limit as u64),
        "CodeGraph 下载超过大小限制"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("CodeGraph 下载中断，请重试")?
    {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= limit,
            "CodeGraph 下载超过大小限制"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn verify_checksum(bytes: &[u8], sums: &str, asset: &str) -> Result<()> {
    let expected = sums
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            (fields.next()?.trim_start_matches('*') == asset).then_some(hash)
        })
        .context("官方校验清单缺少 CodeGraph 安装包，未执行安装")?;
    ensure!(
        expected.len() == 64 && expected.eq_ignore_ascii_case(&crate::fs_util::sha256_hex(bytes)),
        "CodeGraph 安装包校验失败，请重试下载"
    );
    Ok(())
}

fn archive_path(path: &Path, prefix: &str) -> Result<PathBuf> {
    let mut components = path.components();
    ensure!(
        components.next() == Some(Component::Normal(prefix.as_ref())),
        "CodeGraph 安装包根目录无效"
    );
    let mut relative = PathBuf::new();
    for component in components {
        let Component::Normal(name) = component else {
            anyhow::bail!("CodeGraph 安装包包含不安全路径")
        };
        ensure!(
            !name.to_string_lossy().contains([':', '\\']),
            "CodeGraph 安装包包含不安全文件名"
        );
        relative.push(name);
    }
    Ok(relative)
}

fn unpack(bytes: &[u8], stage: &Path, target: &str) -> Result<()> {
    let prefix = format!("codegraph-{target}");
    let mut total = 0u64;
    #[cfg(windows)]
    {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        ensure!(archive.len() <= 30000, "CodeGraph 安装包文件过多");
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let relative = archive_path(Path::new(file.name()), &prefix)?;
            if relative.as_os_str().is_empty() {
                continue;
            }
            ensure!(!file.is_symlink(), "CodeGraph 安装包包含链接");
            total = total.saturating_add(file.size());
            ensure!(total <= MAX_UNPACKED, "CodeGraph 解压超过大小限制");
            let destination = stage.join(relative);
            if file.is_dir() {
                std::fs::create_dir_all(destination)?;
            } else {
                std::fs::create_dir_all(destination.parent().context("安装路径无效")?)?;
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(destination)?;
                std::io::copy(&mut file, &mut output)?;
            }
        }
    }
    #[cfg(unix)]
    {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
        for (i, entry) in archive.entries()?.enumerate() {
            ensure!(i < 30000, "CodeGraph 安装包文件过多");
            let mut file = entry?;
            let relative = archive_path(&file.path()?, &prefix)?;
            if relative.as_os_str().is_empty() {
                continue;
            }
            let kind = file.header().entry_type();
            // npm 的 .bin 链接不参与运行；只提取运行时所需的普通文件和目录。
            if kind.is_symlink() || kind.is_hard_link() {
                continue;
            }
            ensure!(
                kind.is_file() || kind.is_dir(),
                "CodeGraph 安装包文件类型无效"
            );
            total = total.saturating_add(file.size());
            ensure!(total <= MAX_UNPACKED, "CodeGraph 解压超过大小限制");
            let destination = stage.join(relative);
            if kind.is_dir() {
                std::fs::create_dir_all(destination)?;
            } else {
                std::fs::create_dir_all(destination.parent().context("安装路径无效")?)?;
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)?;
                std::io::copy(&mut file, &mut output)?;
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    destination,
                    std::fs::Permissions::from_mode(file.header().mode()? & 0o755),
                )?;
            }
        }
    }
    Ok(())
}

pub(super) async fn install(root: PathBuf, client: reqwest::Client) -> Result<()> {
    if installed(&root) {
        return Ok(());
    }
    let target = platform(std::env::consts::OS, std::env::consts::ARCH)?;
    let asset = format!(
        "codegraph-{target}.{}",
        if cfg!(windows) { "zip" } else { "tar.gz" }
    );
    let base = format!("https://github.com/colbymchenry/codegraph/releases/download/v{VERSION}");
    let sums = download(&client, &format!("{base}/SHA256SUMS"), 64 * 1024).await?;
    let bytes = download(&client, &format!("{base}/{asset}"), MAX_DOWNLOAD).await?;
    verify_checksum(&bytes, std::str::from_utf8(&sums)?, &asset)?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        std::fs::create_dir_all(&root)?;
        let stage = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&root)?;
        unpack(&bytes, stage.path(), &target)?;
        ensure!(
            stage
                .path()
                .join(if cfg!(windows) { "node.exe" } else { "node" })
                .is_file()
                && stage.path().join("lib/dist/bin/codegraph.js").is_file(),
            "CodeGraph 安装包缺少运行文件"
        );
        ensure!(
            !bundle(&root).exists(),
            "CodeGraph 安装目录已存在但不完整，请先检查该目录"
        );
        std::fs::rename(stage.path(), bundle(&root)).context("保存 CodeGraph 运行包失败")?;
        Ok(())
    })
    .await
    .context("CodeGraph 解压任务异常退出")??;
    Ok(())
}

#[cfg(test)]
fn read_output(mut source: impl Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let count = source.read(&mut chunk)?;
        if count == 0 {
            return Ok(output);
        }
        output.extend_from_slice(&chunk[..count]);
        if output.len() > 128 * 1024 {
            output.drain(..output.len() - 128 * 1024);
        }
    }
}

#[cfg(test)]
fn execute(mut command: Command, timeout: Duration) -> Result<Vec<u8>> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::codegraph_runtime::process::OwnedChild::spawn(&mut command)
        .context("无法启动 CodeGraph")?;
    let stdout = child.stdout.take().context("无法读取 CodeGraph 输出")?;
    let stderr = child.stderr.take().context("无法读取 CodeGraph 错误输出")?;
    std::thread::scope(|scope| {
        let output = scope.spawn(move || read_output(stdout));
        let errors = scope.spawn(move || read_output(stderr));
        let start = Instant::now();
        let result = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Err(error) => break Err(anyhow::Error::from(error)),
                _ if start.elapsed() >= timeout => {
                    break Err(anyhow::anyhow!(
                        "CodeGraph 执行超时，任务已停止；可检查索引后重试"
                    ));
                }
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        drop(child);
        let output = output
            .join()
            .map_err(|_| anyhow::anyhow!("CodeGraph 输出读取失败"))??;
        let _errors = errors
            .join()
            .map_err(|_| anyhow::anyhow!("CodeGraph 错误读取失败"))??;
        let status = result?;
        // 上游输出可能包含源码或项目配置，不直接回传或记入宿主日志。
        ensure!(
            status.success(),
            "CodeGraph 执行失败（退出码 {:?}），请检查项目权限、磁盘空间或重建索引",
            status.code()
        );
        Ok(output)
    })
}

#[cfg(test)]
pub(super) async fn run(root: PathBuf, project: PathBuf, action: &'static str) -> Result<Value> {
    tokio::task::spawn_blocking(move || {
        ensure!(installed(&root), "CodeGraph 尚未安装，请点击一键接入项目");
        // status 会向父目录寻找索引，管理界面必须保持用户所选项目的边界。
        if action == "status" && !project.join(".codegraph/codegraph.db").is_file() {
            return Ok(json!({"initialized":false,"projectPath":project}));
        }
        let mut command = Command::new(executable(&root));
        command
            .args(["--liftoff-only", "--disable-warning=ExperimentalWarning"])
            .arg(entry(&root));
        command.arg(action).arg(&project);
        if action == "status" {
            command.arg("--json");
        }
        if action == "init" {
            command.arg("--yes");
        }
        command
            .current_dir(&project)
            .env("CODEGRAPH_TELEMETRY", "0")
            .env("DO_NOT_TRACK", "1")
            .env("NO_COLOR", "1")
            .env_remove("NODE_OPTIONS")
            .env_remove("NODE_PATH")
            .env_remove("CODEGRAPH_DIR");
        let output = execute(
            command,
            Duration::from_secs(if action == "status" { 30 } else { 1800 }),
        )?;
        if action == "status" {
            let value: Value = serde_json::from_slice(&output)
                .context("CodeGraph 状态格式无效，请检查运行包版本")?;
            ensure!(
                value["initialized"].is_boolean(),
                "CodeGraph 状态缺少初始化标记"
            );
            let reported = value["projectPath"]
                .as_str()
                .context("CodeGraph 状态缺少项目路径")?;
            if Path::new(reported).canonicalize().ok() != project.canonicalize().ok() {
                return Ok(json!({"initialized":false,"projectPath":project}));
            }
            Ok(value)
        } else {
            Ok(Value::Null)
        }
    })
    .await
    .context("CodeGraph 执行任务异常退出")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_platforms_and_unsupported_architectures() {
        assert_eq!(platform("windows", "aarch64").unwrap(), "win32-arm64");
        assert_eq!(platform("macos", "x86_64").unwrap(), "darwin-x64");
        assert!(platform("linux", "x86").is_err());
        assert!(platform("other", "x86_64").is_err());
    }
    #[test]
    fn checksums_fail_closed() {
        let hash = crate::fs_util::sha256_hex(b"bundle");
        assert!(verify_checksum(b"bundle", &format!("{hash}  bundle.zip"), "bundle.zip").is_ok());
        assert!(verify_checksum(b"changed", &format!("{hash}  bundle.zip"), "bundle.zip").is_err());
        assert!(verify_checksum(b"bundle", "", "bundle.zip").is_err());
    }
    #[test]
    fn archive_paths_stay_inside_staging() {
        assert_eq!(
            archive_path(
                Path::new("codegraph-win32-x64/lib/main.js"),
                "codegraph-win32-x64"
            )
            .unwrap(),
            PathBuf::from("lib/main.js")
        );
        for path in [
            "../outside",
            "codegraph-win32-x64/../../outside",
            "/absolute",
            "other/file",
            "codegraph-win32-x64/file:stream",
        ] {
            assert!(
                archive_path(Path::new(path), "codegraph-win32-x64").is_err(),
                "{path}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn zip_extraction_preserves_stage_boundary() {
        use std::io::Write;
        let archive = |path: &str| {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            writer
                .start_file(path, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"fixture").unwrap();
            writer.finish().unwrap().into_inner()
        };
        let temp = tempfile::tempdir().unwrap();
        let stage = temp.path().join("stage");
        std::fs::create_dir(&stage).unwrap();
        unpack(
            &archive("codegraph-win32-x64/lib/fixture.js"),
            &stage,
            "win32-x64",
        )
        .unwrap();
        assert_eq!(
            std::fs::read(stage.join("lib/fixture.js")).unwrap(),
            b"fixture"
        );
        assert!(
            unpack(
                &archive("codegraph-win32-x64/../../outside"),
                &stage,
                "win32-x64",
            )
            .is_err()
        );
        assert!(!temp.path().join("outside").exists());
    }

    #[cfg(unix)]
    #[test]
    fn tar_extraction_keeps_permissions_and_does_not_follow_links() {
        let mut writer = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o755);
        header.set_size(7);
        header.set_cksum();
        writer
            .append_data(
                &mut header,
                "codegraph-linux-x64/node",
                b"fixture".as_slice(),
            )
            .unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_mode(0o777);
        link.set_size(0);
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_cksum();
        writer
            .append_link(&mut link, "codegraph-linux-x64/link", "../../outside")
            .unwrap();
        let bytes = writer.into_inner().unwrap().finish().unwrap();
        let stage = tempfile::tempdir().unwrap();
        unpack(&bytes, stage.path(), "linux-x64").unwrap();
        assert_eq!(
            std::fs::read(stage.path().join("node")).unwrap(),
            b"fixture"
        );
        assert!(!stage.path().join("link").exists());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(stage.path().join("node"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[test]
    fn output_is_bounded_without_stopping_the_reader() {
        let bytes = vec![b'x'; 256 * 1024];
        assert_eq!(read_output(bytes.as_slice()).unwrap().len(), 128 * 1024);
    }

    #[tokio::test]
    async fn download_handles_http_errors_and_bounded_bodies() {
        use std::{io::Write, net::TcpListener};
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for (response, succeeds) in [
            ("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok", true),
            (
                "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n",
                false,
            ),
            ("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n", false),
            (
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nlarge\r\n0\r\n\r\n",
                false,
            ),
            ("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nx", false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/bundle", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let _ = stream.read(&mut [0; 8192]);
                let _ = stream.write_all(response.as_bytes());
            });
            assert_eq!(download(&client, &url, 4).await.is_ok(), succeeds);
            server.join().unwrap();
        }
    }

    #[test]
    #[ignore = "仅由 CodeGraph 进程管理测试启动"]
    fn mock_runtime_child() {
        match std::env::var("CODEY_CODEGRAPH_TEST_MODE").as_deref() {
            Ok("timeout") => std::thread::sleep(Duration::from_secs(30)),
            Ok("failure") => {
                eprintln!("private-source-and-token");
                std::process::exit(7);
            }
            _ => println!("mock-runtime-ok"),
        }
    }

    #[test]
    fn process_failures_are_redacted_and_timeouts_release_the_process() {
        let command = |mode: &str| {
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "codegraph_runtime::tests::mock_runtime_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("CODEY_CODEGRAPH_TEST_MODE", mode);
            child
        };
        let output = execute(command("success"), Duration::from_secs(10)).unwrap();
        assert!(String::from_utf8_lossy(&output).contains("mock-runtime-ok"));
        let error = execute(command("failure"), Duration::from_secs(10))
            .unwrap_err()
            .to_string();
        assert!(error.contains('7'));
        assert!(!error.contains("private-source-and-token"));
        let start = Instant::now();
        let error = execute(command("timeout"), Duration::from_millis(150)).unwrap_err();
        assert!(error.to_string().contains("超时"));
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    #[ignore = "下载官方运行包并在临时目录执行端到端验证"]
    async fn official_bundle_smoke() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let project = temp.path().join("中文 project with spaces");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("main.ts"), "export function greet(name: string) { return `Hello ${name}`; }\nexport function main() { return greet('world'); }\n").unwrap();
        let client = reqwest::Client::builder()
            .user_agent("CodeyX-integration-test")
            .build()
            .unwrap();
        install(root.clone(), client).await.unwrap();
        prepare_launcher(&root).unwrap();
        assert!(installed(&root));
        assert_eq!(
            run(root.clone(), project.clone(), "status").await.unwrap()["initialized"],
            false
        );
        run(root.clone(), project.clone(), "init").await.unwrap();
        let result = run(root.clone(), project.clone(), "status").await.unwrap();
        assert_eq!(result["initialized"], true);
        assert!(result["nodeCount"].as_u64().unwrap() >= 2);
        run(root.clone(), project.clone(), "sync").await.unwrap();
        let home = temp.path().join("codex");
        let data = temp.path().join("extensions");
        let service = codey_codex_extensions::ExtensionService::new(
            home.clone(),
            data.clone(),
            temp.path().to_path_buf(),
        );
        let inventory = service
            .inventory(&codey_codex_extensions::Scope::User)
            .unwrap();
        let mut config = mcp_config(&root);
        config["env"]["CODEX_HOME"] = json!(home);
        let saved = service.dispatch(json!({"action":"save_mcp", "id":"codey_codegraph", "configJson":config, "createOnly":true, "confirmed":true, "revision":inventory["revision"]})).unwrap();
        let probe = crate::codex_extensions::dispatch(home, data, temp.path().to_path_buf(), json!({"action":"test_mcp", "id":"codey_codegraph", "confirmed":true, "revision":saved["inventory"]["revision"]})).await.unwrap();
        assert_eq!(probe["ok"], true, "{probe}");
        let child = project.join("child");
        std::fs::create_dir(&child).unwrap();
        assert_eq!(
            run(root.clone(), child, "status").await.unwrap()["initialized"],
            false
        );
    }

    #[tokio::test]
    #[ignore = "下载官方运行包，验证自动建库和实时文件同步"]
    async fn official_automatic_mcp_smoke() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let client = reqwest::Client::builder()
            .user_agent("CodeyX-integration-test")
            .build()
            .unwrap();
        install(root.clone(), client).await.unwrap();
        prepare_launcher(&root).unwrap();
        let mut command = Command::new(executable(&root));
        command
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../tests/builtin-tools-official-smoke.cjs"),
            )
            .arg(&root);
        let output = execute(command, Duration::from_secs(240)).unwrap();
        assert!(String::from_utf8_lossy(&output).contains("PASS:"));
    }
}
