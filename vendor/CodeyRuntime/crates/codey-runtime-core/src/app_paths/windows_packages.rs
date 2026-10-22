use std::cell::OnceCell;
use std::path::PathBuf;

use super::{RegisteredCodexPackage, validate_registered_codex_packages};

pub(super) trait PackageInfo {
    fn codex_identity(&self) -> anyhow::Result<Option<String>>;
    fn install_location(&self) -> anyhow::Result<Option<PathBuf>>;
}

struct PackageRecord<P> {
    package: P,
    location: OnceCell<Result<Option<PathBuf>, String>>,
}

impl<P: PackageInfo> PackageRecord<P> {
    fn location(&self) -> anyhow::Result<Option<&PathBuf>> {
        cached(&self.location, || {
            let Some(location) = self.package.install_location()? else {
                return Ok(None);
            };
            canonical_install_location(&location)
        })
        .map(Option::as_ref)
    }
}

fn canonical_install_location(location: &std::path::Path) -> anyhow::Result<Option<PathBuf>> {
    anyhow::ensure!(location.is_absolute(), "Windows 包注册路径不是绝对路径");
    // InstalledPath can return a removed package's old path successfully. Keep
    // InstalledLocation's missing-folder behavior without creating StorageFolder
    // objects for every package, and retain all other filesystem failures.
    let Some(metadata) = existing_package_path_result(location, std::fs::metadata(location))?
    else {
        return Ok(None);
    };
    anyhow::ensure!(
        metadata.is_dir(),
        "Windows 包注册路径不是目录：{}",
        location.display()
    );
    existing_package_path_result(location, std::fs::canonicalize(location))
}

fn existing_package_path_result<T>(
    location: &std::path::Path,
    result: std::io::Result<T>,
) -> anyhow::Result<Option<T>> {
    use anyhow::Context;

    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("无法核实 Windows 包注册路径：{}", location.display())),
    }
}

// A snapshot belongs to one synchronous discovery/validation operation. Native
// package objects never escape it, and failed reads remain failures for its life.
pub(super) struct WindowsPackageSnapshot<P, F> {
    enumerate: F,
    records: OnceCell<Result<Vec<PackageRecord<P>>, String>>,
    codex: OnceCell<Result<Vec<RegisteredCodexPackage>, String>>,
    locations: OnceCell<Result<Vec<PathBuf>, String>>,
}

fn cached<T>(
    cell: &OnceCell<Result<T, String>>,
    read: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<&T> {
    cell.get_or_init(|| read().map_err(|error| format!("{error:#}")))
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error}"))
}

impl<P: PackageInfo, F: Fn() -> anyhow::Result<Vec<P>>> WindowsPackageSnapshot<P, F> {
    pub(super) fn new(enumerate: F) -> Self {
        Self {
            enumerate,
            records: OnceCell::new(),
            codex: OnceCell::new(),
            locations: OnceCell::new(),
        }
    }

    fn records(&self) -> anyhow::Result<&[PackageRecord<P>]> {
        cached(&self.records, || {
            (self.enumerate)().map(|packages| {
                packages
                    .into_iter()
                    .map(|package| PackageRecord {
                        package,
                        location: OnceCell::new(),
                    })
                    .collect()
            })
        })
        .map(Vec::as_slice)
    }

    pub(super) fn codex_packages(&self) -> anyhow::Result<&[RegisteredCodexPackage]> {
        cached(&self.codex, || {
            let mut packages = Vec::new();
            for record in self.records()? {
                let Some(package_full_name) = record.package.codex_identity()? else {
                    continue;
                };
                if let Some(install_location) = record.location()? {
                    packages.push(RegisteredCodexPackage {
                        package_full_name,
                        install_location: install_location.clone(),
                    });
                }
            }
            validate_registered_codex_packages(packages)
                .ok_or_else(|| anyhow::anyhow!("Windows Codex 包身份、版本或注册路径无效"))
        })
        .map(Vec::as_slice)
    }

    /// Load and canonicalize every identity only when standalone safety needs it.
    pub(super) fn all_install_locations(&self) -> anyhow::Result<&[PathBuf]> {
        cached(&self.locations, || {
            let mut locations = Vec::new();
            for record in self.records()? {
                if let Some(location) = record.location()? {
                    locations.push(location.clone());
                }
            }
            locations.sort();
            locations.dedup();
            Ok(locations)
        })
        .map(Vec::as_slice)
    }
}

