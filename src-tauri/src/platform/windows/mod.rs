//! Win32 / Windows API surface. Compiled only on Windows.

mod elevation;
mod mutate;
mod probe;

use super::{
    BootNextResult, CidataResult, HostInfo, MachineProbe, PrepareResult, RollbackResult,
    StageResult, StateJournal,
};
use crate::cidata::CidataIdentity;
use crate::error::{Error, Result};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
    },
};

pub fn host_info() -> Result<HostInfo> {
    Ok(HostInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        elevated: elevation::is_elevated()?,
        native_windows: true,
        os_version: os_version(),
    })
}

pub fn probe_machine() -> Result<MachineProbe> {
    probe::probe_machine()
}

pub fn relaunch_elevated() -> Result<()> {
    elevation::relaunch_elevated()
}

pub fn reboot_to_firmware() -> Result<()> {
    let status = std::process::Command::new("shutdown")
        .args(["/r", "/fw", "/t", "0"])
        .status()?;
    if !status.success() {
        return Err(Error::Message(
            "failed to reboot into firmware settings (need Administrator)".into(),
        ));
    }
    Ok(())
}

pub fn load_install_state() -> Result<Option<StateJournal>> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return Ok(None);
    };
    let path = std::path::PathBuf::from(local)
        .join("OmarchyInstall")
        .join("state.json");
    match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(body) => Ok(Some(crate::journal::parse_journal(&body)?)),
    }
}

pub fn prepare_installer_partition() -> Result<PrepareResult> {
    mutate::prepare_installer_partition()
}

pub fn stage_bootloader() -> Result<StageResult> {
    mutate::stage_bootloader()
}

pub fn write_cidata(identity: CidataIdentity) -> Result<CidataResult> {
    mutate::write_cidata(identity)
}

pub fn set_boot_next() -> Result<BootNextResult> {
    mutate::set_boot_next()
}

pub fn reboot_to_installer() -> Result<()> {
    mutate::reboot_to_installer()
}

pub fn abort_and_rollback() -> Result<RollbackResult> {
    mutate::abort_and_rollback()
}

pub fn export_support_bundle() -> Result<std::path::PathBuf> {
    mutate::export_support_bundle()
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
