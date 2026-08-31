//! Win32 / Windows API surface. Compiled only on Windows.

use super::HostInfo;
use crate::error::Result;
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE},
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::{
            Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
};

pub fn host_info() -> Result<HostInfo> {
    Ok(HostInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        elevated: is_elevated()?,
        native_windows: true,
        os_version: os_version(),
    })
}

fn is_elevated() -> Result<bool> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;

        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let query = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        // Best-effort close; the query result is what callers care about.
        let _ = CloseHandle(token);
        query?;
        Ok(elevation.TokenIsElevated != 0)
    }
}

fn os_version() -> Option<String> {
    let key = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let product = reg_sz(key, w!("ProductName"))?;
    let display = reg_sz(key, w!("DisplayVersion"));
    let build = reg_sz(key, w!("CurrentBuildNumber"));

    match (display, build) {
        (Some(display), Some(build)) => Some(format!("{product} {display} (build {build})")),
        (Some(display), None) => Some(format!("{product} {display}")),
        (None, Some(build)) => Some(format!("{product} (build {build})")),
        (None, None) => Some(product),
    }
}

fn reg_sz(subkey: PCWSTR, value: PCWSTR) -> Option<String> {
    unsafe {
        let mut size = 0u32;
        let probe = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey,
            value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        );
        if probe != ERROR_SUCCESS || size == 0 {
            return None;
        }

        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        let mut data_size = size;
        let read = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut data_size),
        );
        if read != ERROR_SUCCESS {
            return None;
        }

        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16(&buf[..len]).ok()
    }
}
