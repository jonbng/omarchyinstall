//! Read-only machine probe. Compiled only on Windows.

use crate::error::Result;
use crate::platform::{
    BitlockerVolume, BlockingReason, DiskMap, MachineProbe, PartitionMap, TargetEsp,
};
use crate::probe::{self, volume_is_fve};
use serde::Deserialize;
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, ERROR_SUCCESS, GENERIC_READ, HANDLE},
        Security::{
            AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES,
            SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        Storage::FileSystem::{
            CreateFileW, ReadFile, SetFilePointerEx, FILE_ATTRIBUTE_NORMAL,
            FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{
            Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD},
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
const OMARCHY_WRITE_PROBE: PCWSTR = w!("OmarchyInstallWriteProbe");
const EFI_VARIABLE_ATTRIBUTES: u32 = 0x0000_0001 | 0x0000_0002 | 0x0000_0004;
const LDM_META: &str = "5808c8aa-7e8f-42e0-85d2-e1e90434cfb3";
const LDM_DATA: &str = "af9b60a0-1431-4f62-bc68-3311714a69ad";

pub fn probe_machine() -> Result<MachineProbe> {
    let host = super::host_info()?;
    let uefi = is_uefi();
    let secure_boot = secure_boot_enabled();
    let efi_vars_writable = if uefi {
        efi_variables_writable()
    } else {
        false
    };
    let (ram_installed_bytes, ram_total_phys_bytes, ram_avail_bytes) = ram_bytes();
    let inventory = inventory_from_powershell();
    let tpm_present = inventory
        .as_ref()
        .and_then(|i| i.tpm_present)
        .unwrap_or_else(tpm_present_tbs);

    let disks = inventory
        .as_ref()
        .map(disks_from_inventory)
        .unwrap_or_default();
    let mut bitlocker = inventory
        .as_ref()
        .map(bitlocker_from_inventory)
        .unwrap_or_default();

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
                    component: "Windows storage inventory".into(),
                }],
            )
        });
    if inventory
        .as_ref()
        .and_then(|i| i.bitlocker_error.as_ref())
        .is_some()
    {
        inventory_reasons.push(BlockingReason::ProbeIncomplete {
            component: "BitLocker WMI".into(),
        });
    }
    if inventory.as_ref().is_some_and(|i| {
        i.bitlocker
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .any(|volume| volume.disk_number.is_none())
    }) {
        inventory_reasons.push(BlockingReason::ProbeIncomplete {
            component: "BitLocker volume-to-disk association".into(),
        });
    }

    let linux_by_id = inventory.as_ref().and_then(linux_by_id_from_inventory);

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
    Ok(probe::attach_reasons(probe, true))
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
    if let Some(v) = reg_dword(
        w!("SYSTEM\\CurrentControlSet\\Control\\SecureBoot\\State"),
        w!("UEFISecureBootEnabled"),
    ) {
        return v != 0;
    }
    false
}

fn reg_dword(subkey: PCWSTR, value: PCWSTR) -> Option<u32> {
    unsafe {
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let read = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        );
        if read == ERROR_SUCCESS {
            Some(data)
        } else {
            None
        }
    }
}

