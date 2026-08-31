//! Non-Windows host used for UI and IPC development on macOS/Linux.
//!
//! Mutate IPC is a **dry run**: fake success, optional journal under
//! `$XDG_DATA_HOME/OmarchyInstall` (default `~/.local/share/OmarchyInstall`).
//! Never shrinks disks, never writes EFI, never reboots.

use super::{
    BitlockerVolume, BootNextResult, CidataResult, DiskMap, HostInfo, JournalStep, MachineProbe,
    PartitionMap, PrepareResult, RollbackResult, StageResult, StateJournal, TargetEsp,
};
use crate::cidata::CidataIdentity;
use crate::download::{self, STUB_ISO_BYTES};
use crate::error::{Error, Result};
use crate::grub::emit_grub_cfg;
use crate::journal::{empty_journal, parse_journal, serialize_journal};
use crate::partition;
use crate::paths;
use crate::probe::{self, GIB, MIB};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use zip::write::FileOptions;
use zip::ZipWriter;

const STUB_OMARCHYINST_GUID: &str = r"\\?\Volume{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\";
const STUB_OMARCHYINST_PARTUUID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const STUB_CIDATA_GUID: &str = r"\\?\Volume{bbbbbbbb-cccc-4ddd-8eee-ffffffffffff}\";
const STUB_ESP_GUID: &str = r"\\?\Volume{11111111-1111-1111-1111-111111111111}\";
const STUB_SEARCH: &str = "/boot/cafef00d.uuid";
const STUB_BOOT_ID: &str = "{00000000-0000-0000-0000-0000000000aa}";
const STUB_C_SIZE: u64 = 400 * GIB;

pub fn host_info() -> Result<HostInfo> {
    Ok(HostInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        elevated: false,
        native_windows: false,
        os_version: None,
    })
}

pub fn probe_machine() -> Result<MachineProbe> {
    let mut probe = healthy_fixture();
    let blocks = stub_blocks_from_env();
    apply_stub_blocks(&mut probe, &blocks);
    let check_elevation = blocks.iter().any(|b| b == "not-elevated");
    Ok(probe::attach_reasons(probe, check_elevation))
}

pub fn relaunch_elevated() -> Result<()> {
    Err(crate::error::Error::WindowsOnly)
}

pub fn reboot_to_firmware() -> Result<()> {
    Err(crate::error::Error::WindowsOnly)
}

fn journal_path() -> Result<PathBuf> {
    Ok(paths::install_data_dir()?.join("state.json"))
}

fn stub_dir() -> Result<PathBuf> {
    Ok(paths::install_data_dir()?.join("stub"))
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
    crate::journal::save_atomic(&journal_path()?, journal)
}

fn dry_run_pause() {
    if cfg!(test) {
        return;
    }
    std::thread::sleep(Duration::from_millis(280));
}

pub fn load_install_state() -> Result<Option<StateJournal>> {
    load_journal()
}

