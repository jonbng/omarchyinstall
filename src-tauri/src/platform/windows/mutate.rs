//! Disk staging, cidata, BootNext, rollback. Compiled only on Windows.

use super::{
    host_info,
    iso_mount::MountedIso,
    process::{run_storage_powershell, system_command, SystemTool},
    registry::{get_hklm_dword, set_hklm_dword},
};
use crate::cidata::{self, CidataIdentity};
use crate::download;
use crate::error::{Error, Result};
use crate::grub::{
    self, emit_windows_vm_grub_cfg, ESP_GRUB_CFG, ESP_GRUB_EFI, VM_INITRAMFS_FAT_PATH,
    VM_INITRAMFS_ISO_PATH, VM_KERNEL_FAT_PATH, VM_KERNEL_ISO_PATH,
};
use crate::journal::{
    self, empty_journal, interpret_rollback_output, parse_journal, serialize_journal,
};
use crate::partition::{self, require_cidata_fs, require_omarchyinst_fs};
use crate::paths;
use crate::platform::{
    BootNextResult, CidataResult, JournalStep, PendingOperation, PrepareResult, RollbackResult,
    StageResult, StateJournal,
};
use crate::probe;
use crate::winvol::{gpt_partuuid, windows_volume_path};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use windows::core::w;
use zip::write::FileOptions;
use zip::ZipWriter;

const POWER_SESSION_KEY: windows::core::PCWSTR =
    w!("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Power");
const POWER_KEY: windows::core::PCWSTR = w!("SYSTEM\\CurrentControlSet\\Control\\Power");
const HIBERBOOT_ENABLED: windows::core::PCWSTR = w!("HiberbootEnabled");
const HIBERNATE_ENABLED: windows::core::PCWSTR = w!("HibernateEnabled");

fn local_app_dir() -> Result<PathBuf> {
    paths::install_data_dir()
}

fn journal_path() -> Result<PathBuf> {
    Ok(local_app_dir()?.join("state.json"))
}

fn load_journal() -> Result<Option<StateJournal>> {
    let path = journal_path()?;
    match fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(body) => Ok(Some(parse_journal(&body)?)),
    }
}

fn save_journal(journal: &StateJournal) -> Result<()> {
    journal::save_atomic(&journal_path()?, journal)
}

fn volume_root(unique_id: &str) -> Result<String> {
    windows_volume_path(unique_id)
}

fn volume_reachable(unique_id: &str) -> bool {
    let Ok(path) = windows_volume_path(unique_id) else {
        return false;
    };
    fs::metadata(path).is_ok()
}

fn reuse_prepared(journal: &crate::platform::StateJournal, iso_size: u64) -> Option<PrepareResult> {
    let omarchyinst_guid = journal.omarchyinst_guid.as_deref()?;
    let omarchyinst_partuuid = journal.omarchyinst_partuuid.as_deref()?;
    let cidata_guid = journal.cidata_guid.as_deref()?;
    let old_c = journal.old_c_size_bytes?;
    if omarchyinst_guid.is_empty() || cidata_guid.is_empty() {
        return None;
    }
    if !volume_reachable(omarchyinst_guid) || !volume_reachable(cidata_guid) {
        return None;
    }
    let partition_bytes = partition::omarchyinst_bytes(iso_size);
    let hole = partition::installer_hole_bytes(iso_size);
    Some(PrepareResult {
        omarchyinst_guid: omarchyinst_guid.to_string(),
        omarchyinst_partuuid: omarchyinst_partuuid.to_string(),
        cidata_guid: cidata_guid.to_string(),
        old_c_size_bytes: old_c,
        new_c_size_bytes: old_c.saturating_sub(hole),
        partition_bytes,
    })
}

