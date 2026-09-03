use crate::error::{Error, Result};
use std::path::PathBuf;
use std::process::Command;

use std::os::windows::process::CommandExt;
use windows::Win32::System::{SystemInformation::GetSystemDirectoryW, Threading::CREATE_NO_WINDOW};

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
}
