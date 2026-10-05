use crate::cancel::Cancellation;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub timeout: Duration,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Stream {
    Stdout,
    Stderr,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessLine {
    pub stream: Stream,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
}
impl ProcessResult {
    pub fn check(&self) -> Result<()> {
        ensure!(!self.cancelled, "操作已取消");
        ensure!(!self.timed_out, "子程序逾時");
        ensure!(
            self.exit_code == Some(0),
            "子程序失敗，exit={:?}\n{}\n{}",
            self.exit_code,
            self.stderr,
            self.stdout
        );
        Ok(())
    }
}
pub trait ProcessRunner {
    fn run(
        &self,
        spec: &ProcessSpec,
        cancel: &Cancellation,
        log: &mut dyn FnMut(ProcessLine),
    ) -> Result<ProcessResult>;
}
#[derive(Default)]
pub struct SystemRunner;
impl ProcessRunner for SystemRunner {
    fn run(
        &self,
        spec: &ProcessSpec,
        cancel: &Cancellation,
        log: &mut dyn FnMut(ProcessLine),
    ) -> Result<ProcessResult> {
        cancel.check()?;
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut managed = spawn_managed(&mut command)
            .with_context(|| format!("無法啟動 {}", spec.program.display()))?;
        let stdout = managed.child.stdout.take().unwrap();
        let stderr = managed.child.stderr.take().unwrap();
        let (tx, rx) = crossbeam_channel::bounded(256);
        let readers = [
            read_lines(stdout, Stream::Stdout, tx.clone()),
            read_lines(stderr, Stream::Stderr, tx.clone()),
        ];
        drop(tx);
        let mut result = ProcessResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            timed_out: false,
            cancelled: false,
        };
        let started = Instant::now();
        loop {
            for line in rx.try_iter().take(128) {
                collect_line(&mut result, &line);
                log(line);
            }
            if cancel.is_cancelled() || started.elapsed() >= spec.timeout {
                result.cancelled = cancel.is_cancelled();
                result.timed_out = !result.cancelled;
                managed.terminate_tree()?;
                result.exit_code = managed.child.wait()?.code();
                break;
            }
            if let Some(status) = managed.child.try_wait()? {
                result.exit_code = status.code();
                managed.terminate_tree()?;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Descendants have been killed, so both pipe readers reach EOF.
        for line in rx.iter() {
            collect_line(&mut result, &line);
            log(line);
        }
        for reader in readers {
            let _ = reader.join();
        }
        Ok(result)
    }
}
fn collect_line(result: &mut ProcessResult, line: &ProcessLine) {
    let output = match line.stream {
        Stream::Stdout => &mut result.stdout,
        Stream::Stderr => &mut result.stderr,
    };
    // Bound retained output; every line is still streamed to the caller.
    if output.len() + line.text.len() <= 10 * 1024 * 1024 {
        output.push_str(&line.text);
        output.push('\n');
    }
}
pub(crate) fn read_lines<R: std::io::Read + Send + 'static>(
    reader: R,
    stream: Stream,
    tx: crossbeam_channel::Sender<ProcessLine>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if tx
                        .send(ProcessLine {
                            stream,
                            text: String::from_utf8_lossy(&line)
                                .trim_end_matches(['\r', '\n'])
                                .into(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(ProcessLine {
                        stream: Stream::Stderr,
                        text: format!("讀取子程序輸出失敗：{e}"),
                    });
                    break;
                }
            }
        }
    })
}