#[cfg(windows)]
pub(super) type NativeWindowsPackageSnapshot = WindowsPackageSnapshot<
    windows::ApplicationModel::Package,
    fn() -> anyhow::Result<Vec<windows::ApplicationModel::Package>>,
>;

#[cfg(windows)]
impl Default for NativeWindowsPackageSnapshot {
    fn default() -> Self {
        Self::new(enumerate_registered_windows_packages)
    }
}

#[cfg(windows)]
impl PackageInfo for windows::ApplicationModel::Package {
    fn codex_identity(&self) -> anyhow::Result<Option<String>> {
        let identity = self.Id()?;
        let name = identity.Name()?.to_string_lossy();
        if !super::APP_PACKAGE_SPECS
            .iter()
            .any(|spec| spec.identity.eq_ignore_ascii_case(&name))
        {
            return Ok(None);
        }
        Ok(Some(identity.FullName()?.to_string_lossy()))
    }

    fn install_location(&self) -> anyhow::Result<Option<PathBuf>> {
        super::windows_package_location(installed_path_with_fallback(self.InstalledPath(), || {
            self.InstalledLocation().and_then(|folder| folder.Path())
        }))
    }
}

#[cfg(windows)]
fn installed_path_with_fallback(
    fast: windows::core::Result<windows::core::HSTRING>,
    fallback: impl FnOnce() -> windows::core::Result<windows::core::HSTRING>,
) -> windows::core::Result<windows::core::HSTRING> {
    match fast {
        // InstalledPath uses IPackage8. Older Windows versions do not expose
        // that interface; access and registration failures must not be hidden.
        Err(error) if error.code() == windows::Win32::Foundation::E_NOINTERFACE => fallback(),
        result => result,
    }
}

