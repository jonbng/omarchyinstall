use crate::error::{Error, Result};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            SystemInformation::GetSystemDirectoryW,
            Threading::CREATE_NO_WINDOW,
        },
    },
};

#[derive(Clone, Copy)]
pub enum SystemTool {
    BcdEdit,
    ManageBde,
    PowerCfg,
    PowerShell,
    Shutdown,
}

impl SystemTool {
    fn relative_path(self) -> &'static str {
        match self {
            Self::BcdEdit => "bcdedit.exe",
            Self::ManageBde => "manage-bde.exe",
            Self::PowerCfg => "powercfg.exe",
            Self::PowerShell => r"WindowsPowerShell\v1.0\powershell.exe",
            Self::Shutdown => "shutdown.exe",
        }
    }
}

pub fn system_directory() -> Result<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let len = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if len == 0 || len >= buffer.len() {
        return Err(Error::Message(
            "Windows did not return a valid system directory".into(),
        ));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
}

pub fn system_tool_path(tool: SystemTool) -> Result<PathBuf> {
    let path = system_directory()?.join(tool.relative_path());
    if !path.is_file() {
        return Err(Error::Message(format!(
            "required Windows system tool is missing: {}",
            path.display()
        )));
    }
    Ok(path)
}

pub fn system_command(tool: SystemTool) -> Result<Command> {
    let mut command = Command::new(system_tool_path(tool)?);
    command.creation_flags(CREATE_NO_WINDOW.0);
    Ok(command)
}

pub fn output_with_timeout(
    command: &mut Command,
    timeout: Duration,
    description: &str,
) -> Result<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Message(format!("could not capture {description} stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Message(format!("could not capture {description} stderr")))?;
    let stdout_reader = std::thread::spawn(move || read_pipe(stdout));
    let stderr_reader = std::thread::spawn(move || read_pipe(stderr));

    let status = match child.wait_timeout(timeout)? {
        Some(status) => status,
        None => {
            if let Err(error) = child.kill() {
                if child.try_wait()?.is_none() {
                    return Err(Error::Message(format!(
                        "{description} timed out and could not be terminated: {error}"
                    )));
                }
            }
            child.wait()?;
            let _ = join_reader(stdout_reader, description, "stdout");
            let _ = join_reader(stderr_reader, description, "stderr");
            return Err(Error::Timeout {
                description: description.into(),
                seconds: timeout.as_secs(),
            });
        }
    };
    Ok(Output {
        status,
        stdout: join_reader(stdout_reader, description, "stdout")?,
        stderr: join_reader(stderr_reader, description, "stderr")?,
    })
}

/// Runs a private helper in a kill-on-close Job Object. If it times out, the
/// whole helper process tree is terminated and reaped before this returns.
pub fn output_with_timeout_in_job(
    command: &mut Command,
    timeout: Duration,
    description: &str,
) -> Result<Output> {
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }?;
    let job = OwnedJob(job);
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    if let Err(error) = unsafe { AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle())) } {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Error::Message(format!(
            "could not place {description} in its cleanup job: {error}"
        )));
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Message(format!("could not capture {description} stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Message(format!("could not capture {description} stderr")))?;
    let stdout_reader = std::thread::spawn(move || read_pipe(stdout));
    let stderr_reader = std::thread::spawn(move || read_pipe(stderr));

    let status = match child.wait_timeout(timeout)? {
        Some(status) => status,
        None => {
            let terminate_error = unsafe { TerminateJobObject(job.0, 1) }.err();
            child.wait()?;
            let _ = join_reader(stdout_reader, description, "stdout");
            let _ = join_reader(stderr_reader, description, "stderr");
            if let Some(error) = terminate_error {
                return Err(Error::Message(format!(
                    "{description} timed out and its process job could not be terminated: {error}"
                )));
            }
            return Err(Error::Timeout {
                description: description.into(),
                seconds: timeout.as_secs(),
            });
        }
    };
    Ok(Output {
        status,
        stdout: join_reader(stdout_reader, description, "stdout")?,
        stderr: join_reader(stderr_reader, description, "stderr")?,
    })
}

struct OwnedJob(HANDLE);

impl Drop for OwnedJob {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(
    reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
    description: &str,
    stream: &str,
) -> Result<Vec<u8>> {
    let bytes = reader
        .join()
        .map_err(|_| Error::Message(format!("{description} {stream} reader panicked")))??;
    Ok(bytes)
}

/// Runs an in-box Windows PowerShell command for the Storage/CIM boundary.
/// Callers must keep scripts fixed-format and validate or quote inserted values.
pub fn run_storage_powershell(script: &str) -> Result<String> {
    let output = system_command(SystemTool::PowerShell)?
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()?;
    if !output.status.success() {
        return Err(Error::Message(format!(
            "powershell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Runs the read-only Storage/CIM inventory with a bounded wait. Mutating
/// storage commands deliberately use `run_storage_powershell` without a
/// forced timeout so they cannot be killed halfway through a disk operation.
pub fn run_storage_powershell_read_only(
    script: &str,
    timeout: Duration,
    description: &str,
) -> Result<String> {
    let output = output_with_timeout(
        system_command(SystemTool::PowerShell)?.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ]),
        timeout,
        description,
    )?;
    if !output.status.success() {
        return Err(Error::Message(format!(
            "powershell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_tools_are_absolute_and_present() {
        for tool in [
            SystemTool::BcdEdit,
            SystemTool::ManageBde,
            SystemTool::PowerCfg,
            SystemTool::PowerShell,
            SystemTool::Shutdown,
        ] {
            let path = system_tool_path(tool).unwrap();
            assert!(path.is_absolute(), "{}", path.display());
            assert!(path.is_file(), "{}", path.display());
        }
    }

    #[test]
    fn bounded_output_times_out_and_reaps_the_child() {
        let mut command = system_command(SystemTool::PowerShell).unwrap();
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 5",
        ]);
        let error = output_with_timeout(&mut command, Duration::from_millis(50), "timeout test")
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn job_bounded_output_times_out_and_reaps_the_helper() {
        let mut command = system_command(SystemTool::PowerShell).unwrap();
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 5",
        ]);
        let error =
            output_with_timeout_in_job(&mut command, Duration::from_millis(50), "job timeout test")
                .unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }
}