pub fn prepare_installer_partition(allow_bitlocker: bool) -> Result<PrepareResult> {
    let probe = crate::platform::probe_machine()?;
    let probed_linux_device = probe
        .linux_by_id
        .as_deref()
        .ok_or_else(|| Error::Message("linux /dev/disk/by-id path missing".into()))?;
    // This branch is intentionally a single-machine bridge until the ISO-side
    // by-id canonicalization is released.  Refuse before touching C: elsewhere.
    cidata::windows_vm_archinstall_device(probed_linux_device)?;
    if probe.bitlocker.iter().any(|volume| !volume.fully_decrypted) && !allow_bitlocker {
        return Err(Error::Message(
            "BitLocker is still enabled. Turn it off (recommended), or explicitly accept the BitLocker recovery risk before continuing."
                .into(),
        ));
    }
    let existing = load_journal()?;
    let has_started = existing
        .as_ref()
        .is_some_and(|j| !matches!(j.step, JournalStep::Planned) || j.pending_operation.is_some());
    if probe.blocking_reasons.iter().any(|reason| {
        !has_started
            || !matches!(
                reason,
                crate::platform::BlockingReason::ShrinkTooSmall { .. }
            )
    }) {
        return Err(Error::Message(
            "machine probe has blocking reasons; refuse to shrink".into(),
        ));
    }
    let iso = download::iso_paths()?;
    let iso_size = if iso.iso.exists() {
        fs::metadata(&iso.iso)?.len()
    } else {
        6 * probe::GIB
    };
    require_omarchyinst_fs("ntfs")?;
    require_cidata_fs("fat32")?;
    let partition_bytes = partition::omarchyinst_bytes(iso_size);
    let hole = partition::installer_hole_bytes(iso_size);
    partition::iso_fits_omarchyinst(iso_size, partition_bytes)?;

    let mut journal = existing.unwrap_or_else(empty_journal);
    if let Some(existing) = reuse_prepared(&journal, iso_size) {
        log::info!("prepare: OMARCHYINST and cidata already exist; skip shrink");
        return Ok(existing);
    }
    if journal.target_disk_guid.is_none() {
        let esp = probe
            .target_esp
            .as_ref()
            .ok_or_else(|| Error::Message("target ESP identity missing".into()))?;
        let disk = probe
            .disks
            .iter()
            .find(|d| d.device_id == esp.disk_id)
            .ok_or_else(|| Error::Message("target boot disk disappeared".into()))?;
        let c = disk
            .partitions
            .iter()
            .find(|p| p.letter.as_deref() == Some("C:"))
            .ok_or_else(|| Error::Message("C: partition is not on the target disk".into()))?;
        let new_c = c
            .size_bytes
            .checked_sub(hole)
            .ok_or_else(|| Error::Message("C: is smaller than the installer hole".into()))?;
        journal.target_disk_guid = Some(esp.disk_guid.clone());
        journal.target_disk_number = Some(esp.disk_number);
        journal.windows_partition_guid = c.gpt_guid.clone();
        journal.windows_partition_offset_bytes = Some(c.offset_bytes);
        journal.old_c_size_bytes = Some(c.size_bytes);
        journal.new_c_size_bytes = Some(new_c);
        journal.omarchyinst_offset_bytes = Some(c.offset_bytes + new_c);
        journal.omarchyinst_size_bytes = Some(partition_bytes);
        journal.cidata_offset_bytes = Some(c.offset_bytes + new_c + partition_bytes);
        journal.cidata_size_bytes = Some(partition::CIDATA_BYTES);
        journal.esp_partition_guid = Some(esp.partition_guid.clone());
        journal.esp_volume_guid = Some(windows_volume_path(&esp.volume_guid)?);
        journal.linux_device = probe.linux_by_id.clone();
        journal.step = JournalStep::Planned;
        save_journal(&journal)?;
    }

    let disk_number = required_u32(journal.target_disk_number, "target disk number")?;
    let disk_guid = required(&journal.target_disk_guid, "target disk GUID")?;
    let windows_guid = required(&journal.windows_partition_guid, "Windows partition GUID")?;
    let old_c = required_u64(journal.old_c_size_bytes, "old C: size")?;
    let new_c = required_u64(journal.new_c_size_bytes, "new C: size")?;

    if matches!(journal.step, JournalStep::Planned) {
        if journal.hiberboot_was.is_none() {
            journal.hiberboot_was = get_hklm_dword(POWER_SESSION_KEY, HIBERBOOT_ENABLED);
            journal.hibernation_disabled_by_us =
                get_hklm_dword(POWER_KEY, HIBERNATE_ENABLED).is_some_and(|v| v != 0);
        }
        journal.pending_operation = Some(PendingOperation::DisablePower);
        save_journal(&journal)?;
        set_hklm_dword(POWER_SESSION_KEY, HIBERBOOT_ENABLED, 0)?;
        run_powercfg("off")?;
        journal.pending_operation = None;
        journal.step = JournalStep::PowerPrepared;
        save_journal(&journal)?;
    }

    if matches!(journal.step, JournalStep::PowerPrepared) {
        journal.pending_operation = Some(PendingOperation::ShrinkWindows);
        save_journal(&journal)?;
        run_storage_powershell(&format!(
            r#"$ErrorActionPreference='Stop'; $d=Get-Disk -Number {disk_number}; if (([string]$d.Guid).Trim('{{}}') -ne '{disk_guid}') {{ throw 'target disk GUID changed' }}; $c=Get-Partition -DiskNumber {disk_number} | Where-Object {{ ([string]$_.Guid).Trim('{{}}') -eq '{windows_guid}' }}; if (-not $c) {{ throw 'Windows partition disappeared' }}; if ([uint64]$c.Size -eq [uint64]{old_c}) {{ $supported=Get-PartitionSupportedSize -DiskNumber {disk_number} -PartitionNumber $c.PartitionNumber; if ([uint64]{new_c} -lt [uint64]$supported.SizeMin) {{ throw 'unmovable files; Windows will not shrink C: enough' }}; Resize-Partition -DiskNumber {disk_number} -PartitionNumber $c.PartitionNumber -Size ([uint64]{new_c}) }} elseif ([uint64]$c.Size -ne [uint64]{new_c}) {{ throw 'Windows partition size does not match journal' }}; 'ok'"#,
            disk_guid = ps_guid(&disk_guid),
            windows_guid = ps_guid(&windows_guid),
        ))?;
        journal.pending_operation = None;
        journal.step = JournalStep::WindowsShrunk;
        save_journal(&journal)?;
    }

    if matches!(journal.step, JournalStep::WindowsShrunk) {
        journal.pending_operation = Some(PendingOperation::CreateOmarchyPartition);
        save_journal(&journal)?;
        let v = create_or_recover_partition(
            disk_number,
            &disk_guid,
            required_u64(journal.omarchyinst_offset_bytes, "OMARCHYINST offset")?,
            partition_bytes,
            "NTFS",
            "OMARCHYINST",
            false,
        )?;
        journal.omarchyinst_guid = Some(windows_volume_path(json_str(&v, "volume")?)?);
        journal.omarchyinst_partuuid = Some(gpt_partuuid(json_str(&v, "partuuid")?)?);
        journal.pending_operation = None;
        journal.step = JournalStep::OmarchyPartitionCreated;
        save_journal(&journal)?;
    }

    if matches!(journal.step, JournalStep::OmarchyPartitionCreated) {
        journal.pending_operation = Some(PendingOperation::CreateCidataPartition);
        save_journal(&journal)?;
        let v = create_or_recover_partition(
            disk_number,
            &disk_guid,
            required_u64(journal.cidata_offset_bytes, "cidata offset")?,
            partition::CIDATA_BYTES,
            "FAT32",
            "cidata",
            false,
        )?;
        journal.cidata_guid = Some(windows_volume_path(json_str(&v, "volume")?)?);
        journal.cidata_partuuid = Some(gpt_partuuid(json_str(&v, "partuuid")?)?);
        journal.pending_operation = None;
        journal.step = JournalStep::CidataPartitionCreated;
        save_journal(&journal)?;
    }
    let omarchyinst_guid = required(&journal.omarchyinst_guid, "OMARCHYINST volume")?.to_string();
    let omarchyinst_partuuid =
        required(&journal.omarchyinst_partuuid, "OMARCHYINST PARTUUID")?.to_string();
    let cidata_guid = required(&journal.cidata_guid, "cidata volume")?.to_string();
    Ok(PrepareResult {
        omarchyinst_guid,
        omarchyinst_partuuid,
        cidata_guid,
        old_c_size_bytes: old_c,
        new_c_size_bytes: new_c,
        partition_bytes,
    })
}

