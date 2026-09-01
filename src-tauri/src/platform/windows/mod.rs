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

pub fn pick_local_iso() -> Result<Option<std::path::PathBuf>> {
    // Use the system picker here instead of a WebView API so this also works in
    // the secure browser fallback used on machines without WebView2.
    let script = r#"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
Add-Type -AssemblyName System.Windows.Forms
$picker = New-Object System.Windows.Forms.OpenFileDialog
$picker.Title = 'Select an already downloaded Omarchy ISO'
$picker.Filter = 'Omarchy ISO (omarchy-*.iso)|omarchy-*.iso|ISO images (*.iso)|*.iso'
$picker.CheckFileExists = $true
$picker.Multiselect = $false
if ($picker.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
  [Console]::Out.Write($picker.FileName)
}
"#;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-STA", "-Command", script])
        .output()?;
    if !output.status.success() {
        return Err(Error::Message(format!(
            "ISO file picker failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let path = String::from_utf8(output.stdout)
        .map_err(|_| Error::Message("ISO file picker returned an invalid path".into()))?;
    let path = path.trim();
    Ok((!path.is_empty()).then(|| std::path::PathBuf::from(path)))
}

pub fn prepare_installer_partition(allow_bitlocker: bool) -> Result<PrepareResult> {
    mutate::prepare_installer_partition(allow_bitlocker)
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
