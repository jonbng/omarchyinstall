// Read-only machine probe. Compiled only on Windows.

use super::{
    native_storage::{self, InventoryEnvelope, NativePartition, NativeStorageReport},
    process::run_storage_powershell_read_only,
    registry::get_hklm_dword,
};
use crate::error::{Error, Result};
use crate::platform::{
    BitlockerVolume, BlockingReason, DiskMap, MachineProbe, PartitionMap, TargetEsp,
};
use crate::probe::{self, volume_is_fve};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    time::{Duration, Instant},
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, GENERIC_READ, HANDLE},
        Security::{
            AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES,
            SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        Storage::FileSystem::{
            CreateFileW, ReadFile, SetFilePointerEx, FILE_ATTRIBUTE_NORMAL,
            FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{
            SystemInformation::{
                FirmwareTypeUefi, GetFirmwareType, GetPhysicallyInstalledSystemMemory,
                GlobalMemoryStatusEx, FIRMWARE_TYPE, MEMORYSTATUSEX,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
            WindowsProgramming::{
                GetFirmwareEnvironmentVariableW, SetFirmwareEnvironmentVariableExW,
            },
        },
    },
};

const OMARCHY_VENDOR: PCWSTR = w!("{FDCA2A4E-3D8D-4EB7-AE97-80598A4D5DB4}");
const EFI_VARIABLE_ATTRIBUTES: u32 = 0x0000_0001 | 0x0000_0002 | 0x0000_0004;
const LDM_META: &str = "5808c8aa-7e8f-42e0-85d2-e1e90434cfb3";
const LDM_DATA: &str = "af9b60a0-1431-4f62-bc68-3311714a69ad";
const PRECHECK_SELF_TEST_ARG: &str = "--precheck-self-test";
const PRECHECK_INJECT_ARG: &str = "--inject-provider-fault";
const PRECHECK_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderFault {
    StorageTimeout,
    StorageMalformed,
    StorageHelperFailure,
    StorageReportFailure,
    BitlockerTimeout,
    BitlockerMalformed,
    BitlockerEmpty,
    BitlockerWorkerPanic,
}

impl ProviderFault {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "storage-timeout" => Some(Self::StorageTimeout),
            "storage-malformed" => Some(Self::StorageMalformed),
            "storage-helper-failure" => Some(Self::StorageHelperFailure),
            "storage-report-failure" => Some(Self::StorageReportFailure),
            "bitlocker-timeout" => Some(Self::BitlockerTimeout),
            "bitlocker-malformed" => Some(Self::BitlockerMalformed),
            "bitlocker-empty" => Some(Self::BitlockerEmpty),
            "bitlocker-worker-panic" => Some(Self::BitlockerWorkerPanic),
            _ => None,
        }
    }

    fn storage_error(self) -> Option<&'static str> {
        match self {
            Self::StorageTimeout => {
                Some("native storage inventory timed out after 10 seconds (injected)")
            }
            Self::StorageMalformed => {
                Some("storage inventory helper returned invalid JSON (injected)")
            }
            Self::StorageHelperFailure => Some("native storage inventory helper failed (injected)"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeStageTiming {
    pub stage: String,
    pub elapsed_ms: u128,
    pub succeeded: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeProbeDiagnostics {
    pub protocol_version: u32,
    pub elapsed_ms: u128,
    pub inventory_envelope: Option<InventoryEnvelope>,
    pub inventory: Option<NativeStorageReport>,
    pub inventory_error: Option<String>,
    // Kept as null for support-bundle schema compatibility. Running Windows'
    // shrink-capacity query here can start a defrag analysis that outlives the
    // probe and blocks later Storage operations.
    pub shrink: Option<serde_json::Value>,
    pub shrink_error: Option<String>,
    pub shrink_status: String,
    pub bitlocker: BitlockerDiagnostics,
    pub timings: Vec<ProbeStageTiming>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PrecheckSelfTestEnvelope {
    protocol_version: u32,
    elapsed_ms: u128,
    probe: Option<MachineProbe>,
    diagnostics: Option<NativeProbeDiagnostics>,
    error: Option<String>,
}

pub(super) fn run_self_test_if_requested() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(PRECHECK_SELF_TEST_ARG) {
        return false;
    }
    let started = Instant::now();
    let fault = match args.as_slice() {
        [_] => Ok(None),
        [_, inject, value] if inject == PRECHECK_INJECT_ARG => ProviderFault::parse(value)
            .map(Some)
            .ok_or_else(|| format!("unknown provider fault {value:?}")),
        _ => Err(format!(
            "usage: {PRECHECK_SELF_TEST_ARG} [{PRECHECK_INJECT_ARG} <fault>]"
        )),
    };
    let result = fault
        .map_err(Error::Message)
        .and_then(probe_machine_detailed_with_fault);
    let envelope = match result {
        Ok((probe, diagnostics)) => PrecheckSelfTestEnvelope {
            protocol_version: PRECHECK_PROTOCOL_VERSION,
            elapsed_ms: started.elapsed().as_millis(),
            probe: Some(probe),
            diagnostics: Some(diagnostics),
            error: None,
        },
        Err(error) => PrecheckSelfTestEnvelope {
            protocol_version: PRECHECK_PROTOCOL_VERSION,
            elapsed_ms: started.elapsed().as_millis(),
            probe: None,
            diagnostics: None,
            error: Some(error.to_string()),
        },
    };
    let output = serde_json::to_vec(&envelope).unwrap_or_else(|_| {
        br#"{"protocolVersion":1,"elapsedMs":0,"probe":null,"diagnostics":null,"error":"serialization failed"}"#.to_vec()
    });
    let _ = std::io::stdout().write_all(&output);
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BitlockerDiagnostics {
    pub elapsed_ms: u128,
    pub volumes: Vec<PsBitlocker>,
    pub error: Option<String>,
}

pub fn probe_machine() -> Result<MachineProbe> {
    Ok(probe_machine_detailed()?.0)
}

pub(crate) fn probe_machine_detailed() -> Result<(MachineProbe, NativeProbeDiagnostics)> {
    probe_machine_detailed_with_fault(None)
}

fn query_native_storage(
    injected_fault: Option<ProviderFault>,
) -> (
    Option<InventoryEnvelope>,
    Option<NativeStorageReport>,
    Option<String>,
) {
    if injected_fault == Some(ProviderFault::StorageReportFailure) {
        let error = "native storage inventory failed in helper (injected)".to_string();
        return (
            Some(InventoryEnvelope {
                protocol_version: native_storage::PROTOCOL_VERSION,
                elapsed_ms: 0,
                report: None,
                error: Some(error.clone()),
            }),
            None,
            Some(error),
        );
    }
    if let Some(error) = injected_fault.and_then(ProviderFault::storage_error) {
        return (None, None, Some(error.into()));
    }
    match native_storage::inventory_report() {
        Ok(envelope) => {
            let failure = envelope.error.clone();
            let inventory = envelope.report.clone();
            if let Some(inventory) = &inventory {
                log::info!(
                    "native storage inventory completed in {} ms",
                    inventory.elapsed_ms
                );
            }
            (Some(envelope), inventory, failure)
        }
        Err(error) => {
            log::warn!("native storage inventory failed: {error}");
            (None, None, Some(error.to_string()))
        }
    }
}

fn probe_machine_detailed_with_fault(
    injected_fault: Option<ProviderFault>,
) -> Result<(MachineProbe, NativeProbeDiagnostics)> {
    let probe_started = Instant::now();
    let mut timings = Vec::new();
    let stage = Instant::now();
    let host = super::host_info()?;
    timings.push(ProbeStageTiming {
        stage: "host".into(),
        elapsed_ms: stage.elapsed().as_millis(),
        succeeded: true,
    });
    // WMI is independent and occasionally slow. Let it overlap native storage
    // inventory so the two read-only checks do not serialize the precheck.
    let bitlocker_thread = std::thread::spawn(move || query_bitlocker(injected_fault));
    let stage = Instant::now();
    let uefi = is_uefi();
    let secure_boot = secure_boot_enabled();
    let efi_vars_writable = if uefi {
        efi_variables_writable()
    } else {
        false
    };
    timings.push(ProbeStageTiming {
        stage: "firmware-security".into(),
        elapsed_ms: stage.elapsed().as_millis(),
        succeeded: true,
    });
    let stage = Instant::now();
    let (ram_installed_bytes, ram_total_phys_bytes, ram_avail_bytes) = ram_bytes();
    timings.push(ProbeStageTiming {
        stage: "memory".into(),
        elapsed_ms: stage.elapsed().as_millis(),
        succeeded: true,
    });
    let stage = Instant::now();
    let (inventory_envelope, inventory, inventory_failure) = query_native_storage(injected_fault);
    timings.push(ProbeStageTiming {
        stage: "storage".into(),
        elapsed_ms: stage.elapsed().as_millis(),
        succeeded: inventory_failure.is_none(),
    });
    let stage = Instant::now();
    let tpm_present = tpm_present_tbs();
    timings.push(ProbeStageTiming {
        stage: "tpm".into(),
        elapsed_ms: stage.elapsed().as_millis(),
        succeeded: true,
    });

    let stage = Instant::now();
    let bitlocker_diagnostics = bitlocker_thread
        .join()
        .unwrap_or_else(|_| BitlockerDiagnostics {
            elapsed_ms: 0,
            volumes: Vec::new(),
            error: Some("BitLocker WMI worker panicked".into()),
        });
    timings.push(ProbeStageTiming {
        stage: "bitlocker-wait".into(),
        elapsed_ms: stage.elapsed().as_millis(),
        succeeded: bitlocker_diagnostics.error.is_none(),
    });
    let stage = Instant::now();
    let bitlocker_inventory = bitlocker_diagnostics.volumes.clone();
    let bitlocker_error = bitlocker_diagnostics.error.clone();
    let disks = inventory
        .as_ref()
        .map(disks_from_inventory)
        .unwrap_or_default();
    let (mut bitlocker, bitlocker_association_failed) = if bitlocker_error.is_none() {
        inventory
            .as_ref()
            .map(|inventory| bitlocker_for_boot_disk(inventory, bitlocker_inventory))
            .unwrap_or_default()
    } else {
        (Vec::new(), false)
    };

    overlay_fve_signatures(&mut bitlocker, &disks);

    let recommended_disk_id = disks
        .iter()
        .find(|d| d.is_boot)
        .map(|d| d.device_id.clone())
        .or_else(|| disks.first().map(|d| d.device_id.clone()));
    let (target_esp, mut inventory_reasons) = inventory
        .as_ref()
        .map(target_esp_from_inventory)
        .unwrap_or_else(|| {
            (
                None,
                vec![BlockingReason::ProbeIncomplete {
                    component: "native Windows storage inventory".into(),
                    detail: inventory_failure
                        .clone()
                        .unwrap_or_else(|| "Windows returned no storage data.".into()),
                }],
            )
        });
    if let Some(detail) = bitlocker_error {
        inventory_reasons.push(BlockingReason::ProbeIncomplete {
            component: "BitLocker WMI".into(),
            detail,
        });
    }
    if bitlocker_association_failed {
        inventory_reasons.push(BlockingReason::ProbeIncomplete {
            component: "BitLocker volume-to-disk association".into(),
            detail: "BitLocker did not return exactly one complete record for Windows C: associated with one extent on the boot disk."
                .into(),
        });
    }
    if inventory.as_ref().is_some_and(|inventory| {
        !inventory
            .disks
            .iter()
            .find(|disk| disk.number == inventory.boot_disk_number)
            .is_some_and(|disk| {
                disk.partitions.iter().any(|partition| {
                    partition
                        .mount_paths
                        .iter()
                        .any(|path| drive_letter(path).as_deref() == Some("C:"))
                })
            })
    }) {
        inventory_reasons.push(BlockingReason::ProbeIncomplete {
            component: "Windows system partition association".into(),
            detail: "The native volume inventory could not associate C: with exactly one partition on the Windows boot disk."
                .into(),
        });
    }

    let linux_by_id = inventory.as_ref().and_then(linux_by_id_from_inventory);
    if let Some(inventory) = &inventory {
        if let Some(target) = inventory
            .disks
            .iter()
            .find(|disk| disk.number == inventory.boot_disk_number)
        {
            if target.bus.eq_ignore_ascii_case("unknown") {
                inventory_reasons.push(BlockingReason::ProbeIncomplete {
                    component: "target disk bus classification".into(),
                    detail: "Windows returned an unknown bus type for the boot target.".into(),
                });
            }
            let c_partition = target.partitions.iter().find(|partition| {
                partition
                    .mount_paths
                    .iter()
                    .any(|path| drive_letter(path).as_deref() == Some("C:"))
            });
            if c_partition.is_some_and(|partition| partition.gpt_guid.is_none()) {
                inventory_reasons.push(BlockingReason::ProbeIncomplete {
                    component: "Windows C: GPT identity".into(),
                    detail: "The C: partition has no stable GPT partition GUID.".into(),
                });
            }
        }
        if linux_by_id.is_none() {
            inventory_reasons.push(BlockingReason::ProbeIncomplete {
                component: "target Linux disk identity".into(),
                detail: "The target disk did not expose enough model/serial identity to construct /dev/disk/by-id.".into(),
            });
        }
    }

    let probe = MachineProbe {
        host,
        uefi,
        secure_boot,
        efi_vars_writable,
        ram_installed_bytes,
        ram_total_phys_bytes,
        ram_avail_bytes,
        ram_ok_for_copytoram: false,
        tpm_present,
        recommended_disk_id,
        target_esp,
        linux_by_id,
        bitlocker,
        disks,
        blocking_reasons: inventory_reasons,
    };
    let mut diagnostics = NativeProbeDiagnostics {
        protocol_version: 2,
        elapsed_ms: probe_started.elapsed().as_millis(),
        inventory_envelope,
        inventory,
        inventory_error: inventory_failure,
        shrink: None,
        shrink_error: None,
        shrink_status: "deferred-to-resize".into(),
        bitlocker: bitlocker_diagnostics,
        timings,
    };
    let probe = probe::attach_reasons(probe, true);
    diagnostics.timings.push(ProbeStageTiming {
        stage: "association-policy".into(),
        elapsed_ms: stage.elapsed().as_millis(),
        succeeded: true,
    });
    Ok((probe, diagnostics))
}

fn is_uefi() -> bool {
    unsafe {
        let mut kind = FIRMWARE_TYPE::default();
        GetFirmwareType(&mut kind)
            .ok()
            .map(|_| kind == FirmwareTypeUefi)
            .unwrap_or(false)
    }
}

fn secure_boot_enabled() -> bool {
    if let Some(v) = get_hklm_dword(
        w!("SYSTEM\\CurrentControlSet\\Control\\SecureBoot\\State"),
        w!("UEFISecureBootEnabled"),
    ) {
        return v != 0;
    }
    false
}

fn efi_variables_writable() -> bool {
    if enable_system_environment_privilege().is_err() {
        return false;
    }
    // Separate app processes can probe concurrently (support tools and test
    // harnesses do this even though the GUI is single-instance). A shared
    // variable name lets one process read or delete another process's nonce,
    // producing a false EfiVarsLocked blocker.
    let variable_name: Vec<u16> = format!("OmarchyInstallWriteProbe-{}", std::process::id())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let variable_name = PCWSTR(variable_name.as_ptr());
    let nonce = std::process::id().to_le_bytes();
    let write = unsafe {
        SetFirmwareEnvironmentVariableExW(
            variable_name,
            OMARCHY_VENDOR,
            Some(nonce.as_ptr().cast()),
            nonce.len() as u32,
            EFI_VARIABLE_ATTRIBUTES,
        )
    };
    if let Err(error) = write {
        log::warn!("EFI variable write probe failed: {error}");
        return false;
    }
    let mut readback = [0u8; 4];
    let read = unsafe {
        GetFirmwareEnvironmentVariableW(
            variable_name,
            OMARCHY_VENDOR,
            Some(readback.as_mut_ptr().cast()),
            readback.len() as u32,
        )
    };
    let deleted = unsafe {
        SetFirmwareEnvironmentVariableExW(
            variable_name,
            OMARCHY_VENDOR,
            None,
            0,
            EFI_VARIABLE_ATTRIBUTES,
        )
    }
    .inspect_err(|error| log::warn!("EFI variable cleanup probe failed: {error}"))
    .is_ok();
    let readable = read == nonce.len() as u32 && readback == nonce;
    if !readable {
        log::warn!(
            "EFI variable read probe failed: bytes_read={read}, expected={}",
            nonce.len()
        );
    }
    readable && deleted
}

fn enable_system_environment_privilege() -> Result<()> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )?;
        let mut luid = windows::Win32::Foundation::LUID::default();
        LookupPrivilegeValueW(
            PCWSTR::null(),
            w!("SeSystemEnvironmentPrivilege"),
            &mut luid,
        )?;
        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let result = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None);
        let _ = CloseHandle(token);
        result?;
        Ok(())
    }
}