pub fn prepare_installer_partition() -> Result<PrepareResult> {
    dry_run_pause();
    if let Some(journal) = load_journal()? {
        if let (Some(om), Some(partuuid), Some(ci), Some(old_c)) = (
            journal.omarchyinst_guid.clone(),
            journal.omarchyinst_partuuid.clone(),
            journal.cidata_guid.clone(),
            journal.old_c_size_bytes,
        ) {
            if !om.is_empty() && !ci.is_empty() {
                let partition_bytes = partition::omarchyinst_bytes(STUB_ISO_BYTES);
                let hole = partition::installer_hole_bytes(STUB_ISO_BYTES);
                log::info!("dry-run: prepare skipped (journal already has volumes)");
                return Ok(PrepareResult {
                    omarchyinst_guid: om,
                    omarchyinst_partuuid: partuuid,
                    cidata_guid: ci,
                    old_c_size_bytes: old_c,
                    new_c_size_bytes: old_c.saturating_sub(hole),
                    partition_bytes,
                });
            }
        }
    }
    let probe = probe_machine()?;
    if !probe.blocking_reasons.is_empty() {
        return Err(Error::Message(
            "machine probe has blocking reasons; refuse to shrink".into(),
        ));
    }
    let partition_bytes = partition::omarchyinst_bytes(STUB_ISO_BYTES);
    let hole = partition::installer_hole_bytes(STUB_ISO_BYTES);
    partition::iso_fits_omarchyinst(STUB_ISO_BYTES, partition_bytes)?;
    let old_c = probe
        .disks
        .iter()
        .find(|d| d.is_boot)
        .and_then(|d| {
            d.partitions
                .iter()
                .find(|p| p.letter.as_deref() == Some("C:"))
                .map(|p| p.size_bytes)
        })
        .unwrap_or(STUB_C_SIZE);
    let new_c = old_c.saturating_sub(hole);
    let mut journal = load_journal()?.unwrap_or_else(empty_journal);
    let esp = probe.target_esp.as_ref().expect("stub ESP");
    journal.target_disk_guid = Some(esp.disk_guid.clone());
    journal.target_disk_number = Some(esp.disk_number);
    journal.windows_partition_guid = Some("{33333333-3333-3333-3333-333333333333}".into());
    journal.windows_partition_offset_bytes = Some(117 * MIB);
    journal.new_c_size_bytes = Some(new_c);
    journal.omarchyinst_offset_bytes = Some(117 * MIB + new_c);
    journal.omarchyinst_size_bytes = Some(partition_bytes);
    journal.omarchyinst_guid = Some(STUB_OMARCHYINST_GUID.into());
    journal.omarchyinst_partuuid = Some(STUB_OMARCHYINST_PARTUUID.into());
    journal.cidata_guid = Some(STUB_CIDATA_GUID.into());
    journal.cidata_partuuid = Some("bbbbbbbb-cccc-4ddd-8eee-ffffffffffff".into());
    journal.cidata_offset_bytes = Some(117 * MIB + new_c + partition_bytes);
    journal.cidata_size_bytes = Some(partition::CIDATA_BYTES);
    journal.esp_partition_guid = Some(esp.partition_guid.clone());
    journal.esp_volume_guid = Some(esp.volume_guid.clone());
    journal.old_c_size_bytes = Some(old_c);
    journal.linux_device = probe.linux_by_id.clone();
    journal.iso_sha256 = Some(download::stub_iso_sha256());
    journal.step = JournalStep::CidataPartitionCreated;
    save_journal(&journal)?;
    log::info!("dry-run: prepare_installer_partition (no disk writes)");
    Ok(PrepareResult {
        omarchyinst_guid: STUB_OMARCHYINST_GUID.into(),
        omarchyinst_partuuid: STUB_OMARCHYINST_PARTUUID.into(),
        cidata_guid: STUB_CIDATA_GUID.into(),
        old_c_size_bytes: old_c,
        new_c_size_bytes: new_c,
        partition_bytes,
    })
}

pub fn stage_bootloader() -> Result<StageResult> {
    dry_run_pause();
    let mut journal = load_journal()?.ok_or_else(|| Error::Message("no state.json".into()))?;
    let cfg = emit_grub_cfg(STUB_OMARCHYINST_PARTUUID, STUB_ISO_BYTES);
    let mut hasher = Sha256::new();
    hasher.update(cfg.as_bytes());
    let grub_cfg_sha256 = format!("{:x}", hasher.finalize());
    let grub_path = stub_dir()?.join("esp").join("boot").join("grub");
    fs::create_dir_all(&grub_path)?;
    fs::write(grub_path.join("grub.cfg"), cfg.as_bytes())?;
    journal.search_filename = Some(STUB_SEARCH.into());
    journal.step = JournalStep::Staged;
    save_journal(&journal)?;
    log::info!("dry-run: stage_bootloader (wrote stub grub.cfg only)");
    Ok(StageResult {
        esp_guid: STUB_ESP_GUID.into(),
        search_filename: STUB_SEARCH.into(),
        grub_cfg_sha256,
    })
}

pub fn write_cidata(mut identity: CidataIdentity) -> Result<CidataResult> {
    dry_run_pause();
    let journal = load_journal()?.ok_or_else(|| Error::Message("no state.json".into()))?;
    let guid = journal
        .cidata_guid
        .clone()
        .ok_or_else(|| Error::Message("cidata guid missing".into()))?;
    let linux = journal
        .linux_device
        .clone()
        .or_else(|| probe_machine().ok().and_then(|p| p.linux_by_id))
        .ok_or_else(|| Error::Message("linux /dev/disk/by-id path missing".into()))?;
    crate::cidata::assert_linux_by_id(&linux)?;
    let disk_bytes = probe_machine()?
        .disks
        .iter()
        .find(|d| d.is_boot)
        .map(|d| d.size_bytes)
        .unwrap_or(512 * GIB);
    let encrypt = identity.encrypt;
    let files = crate::cidata::build_cidata_files(&identity, &linux, disk_bytes)?;
    identity.password.clear();
    let dir = stub_dir()?.join("cidata");
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("user_configuration.json"),
        files.user_configuration.as_bytes(),
    )?;
    fs::write(
        dir.join("user_credentials.json"),
        files.user_credentials.as_bytes(),
    )?;
    fs::write(
        dir.join("user_encrypt_installation.txt"),
        files.user_encrypt_installation.as_bytes(),
    )?;
    if let Some(name) = files.user_full_name {
        fs::write(dir.join("user_full_name.txt"), name.as_bytes())?;
    }
    if let Some(email) = files.user_email {
        fs::write(dir.join("user_email_address.txt"), email.as_bytes())?;
    }
    let body = fs::read_to_string(journal_path()?)?;
    crate::journal::refuse_password_field(
        &serde_json::from_str(&body).unwrap_or(serde_json::Value::Null),
    )?;
    log::info!("dry-run: write_cidata (app data only)");
    Ok(CidataResult {
        cidata_guid: guid,
        linux_device: linux,
        encrypt,
    })
}