fn required<'a>(value: &'a Option<String>, name: &str) -> Result<&'a str> {
    value
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Message(format!("journal missing {name}")))
}

fn required_u64(value: Option<u64>, name: &str) -> Result<u64> {
    value.ok_or_else(|| Error::Message(format!("journal missing {name}")))
}

fn required_u32(value: Option<u32>, name: &str) -> Result<u32> {
    value.ok_or_else(|| Error::Message(format!("journal missing {name}")))
}

fn ps_guid(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '{' || c == '}')
        .replace('\'', "''")
}

fn json_str<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Message(format!("partition result missing {key}")))
}

fn create_or_recover_partition(
    disk_number: u32,
    disk_guid: &str,
    offset: u64,
    size: u64,
    filesystem: &str,
    label: &str,
    hidden: bool,
) -> Result<serde_json::Value> {
    let raw = run_storage_powershell(&format!(
        r#"
$ErrorActionPreference='Stop'
$d=Get-Disk -Number {disk_number}
if (([string]$d.Guid).Trim('{{}}') -ne '{disk_guid}') {{ throw 'target disk GUID changed' }}
$matches=@(Get-Partition -DiskNumber {disk_number} | Where-Object {{ [uint64]$_.Offset -eq [uint64]{offset} }})
if ($matches.Count -gt 1) {{ throw 'ambiguous partition at planned offset' }}
if ($matches.Count -eq 0) {{ $p=New-Partition -DiskNumber {disk_number} -Offset ([uint64]{offset}) -Size ([uint64]{size}) -GptType '{{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}}' }} else {{ $p=$matches[0] }}
if ([uint64]$p.Size -ne [uint64]{size}) {{ throw 'partition at planned offset has wrong size' }}
$v=$null
try {{ $v=Get-Volume -Partition $p -ErrorAction Stop }} catch {{}}
if (-not $v -or [string]$v.FileSystemType -ne '{filesystem}' -or [string]$v.FileSystemLabel -ne '{label}') {{ Format-Volume -Partition $p -FileSystem {filesystem} -NewFileSystemLabel '{label}' -Confirm:$false | Out-Null; $v=Get-Volume -Partition $p }}
Set-Partition -DiskNumber {disk_number} -PartitionNumber $p.PartitionNumber -NoDefaultDriveLetter $true -IsHidden {hidden}
$p=Get-Partition -DiskNumber {disk_number} -PartitionNumber $p.PartitionNumber
if ($p.DriveLetter) {{ Remove-PartitionAccessPath -DiskNumber {disk_number} -PartitionNumber $p.PartitionNumber -AccessPath ($p.DriveLetter + ':\') }}
$p=Get-Partition -DiskNumber {disk_number} -PartitionNumber $p.PartitionNumber
$v=Get-Volume -Partition $p
if (-not $p.NoDefaultDriveLetter -or $p.DriveLetter) {{ throw 'partition automount hardening failed' }}
if ({hidden} -and -not $p.IsHidden) {{ throw 'cidata hidden attribute was not set' }}
@{{ volume=[string]$v.UniqueId; partuuid=[string]$p.Guid }} | ConvertTo-Json -Compress
"#,
        hidden = if hidden { "$true" } else { "$false" },
        disk_guid = ps_guid(disk_guid),
    ))?;
    serde_json::from_str(&raw).map_err(|e| Error::Message(format!("partition json: {e} {raw}")))
}

pub fn stage_bootloader() -> Result<StageResult> {
    let mut journal = load_journal()?.ok_or_else(|| Error::Message("no state.json".into()))?;
    journal.pending_operation = Some(PendingOperation::StageEsp);
    save_journal(&journal)?;
    let omarchy = journal
        .omarchyinst_guid
        .clone()
        .ok_or_else(|| Error::Message("OMARCHYINST guid missing".into()))?;
    validate_staging_volume(
        &journal,
        required(&journal.omarchyinst_partuuid, "OMARCHYINST PARTUUID")?,
        &omarchy,
        "OMARCHYINST",
        "NTFS",
        false,
    )?;
    let iso_src = download::iso_paths()?.iso;
    if !iso_src.exists() {
        return Err(Error::Message("verified ISO is not in the cache".into()));
    }
    let dest_iso = PathBuf::from(format!("{}omarchy.iso", volume_root(&omarchy)?));
    if !dest_iso.exists() {
        fs::copy(&iso_src, &dest_iso)?;
    }
    let iso_size = fs::metadata(&dest_iso)?.len();
    partition::iso_fits_omarchyinst(iso_size, partition::omarchyinst_bytes(iso_size))?;
    let stable_device = required(&journal.linux_device, "Linux stable disk identity")?;
    cidata::windows_vm_archinstall_device(stable_device)?;
    if required_u64(journal.cidata_size_bytes, "cidata size")? < partition::CIDATA_BYTES {
        return Err(Error::Message(
            "the existing cidata partition is from the old 64 MiB layout; undo it and prepare again"
                .into(),
        ));
    }

    let mounted_iso = MountedIso::attach(&iso_src)?;
    let iso_paths = collect_relative_files(mounted_iso.root())?;
    let search = grub::discover_search_filename(iso_paths.iter().map(String::as_str))?;
    for dest in grub::esp_stage_destinations(&search)? {
        grub::assert_esp_write_allowed(&dest)?;
    }
    let bait_windows = grub::esp_bait_windows_path(&search)?;
    journal.search_filename = Some(search.clone());
    save_journal(&journal)?;

    let esp = required(&journal.esp_volume_guid, "target ESP volume GUID")?.to_string();
    validate_esp_identity(&journal)?;
    let esp_root = volume_root(&esp)?;
    let efi_dest = PathBuf::from(format!("{}{}", esp_root, ESP_GRUB_EFI.replace('/', "\\")));
    let efi_dest_dir = efi_dest
        .parent()
        .ok_or_else(|| Error::Message("GRUB EFI destination has no parent directory".into()))?;
    // PowerShell's filesystem provider rejects directory creation through some
    // valid volume-GUID paths. Rust passes these verbatim paths to Windows.
    fs::create_dir_all(efi_dest_dir)?;
    if let Some((parent, _)) = bait_windows
        .rsplit_once('\\')
        .filter(|(parent, _)| !parent.is_empty())
    {
        fs::create_dir_all(PathBuf::from(format!("{esp_root}{parent}")))?;
    }
    let efi_src = mounted_iso
        .root()
        .join("EFI")
        .join("BOOT")
        .join("BOOTX64.EFI");
    if !efi_src.is_file() {
        return Err(Error::Message(
            "mounted ISO is missing EFI/BOOT/BOOTX64.EFI".into(),
        ));
    }
    let bait_src = mounted_iso.root().join(&bait_windows);
    if !bait_src.is_file() {
        return Err(Error::Message(
            "discovered ISO search bait is missing".into(),
        ));
    }
    fs::copy(efi_src, &efi_dest)?;
    fs::copy(bait_src, PathBuf::from(format!("{esp_root}{bait_windows}")))?;

    let cidata_guid = required(&journal.cidata_guid, "cidata volume GUID")?;
    validate_staging_volume(
        &journal,
        required(&journal.cidata_partuuid, "cidata PARTUUID")?,
        cidata_guid,
        "cidata",
        "FAT32",
        false,
    )?;
    let cidata_root = volume_root(cidata_guid)?;
    let cidata_partition_number = staging_partition_number(
        &journal,
        required(&journal.cidata_partuuid, "cidata PARTUUID")?,
    )?;
    copy_vm_boot_file(
        mounted_iso.root(),
        VM_KERNEL_ISO_PATH,
        &cidata_root,
        VM_KERNEL_FAT_PATH,
    )?;
    copy_vm_boot_file(
        mounted_iso.root(),
        VM_INITRAMFS_ISO_PATH,
        &cidata_root,
        VM_INITRAMFS_FAT_PATH,
    )?;
    drop(mounted_iso);

    let partuuid = journal
        .omarchyinst_partuuid
        .as_deref()
        .map(gpt_partuuid)
        .transpose()?
        .ok_or_else(|| {
            Error::Message(
                "OMARCHYINST PARTUUID missing; Get-Partition.Guid was not journaled".into(),
            )
        })?;
    let cfg = emit_windows_vm_grub_cfg(&partuuid, cidata_partition_number, iso_size);
    let cfg_path = PathBuf::from(format!("{}{}", esp_root, ESP_GRUB_CFG.replace('/', "\\")));
    if let Some(parent) = cfg_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&cfg_path, cfg.as_bytes())?;
    let mut hasher = Sha256::new();
    hasher.update(cfg.as_bytes());
    let grub_cfg_sha256 = format!("{:x}", hasher.finalize());
    journal.pending_operation = None;
    journal.step = crate::platform::JournalStep::Staged;
    save_journal(&journal)?;
    Ok(StageResult {
        esp_guid: esp,
        search_filename: search,
        grub_cfg_sha256,
    })
}

fn copy_vm_boot_file(
    iso_root: &std::path::Path,
    iso_relative: &str,
    fat_root: &str,
    fat_relative: &str,
) -> Result<()> {
    let source = iso_root.join(iso_relative);
    if !source.is_file() {
        return Err(Error::Message(format!(
            "mounted ISO is missing VM boot file {iso_relative}"
        )));
    }
    let destination = PathBuf::from(format!("{}{}", fat_root, fat_relative.replace('/', "\\")));
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&source, &destination)?;
    if fs::metadata(&source)?.len() != fs::metadata(&destination)?.len() {
        return Err(Error::Message(format!(
            "VM boot file copy was truncated: {fat_relative}"
        )));
    }
    Ok(())
}

fn staging_partition_number(journal: &StateJournal, part_guid: &str) -> Result<u32> {
    let disk_number = required_u32(journal.target_disk_number, "target disk number")?;
    let raw = run_storage_powershell(&format!(
        r#"$ErrorActionPreference='Stop'; $p=Get-Partition -DiskNumber {disk_number} | Where-Object {{ ([string]$_.Guid).Trim('{{}}') -eq '{part_guid}' }}; if (-not $p) {{ throw 'journaled staging partition disappeared' }}; [string]$p.PartitionNumber"#,
        part_guid = ps_guid(part_guid),
    ))?;
    raw.trim()
        .parse()
        .map_err(|_| Error::Message(format!("invalid staging partition number: {raw}")))
}

fn collect_relative_files(root: &std::path::Path) -> Result<Vec<String>> {
    fn visit(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                visit(root, &path, out)?;
            } else if entry.file_type()?.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| Error::Message("mounted ISO path escaped its root".into()))?;
                out.push(format!(
                    "/{}",
                    relative.to_string_lossy().replace('\\', "/")
                ));
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

fn validate_esp_identity(journal: &StateJournal) -> Result<()> {
    let disk_number = required_u32(journal.target_disk_number, "target disk number")?;
    let disk_guid = required(&journal.target_disk_guid, "target disk GUID")?;
    let esp_guid = required(&journal.esp_partition_guid, "target ESP partition GUID")?;
    let esp_volume = required(&journal.esp_volume_guid, "target ESP volume GUID")?;
    run_storage_powershell(&format!(
        r#"$ErrorActionPreference='Stop'; $d=Get-Disk -Number {disk_number}; if (([string]$d.Guid).Trim('{{}}') -ne '{disk_guid}') {{ throw 'target disk GUID changed' }}; $p=Get-Partition -DiskNumber {disk_number} | Where-Object {{ ([string]$_.Guid).Trim('{{}}') -eq '{esp_guid}' }}; if (-not $p -or ([string]$p.GptType).Trim('{{}}') -ne 'c12a7328-f81f-11d2-ba4b-00a0c93ec93b') {{ throw 'journaled ESP is missing or no longer an ESP' }}; $v=Get-Volume -Partition $p; if ([string]$v.UniqueId -ne '{esp_volume}') {{ throw 'journaled ESP volume identity changed' }}; 'ok'"#,
        disk_guid = ps_guid(disk_guid),
        esp_guid = ps_guid(esp_guid),
        esp_volume = esp_volume.replace('\'', "''"),
    ))?;
    Ok(())
}

fn validate_staging_volume(
    journal: &StateJournal,
    part_guid: &str,
    volume_guid: &str,
    label: &str,
    filesystem: &str,
    hidden: bool,
) -> Result<()> {
    let disk_number = required_u32(journal.target_disk_number, "target disk number")?;
    let disk_guid = required(&journal.target_disk_guid, "target disk GUID")?;
    run_storage_powershell(&format!(
        r#"$ErrorActionPreference='Stop'; $d=Get-Disk -Number {disk_number}; if (([string]$d.Guid).Trim('{{}}') -ne '{disk_guid}') {{ throw 'target disk GUID changed' }}; $p=Get-Partition -DiskNumber {disk_number} | Where-Object {{ ([string]$_.Guid).Trim('{{}}') -eq '{part_guid}' }}; if (-not $p) {{ throw 'journaled staging partition disappeared' }}; $v=Get-Volume -Partition $p; if ([string]$v.UniqueId -ne '{volume_guid}' -or [string]$v.FileSystemLabel -ne '{label}' -or [string]$v.FileSystemType -ne '{filesystem}') {{ throw 'journaled staging volume identity changed' }}; if (-not $p.NoDefaultDriveLetter -or $p.DriveLetter) {{ throw 'staging volume automount hardening is missing' }}; if ({hidden} -and -not $p.IsHidden) {{ throw 'cidata hidden attribute is missing' }}; 'ok'"#,
        disk_guid = ps_guid(disk_guid),
        part_guid = ps_guid(part_guid),
        volume_guid = volume_guid.replace('\'', "''"),
        label = label.replace('\'', "''"),
        filesystem = filesystem.replace('\'', "''"),
        hidden = if hidden { "$true" } else { "$false" },
    ))?;
    Ok(())
}

pub fn write_cidata(mut identity: CidataIdentity) -> Result<CidataResult> {
    let journal = load_journal()?.ok_or_else(|| Error::Message("no state.json".into()))?;
    let guid = journal
        .cidata_guid
        .clone()
        .ok_or_else(|| Error::Message("cidata guid missing".into()))?;
    validate_staging_volume(
        &journal,
        required(&journal.cidata_partuuid, "cidata PARTUUID")?,
        &guid,
        "cidata",
        "FAT32",
        false,
    )?;
    let linux = journal
        .linux_device
        .clone()
        .or_else(|| {
            crate::platform::probe_machine()
                .ok()
                .and_then(|p| p.linux_by_id)
        })
        .ok_or_else(|| Error::Message("linux /dev/disk/by-id path missing".into()))?;
    let install_device = cidata::windows_vm_archinstall_device(&linux)?;
    let disk_bytes = crate::platform::probe_machine()?
        .disks
        .iter()
        .find(|d| d.is_boot)
        .map(|d| d.size_bytes)
        .unwrap_or(512 * probe::GIB);
    let encrypt = identity.encrypt;
    let files = cidata::build_cidata_files(&identity, install_device, disk_bytes)?;
    identity.password.clear();
    let root = volume_root(&guid)?;
    fs::write(
        format!("{root}user_configuration.json"),
        files.user_configuration.as_bytes(),
    )?;
    fs::write(
        format!("{root}user_credentials.json"),
        files.user_credentials.as_bytes(),
    )?;
    fs::write(
        format!("{root}user_encrypt_installation.txt"),
        files.user_encrypt_installation.as_bytes(),
    )?;
    if let Some(name) = files.user_full_name {
        fs::write(format!("{root}user_full_name.txt"), name.as_bytes())?;
    }
    if let Some(email) = files.user_email {
        fs::write(format!("{root}user_email_address.txt"), email.as_bytes())?;
    }
    let body = fs::read_to_string(journal_path()?)?;
    journal::refuse_password_field(
        &serde_json::from_str(&body).unwrap_or(serde_json::Value::Null),
    )?;
    Ok(CidataResult {
        cidata_guid: guid,
        linux_device: install_device.into(),
        encrypt,
    })
}

pub fn set_boot_next() -> Result<BootNextResult> {
    let mut journal = load_journal()?.ok_or_else(|| Error::Message("no state.json".into()))?;
    validate_esp_identity(&journal)?;
    let description = journal
        .boot_description
        .clone()
        .unwrap_or_else(|| format!("Omarchy Install {}", journal.operation_id));
    journal.boot_description = Some(description.clone());
    journal.pending_operation = Some(PendingOperation::CreateBootEntry);
    save_journal(&journal)?;

    let disk_number = required_u32(journal.target_disk_number, "target disk number")?;
    let esp_guid = required(&journal.esp_partition_guid, "target ESP partition GUID")?;
    let existing = journal.boot_id.as_deref().unwrap_or("");
    let raw = run_storage_powershell(&format!(
        r#"
$ErrorActionPreference='Stop'
$bcdedit=Join-Path ([Environment]::SystemDirectory) 'bcdedit.exe'
$p=Get-Partition -DiskNumber {disk_number} | Where-Object {{ ([string]$_.Guid).Trim('{{}}') -eq '{esp_guid}' }}
if (-not $p) {{ throw 'journaled ESP disappeared' }}
$assigned=$false
if (-not $p.DriveLetter) {{ $p | Add-PartitionAccessPath -AssignDriveLetter; $assigned=$true; $p=Get-Partition -DiskNumber {disk_number} -PartitionNumber $p.PartitionNumber }}
try {{
  $id='{existing}'
  if (-not $id) {{
    $all=(& $bcdedit /enum firmware /v | Out-String)
    foreach ($block in ($all -split '(?:\r?\n){{2,}}')) {{ if ($block -match [regex]::Escape('{description}')) {{ $m=[regex]::Match($block,'\{{[0-9a-fA-F-]+\}}'); if ($m.Success) {{ $id=$m.Value; break }} }} }}
  }}
  if (-not $id) {{ $created=(& $bcdedit /copy '{{bootmgr}}' /d '{description}' | Out-String); if ($LASTEXITCODE -ne 0) {{ throw "bcdedit copy failed: $created" }}; $m=[regex]::Match($created,'\{{[0-9a-fA-F-]+\}}'); if (-not $m.Success) {{ throw "bcdedit did not return a firmware identifier: $created" }}; $id=$m.Value }}
  $device="partition=$($p.DriveLetter):"
  $setDevice=(& $bcdedit /set $id device $device | Out-String); if ($LASTEXITCODE -ne 0) {{ throw "bcdedit device failed: $setDevice" }}
  $setPath=(& $bcdedit /set $id path '\EFI\OmarchyInstall\BOOTX64.EFI' | Out-String); if ($LASTEXITCODE -ne 0) {{ throw "bcdedit path failed: $setPath" }}
  $entry=(& $bcdedit /enum $id /v | Out-String); if ($LASTEXITCODE -ne 0 -or $entry -notmatch [regex]::Escape('\EFI\OmarchyInstall\BOOTX64.EFI') -or $entry -notmatch 'partition=') {{ throw 'firmware entry validation failed' }}
  @{{ id=$id }} | ConvertTo-Json -Compress
}} finally {{
  if ($assigned) {{ Remove-PartitionAccessPath -DiskNumber {disk_number} -PartitionNumber $p.PartitionNumber -AccessPath ($p.DriveLetter + ':\') -ErrorAction SilentlyContinue }}
}}
"#,
        esp_guid = ps_guid(esp_guid),
        existing = existing.replace('\'', "''"),
        description = description.replace('\'', "''"),
    ))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| Error::Message(format!("firmware entry json: {e} {raw}")))?;
    let boot_id = json_str(&value, "id")?.to_string();
    journal.boot_id = Some(boot_id.clone());
    journal.pending_operation = None;
    journal.step = JournalStep::BootEntryCreated;
    save_journal(&journal)?;

    journal.pending_operation = Some(PendingOperation::SetBootNext);
    save_journal(&journal)?;
    run_storage_powershell(&format!(
        r#"$ErrorActionPreference='Stop'; $bcdedit=Join-Path ([Environment]::SystemDirectory) 'bcdedit.exe'; $out=(& $bcdedit /set '{{fwbootmgr}}' bootsequence '{}' | Out-String); if ($LASTEXITCODE -ne 0) {{ throw "bcdedit bootsequence failed: $out" }}; 'ok'"#,
        boot_id.replace('\'', "''")
    ))?;
    journal.pending_operation = None;
    journal.step = JournalStep::BootNextSet;
    save_journal(&journal)?;
    Ok(BootNextResult {
        boot_id: boot_id.clone(),
        bcd_firmware_id: Some(boot_id),
        appended_boot_order: false,
    })
}

pub fn reboot_to_installer() -> Result<()> {
    let status = system_command(SystemTool::Shutdown)?
        .args(["/r", "/t", "0"])
        .status()?;
    if !status.success() {
        return Err(Error::Message("shutdown /r failed".into()));
    }
    Ok(())
}

pub fn abort_and_rollback() -> Result<RollbackResult> {
    let Some(mut journal) = load_journal()? else {
        return Ok(RollbackResult {
            removed_partition: false,
            extended_ntfs: false,
            restored_power_settings: false,
        });
    };
    journal.pending_operation = Some(PendingOperation::Rollback);
    save_journal(&journal)?;
    let _rollback_files = grub::esp_rollback_relpaths(journal.search_filename.as_deref())?;
    let bait_win = journal
        .search_filename
        .as_deref()
        .map(grub::esp_bait_windows_path)
        .transpose()?;
    let old_c = journal.old_c_size_bytes.unwrap_or(0);
    let restore_hiber = journal.hibernation_disabled_by_us;
    let bait_remove = bait_win
        .as_ref()
        .map(|p| format!("Remove-Item -Force ($root + '{p}') -ErrorAction SilentlyContinue"))
        .unwrap_or_default();
    let disk_number = required_u32(journal.target_disk_number, "target disk number")?;
    let disk_guid = required(&journal.target_disk_guid, "target disk GUID")?;
    let windows_guid = required(&journal.windows_partition_guid, "Windows partition GUID")?;
    let om_guid = journal.omarchyinst_partuuid.as_deref().unwrap_or("");
    let ci_guid = journal.cidata_partuuid.as_deref().unwrap_or("");
    let om_offset = journal.omarchyinst_offset_bytes.unwrap_or(0);
    let om_size = journal.omarchyinst_size_bytes.unwrap_or(0);
    let ci_offset = journal.cidata_offset_bytes.unwrap_or(0);
    let ci_size = journal.cidata_size_bytes.unwrap_or(0);
    let esp_root = journal.esp_volume_guid.as_deref().unwrap_or("");
    let boot_id = journal.boot_id.as_deref().unwrap_or("");
    let boot_description = journal.boot_description.as_deref().unwrap_or("");
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$bcdedit=Join-Path ([Environment]::SystemDirectory) 'bcdedit.exe'
$d=Get-Disk -Number {disk_number}
if (([string]$d.Guid).Trim('{{}}') -ne '{disk_guid}') {{ throw 'target disk GUID changed; refusing rollback' }}
$bootId='{boot_id}'
if (-not $bootId -and '{boot_description}') {{
  $all=(& $bcdedit /enum firmware /v | Out-String)
  foreach ($block in ($all -split '(?:\r?\n){{2,}}')) {{ if ($block -match [regex]::Escape('{boot_description}')) {{ $m=[regex]::Match($block,'\{{[0-9a-fA-F-]+\}}'); if ($m.Success) {{ $bootId=$m.Value; break }} }} }}
}}
if ($bootId) {{
  $fw=(& $bcdedit /enum '{{fwbootmgr}}' /v | Out-String)
  $sequence=[regex]::Match($fw,'(?im)^bootsequence\s+(\{{[0-9a-fA-F-]+\}}(?:\r?\n\s+\{{[0-9a-fA-F-]+\}})*)')
  if ($sequence.Success -and $sequence.Value -match [regex]::Escape($bootId)) {{ $clear=(& $bcdedit /deletevalue '{{fwbootmgr}}' bootsequence | Out-String); if ($LASTEXITCODE -ne 0) {{ throw "could not clear our boot sequence: $clear" }} }}
  $entry=(& $bcdedit /enum $bootId /v 2>&1 | Out-String)
  if ($LASTEXITCODE -eq 0) {{ $del=(& $bcdedit /delete $bootId /cleanup | Out-String); if ($LASTEXITCODE -ne 0) {{ throw "could not delete firmware entry: $del" }} }}
}}
if ('{esp_root}') {{
  $esp=Get-Partition -DiskNumber {disk_number} | Where-Object {{ ([string]$_.Guid).Trim('{{}}') -eq '{esp_partition_guid}' }}
  if (-not $esp -or ([string]$esp.GptType).Trim('{{}}') -ne 'c12a7328-f81f-11d2-ba4b-00a0c93ec93b') {{ throw 'journaled ESP identity mismatch' }}
  $espVol=Get-Volume -Partition $esp
  if ([string]$espVol.UniqueId -ne '{esp_root}') {{ throw 'journaled ESP volume mismatch' }}
  $root='{esp_root}'
  Remove-Item -Recurse -Force ($root + 'EFI\OmarchyInstall') -ErrorAction SilentlyContinue
  Remove-Item -Force ($root + 'boot\grub\grub.cfg') -ErrorAction SilentlyContinue
  {bait_remove}
}}
function Remove-Staged([string]$guid,[uint64]$offset,[uint64]$size,[string]$label) {{
  $matches=@()
  if ($guid) {{ $matches=@(Get-Partition -DiskNumber {disk_number} | Where-Object {{ ([string]$_.Guid).Trim('{{}}') -eq $guid.Trim('{{}}') }}) }}
  elseif ($offset -gt 0) {{ $matches=@(Get-Partition -DiskNumber {disk_number} | Where-Object {{ [uint64]$_.Offset -eq $offset }}) }}
  if ($matches.Count -gt 1) {{ throw "ambiguous rollback target for $label" }}
  if ($matches.Count -eq 1) {{
    $p=$matches[0]
    if ($size -gt 0 -and [uint64]$p.Size -ne $size) {{ throw "rollback size mismatch for $label" }}
    if (([string]$p.GptType).Trim('{{}}') -ne 'ebd0a0a2-b9e5-4433-87c0-68b6b72699c7') {{ throw "rollback type mismatch for $label" }}
    $v=$null; try {{ $v=Get-Volume -Partition $p -ErrorAction Stop }} catch {{}}
    if ($v -and [string]$v.FileSystemLabel -ne $label -and -not ($label -eq 'cidata' -and [string]$v.FileSystemLabel -eq 'CIDATA')) {{ throw "rollback label mismatch for $label" }}
    Remove-Partition -DiskNumber {disk_number} -PartitionNumber $p.PartitionNumber -Confirm:$false
  }}
}}
Remove-Staged '{ci_guid}' ([uint64]{ci_offset}) ([uint64]{ci_size}) 'cidata'
Remove-Staged '{om_guid}' ([uint64]{om_offset}) ([uint64]{om_size}) 'OMARCHYINST'
if ({old_c} -gt 0) {{
  $c=Get-Partition -DiskNumber {disk_number} | Where-Object {{ ([string]$_.Guid).Trim('{{}}') -eq '{windows_guid}' }}
  if (-not $c) {{ throw 'journaled Windows partition disappeared' }}
  if ([uint64]$c.Size -lt [uint64]{old_c}) {{ $supported=Get-PartitionSupportedSize -DiskNumber {disk_number} -PartitionNumber $c.PartitionNumber; if ([uint64]{old_c} -gt [uint64]$supported.SizeMax) {{ throw 'Windows partition cannot be restored to its old size' }}; Resize-Partition -DiskNumber {disk_number} -PartitionNumber $c.PartitionNumber -Size ([uint64]{old_c}) }}
  elseif ([uint64]$c.Size -ne [uint64]{old_c}) {{ throw 'Windows partition is larger than its journaled original size' }}
}}
Write-Output 'ok'
"#,
        disk_guid = ps_guid(disk_guid),
        windows_guid = ps_guid(windows_guid),
        om_guid = ps_guid(om_guid),
        ci_guid = ps_guid(ci_guid),
        esp_root = esp_root.replace('\'', "''"),
        esp_partition_guid = ps_guid(journal.esp_partition_guid.as_deref().unwrap_or("")),
        boot_id = boot_id.replace('\'', "''"),
        boot_description = boot_description.replace('\'', "''"),
        bait_remove = bait_remove,
        om_offset = om_offset,
        om_size = om_size,
        ci_offset = ci_offset,
        ci_size = ci_size,
        old_c = old_c,
    );
    let out = run_storage_powershell(&script)?;
    let restored_power = restore_hiber || journal.hiberboot_was.is_some();
    let result = interpret_rollback_output(true, &out, old_c, restored_power)?;
    if restore_hiber {
        run_powercfg("on")?;
    }
    if let Some(value) = journal.hiberboot_was {
        set_hklm_dword(POWER_SESSION_KEY, HIBERBOOT_ENABLED, value)?;
    }
    let path = journal_path()?;
    fs::remove_file(path).ok();
    Ok(result)
}

pub fn export_support_bundle() -> Result<PathBuf> {
    let dir = local_app_dir()?;
    let zip_path = dir.join("support-bundle.zip");
    let file = fs::File::create(&zip_path)?;
    let mut zip = ZipWriter::new(file);
    let opts = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    if let Ok(Some(j)) = load_journal() {
        let body = serialize_journal(&j)?;
        let redacted = journal::redact_journal_json(&body)?;
        zip.start_file("state.json", opts)?;
        zip.write_all(redacted.as_bytes())?;
    }
    if let Ok(probe) = crate::platform::probe_machine() {
        zip.start_file("probe.json", opts)?;
        zip.write_all(
            serde_json::to_vec_pretty(&probe)
                .unwrap_or_default()
                .as_slice(),
        )?;
    }
    zip.start_file("host.txt", opts)?;
    zip.write_all(format!("{:?}", host_info()?).as_bytes())?;
    for (name, tool, args) in [
        (
            "firmware-bcd.txt",
            SystemTool::BcdEdit,
            vec!["/enum", "firmware", "/v"],
        ),
        (
            "bitlocker-manage-bde.txt",
            SystemTool::ManageBde,
            vec!["-status"],
        ),
    ] {
        zip.start_file(name, opts)?;
        zip.write_all(diagnostic_output(tool, &args).as_bytes())?;
    }
    zip.start_file("bitlocker-wmi.json", opts)?;
    zip.write_all(
        diagnostic_output(
            SystemTool::PowerShell,
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-CimInstance -Namespace 'root/cimv2/Security/MicrosoftVolumeEncryption' -ClassName Win32_EncryptableVolume | Select-Object DeviceID,DriveLetter,ProtectionStatus,ConversionStatus | ConvertTo-Json -Depth 4",
            ],
        )
        .as_bytes(),
    )?;
    let logs = paths::install_logs_dir().unwrap_or_else(|_| dir.join("logs"));
    if logs.is_dir() {
        if let Ok(rd) = fs::read_dir(&logs) {
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_file() {
                    if let Ok(mut f) = fs::File::open(&p) {
                        zip.start_file(
                            format!("logs/{}", ent.file_name().to_string_lossy()),
                            opts,
                        )?;
                        let mut buf = Vec::new();
                        let _ = f.read_to_end(&mut buf);
                        zip.write_all(&buf)?;
                    }
                }
            }
        }
    }
    zip.finish()?;
    Ok(zip_path)
}

fn diagnostic_output(tool: SystemTool, args: &[&str]) -> String {
    match system_command(tool).and_then(|mut command| Ok(command.args(args).output()?)) {
        Ok(output) => format!(
            "exit: {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) => format!("failed to execute Windows system tool: {error}"),
    }
}

fn run_powercfg(state: &str) -> Result<()> {
    let output = system_command(SystemTool::PowerCfg)?
        .args(["/h", state])
        .output()?;
    if !output.status.success() {
        return Err(Error::Message(format!(
            "powercfg /h {state} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounted_tree_paths_use_iso_separators() {
        let root = std::env::temp_dir().join(format!(
            "omarchy-mounted-tree-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let boot = root.join("boot");
        fs::create_dir_all(&boot).unwrap();
        fs::write(boot.join("deadbeef.uuid"), []).unwrap();

        let paths = collect_relative_files(&root).unwrap();
        assert_eq!(paths, ["/boot/deadbeef.uuid"]);

        fs::remove_dir_all(root).unwrap();
    }
}