fn ram_bytes() -> (u64, u64, u64) {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    let (total, avail) = unsafe {
        match GlobalMemoryStatusEx(&mut status) {
            Ok(()) => (status.ullTotalPhys, status.ullAvailPhys),
            Err(_) => (0, 0),
        }
    };
    let mut kb = 0u64;
    let installed = unsafe {
        GetPhysicallyInstalledSystemMemory(&mut kb)
            .ok()
            .map(|_| kb.saturating_mul(1024))
            .filter(|n| *n > 0)
            .unwrap_or(total)
    };
    (installed, total, avail)
}

fn tpm_present_tbs() -> bool {
    // TBS is optional on some SKUs, so retain the registry fallback.
    std::path::Path::new(r"\\.\TPM").exists()
        || get_hklm_dword(w!("SYSTEM\\CurrentControlSet\\Services\\TPM"), w!("Start")).is_some()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Inventory {
    bitlocker: Option<Vec<PsBitlocker>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PsBitlocker {
    device_id: Option<String>,
    mount: Option<String>,
    protection: Option<u32>,
    conversion: Option<u32>,
}

const BITLOCKER_INVENTORY_PS: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new()
$bitlocker = @(Get-CimInstance -Namespace 'root/cimv2/Security/MicrosoftVolumeEncryption' -ClassName Win32_EncryptableVolume | ForEach-Object {
    [ordered]@{
      deviceId = [string]$_.DeviceID
      mount = [string]$_.DriveLetter
      protection = [uint32]$_.ProtectionStatus
      conversion = [uint32]$_.ConversionStatus
    }
})
@{ bitlocker = $bitlocker } | ConvertTo-Json -Depth 4 -Compress
"#;

fn run_inventory_stage(description: &str, script: &str, timeout_seconds: u64) -> Result<Inventory> {
    let started = Instant::now();
    let result =
        run_storage_powershell_read_only(script, Duration::from_secs(timeout_seconds), description)
            .and_then(|stdout| {
                serde_json::from_str(stdout.trim()).map_err(|error| {
                    Error::Message(format!("{description} returned invalid JSON: {error}"))
                })
            });
    let elapsed_ms = started.elapsed().as_millis();
    match &result {
        Ok(_) => log::info!("{description} completed in {elapsed_ms} ms"),
        Err(error) => log::warn!("{description} failed after {elapsed_ms} ms: {error}"),
    }
    result
}

fn overlay_fve_signatures(bitlocker: &mut Vec<BitlockerVolume>, disks: &[DiskMap]) {
    for vol in bitlocker {
        let (Some(disk_id), Some(mount)) = (vol.disk_id.as_deref(), vol.mount.as_deref()) else {
            continue;
        };
        let Some(offset) = disks
            .iter()
            .find(|disk| disk.device_id.eq_ignore_ascii_case(disk_id))
            .and_then(|disk| {
                disk.partitions
                    .iter()
                    .find(|part| part.letter.as_deref() == Some(mount))
            })
            .map(|part| part.offset_bytes)
        else {
            continue;
        };
        let fve = read_fve(disk_id, offset).unwrap_or(false);
        vol.fully_decrypted =
            probe::bitlocker_fully_decrypted(vol.protection_status, vol.conversion_status, fve);
        if fve && vol.conversion_status == 0 {
            vol.conversion_status = 1;
            vol.fully_decrypted = false;
        }
    }
}

fn query_bitlocker(injected_fault: Option<ProviderFault>) -> BitlockerDiagnostics {
    let started = Instant::now();
    if injected_fault == Some(ProviderFault::BitlockerEmpty) {
        return BitlockerDiagnostics {
            elapsed_ms: started.elapsed().as_millis(),
            volumes: Vec::new(),
            error: None,
        };
    }
    if let Some(error) = match injected_fault {
        Some(ProviderFault::BitlockerTimeout) => {
            Some("BitLocker WMI timed out after 15 seconds (injected)")
        }
        Some(ProviderFault::BitlockerMalformed) => {
            Some("BitLocker WMI returned invalid JSON (injected)")
        }
        Some(ProviderFault::BitlockerWorkerPanic) => {
            panic!("injected BitLocker WMI worker panic")
        }
        _ => None,
    } {
        return BitlockerDiagnostics {
            elapsed_ms: started.elapsed().as_millis(),
            volumes: Vec::new(),
            error: Some(error.into()),
        };
    }
    match run_inventory_stage("BitLocker WMI", BITLOCKER_INVENTORY_PS, 15) {
        Ok(inventory) => BitlockerDiagnostics {
            elapsed_ms: started.elapsed().as_millis(),
            volumes: inventory.bitlocker.unwrap_or_default(),
            error: None,
        },
        Err(error) => BitlockerDiagnostics {
            elapsed_ms: started.elapsed().as_millis(),
            volumes: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

fn disks_from_inventory(inventory: &NativeStorageReport) -> Vec<DiskMap> {
    inventory
        .disks
        .iter()
        .map(|disk| {
            let bus_l = disk.bus.to_ascii_lowercase();
            let identity = format!(
                "{} {} {}",
                disk.bus,
                disk.model.as_deref().unwrap_or(""),
                disk.serial.as_deref().unwrap_or("")
            );
            let is_dynamic = disk.partitions.iter().any(|partition| {
                partition.type_guid.as_deref().is_some_and(|guid| {
                    let guid = normalize_guid(guid);
                    guid == LDM_META || guid == LDM_DATA
                })
            });
            DiskMap {
                device_id: disk.device_id.clone(),
                size_bytes: disk.size_bytes,
                partition_style: disk.partition_style.clone(),
                bus: Some(disk.bus.clone()),
                is_boot: disk.number == inventory.boot_disk_number,
                is_rst: bus_l == "raid"
                    || identity.to_ascii_lowercase().contains("iasta")
                    || identity.to_ascii_lowercase().contains("iastor")
                    || identity.to_ascii_lowercase().contains("intel rst")
                    || identity.to_ascii_lowercase().contains("vmd"),
                is_dynamic,
                is_storage_spaces: bus_l == "storagespaces",
                max_shrink_bytes: None,
                partitions: disk.partitions.iter().map(partition_from_native).collect(),
            }
        })
        .collect()
}

fn partition_from_native(partition: &NativePartition) -> PartitionMap {
    let letter = partition
        .mount_paths
        .iter()
        .find_map(|path| drive_letter(path));
    PartitionMap {
        gpt_guid: partition.gpt_guid.clone(),
        type_guid: partition.type_guid.clone(),
        letter,
        label: partition.label.clone(),
        size_bytes: partition.size_bytes,
        offset_bytes: partition.offset_bytes,
        fs: partition.fs.clone(),
    }
}

fn drive_letter(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic())
        .then(|| format!("{}:", (bytes[0] as char).to_ascii_uppercase()))
}

fn bitlocker_for_boot_disk(
    inventory: &NativeStorageReport,
    bitlocker: Vec<PsBitlocker>,
) -> (Vec<BitlockerVolume>, bool) {
    let target_volumes = inventory.volumes.iter().filter(|volume| {
        volume
            .mount_paths
            .iter()
            .any(|path| drive_letter(path).as_deref() == Some("C:"))
            && volume.extents.len() == 1
            && volume.extents[0].disk_number == inventory.boot_disk_number
    });
    let target_volume_count = target_volumes.count();
    let mut target_matches = 0usize;
    let mut association_failed = target_volume_count != 1;
    let mut output = Vec::new();
    for item in bitlocker {
        let matched = inventory.volumes.iter().find(|volume| {
            volume
                .volume_guid
                .as_str()
                .eq_ignore_ascii_case(item.device_id.as_deref().unwrap_or(""))
                || volume.mount_paths.iter().any(|path| {
                    item.mount.as_deref().is_some_and(|mount| {
                        path.trim_end_matches('\\')
                            .eq_ignore_ascii_case(mount.trim_end_matches([':', '\\']))
                            || drive_letter(path).as_deref().is_some_and(|letter| {
                                letter.eq_ignore_ascii_case(mount.trim_end_matches('\\'))
                            })
                    })
                })
        });
        let affects_target = item.mount.as_deref().is_some_and(|mount| {
            drive_letter(mount).as_deref() == Some("C:")
                || mount
                    .trim_end_matches(['\\', ':'])
                    .eq_ignore_ascii_case("C")
        });
        let Some(volume) = matched else {
            association_failed |= affects_target;
            continue;
        };
        if volume.extents.len() != 1 {
            association_failed |= affects_target;
            continue;
        }
        if volume.extents[0].disk_number != inventory.boot_disk_number {
            continue;
        }
        let is_target = volume
            .mount_paths
            .iter()
            .any(|path| drive_letter(path).as_deref() == Some("C:"));
        if is_target {
            target_matches += 1;
            association_failed |= item.protection.is_none() || item.conversion.is_none();
        }
        let conversion_status = item.conversion.unwrap_or(u32::MAX);
        let protection_status = item.protection.unwrap_or(u32::MAX);
        output.push(BitlockerVolume {
            device_id: item.device_id,
            disk_id: Some(format!(r"\\.\PHYSICALDRIVE{}", inventory.boot_disk_number)),
            mount: item.mount.filter(|mount| !mount.trim().is_empty()),
            protection_status,
            conversion_status,
            fully_decrypted: protection_status == 0 && conversion_status == 0,
        });
    }
    association_failed |= target_matches != 1;
    (output, association_failed)
}

fn target_esp_from_inventory(
    inventory: &NativeStorageReport,
) -> (Option<TargetEsp>, Vec<BlockingReason>) {
    let Some(disk) = inventory
        .disks
        .iter()
        .find(|disk| disk.number == inventory.boot_disk_number)
    else {
        return (
            None,
            vec![BlockingReason::ProbeIncomplete {
                component: "Windows boot disk identity".into(),
                detail: format!(
                    "Native inventory did not contain PhysicalDrive{}.",
                    inventory.boot_disk_number
                ),
            }],
        );
    };
    let candidates: Vec<&NativePartition> = disk
        .partitions
        .iter()
        .filter(|partition| {
            partition
                .type_guid
                .as_deref()
                .map(normalize_guid)
                .as_deref()
                == Some("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")
        })
        .collect();
    if candidates.is_empty() {
        return (
            None,
            vec![BlockingReason::MissingEsp {
                disk_id: disk.device_id.clone(),
            }],
        );
    }
    if candidates.len() != 1 {
        return (
            None,
            vec![BlockingReason::AmbiguousEsp {
                disk_id: disk.device_id.clone(),
                count: candidates.len() as u32,
            }],
        );
    }
    let partition = candidates[0];
    let Some(disk_guid) = disk.disk_guid.clone() else {
        return (
            None,
            vec![BlockingReason::ProbeIncomplete {
                component: "target GPT disk GUID".into(),
                detail: "The native drive layout did not contain a GPT disk GUID.".into(),
            }],
        );
    };
    let (Some(partition_guid), Some(volume_guid)) =
        (partition.gpt_guid.clone(), partition.volume_guid.clone())
    else {
        return (None, vec![BlockingReason::ProbeIncomplete {
            component: "target ESP identity".into(),
            detail: "The native inventory could not associate the ESP with stable partition and volume GUIDs."
                .into(),
        }]);
    };
    (
        Some(TargetEsp {
            disk_id: disk.device_id.clone(),
            disk_guid,
            disk_number: disk.number,
            partition_guid,
            volume_guid,
        }),
        vec![],
    )
}

fn linux_by_id_from_inventory(inventory: &NativeStorageReport) -> Option<String> {
    let disk = inventory
        .disks
        .iter()
        .find(|disk| disk.number == inventory.boot_disk_number)?;
    let serial = disk
        .serial
        .as_deref()
        .map(sanitize_id)
        .filter(|s| !s.is_empty())?;
    let model = disk
        .model
        .as_deref()
        .map(sanitize_id)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "disk".into());
    let prefix = if disk.bus.eq_ignore_ascii_case("nvme") {
        "nvme"
    } else {
        "ata"
    };
    Some(format!("/dev/disk/by-id/{prefix}-{model}_{serial}"))
}

fn normalize_guid(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '{' || c == '}')
        .to_ascii_lowercase()
}

fn read_fve(disk: &str, offset: u64) -> Option<bool> {
    let path = disk.trim_end_matches('\\');
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let handle = CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_NORMAL.0),
            None,
        )
        .ok()?;
        let seek = SetFilePointerEx(
            handle,
            i64::try_from(offset).ok()?,
            None,
            Default::default(),
        );
        if seek.is_err() {
            let _ = CloseHandle(handle);
            return None;
        }
        // Raw disk reads must be sector-sized even though the signature is in
        // bytes 3..11. A short read fails with ERROR_INVALID_PARAMETER.
        let mut buf = [0u8; 512];
        let mut read = 0u32;
        let ok = ReadFile(handle, Some(buf.as_mut_slice()), Some(&mut read), None).is_ok();
        let _ = CloseHandle(handle);
        if !ok || read < 11 {
            return None;
        }
        Some(volume_is_fve(&buf))
    }
}