pub fn set_boot_next() -> Result<BootNextResult> {
    dry_run_pause();
    let mut journal = load_journal()?.ok_or_else(|| Error::Message("no state.json".into()))?;
    if let Some(id) = journal.boot_id.clone().filter(|s| !s.is_empty()) {
        journal.step = JournalStep::BootNextSet;
        save_journal(&journal)?;
        log::info!("dry-run: set_boot_next reused {id}");
        return Ok(BootNextResult {
            boot_id: id,
            bcd_firmware_id: None,
            appended_boot_order: false,
        });
    }
    journal.boot_id = Some(STUB_BOOT_ID.into());
    journal.boot_description = Some(format!("Omarchy Install {}", journal.operation_id));
    journal.step = JournalStep::BootNextSet;
    save_journal(&journal)?;
    log::info!("dry-run: set_boot_next (NVRAM not written)");
    Ok(BootNextResult {
        boot_id: STUB_BOOT_ID.into(),
        bcd_firmware_id: None,
        appended_boot_order: false,
    })
}

pub fn reboot_to_installer() -> Result<()> {
    dry_run_pause();
    log::info!("dry-run: reboot_to_installer skipped");
    Ok(())
}

pub fn abort_and_rollback() -> Result<RollbackResult> {
    dry_run_pause();
    let had = load_journal()?.is_some();
    if let Ok(path) = journal_path() {
        let _ = fs::remove_file(path);
    }
    if let Ok(dir) = stub_dir() {
        let _ = fs::remove_dir_all(dir);
    }
    log::info!("dry-run: abort_and_rollback (removed stub journal)");
    Ok(RollbackResult {
        removed_partition: had,
        extended_ntfs: false,
        restored_power_settings: false,
    })
}

pub fn export_support_bundle() -> Result<PathBuf> {
    let dir = paths::install_data_dir()?;
    let zip_path = dir.join("support-bundle.zip");
    let file = fs::File::create(&zip_path)?;
    let mut zip = ZipWriter::new(file);
    let opts = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("dry-run.txt", opts)
        .map_err(|e| Error::Message(format!("zip: {e}")))?;
    zip.write_all(b"omarchy-install stub host: no disk or EFI mutation\n")?;
    if let Ok(Some(j)) = load_journal() {
        let body = serialize_journal(&j)?;
        let redacted = crate::journal::redact_journal_json(&body)?;
        zip.start_file("state.json", opts)
            .map_err(|e| Error::Message(format!("zip: {e}")))?;
        zip.write_all(redacted.as_bytes())?;
    }
    if let Ok(probe) = probe_machine() {
        zip.start_file("probe.json", opts)
            .map_err(|e| Error::Message(format!("zip: {e}")))?;
        zip.write_all(
            serde_json::to_vec_pretty(&probe)
                .unwrap_or_default()
                .as_slice(),
        )?;
    }
    zip.finish()
        .map_err(|e| Error::Message(format!("zip: {e}")))?;
    Ok(zip_path)
}