fn efi_variables_writable() -> bool {
    if enable_system_environment_privilege().is_err() {
        return false;
    }
    let nonce = std::process::id().to_le_bytes();
    let write = unsafe {
        SetFirmwareEnvironmentVariableExW(
            OMARCHY_WRITE_PROBE,
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
            OMARCHY_WRITE_PROBE,
            OMARCHY_VENDOR,
            Some(readback.as_mut_ptr().cast()),
            readback.len() as u32,
        )
    };
    let deleted = unsafe {
        SetFirmwareEnvironmentVariableExW(
            OMARCHY_WRITE_PROBE,
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
    // TBS is optional on some SKUs; the Storage/TPM PowerShell inventory is preferred.
    std::path::Path::new(r"\\.\TPM").exists()
        || reg_dword(w!("SYSTEM\\CurrentControlSet\\Services\\TPM"), w!("Start")).is_some()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Inventory {
    disks: Option<Vec<PsDisk>>,
    partitions: Option<Vec<PsPart>>,
    shrink: Option<Vec<PsShrink>>,
    bitlocker: Option<Vec<PsBitlocker>>,
    bitlocker_error: Option<String>,
    tpm_present: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PsDisk {
    number: Option<u32>,
    size: Option<u64>,
    partition_style: Option<String>,
    is_boot: Option<bool>,
    bus_type: Option<serde_json::Value>,
    serial_number: Option<String>,
    model: Option<String>,
    friendly_name: Option<String>,
    guid: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PsPart {
    disk_number: Option<u32>,
    drive_letter: Option<String>,
    size: Option<u64>,
    gpt_type: Option<String>,
    guid: Option<String>,
    file_system: Option<String>,
    file_system_label: Option<String>,
    offset: Option<u64>,
    volume_unique_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PsShrink {
    disk_number: Option<u32>,
    size_min: Option<u64>,
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PsBitlocker {
    device_id: Option<String>,
    disk_number: Option<u32>,
    mount: Option<String>,
    protection: Option<u32>,
    conversion: Option<u32>,
}

const INVENTORY_PS: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new()
$disks = @(Get-Disk | ForEach-Object {
  [ordered]@{
    number = [int]$_.Number
    size = [uint64]$_.Size
    partitionStyle = [string]$_.PartitionStyle
    isBoot = [bool]$_.IsBoot
    busType = [string]$_.BusType
    serialNumber = [string]$_.SerialNumber
    model = [string]$_.Model
    friendlyName = [string]$_.FriendlyName
    guid = [string]$_.Guid
  }
})
$parts = @(Get-Partition | ForEach-Object {
  $letter = if ($_.DriveLetter) { [string]$_.DriveLetter } else { $null }
  $vol = $null
  try { $vol = Get-Volume -Partition $_ -ErrorAction Stop } catch {}
  [ordered]@{
    diskNumber = [int]$_.DiskNumber
    driveLetter = $letter
    size = [uint64]$_.Size
    gptType = [string]$_.GptType
    guid = [string]$_.Guid
    type = [string]$_.Type
    isBoot = [bool]$_.IsBoot
    isSystem = [bool]$_.IsSystem
    isHidden = [bool]$_.IsHidden
    fileSystem = if ($vol) { [string]$vol.FileSystemType } else { $null }
    fileSystemLabel = if ($vol) { [string]$vol.FileSystemLabel } else { $null }
    offset = [uint64]$_.Offset
    volumeUniqueId = if ($vol) { [string]$vol.UniqueId } else { $null }
  }
})
$shrink = @()
foreach ($p in Get-Partition) {
  if (-not $p.DriveLetter) { continue }
  try {
    $s = Get-PartitionSupportedSize -DiskNumber $p.DiskNumber -PartitionNumber $p.PartitionNumber
    $shrink += [ordered]@{
      diskNumber = [int]$p.DiskNumber
      sizeMin = [uint64]$s.SizeMin
      size = [uint64]$p.Size
    }
  } catch {}
}
$bitlocker = @()
$bitlockerError = $null
try {
  $bitlocker = @(Get-CimInstance -Namespace 'root/cimv2/Security/MicrosoftVolumeEncryption' -ClassName Win32_EncryptableVolume -ErrorAction Stop | ForEach-Object {
    $device = [string]$_.DeviceID
    $partMatches = @($parts | Where-Object { $_.volumeUniqueId -eq $device })
    $part = if ($partMatches.Count -eq 1) { $partMatches[0] } else { $null }
    [ordered]@{
      deviceId = $device
      diskNumber = if ($part) { [int]$part.diskNumber } else { $null }
      mount = [string]$_.DriveLetter
      protection = [uint32]$_.ProtectionStatus
      conversion = [uint32]$_.ConversionStatus
    }
  })
} catch { $bitlockerError = $_.Exception.Message }
$tpmPresent = $false
try { $tpmPresent = [bool]((Get-Tpm).TpmPresent) } catch {}
@{
  disks = $disks
  partitions = $parts
  shrink = $shrink
  bitlocker = $bitlocker
  bitlockerError = $bitlockerError
  tpmPresent = $tpmPresent
} | ConvertTo-Json -Depth 6 -Compress
"#;

fn inventory_from_powershell() -> Option<Inventory> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            INVENTORY_PS,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        log::warn!(
            "storage inventory powershell failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .map_err(|e| {
            log::warn!("storage inventory json: {e}");
            e
        })
        .ok()
}

fn disks_from_inventory(inv: &Inventory) -> Vec<DiskMap> {
    let parts = inv.partitions.as_deref().unwrap_or(&[]);
    let shrink = inv.shrink.as_deref().unwrap_or(&[]);
    let mut out = Vec::new();
    for disk in inv.disks.as_deref().unwrap_or(&[]) {
        let number = disk.number.unwrap_or(0);
        let style = disk
            .partition_style
            .as_deref()
            .unwrap_or("raw")
            .to_ascii_lowercase();
        let bus = bus_string(&disk.bus_type);
        let is_rst = bus_is_rst(&bus, disk.friendly_name.as_deref(), disk.model.as_deref());
        let is_storage_spaces = bus.to_ascii_lowercase().contains("storagespaces")
            || disk
                .friendly_name
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains("storage space");
        let disk_parts: Vec<PartitionMap> = parts
            .iter()
            .filter(|p| p.disk_number == Some(number))
            .map(partition_from_ps)
            .collect();
        let is_dynamic = disk_parts.iter().any(|p| {
            p.type_guid
                .as_deref()
                .map(|g| {
                    let g = g
                        .trim_matches(|c| c == '{' || c == '}')
                        .to_ascii_lowercase();
                    g == LDM_META || g == LDM_DATA
                })
                .unwrap_or(false)
        });
        let max_shrink_bytes = shrink
            .iter()
            .filter(|s| s.disk_number == Some(number))
            .filter_map(|s| match (s.size, s.size_min) {
                (Some(size), Some(min)) if size > min => Some(size - min),
                _ => None,
            })
            .max();
        out.push(DiskMap {
            device_id: format!(r"\\.\PHYSICALDRIVE{number}"),
            size_bytes: disk.size.unwrap_or(0),
            partition_style: style,
            bus: Some(bus),
            is_boot: disk.is_boot.unwrap_or(false),
            is_rst,
            is_dynamic,
            is_storage_spaces,
            max_shrink_bytes,
            partitions: disk_parts,
        });
    }
    out
}

fn partition_from_ps(p: &PsPart) -> PartitionMap {
    let letter = p.drive_letter.as_ref().and_then(|s| {
        let s = s.trim();
        if s.is_empty() {
            None
        } else if s.ends_with(':') {
            Some(s.to_string())
        } else {
            Some(format!("{s}:"))
        }
    });
    PartitionMap {
        gpt_guid: p.guid.clone().filter(|s| !s.is_empty()),
        type_guid: p.gpt_type.clone().filter(|s| !s.is_empty()),
        letter,
        label: p.file_system_label.clone().filter(|s| !s.is_empty()),
        size_bytes: p.size.unwrap_or(0),
        offset_bytes: p.offset.unwrap_or(0),
        fs: p.file_system.clone().filter(|s| !s.is_empty()),
    }
}

fn bus_string(v: &Option<serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn bus_is_rst(bus: &str, friendly: Option<&str>, model: Option<&str>) -> bool {
    let bus_l = bus.to_ascii_lowercase();
    if bus_l == "raid" || bus_l == "8" {
        return true;
    }
    let blob =
        format!("{} {} {}", bus, friendly.unwrap_or(""), model.unwrap_or("")).to_ascii_lowercase();
    blob.contains("iasta")
        || blob.contains("iastor")
        || blob.contains("intel rst")
        || blob.contains("vmd")
        || blob.contains("raid volume")
}

fn bitlocker_from_inventory(inv: &Inventory) -> Vec<BitlockerVolume> {
    let target_number = inv
        .disks
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find(|d| d.is_boot.unwrap_or(false))
        .and_then(|d| d.number);
    inv.bitlocker
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter(|b| target_number.is_some() && b.disk_number == target_number)
        .map(|b| {
            let conversion_status = b.conversion.unwrap_or(u32::MAX);
            let protection = b.protection.unwrap_or(0);
            BitlockerVolume {
                device_id: b.device_id.clone(),
                disk_id: target_number.map(|n| format!(r"\\.\PHYSICALDRIVE{n}")),
                mount: b.mount.clone().filter(|s| !s.trim().is_empty()),
                protection_status: protection,
                conversion_status,
                fully_decrypted: protection == 0 && conversion_status == 0,
            }
        })
        .collect()
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

fn target_esp_from_inventory(inv: &Inventory) -> (Option<TargetEsp>, Vec<BlockingReason>) {
    let Some(disk) = inv
        .disks
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find(|d| d.is_boot.unwrap_or(false))
    else {
        return (
            None,
            vec![BlockingReason::ProbeIncomplete {
                component: "Windows boot disk identity".into(),
            }],
        );
    };
    let number = disk.number.unwrap_or(u32::MAX);
    let disk_id = format!(r"\\.\PHYSICALDRIVE{number}");
    let candidates: Vec<&PsPart> = inv
        .partitions
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter(|p| {
            p.disk_number == Some(number)
                && p.gpt_type.as_deref().map(normalize_guid).as_deref()
                    == Some("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")
        })
        .collect();
    if candidates.is_empty() {
        return (None, vec![BlockingReason::MissingEsp { disk_id }]);
    }
    if candidates.len() != 1 {
        return (
            None,
            vec![BlockingReason::AmbiguousEsp {
                disk_id,
                count: candidates.len() as u32,
            }],
        );
    }
    let part = candidates[0];
    let Some(disk_guid) = disk.guid.clone().filter(|s| !s.is_empty()) else {
        return (
            None,
            vec![BlockingReason::ProbeIncomplete {
                component: "target GPT disk GUID".into(),
            }],
        );
    };
    let (Some(partition_guid), Some(volume_guid)) = (
        part.guid.clone().filter(|s| !s.is_empty()),
        part.volume_unique_id.clone().filter(|s| !s.is_empty()),
    ) else {
        return (
            None,
            vec![BlockingReason::ProbeIncomplete {
                component: "target ESP identity".into(),
            }],
        );
    };
    (
        Some(TargetEsp {
            disk_id: disk_id.clone(),
            disk_guid,
            disk_number: number,
            partition_guid,
            volume_guid,
        }),
        vec![],
    )
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
        let seek = SetFilePointerEx(handle, i64::try_from(offset).ok()?, None, Default::default());
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

fn linux_by_id_from_inventory(inv: &Inventory) -> Option<String> {
    let disk = inv
        .disks
        .as_ref()?
        .iter()
        .find(|d| d.is_boot.unwrap_or(false))?;
    let serial = disk
        .serial_number
        .as_deref()
        .map(sanitize_id)
        .filter(|s| !s.is_empty())?;
    let model = disk
        .model
        .as_deref()
        .map(sanitize_id)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "disk".into());
    let bus = bus_string(&disk.bus_type).to_ascii_lowercase();
    let prefix = if bus.contains("nvme") || bus == "17" {
        "nvme"
    } else {
        "ata"
    };
    Some(format!("/dev/disk/by-id/{prefix}-{model}_{serial}"))
}

fn sanitize_id(s: &str) -> String {
    s.trim().replace([' ', '/'], "_")
}
