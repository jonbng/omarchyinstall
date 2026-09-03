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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MachineProbe {
    pub host: HostInfo,
    pub uefi: bool,
    pub secure_boot: bool,
    pub efi_vars_writable: bool,
    pub ram_installed_bytes: u64,
    pub ram_total_phys_bytes: u64,
    pub ram_avail_bytes: u64,
    pub ram_ok_for_copytoram: bool,
    pub tpm_present: bool,
    pub recommended_disk_id: Option<String>,
    pub target_esp: Option<TargetEsp>,
    pub linux_by_id: Option<String>,
    pub bitlocker: Vec<BitlockerVolume>,
    pub disks: Vec<DiskMap>,
    pub blocking_reasons: Vec<BlockingReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum BlockingReason {
    NotElevated,
    NotUefi,
    NotGpt {
        #[serde(rename = "diskId")]
        disk_id: String,
    },
    SecureBoot,
    BitLocker {
        mount: Option<String>,
    },
    Ram {
        #[serde(rename = "haveInstalled")]
        have_installed: u64,
        #[serde(rename = "haveTotalPhys")]
        have_total_phys: u64,
        #[serde(rename = "needInstalled")]
        need_installed: u64,
        #[serde(rename = "needTotalPhys")]
        need_total_phys: u64,
    },
    EfiVarsLocked,
    ProbeIncomplete {
        component: String,
    },
    MissingEsp {
        disk_id: String,
    },
    AmbiguousEsp {
        disk_id: String,
        count: u32,
    },
    Rst {
        #[serde(rename = "diskId")]
        disk_id: String,
    },
    Dynamic {
        #[serde(rename = "diskId")]
        disk_id: String,
    },
    StorageSpaces {
        #[serde(rename = "diskId")]
        disk_id: String,
    },
    ShrinkTooSmall {
        have: u64,
        need: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BitlockerVolume {
    pub device_id: Option<String>,
    pub disk_id: Option<String>,
    pub mount: Option<String>,
    pub protection_status: u32,
    pub conversion_status: u32,
    pub fully_decrypted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetEsp {
    pub disk_id: String,
    pub disk_guid: String,
    pub disk_number: u32,
    pub partition_guid: String,
    pub volume_guid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiskMap {
    pub device_id: String,
    pub size_bytes: u64,
    pub partition_style: String,
    pub bus: Option<String>,
    pub is_boot: bool,
    pub is_rst: bool,
    pub is_dynamic: bool,
    pub is_storage_spaces: bool,
    pub max_shrink_bytes: Option<u64>,
    pub partitions: Vec<PartitionMap>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PartitionMap {
    pub gpt_guid: Option<String>,
    pub type_guid: Option<String>,
    pub letter: Option<String>,
    pub label: Option<String>,
    pub size_bytes: u64,
    pub offset_bytes: u64,
    pub fs: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum JournalStep {
    Planned,
    PowerPrepared,
    WindowsShrunk,
    OmarchyPartitionCreated,
    CidataPartitionCreated,
    Staged,
    BootEntryCreated,
    BootNextSet,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PendingOperation {
    DisablePower,
    ShrinkWindows,
    CreateOmarchyPartition,
    CreateCidataPartition,
    StageEsp,
    CreateBootEntry,
    SetBootNext,
    Rollback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StateJournal {
    pub version: u32,
    pub operation_id: String,
    pub step: JournalStep,
    pub pending_operation: Option<PendingOperation>,
    pub target_disk_guid: Option<String>,
    pub target_disk_number: Option<u32>,
    pub windows_partition_guid: Option<String>,
    pub windows_partition_offset_bytes: Option<u64>,
    pub new_c_size_bytes: Option<u64>,
    pub omarchyinst_offset_bytes: Option<u64>,
    pub omarchyinst_size_bytes: Option<u64>,
    pub omarchyinst_guid: Option<String>,
    pub cidata_guid: Option<String>,
    pub cidata_partuuid: Option<String>,
    pub cidata_offset_bytes: Option<u64>,
    pub cidata_size_bytes: Option<u64>,
    pub esp_partition_guid: Option<String>,
    pub esp_volume_guid: Option<String>,
    pub linux_device: Option<String>,
    pub old_c_size_bytes: Option<u64>,
    pub iso_sha256: Option<String>,
    pub search_filename: Option<String>,
    pub boot_id: Option<String>,
    pub boot_description: Option<String>,
    pub hiberboot_was: Option<u32>,
    pub hibernation_disabled_by_us: bool,
    /// GPT partition GUID for `img_dev=PARTUUID=`. Not Get-Volume UniqueId.
    #[serde(default)]
    pub omarchyinst_partuuid: Option<String>,
}

pub fn host_info() -> crate::error::Result<HostInfo> {
    imp::host_info()
}

pub fn probe_machine() -> crate::error::Result<MachineProbe> {
    imp::probe_machine()
}

pub fn relaunch_elevated() -> crate::error::Result<()> {
    require_windows()?;
    imp::relaunch_elevated()
}

pub fn reboot_to_firmware() -> crate::error::Result<()> {
    require_windows()?;
    imp::reboot_to_firmware()
}

/// Linux/macOS `tauri dev`. Mutate IPC is a dry run; no disk, EFI, or reboot.
pub fn is_stub_host() -> bool {
    !cfg!(windows)
}

pub fn load_install_state() -> crate::error::Result<Option<StateJournal>> {
    imp::load_install_state()
}

pub fn pick_local_iso(owner: Option<isize>) -> crate::error::Result<Option<std::path::PathBuf>> {
    imp::pick_local_iso(owner)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrepareResult {
    pub omarchyinst_guid: String,
    pub omarchyinst_partuuid: String,
    pub cidata_guid: String,
    pub old_c_size_bytes: u64,
    pub new_c_size_bytes: u64,
    pub partition_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StageResult {
    pub esp_guid: String,
    pub search_filename: String,
    pub grub_cfg_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CidataResult {
    pub cidata_guid: String,
    pub linux_device: String,
    pub encrypt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BootNextResult {
    pub boot_id: String,
    pub bcd_firmware_id: Option<String>,
    pub appended_boot_order: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RollbackResult {
    pub removed_partition: bool,
    pub extended_ntfs: bool,
    pub restored_power_settings: bool,
}

pub fn prepare_installer_partition(allow_bitlocker: bool) -> crate::error::Result<PrepareResult> {
    imp::prepare_installer_partition(allow_bitlocker)
}

pub fn stage_bootloader() -> crate::error::Result<StageResult> {
    imp::stage_bootloader()
}

pub fn write_cidata(identity: crate::cidata::CidataIdentity) -> crate::error::Result<CidataResult> {
    imp::write_cidata(identity)
}

pub fn set_boot_next() -> crate::error::Result<BootNextResult> {
    imp::set_boot_next()
}

pub fn reboot_to_installer() -> crate::error::Result<()> {
    imp::reboot_to_installer()
}

pub fn abort_and_rollback() -> crate::error::Result<RollbackResult> {
    imp::abort_and_rollback()
}

pub fn export_support_bundle() -> crate::error::Result<std::path::PathBuf> {
    imp::export_support_bundle()
}

/// Gate commands that must not run off Windows.
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
        assert_eq!(is_stub_host(), !cfg!(windows));
    }

    #[test]
    fn blocking_reason_serde_is_internally_tagged() {
        let json = serde_json::to_string(&BlockingReason::NotGpt {
            disk_id: r"\\.\PHYSICALDRIVE0".into(),
        })
        .unwrap();
        assert!(json.contains(r#""type":"notGpt""#), "{json}");
        assert!(json.contains(r#""diskId""#), "{json}");

        let ram = serde_json::to_string(&BlockingReason::Ram {
            have_installed: 1,
            have_total_phys: 2,
            need_installed: 3,
            need_total_phys: 4,
        })
        .unwrap();
        assert!(ram.contains(r#""type":"ram""#), "{ram}");
        assert!(ram.contains("haveInstalled"), "{ram}");
    }
}
