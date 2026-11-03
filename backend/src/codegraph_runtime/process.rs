// Adapted from fraternity-z/CodeyX (30e1daa), AGPL-3.0-only.
// Only processes spawned by this guard are attached to its group or job.

pub(crate) struct OwnedChild {
    child: std::process::Child,
    #[cfg(windows)]
    job: windows::Win32::Foundation::HANDLE,
}

impl OwnedChild {
    pub(crate) fn spawn(command: &mut std::process::Command) -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
            command.creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0);
        }
        let process = Self {
            child: command.spawn()?,
            #[cfg(windows)]
            job: windows::Win32::Foundation::HANDLE::default(),
        };
        #[cfg(windows)]
        let process = {
            let mut process = process;
            process.attach_job()?;
            process.resume()?;
            process
        };
        Ok(process)
    }

    #[cfg(windows)]
    fn attach_job(&mut self) -> anyhow::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{
            Foundation::HANDLE,
            System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
        };
        unsafe {
            self.job = CreateJobObjectW(None, None)?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                self.job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )?;
            AssignProcessToJobObject(self.job, HANDLE(self.child.as_raw_handle()))?;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn resume(&self) -> anyhow::Result<()> {
        use windows::Win32::{
            Foundation::CloseHandle,
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                    Thread32Next,
                },
                Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
            },
        };
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let mut next = Thread32First(snapshot, &mut entry).is_ok();
            let mut thread_id = None;
            while next {
                if entry.th32OwnerProcessID == self.child.id() {
                    thread_id = Some(entry.th32ThreadID);
                    break;
                }
                next = Thread32Next(snapshot, &mut entry).is_ok();
            }
            let _ = CloseHandle(snapshot);
            let thread_id = thread_id.ok_or_else(|| anyhow::anyhow!("无法找到暂停的子进程线程"))?;
            let thread = OpenThread(THREAD_SUSPEND_RESUME, false, thread_id)?;
            let resumed = ResumeThread(thread);
            let _ = CloseHandle(thread);
            anyhow::ensure!(resumed != u32::MAX, "无法恢复子进程");
        }
        Ok(())
    }
}

impl std::ops::Deref for OwnedChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        // 只在子进程仍运行时回收整个进程组；已退出并被回收的 pid 可能被复用为
        // 无关进程组的组号，此时发信号会误杀其他进程。
        #[cfg(unix)]
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = crate::process_tree::signal_process_group(Some(self.child.id()), libc::SIGKILL);
        }
        #[cfg(windows)]
        if !self.job.is_invalid() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(self.job);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod owned_tests {
    use super::OwnedChild;
    use std::{
        fs,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    #[test]
    #[ignore = "仅由进程树回收测试启动"]
    fn owned_process_child() {
        let Some(marker) = std::env::var_os("CODEY_MANAGED_TOOL_PROCESS_TEST_MARKER") else {
            return;
        };
        if std::env::var_os("CODEY_MANAGED_TOOL_PROCESS_TEST_DESCENDANT").is_none() {
            let mut child = child_command(&marker)
                .env("CODEY_MANAGED_TOOL_PROCESS_TEST_DESCENDANT", "1")
                .spawn()
                .unwrap();
            let _ = child.wait();
        } else {
            loop {
                fs::write(&marker, b"running").unwrap();
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn child_command(marker: &std::ffi::OsStr) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "codegraph_runtime::process::owned_tests::owned_process_child",
                "--ignored",
                "--nocapture",
            ])
            .env("CODEY_MANAGED_TOOL_PROCESS_TEST_MARKER", marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    #[test]
    fn dropping_owned_child_terminates_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("heartbeat");
        let process = OwnedChild::spawn(&mut child_command(marker.as_os_str())).unwrap();
        let start = Instant::now();
        while !marker.exists() && start.elapsed() < Duration::from_secs(10) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists());
        drop(process);
        thread::sleep(Duration::from_millis(50));
        let modified = fs::metadata(&marker).unwrap().modified().unwrap();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(fs::metadata(&marker).unwrap().modified().unwrap(), modified);
    }
}
