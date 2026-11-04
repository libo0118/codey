//! No-follow handle walks and immutable snapshots, adapted from
//! codex-mobile-bridge bridge/features/workspace/workspace.py (MIT).
//! See third_party/codex-mobile-bridge.LICENSE.
use std::{
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path},
    time::SystemTime,
};

use base64::{engine::general_purpose::STANDARD, read::DecoderReader};
use sha2::{Digest, Sha256};

pub(super) const MAX_FILE: u64 = 200 * 1024 * 1024;
pub(super) const CHUNK: usize = 64 * 1024;

pub(super) struct Opened {
    pub file: File,
    // Windows directory handles deny delete/rename until the file is opened and checked.
    _parents: Vec<File>,
}

impl Opened {
    pub fn canonical(&self, path: &Path) -> Result<std::path::PathBuf, String> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let _ = path;
            std::fs::read_link(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
                .map_err(|_| "无法核验文件真实位置".to_string())
        }
        #[cfg(not(target_os = "linux"))]
        {
            // Windows holds no-delete handles for this file and every parent.
            path.canonicalize()
                .map_err(|_| "无法核验文件真实位置".to_string())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    size: u64,
    modified: SystemTime,
    identity: (u64, u64),
    change: (i64, i64),
}

impl Stamp {
    pub fn size(&self) -> u64 {
        self.size
    }
}

fn ordinary(metadata: &Metadata, directory: bool) -> bool {
    let mut valid = if directory {
        metadata.is_dir()
    } else {
        metadata.is_file()
    };
    valid &= !metadata.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        valid &= metadata.file_attributes() & 0x400 == 0;
    }
    valid
}

pub(super) fn stamp(file: &File) -> Result<Stamp, String> {
    let metadata = file.metadata().map_err(|_| "文件元数据不可读取")?;
    if !ordinary(&metadata, false) {
        return Err("只支持普通文件，不支持链接或目录".into());
    }
    #[cfg(unix)]
    let (identity, change) = {
        use std::os::unix::fs::MetadataExt;
        (
            (metadata.dev(), metadata.ino()),
            (metadata.ctime(), metadata.ctime_nsec()),
        )
    };
    #[cfg(windows)]
    let (identity, change) = {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{
                BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FileBasicInfo,
                GetFileInformationByHandle, GetFileInformationByHandleEx,
            },
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: file owns this handle and info is a valid output buffer.
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
            .map_err(|_| "文件身份不可读取")?;
        let mut basic = FILE_BASIC_INFO::default();
        // SAFETY: basic has the layout and size required by FileBasicInfo.
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        }
        .map_err(|_| "文件变更时间不可读取")?;
        (
            (
                u64::from(info.dwVolumeSerialNumber),
                (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            ),
            (basic.CreationTime, basic.ChangeTime),
        )
    };
    Ok(Stamp {
        size: metadata.len(),
        modified: metadata.modified().map_err(|_| "文件修改时间不可读取")?,
        identity,
        change,
    })
}

pub(super) fn open(path: &Path) -> Result<Opened, String> {
    open_kind(path, false)
}

pub(super) fn directory(path: &Path) -> Result<Opened, String> {
    open_kind(path, true)
}