#[cfg(windows)]
fn enumerate_registered_windows_packages() -> anyhow::Result<Vec<windows::ApplicationModel::Package>>
{
    use anyhow::Context;
    use windows::Management::Deployment::PackageManager;
    use windows::core::HSTRING;

    let manager = PackageManager::new().context("无法打开 Windows 包管理器")?;
    let packages = manager
        .FindPackagesByUserSecurityId(&HSTRING::new())
        .context("无法查询当前用户的 Windows 包注册信息")?;
    let iterator = packages.First().context("无法枚举 Windows 包注册信息")?;
    let mut has_current = iterator
        .HasCurrent()
        .context("无法读取 Windows 包枚举状态")?;
    let mut values = Vec::new();
    // IntoIterator discards iteration errors; partial results cannot authorize
    // standalone launch.
    while has_current {
        values.push(iterator.Current().context("无法读取 Windows 包注册信息")?);
        has_current = iterator
            .MoveNext()
            .context("无法继续枚举 Windows 包注册信息")?;
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Clone)]
    struct FakePackage {
        identity: Option<String>,
        location: Option<PathBuf>,
        reads: Rc<Cell<usize>>,
        fail: bool,
    }

    impl PackageInfo for FakePackage {
        fn codex_identity(&self) -> anyhow::Result<Option<String>> {
            Ok(self.identity.clone())
        }
        fn install_location(&self) -> anyhow::Result<Option<PathBuf>> {
            self.reads.set(self.reads.get() + 1);
            anyhow::ensure!(!self.fail, "access denied");
            Ok(self.location.clone())
        }
    }

    #[test]
    fn snapshot_enumerates_once_and_defers_unknown_paths_until_needed() {
        let root = tempfile::tempdir().unwrap();
        let known_reads = Rc::new(Cell::new(0));
        let unknown_reads = Rc::new(Cell::new(0));
        let packages = vec![
            FakePackage {
                identity: Some("OpenAI.Codex_26.924.22138.0_x64__2p2nqsd0c76g0".into()),
                location: Some(root.path().into()),
                reads: known_reads.clone(),
                fail: false,
            },
            FakePackage {
                identity: None,
                location: Some(root.path().into()),
                reads: unknown_reads.clone(),
                fail: false,
            },
        ];
        let enumerations = Cell::new(0);
        let snapshot = WindowsPackageSnapshot::new(|| {
            enumerations.set(enumerations.get() + 1);
            Ok(packages.clone())
        });
        for _ in 0..3 {
            assert_eq!(snapshot.codex_packages().unwrap().len(), 1);
        }
        assert_eq!(known_reads.get(), 1);
        assert_eq!(unknown_reads.get(), 0);
        for _ in 0..3 {
            assert_eq!(snapshot.all_install_locations().unwrap().len(), 1);
        }
        assert_eq!(known_reads.get(), 1);
        assert_eq!(unknown_reads.get(), 1);
        assert_eq!(enumerations.get(), 1);
    }

    #[test]
    fn snapshot_keeps_unknown_package_errors_and_invalid_paths_fatal() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, []).unwrap();
        for (location, fail) in [
            (root.path().to_path_buf(), true),
            ("relative".into(), false),
            (file, false),
        ] {
            let reads = Rc::new(Cell::new(0));
            let record = FakePackage {
                identity: None,
                location: Some(location),
                reads: reads.clone(),
                fail,
            };
            let snapshot = WindowsPackageSnapshot::new(|| Ok(vec![record.clone()]));
            assert!(snapshot.codex_packages().unwrap().is_empty());
            assert!(snapshot.all_install_locations().is_err());
            assert!(snapshot.all_install_locations().is_err());
            assert_eq!(reads.get(), 1);
        }
    }

    #[test]
    fn unknown_registered_packages_still_block_standalone_classification() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        std::fs::create_dir(&app).unwrap();
        let package = FakePackage {
            identity: None,
            location: Some(root.path().into()),
            reads: Rc::new(Cell::new(0)),
            fail: false,
        };
        let snapshot = WindowsPackageSnapshot::new(|| Ok(vec![package.clone()]));
        assert!(snapshot.codex_packages().unwrap().is_empty());
        assert!(super::super::app_dir_is_within_canonical_package(
            &std::fs::canonicalize(app).unwrap(),
            snapshot.all_install_locations().unwrap(),
        ));
    }

    #[test]
    fn removed_package_records_do_not_require_a_live_identity_or_location() {
        let package = FakePackage {
            identity: Some("OpenAI.Codex_removed".into()),
            location: None,
            reads: Rc::new(Cell::new(0)),
            fail: false,
        };
        let snapshot = WindowsPackageSnapshot::new(|| Ok(vec![package.clone()]));
        assert!(snapshot.codex_packages().unwrap().is_empty());
        assert!(snapshot.all_install_locations().unwrap().is_empty());
        assert_eq!(package.reads.get(), 1);
    }

    #[test]
    fn successful_stale_paths_are_skipped_for_codex_and_unknown_packages() {
        let root = tempfile::tempdir().unwrap();
        for identity in [
            Some("OpenAI.Codex_26.924.22138.0_x64__2p2nqsd0c76g0"),
            Some("OpenAI.Codex_removed"),
            None,
        ] {
            let package = FakePackage {
                identity: identity.map(str::to_owned),
                location: Some(root.path().join("removed/package")),
                reads: Rc::new(Cell::new(0)),
                fail: false,
            };
            let snapshot = WindowsPackageSnapshot::new(|| Ok(vec![package.clone()]));
            assert!(snapshot.codex_packages().unwrap().is_empty());
            assert!(snapshot.all_install_locations().unwrap().is_empty());
            assert!(snapshot.codex_packages().unwrap().is_empty());
            assert_eq!(package.reads.get(), 1);
        }
    }

    #[test]
    fn snapshot_reuses_canonical_locations_between_views() {
        let root = tempfile::tempdir().unwrap();
        for codex_first in [true, false] {
            let location = root.path().join("package");
            std::fs::create_dir(&location).unwrap();
            let canonical = std::fs::canonicalize(&location).unwrap();
            let package = FakePackage {
                identity: Some("OpenAI.Codex_26.924.22138.0_x64__2p2nqsd0c76g0".into()),
                location: Some(location.clone()),
                reads: Rc::new(Cell::new(0)),
                fail: false,
            };
            let snapshot = WindowsPackageSnapshot::new(|| Ok(vec![package.clone()]));
            if codex_first {
                assert_eq!(
                    snapshot.codex_packages().unwrap()[0].install_location,
                    canonical
                );
            } else {
                assert_eq!(
                    snapshot.all_install_locations().unwrap(),
                    std::slice::from_ref(&canonical)
                );
            }
            std::fs::remove_dir(&location).unwrap();
            assert_eq!(
                snapshot.codex_packages().unwrap()[0].install_location,
                canonical
            );
            assert_eq!(snapshot.all_install_locations().unwrap(), [canonical]);
            assert_eq!(package.reads.get(), 1);
        }
    }

    #[test]
    fn registered_codex_path_errors_remain_fatal() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, []).unwrap();
        for (location, fail) in [
            (root.path().to_path_buf(), true),
            ("relative".into(), false),
            (file, false),
        ] {
            let package = FakePackage {
                identity: Some("OpenAI.Codex_26.924.22138.0_x64__2p2nqsd0c76g0".into()),
                location: Some(location),
                reads: Rc::new(Cell::new(0)),
                fail,
            };
            let snapshot = WindowsPackageSnapshot::new(|| Ok(vec![package.clone()]));
            assert!(snapshot.codex_packages().is_err());
            assert!(snapshot.all_install_locations().is_err());
            assert_eq!(package.reads.get(), 1);
        }
    }

    #[test]
    fn filesystem_errors_skip_only_definitely_missing_paths() {
        let root = tempfile::tempdir().unwrap();
        let read_error = |kind| {
            existing_package_path_result::<PathBuf>(root.path(), Err(std::io::Error::from(kind)))
        };
        assert!(read_error(std::io::ErrorKind::NotFound).unwrap().is_none());
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::NotADirectory,
            std::io::ErrorKind::Other,
        ] {
            let error = read_error(kind).unwrap_err();
            assert_eq!(error.downcast_ref::<std::io::Error>().unwrap().kind(), kind);
            assert!(
                error
                    .to_string()
                    .contains(&root.path().display().to_string())
            );
        }
    }

    #[test]
    fn snapshot_caches_enumeration_failure_without_partial_authorization() {
        let calls = Cell::new(0);
        let snapshot = WindowsPackageSnapshot::<FakePackage, _>::new(|| {
            calls.set(calls.get() + 1);
            anyhow::bail!("incomplete enumeration")
        });
        assert!(snapshot.codex_packages().is_err());
        assert!(snapshot.all_install_locations().is_err());
        assert_eq!(calls.get(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn installed_path_falls_back_only_when_the_interface_is_unavailable() {
        use windows::Win32::Foundation::{
            E_NOINTERFACE, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
        };
        use windows::core::{Error, HRESULT, HSTRING};
        let calls = Cell::new(0);
        let fallback = || {
            calls.set(calls.get() + 1);
            Ok(HSTRING::from(r"C:\legacy"))
        };
        assert_eq!(
            installed_path_with_fallback(Ok(HSTRING::from(r"C:\fast")), fallback).unwrap(),
            HSTRING::from(r"C:\fast")
        );
        assert_eq!(calls.get(), 0);
        assert_eq!(
            installed_path_with_fallback(Err(Error::from_hresult(E_NOINTERFACE)), fallback)
                .unwrap(),
            HSTRING::from(r"C:\legacy")
        );
        assert_eq!(calls.get(), 1);
        for code in [
            ERROR_ACCESS_DENIED,
            ERROR_FILE_NOT_FOUND,
            ERROR_PATH_NOT_FOUND,
        ] {
            let hresult = HRESULT::from_win32(code.0);
            assert_eq!(
                installed_path_with_fallback(Err(Error::from_hresult(hresult)), fallback)
                    .unwrap_err()
                    .code(),
                hresult
            );
        }
        assert_eq!(calls.get(), 1);
        let denied = HRESULT::from_win32(ERROR_ACCESS_DENIED.0);
        assert_eq!(
            installed_path_with_fallback(Err(Error::from_hresult(E_NOINTERFACE)), || Err(
                Error::from_hresult(denied)
            ))
            .unwrap_err()
            .code(),
            denied
        );
    }
}