pub struct ManagedChild {
    pub child: Child,
    #[cfg(windows)]
    job: Job,
}
impl ManagedChild {
    pub fn terminate_tree(&mut self) -> Result<()> {
        #[cfg(windows)]
        {
            self.job.terminate()?;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        Ok(())
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        let _ = self.terminate_tree();
        let _ = self.child.wait();
    }
}
pub fn spawn_managed(command: &mut Command) -> Result<ManagedChild> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        let job = Job::new()?;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let mut child = command.spawn()?;
        if let Err(error) = job.assign_and_resume(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(ManagedChild { child, job })
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        Ok(ManagedChild {
            child: command.spawn()?,
        })
    }
}
#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Job {
    fn new() -> Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            ensure!(
                !handle.is_null(),
                "CreateJobObjectW：{}",
                std::io::Error::last_os_error()
            );
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            ensure!(
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&info) as u32
                ) != 0,
                "SetInformationJobObject：{}",
                std::io::Error::last_os_error()
            );
            Ok(job)
        }
    }
    fn assign_and_resume(&self, child: &Child) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::{
                Diagnostics::ToolHelp::*,
                JobObjects::AssignProcessToJobObject,
                Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
            },
        };
        unsafe {
            ensure!(
                AssignProcessToJobObject(self.0, child.as_raw_handle()) != 0,
                "AssignProcessToJobObject：{}",
                std::io::Error::last_os_error()
            );
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            ensure!(snapshot != INVALID_HANDLE_VALUE, "無法列舉子程序執行緒");
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            let mut found = false;
            let mut more = Thread32First(snapshot, &mut entry);
            while more != 0 {
                if entry.th32OwnerProcessID == child.id() {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        found = ResumeThread(thread) != u32::MAX;
                        CloseHandle(thread);
                    }
                    if found {
                        break;
                    }
                }
                more = Thread32Next(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            ensure!(found, "無法恢復子程序執行緒");
            Ok(())
        }
    }
    fn terminate(&self) -> Result<()> {
        unsafe {
            ensure!(
                windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1) != 0,
                "終止子程序樹失敗：{}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
}
#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn captures_both_streams_and_exit() {
        let spec = ProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo 中文; echo failure >&2; exit 7".into()],
            timeout: Duration::from_secs(2),
        };
        let result = SystemRunner
            .run(&spec, &Cancellation::default(), &mut |_| {})
            .unwrap();
        assert!(result.stdout.contains("中文"));
        assert!(result.stderr.contains("failure"));
        assert_eq!(result.exit_code, Some(7));
        assert!(result.check().is_err());
    }
    #[test]
    fn timeout_kills_grandchild_and_closes_pipes() {
        let spec = ProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "sleep 30 & wait".into()],
            timeout: Duration::from_millis(100),
        };
        let start = Instant::now();
        let result = SystemRunner
            .run(&spec, &Cancellation::default(), &mut |_| {})
            .unwrap();
        assert!(result.timed_out);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn cancellation_kills_tree() {
        let cancel = Cancellation::default();
        let signal = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            signal.cancel();
        });
        let spec = ProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "sleep 30 & wait".into()],
            timeout: Duration::from_secs(30),
        };
        assert!(
            SystemRunner
                .run(&spec, &cancel, &mut |_| {})
                .unwrap()
                .cancelled
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    #[test]
    fn windows_job_captures_exit_and_terminates_descendants() {
        let spec = ProcessSpec {
            program: "cmd.exe".into(),
            args: vec![
                "/D".into(),
                "/C".into(),
                "echo output & echo error 1>&2 & exit /B 7".into(),
            ],
            timeout: Duration::from_secs(5),
        };
        let result = SystemRunner
            .run(&spec, &Cancellation::default(), &mut |_| {})
            .unwrap();
        assert_eq!(result.exit_code, Some(7));
        assert!(result.stdout.contains("output"));
        assert!(result.stderr.contains("error"));
        let spec = ProcessSpec {
            program: "cmd.exe".into(),
            args: vec![
                "/D".into(),
                "/C".into(),
                "ping.exe 127.0.0.1 -n 30 >nul".into(),
            ],
            timeout: Duration::from_millis(200),
        };
        let start = Instant::now();
        let result = SystemRunner
            .run(&spec, &Cancellation::default(), &mut |_| {})
            .unwrap();
        assert!(result.timed_out);
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn windows_job_cancel_returns_promptly() {
        let token = Cancellation::default();
        let signal = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            signal.cancel();
        });
        let spec = ProcessSpec {
            program: "cmd.exe".into(),
            args: vec![
                "/D".into(),
                "/C".into(),
                "ping.exe 127.0.0.1 -n 30 >nul".into(),
            ],
            timeout: Duration::from_secs(30),
        };
        let result = SystemRunner.run(&spec, &token, &mut |_| {}).unwrap();
        assert!(result.cancelled);
    }
}