fn open_kind(path: &Path, directory_only: bool) -> Result<Opened, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("文件路径没有本机映射或包含路径穿越".into());
    }
    #[cfg(unix)]
    {
        use std::{
            ffi::CString,
            os::{
                fd::{AsRawFd, FromRawFd},
                unix::{ffi::OsStrExt, fs::OpenOptionsExt},
            },
        };
        let names: Vec<_> = path
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        if names.is_empty() {
            return Err("只支持普通文件".into());
        }
        let mut directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open("/")
            .map_err(|_| "本地目录不可访问")?;
        for (index, name) in names.iter().enumerate() {
            let name = CString::new(name.as_bytes()).map_err(|_| "文件路径无效")?;
            let last = index + 1 == names.len();
            let flags = libc::O_RDONLY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK
                | if last && !directory_only {
                    0
                } else {
                    libc::O_DIRECTORY
                };
            // SAFETY: the live directory fd and NUL-terminated name outlive openat.
            let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                return Err("文件不可访问，不支持链接或不存在的文件".into());
            }
            // SAFETY: successful openat returned a new owned descriptor.
            let file = unsafe { File::from_raw_fd(fd) };
            if last {
                if !directory_only {
                    stamp(&file)?;
                }
                return Ok(Opened {
                    file,
                    _parents: vec![],
                });
            }
            directory = file;
        }
        unreachable!("absolute path has a final component")
    }
    #[cfg(windows)]
    {
        use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
        let root = path.ancestors().last().ok_or("文件没有本地盘符")?;
        let root: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: root is a NUL-terminated local drive root; no file is opened.
        if unsafe {
            windows::Win32::Storage::FileSystem::GetDriveTypeW(windows::core::PCWSTR(root.as_ptr()))
        } == 4
        {
            return Err("网络映射盘没有受支持的本机下载映射".into());
        }
        let mut current = std::path::PathBuf::new();
        let components: Vec<_> = path.components().collect();
        let mut parents = Vec::new();
        for (index, component) in components.iter().enumerate() {
            current.push(component);
            if !matches!(component, Component::Normal(_)) {
                continue;
            }
            let last = index + 1 == components.len();
            let mut options = std::fs::OpenOptions::new();
            if last && !directory_only {
                options.read(true).share_mode(1).custom_flags(0x00200000);
            } else {
                options
                    .access_mode(0x80)
                    .share_mode(1 | 2)
                    .custom_flags(0x00200000 | 0x02000000);
            }
            let file = options
                .open(&current)
                .map_err(|_| "文件不可访问、正被修改或路径含链接")?;
            if !ordinary(
                &file.metadata().map_err(|_| "文件元数据不可读取")?,
                !last || directory_only,
            ) {
                return Err("不支持链接、reparse point 或特殊文件".into());
            }
            if last {
                return Ok(Opened {
                    file,
                    _parents: parents,
                });
            }
            parents.push(file);
        }
        Err("只支持普通文件".into())
    }
}

pub(super) fn snapshot(
    source: &mut File,
    expected: &Stamp,
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<(File, String), String> {
    let before = stamp(source)?;
    if &before != expected {
        return Err("源文件已变化，请重新解析下载".into());
    }
    if before.size > MAX_FILE {
        return Err("文件超过 200 MiB 下载上限".into());
    }
    let mut destination = private_snapshot()?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; CHUNK];
    loop {
        check()?;
        let count = source.read(&mut buffer).map_err(|_| "读取文件失败")?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_FILE {
            return Err("文件超过 200 MiB 下载上限".into());
        }
        if total > before.size {
            return Err("源文件正在变化，请稍后重新解析下载".into());
        }
        digest.update(&buffer[..count]);
        destination
            .write_all(&buffer[..count])
            .map_err(|_| "写入私有下载快照失败")?;
    }
    check()?;
    if before != stamp(source)? || before.size != total {
        return Err("源文件正在变化，请稍后重新解析下载".into());
    }
    destination
        .seek(SeekFrom::Start(0))
        .map_err(|_| "读取私有下载快照失败")?;
    Ok((destination, format!("{:x}", digest.finalize())))
}

fn private_snapshot() -> Result<File, String> {
    // tempfile is unlinked on Unix; Windows uses share_mode(0) and DELETE_ON_CLOSE.
    let file = tempfile::tempfile().map_err(|_| "无法创建私有下载快照")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "无法限制下载快照权限")?;
    }
    Ok(file)
}

pub(super) fn snapshot_inline_image(
    encoded: &str,
    expected: u64,
    mime: &str,
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<(File, String), String> {
    if encoded.len() > super::MAX_INLINE_IMAGE_ENCODED {
        return Err("内嵌图片超过下载上限".into());
    }
    check()?;
    let mut source = DecoderReader::new(encoded.as_bytes(), &STANDARD);
    let mut destination = private_snapshot()?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut header = [0u8; 12];
    let mut header_count = 0;
    let mut buffer = [0u8; CHUNK];
    loop {
        check()?;
        let count = source
            .read(&mut buffer)
            .map_err(|_| "内嵌图片 Base64 编码无效")?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > expected {
            return Err("内嵌图片大小与解析结果不匹配".into());
        }
        let copy = count.min(header.len() - header_count);
        header[header_count..header_count + copy].copy_from_slice(&buffer[..copy]);
        header_count += copy;
        digest.update(&buffer[..count]);
        destination
            .write_all(&buffer[..count])
            .map_err(|_| "写入私有下载快照失败")?;
    }
    check()?;
    if total != expected || image_mime(&header[..header_count]) != Some(mime) {
        return Err("内嵌图片大小或类型与解析结果不匹配".into());
    }
    destination
        .seek(SeekFrom::Start(0))
        .map_err(|_| "读取私有下载快照失败")?;
    Ok((destination, format!("{:x}", digest.finalize())))
}

pub(super) fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

pub(super) fn read_at(file: &File, offset: u64, buffer: &mut [u8]) -> std::io::Result<usize> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_at(file, buffer, offset)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(file, buffer, offset)
    }
}
