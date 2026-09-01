//! Pure probe policy. No Win32 — unit-tested on every host.

use crate::platform::{BlockingReason, MachineProbe};

/// 12 GiB installed. Machines below the 14 GiB recommendation get a UI warning.
pub const RAM_INSTALLED_MIN: u64 = 12 * GIB;
/// Allow up to a 2 GiB firmware/iGPU carve-out on a 12 GiB machine.
pub const RAM_TOTAL_PHYS_MIN: u64 = 10 * GIB;

pub const GIB: u64 = 1024 * 1024 * 1024;
pub const MIB: u64 = 1024 * 1024;

/// Default hole we need to shrink: 8 GiB `OMARCHYINST` + 64 MiB `cidata`.
pub const INSTALLER_HOLE_BYTES: u64 = 8 * GIB + 64 * MIB;

pub fn ram_ok_for_copytoram(installed: u64, total_phys: u64) -> bool {
    installed >= RAM_INSTALLED_MIN && total_phys >= RAM_TOTAL_PHYS_MIN
}

/// Derive blockers from probe fields so Windows and the stub cannot drift.
///
/// `check_elevation`: on a real Windows probe this is true. On the Linux stub it
/// is false unless `OMARCHY_STUB_BLOCKS` includes `not-elevated`, so the wizard
/// stays walkable on dev hosts.
pub fn blocking_reasons(probe: &MachineProbe, check_elevation: bool) -> Vec<BlockingReason> {
    let mut out = probe.blocking_reasons.clone();

    if check_elevation && !probe.host.elevated {
        out.push(BlockingReason::NotElevated);
    }
    if !probe.uefi {
        out.push(BlockingReason::NotUefi);
    }
    if probe.secure_boot {
        out.push(BlockingReason::SecureBoot);
    }
    if probe.uefi && !probe.efi_vars_writable && probe.host.native_windows {
        out.push(BlockingReason::EfiVarsLocked);
    }
    if probe.host.native_windows
        && probe.target_esp.is_none()
        && !out.iter().any(|reason| {
            matches!(
                reason,
                BlockingReason::MissingEsp { .. } | BlockingReason::AmbiguousEsp { .. }
            )
        })
    {
        out.push(BlockingReason::MissingEsp {
            disk_id: probe
                .recommended_disk_id
                .clone()
                .unwrap_or_else(|| "Windows boot disk".into()),
        });
    }
    if !ram_ok_for_copytoram(probe.ram_installed_bytes, probe.ram_total_phys_bytes) {
        out.push(BlockingReason::Ram {
            have_installed: probe.ram_installed_bytes,
            have_total_phys: probe.ram_total_phys_bytes,
            need_installed: RAM_INSTALLED_MIN,
            need_total_phys: RAM_TOTAL_PHYS_MIN,
        });
    }
    for disk in &probe.disks {
        if disk.partition_style != "gpt" && disk.is_boot {
            out.push(BlockingReason::NotGpt {
                disk_id: disk.device_id.clone(),
            });
        }
        if disk.is_rst {
            out.push(BlockingReason::Rst {
                disk_id: disk.device_id.clone(),
            });
        }
        if disk.is_dynamic {
            out.push(BlockingReason::Dynamic {
                disk_id: disk.device_id.clone(),
            });
        }
        if disk.is_storage_spaces {
            out.push(BlockingReason::StorageSpaces {
                disk_id: disk.device_id.clone(),
            });
        }
        if disk.is_boot {
            if let Some(have) = disk.max_shrink_bytes {
                if have < INSTALLER_HOLE_BYTES {
                    out.push(BlockingReason::ShrinkTooSmall {
                        have,
                        need: INSTALLER_HOLE_BYTES,
                    });
                }
            }
        }
    }
    out
}

pub fn attach_reasons(mut probe: MachineProbe, check_elevation: bool) -> MachineProbe {
    probe.ram_ok_for_copytoram =
        ram_ok_for_copytoram(probe.ram_installed_bytes, probe.ram_total_phys_bytes);
    probe.blocking_reasons = blocking_reasons(&probe, check_elevation);
    probe
}

/// True when a BitLocker WMI/PS volume is fully decrypted *and* the on-disk
/// partition boot sector is not still `-FVE-FS-` (suspend leaves the signature).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn bitlocker_fully_decrypted(
    protection_status: u32,
    conversion_status: u32,
    fve_signature: bool,
) -> bool {
    protection_status == 0 && conversion_status == 0 && !fve_signature
}