fn sanitize_id(s: &str) -> String {
    s.trim().replace([' ', '/'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::windows::process::{system_command, SystemTool};

    #[test]
    fn provider_fault_vocabulary_is_stable() {
        for name in [
            "storage-timeout",
            "storage-malformed",
            "storage-helper-failure",
            "storage-report-failure",
            "bitlocker-timeout",
            "bitlocker-malformed",
            "bitlocker-empty",
            "bitlocker-worker-panic",
        ] {
            assert!(ProviderFault::parse(name).is_some(), "{name}");
        }
        assert!(ProviderFault::parse("unknown").is_none());
    }

    #[test]
    fn injected_bitlocker_failures_return_immediately() {
        for fault in [
            ProviderFault::BitlockerTimeout,
            ProviderFault::BitlockerMalformed,
        ] {
            let result = query_bitlocker(Some(fault));
            assert!(result.elapsed_ms < 1_000, "{result:?}");
            assert!(result.volumes.is_empty());
            assert!(result.error.is_some());
        }
    }

    fn bitlocker_test_inventory() -> NativeStorageReport {
        NativeStorageReport {
            elapsed_ms: 0,
            boot_volume_guid: r"\\?\Volume{cccccccc-cccc-cccc-cccc-cccccccccccc}\".into(),
            boot_disk_number: 0,
            disks: Vec::new(),
            volumes: vec![native_storage::NativeVolume {
                volume_guid: r"\\?\Volume{cccccccc-cccc-cccc-cccc-cccccccccccc}\".into(),
                mount_paths: vec![r"C:\".into()],
                label: None,
                fs: Some("NTFS".into()),
                extents: vec![native_storage::NativeExtent {
                    disk_number: 0,
                    offset_bytes: 1_048_576,
                    length_bytes: 100_000,
                }],
            }],
            warnings: Vec::new(),
            issues: Vec::new(),
            timings: Vec::new(),
        }
    }

    #[test]
    fn bitlocker_inventory_must_report_the_windows_volume_exactly_once() {
        let inventory = bitlocker_test_inventory();
        assert!(bitlocker_for_boot_disk(&inventory, Vec::new()).1);

        let complete = PsBitlocker {
            device_id: Some(inventory.boot_volume_guid.clone()),
            mount: Some("C:".into()),
            protection: Some(0),
            conversion: Some(0),
        };
        let (volumes, failed) = bitlocker_for_boot_disk(&inventory, vec![complete.clone()]);
        assert!(!failed);
        assert_eq!(volumes.len(), 1);
        assert!(volumes[0].fully_decrypted);

        assert!(bitlocker_for_boot_disk(&inventory, vec![complete.clone(), complete]).1);
    }

    #[test]
    fn incomplete_bitlocker_status_fails_closed() {
        let inventory = bitlocker_test_inventory();
        let incomplete = PsBitlocker {
            device_id: Some(inventory.boot_volume_guid.clone()),
            mount: Some("C:".into()),
            protection: None,
            conversion: Some(0),
        };
        assert!(bitlocker_for_boot_disk(&inventory, vec![incomplete]).1);
    }

    #[test]
    fn staged_inventory_powershell_is_syntactically_valid() {
        const SCRIPT_ENV: &str = "OMARCHY_INSTALL_PROBE_PS_SYNTAX_TEST";
        for script in [BITLOCKER_INVENTORY_PS] {
            let output = system_command(SystemTool::PowerShell)
                .unwrap()
                .env(SCRIPT_ENV, script)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[scriptblock]::Create([Environment]::GetEnvironmentVariable('OMARCHY_INSTALL_PROBE_PS_SYNTAX_TEST')) | Out-Null",
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
