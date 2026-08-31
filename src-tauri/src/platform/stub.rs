//! Non-Windows host used for UI and IPC development on macOS/Linux.
//! Do not put production behavior here.

use super::HostInfo;
use crate::error::Result;

pub fn host_info() -> Result<HostInfo> {
    Ok(HostInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        elevated: false,
        native_windows: false,
        os_version: None,
    })
}