#[allow(dead_code)]
pub fn volume_is_fve(boot_sector: &[u8]) -> bool {
    boot_sector.len() >= 11 && &boot_sector[3..11] == b"-FVE-FS-"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{BitlockerVolume, DiskMap, HostInfo, MachineProbe, PartitionMap};

    fn host(native: bool, elevated: bool) -> HostInfo {
        HostInfo {
            os: "linux".into(),
            arch: "x86_64".into(),
            elevated,
            native_windows: native,
            os_version: None,
        }
    }

    fn probe() -> MachineProbe {
        MachineProbe {
            host: host(true, true),
            uefi: true,
            secure_boot: false,
            efi_vars_writable: true,
            ram_installed_bytes: 16 * GIB,
            ram_total_phys_bytes: 15 * GIB,
            ram_avail_bytes: 8 * GIB,
            ram_ok_for_copytoram: true,
            tpm_present: true,
            recommended_disk_id: Some(r"\\.\PHYSICALDRIVE0".into()),
            target_esp: Some(crate::platform::TargetEsp {
                disk_id: r"\\.\PHYSICALDRIVE0".into(),
                disk_guid: "{aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa}".into(),
                disk_number: 0,
                partition_guid: "{bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb}".into(),
                volume_guid: r"\\?\Volume{cccccccc-cccc-cccc-cccc-cccccccccccc}\".into(),
            }),
            linux_by_id: Some("/dev/disk/by-id/nvme-TEST_1234".into()),
            bitlocker: vec![BitlockerVolume {
                device_id: Some(r"\\?\Volume{dddddddd-dddd-dddd-dddd-dddddddddddd}\".into()),
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
                partitions: vec![PartitionMap {
                    gpt_guid: Some("{esp}".into()),
                    type_guid: Some("{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}".into()),
                    letter: None,
                    label: Some("SYSTEM".into()),
                    size_bytes: 100 * MIB,
                    offset_bytes: MIB,
                    fs: Some("fat32".into()),
                }],
            }],
            blocking_reasons: vec![],
        }
    }

    #[test]
    fn ram_gate_table() {
        assert!(!ram_ok_for_copytoram(12 * GIB - 1, 12 * GIB));
        assert!(!ram_ok_for_copytoram(12 * GIB, 10 * GIB - 1));
        assert!(ram_ok_for_copytoram(12 * GIB, 10 * GIB));
        assert!(ram_ok_for_copytoram(12 * GIB, 12 * GIB));
        assert!(ram_ok_for_copytoram(32 * GIB, 30 * GIB));
    }

    #[test]
    fn healthy_probe_has_no_blocks() {
        let p = attach_reasons(probe(), true);
        assert!(p.blocking_reasons.is_empty());
        assert!(p.ram_ok_for_copytoram);
    }

    #[test]
    fn stub_skips_elevation_block() {
        let mut p = probe();
        p.host = host(false, false);
        let p = attach_reasons(p, false);
        assert!(p.blocking_reasons.is_empty());
    }

    #[test]
    fn elevation_block_when_checked() {
        let mut p = probe();
        p.host.elevated = false;
        let p = attach_reasons(p, true);
        assert!(p
            .blocking_reasons
            .iter()
            .any(|b| matches!(b, BlockingReason::NotElevated)));
    }

    #[test]
    fn fve_signature_is_not_decrypted() {
        let mut sector = vec![0u8; 16];
        sector[3..11].copy_from_slice(b"-FVE-FS-");
        assert!(volume_is_fve(&sector));
        assert!(!bitlocker_fully_decrypted(0, 0, true));
        assert!(bitlocker_fully_decrypted(0, 0, false));
        assert!(!bitlocker_fully_decrypted(0, 1, false));
    }

    #[test]
    fn active_bitlocker_is_a_warning_not_a_blocker() {
        let mut p = probe();
        p.bitlocker[0].protection_status = 1;
        p.bitlocker[0].conversion_status = 1;
        p.bitlocker[0].fully_decrypted = false;
        let p = attach_reasons(p, true);
        assert!(p.blocking_reasons.is_empty(), "{:?}", p.blocking_reasons);
        assert!(p.bitlocker.iter().any(|volume| !volume.fully_decrypted));
    }

    #[test]
    fn rst_and_secure_boot_block() {
        let mut p = probe();
        p.secure_boot = true;
        p.disks[0].is_rst = true;
        let p = attach_reasons(p, true);
        assert!(p
            .blocking_reasons
            .iter()
            .any(|b| matches!(b, BlockingReason::SecureBoot)));
        assert!(p
            .blocking_reasons
            .iter()
            .any(|b| matches!(b, BlockingReason::Rst { .. })));
    }

    #[test]
    fn shrink_too_small_on_boot_disk() {
        let mut p = probe();
        p.disks[0].max_shrink_bytes = Some(GIB);
        let p = attach_reasons(p, true);
        match &p.blocking_reasons[..] {
            [BlockingReason::ShrinkTooSmall { have, need }] => {
                assert_eq!(*have, GIB);
                assert_eq!(*need, INSTALLER_HOLE_BYTES);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn preserves_probe_failures_without_duplicate_missing_esp() {
        let mut p = probe();
        p.target_esp = None;
        p.blocking_reasons = vec![BlockingReason::MissingEsp {
            disk_id: r"\\.\PHYSICALDRIVE0".into(),
        }];
        let p = attach_reasons(p, true);
        assert_eq!(
            p.blocking_reasons
                .iter()
                .filter(|reason| matches!(reason, BlockingReason::MissingEsp { .. }))
                .count(),
            1
        );

        let mut p = probe();
        p.blocking_reasons = vec![BlockingReason::ProbeIncomplete {
            component: "BitLocker WMI".into(),
        }];
        let p = attach_reasons(p, true);
        assert!(p.blocking_reasons.iter().any(|reason| matches!(
            reason,
            BlockingReason::ProbeIncomplete { component } if component == "BitLocker WMI"
        )));
    }
}
