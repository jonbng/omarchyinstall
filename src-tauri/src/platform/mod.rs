//! OS integration. Production Win32 lives in `windows`; macOS/Linux use `stub`
//! so the UI and IPC still compile during development.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows as imp;

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
use self::stub as imp;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub os: String,
    pub arch: String,
    /// Administrator token on Windows; always `false` on other hosts.
    pub elevated: bool,
    /// `true` only when compiled against Win32 (`windows` module).
    pub native_windows: bool,
    pub os_version: Option<String>,
}

pub fn host_info() -> crate::error::Result<HostInfo> {
    imp::host_info()
}

/// Gate commands that must not run off Windows.
#[allow(dead_code)]
pub fn require_windows() -> crate::error::Result<()> {
    if cfg!(windows) {
        Ok(())
    } else {
        Err(crate::error::Error::WindowsOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_info_matches_compile_target() {
        let info = host_info().expect("host_info");
        assert_eq!(info.os, std::env::consts::OS);
        assert_eq!(info.arch, std::env::consts::ARCH);
        assert_eq!(info.native_windows, cfg!(windows));
        assert_eq!(require_windows().is_ok(), cfg!(windows));
    }
}