fn healthy_fixture() -> MachineProbe {
    let c_size = 400 * GIB;
    MachineProbe {
        host: host_info().expect("stub host"),
        uefi: true,
        secure_boot: false,
        efi_vars_writable: true,
        ram_installed_bytes: 16 * GIB,
        ram_total_phys_bytes: 15 * GIB + 512 * MIB,
        ram_avail_bytes: 6 * GIB,
        ram_ok_for_copytoram: true,
        tpm_present: true,
        recommended_disk_id: Some(r"\\.\PHYSICALDRIVE0".into()),
        target_esp: Some(TargetEsp {
            disk_id: r"\\.\PHYSICALDRIVE0".into(),
            disk_guid: "{dddddddd-dddd-4ddd-8ddd-dddddddddddd}".into(),
            disk_number: 0,
            partition_guid: "{11111111-1111-1111-1111-111111111111}".into(),
            volume_guid: STUB_ESP_GUID.into(),
        }),
        linux_by_id: Some("/dev/disk/by-id/nvme-VENDOR_DISK_1234".into()),
        bitlocker: vec![BitlockerVolume {
            device_id: Some(r"\\?\Volume{33333333-3333-3333-3333-333333333333}\".into()),
            disk_id: Some(r"\\.\PHYSICALDRIVE0".into()),
            mount: Some("C:".into()),
            protection_status: 0,
            conversion_status: 0,
            fully_decrypted: true,
        }],
        disks: vec![DiskMap {
            device_id: r"\\.\PHYSICALDRIVE0".into(),
            size_bytes: 512 * GIB,
            partition_style: "gpt".into(),
            bus: Some("NVMe".into()),
            is_boot: true,
            is_rst: false,
            is_dynamic: false,
            is_storage_spaces: false,
            max_shrink_bytes: Some(80 * GIB),
            partitions: vec![
                PartitionMap {
                    gpt_guid: Some("{11111111-1111-1111-1111-111111111111}".into()),
                    type_guid: Some("{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}".into()),
                    letter: None,
                    label: Some("SYSTEM".into()),
                    size_bytes: 100 * MIB,
                    offset_bytes: MIB,
                    fs: Some("fat32".into()),
                },
                PartitionMap {
                    gpt_guid: Some("{22222222-2222-2222-2222-222222222222}".into()),
                    type_guid: Some("{e3c9e316-0b5c-4db8-817d-f92df00215ae}".into()),
                    letter: None,
                    label: None,
                    size_bytes: 16 * MIB,
                    offset_bytes: 101 * MIB,
                    fs: None,
                },
                PartitionMap {
                    gpt_guid: Some("{33333333-3333-3333-3333-333333333333}".into()),
                    type_guid: Some("{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}".into()),
                    letter: Some("C:".into()),
                    label: Some("Windows".into()),
                    size_bytes: c_size,
                    offset_bytes: 117 * MIB,
                    fs: Some("ntfs".into()),
                },
                PartitionMap {
                    gpt_guid: Some("{44444444-4444-4444-4444-444444444444}".into()),
                    type_guid: Some("{de94bba4-06d1-4d40-a16a-bfd50179d6ac}".into()),
                    letter: None,
                    label: Some("WinRE".into()),
                    size_bytes: 800 * MIB,
                    offset_bytes: 117 * MIB + c_size,
                    fs: Some("ntfs".into()),
                },
            ],
        }],
        blocking_reasons: vec![],
    }
}

fn stub_blocks_from_env() -> Vec<String> {
    match std::env::var("OMARCHY_STUB_BLOCKS") {
        Ok(s) => parse_stub_blocks(&s),
        Err(_) => Vec::new(),
    }
}

pub fn parse_stub_blocks(s: &str) -> Vec<String> {
    s.split(',')
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

pub fn apply_stub_blocks(probe: &mut MachineProbe, blocks: &[String]) {
    for block in blocks {
        match block.as_str() {
            "secure-boot" | "secureboot" => probe.secure_boot = true,
            "rst" => {
                if let Some(d) = probe.disks.first_mut() {
                    d.is_rst = true;
                }
            }
            "ram" => {
                probe.ram_installed_bytes = 8 * GIB;
                probe.ram_total_phys_bytes = 7 * GIB;
            }
            "not-uefi" | "bios" => probe.uefi = false,
            "bitlocker" => {
                if let Some(v) = probe.bitlocker.first_mut() {
                    v.fully_decrypted = false;
                    v.protection_status = 1;
                    v.conversion_status = 1;
                }
            }
            "efi-vars" | "efivarslocked" => probe.efi_vars_writable = false,
            "not-elevated" => probe.host.elevated = false,
            "dynamic" => {
                if let Some(d) = probe.disks.first_mut() {
                    d.is_dynamic = true;
                }
            }
            "storage-spaces" | "spaces" => {
                if let Some(d) = probe.disks.first_mut() {
                    d.is_storage_spaces = true;
                }
            }
            "shrink" => {
                if let Some(d) = probe.disks.first_mut() {
                    d.max_shrink_bytes = Some(GIB);
                }
            }
            "not-gpt" | "mbr" => {
                if let Some(d) = probe.disks.first_mut() {
                    d.partition_style = "mbr".into();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::BlockingReason;

    #[test]
    fn default_stub_is_walkable() {
        let p = probe_machine().expect("probe");
        assert!(!p.host.native_windows);
        assert!(!p.host.elevated);
        assert!(p.blocking_reasons.is_empty(), "{:?}", p.blocking_reasons);
        assert!(p.ram_ok_for_copytoram);
        assert!(p.uefi);
        assert!(!p.secure_boot);
        assert!(p.tpm_present);
    }

    #[test]
    fn stub_blocks_inject_reasons() {
        let mut p = healthy_fixture();
        apply_stub_blocks(&mut p, &parse_stub_blocks("secure-boot,rst,ram"));
        let p = probe::attach_reasons(p, false);
        assert!(p
            .blocking_reasons
            .iter()
            .any(|b| matches!(b, BlockingReason::SecureBoot)));
        assert!(p
            .blocking_reasons
            .iter()
            .any(|b| matches!(b, BlockingReason::Rst { .. })));
        assert!(p
            .blocking_reasons
            .iter()
            .any(|b| matches!(b, BlockingReason::Ram { .. })));
        assert!(!p.ram_ok_for_copytoram);
    }

    #[test]
    fn firmware_and_elevation_stay_windows_only() {
        assert!(matches!(
            relaunch_elevated(),
            Err(crate::error::Error::WindowsOnly)
        ));
        assert!(matches!(
            reboot_to_firmware(),
            Err(crate::error::Error::WindowsOnly)
        ));
    }

    #[test]
    fn dry_run_walks_full_mutate_without_touching_the_host() {
        let prev_data = std::env::var_os("XDG_DATA_HOME");
        let home = std::env::temp_dir().join(format!(
            "omarchyinstall-stub-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&home).unwrap();
        std::env::set_var("XDG_DATA_HOME", &home);

        let prepared = prepare_installer_partition().expect("prepare");
        assert_eq!(prepared.omarchyinst_partuuid, STUB_OMARCHYINST_PARTUUID);
        assert!(prepared.partition_bytes >= 8 * GIB);

        let staged = stage_bootloader().expect("stage");
        assert_eq!(staged.search_filename, STUB_SEARCH);
        assert_eq!(staged.grub_cfg_sha256.len(), 64);
        let grub = home
            .join("OmarchyInstall")
            .join("stub")
            .join("esp")
            .join("boot")
            .join("grub")
            .join("grub.cfg");
        let cfg = fs::read_to_string(&grub).unwrap();
        assert!(cfg.contains("copytoram=y"));
        assert!(cfg.contains(STUB_OMARCHYINST_PARTUUID));

        let cidata = write_cidata(CidataIdentity {
            username: "dryrun".into(),
            password: "vector-pass".into(),
            hostname: "box".into(),
            timezone: "UTC".into(),
            keyboard: "us".into(),
            encrypt: false,
            full_name: None,
            email: None,
        })
        .expect("cidata");
        assert!(cidata.linux_device.starts_with("/dev/disk/by-id/"));
        assert!(!cidata.encrypt);
        let creds = fs::read_to_string(
            home.join("OmarchyInstall")
                .join("stub")
                .join("cidata")
                .join("user_credentials.json"),
        )
        .unwrap();
        assert!(creds.contains("$6$"));
        assert!(!creds.contains("vector-pass"));

        let boot = set_boot_next().expect("bootnext");
        assert_eq!(boot.boot_id, STUB_BOOT_ID);
        reboot_to_installer().expect("reboot is a no-op");

        let state = load_install_state().unwrap().expect("journal");
        assert!(matches!(state.step, JournalStep::BootNextSet));
        assert!(!serialize_journal(&state).unwrap().contains("password"));

        let bundle = export_support_bundle().expect("bundle");
        assert!(bundle.exists());

        let prepared_again = prepare_installer_partition().expect("prepare idempotent");
        assert_eq!(prepared_again.omarchyinst_guid, prepared.omarchyinst_guid);
        let boot_again = set_boot_next().expect("bootnext idempotent");
        assert_eq!(boot_again.boot_id, STUB_BOOT_ID);

        let rb = abort_and_rollback().expect("rollback");
        assert!(rb.removed_partition);
        assert!(!rb.extended_ntfs);
        assert!(load_install_state().unwrap().is_none());
        assert!(!home.join("OmarchyInstall").join("stub").exists());

        match prev_data {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }
}
