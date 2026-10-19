//! Pure probe policy. No Win32 — unit-tested on every host.

#[cfg(any(windows, test))]
use crate::error::{Error, Result};
use crate::platform::{BlockingReason, MachineProbe};

/// Refuses consumers that require trustworthy disk identity when any probe
/// component was incomplete, while preserving the component's actual error.
#[cfg(any(windows, test))]
pub(crate) fn require_complete(probe: &MachineProbe) -> Result<()> {
    if let Some((component, detail)) = probe.blocking_reasons.iter().find_map(|reason| {
        if let BlockingReason::ProbeIncomplete { component, detail } = reason {
            Some((component, detail))
        } else {
            None
        }
    }) {
        return Err(Error::Message(format!(
            "{component} check did not complete: {detail} No disk changes were made; run the system check again."
        )));
    }
    Ok(())
}

/// 12 GiB installed. Machines below the 14 GiB recommendation get a UI warning.
pub const RAM_INSTALLED_MIN: u64 = 12 * GIB;
/// Allow up to a 2 GiB firmware/iGPU carve-out on a 12 GiB machine.
pub const RAM_TOTAL_PHYS_MIN: u64 = 10 * GIB;

pub const GIB: u64 = 1024 * 1024 * 1024;
pub const MIB: u64 = 1024 * 1024;

/// Default hole we need to shrink: 8 GiB `OMARCHYINST` + 512 MiB boot/cidata.
pub const INSTALLER_HOLE_BYTES: u64 = 8 * GIB + 512 * MIB;
/// Keep a small safety margin for filesystem changes between precheck and resize.
pub const SHRINK_HEADROOM_BYTES: u64 = 64 * MIB;

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
                BlockingReason::MissingEsp { .. }
                    | BlockingReason::AmbiguousEsp { .. }
                    | BlockingReason::ProbeIncomplete { .. }
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
        if disk.is_boot && disk.is_rst {
            out.push(BlockingReason::Rst {
                disk_id: disk.device_id.clone(),
            });
        }
        if disk.is_boot && disk.is_dynamic {
            out.push(BlockingReason::Dynamic {
                disk_id: disk.device_id.clone(),
            });
        }
        if disk.is_boot && disk.is_storage_spaces {
            out.push(BlockingReason::StorageSpaces {
                disk_id: disk.device_id.clone(),
            });
        }
        if disk.is_boot {
            if let Some(have) = disk.max_shrink_bytes {
                let need = INSTALLER_HOLE_BYTES.saturating_add(SHRINK_HEADROOM_BYTES);
                if have < need {
                    out.push(BlockingReason::ShrinkTooSmall { have, need });
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

/// Applies deterministic failures for policy tests and the non-Windows
/// development UI. Production Windows probing never calls this function.
#[cfg(any(test, not(windows)))]
pub(crate) fn apply_named_fault(probe: &mut MachineProbe, fault: &str) -> bool {
    match fault {
        "not-elevated" => probe.host.elevated = false,
        "legacy-bios" => {
            probe.uefi = false;
            probe.efi_vars_writable = false;
        }
        "secure-boot" => probe.secure_boot = true,
        "efi-vars-locked" => probe.efi_vars_writable = false,
        "low-installed-ram" => probe.ram_installed_bytes = RAM_INSTALLED_MIN - 1,
        "low-usable-ram" => probe.ram_total_phys_bytes = RAM_TOTAL_PHYS_MIN - 1,
        "mbr-boot-disk" => probe.disks[0].partition_style = "mbr".into(),
        "rst-boot-disk" => probe.disks[0].is_rst = true,
        "dynamic-boot-disk" => probe.disks[0].is_dynamic = true,
        "storage-spaces-boot-disk" => probe.disks[0].is_storage_spaces = true,
        "shrink-too-small" => probe.disks[0].max_shrink_bytes = Some(GIB),
        "deferred-shrink" => probe.disks[0].max_shrink_bytes = None,
        "missing-esp" => probe.target_esp = None,
        "ambiguous-esp" => {
            probe.target_esp = None;
            probe.blocking_reasons.push(BlockingReason::AmbiguousEsp {
                disk_id: probe.disks[0].device_id.clone(),
                count: 2,
            });
        }
        "incomplete-storage" => {
            probe.target_esp = None;
            probe
                .blocking_reasons
                .push(BlockingReason::ProbeIncomplete {
                    component: "native Windows storage inventory".into(),
                    detail: "injected timeout".into(),
                });
        }
        "incomplete-bitlocker" => {
            probe
                .blocking_reasons
                .push(BlockingReason::ProbeIncomplete {
                    component: "BitLocker WMI".into(),
                    detail: "injected timeout".into(),
                });
        }
        "missing-linux-identity" => {
            probe.linux_by_id = None;
            probe
                .blocking_reasons
                .push(BlockingReason::ProbeIncomplete {
                    component: "target Linux disk identity".into(),
                    detail: "injected missing serial".into(),
                });
        }
        "active-bitlocker" => {
            probe.bitlocker[0].protection_status = 1;
            probe.bitlocker[0].conversion_status = 1;
            probe.bitlocker[0].fully_decrypted = false;
        }
        "no-tpm" => probe.tpm_present = false,
        "secondary-unsupported" => {
            let mut secondary = probe.disks[0].clone();
            secondary.device_id = format!(r"\\.\PHYSICALDRIVE{}", probe.disks.len());
            secondary.is_boot = false;
            secondary.is_rst = true;
            secondary.is_dynamic = true;
            secondary.is_storage_spaces = true;
            probe.disks.push(secondary);
        }
        _ => return false,
    }
    true
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
    use serde::Deserialize;
    use std::collections::HashSet;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ScenarioFile {
        schema_version: u32,
        cases: Vec<ScenarioCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ScenarioCase {
        name: String,
        faults: Vec<String>,
        expected_blockers: Vec<String>,
    }

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

    fn blocker_types(probe: &MachineProbe) -> Vec<String> {
        let mut types: Vec<String> = probe
            .blocking_reasons
            .iter()
            .map(|reason| {
                serde_json::to_value(reason).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        types.sort();
        types
    }

    #[test]
    fn data_driven_precheck_scenario_matrix() {
        let scenarios: ScenarioFile =
            serde_json::from_str(include_str!("../../scripts/precheck-scenarios.json")).unwrap();
        assert_eq!(scenarios.schema_version, 1);
        assert!(scenarios.cases.len() >= 20);

        let mut names = HashSet::new();
        let mut exercised_faults = HashSet::new();
        for scenario in scenarios.cases {
            assert!(
                names.insert(scenario.name.clone()),
                "duplicate scenario name"
            );
            let mut candidate = probe();
            for fault in &scenario.faults {
                exercised_faults.insert(fault.clone());
                assert!(
                    apply_named_fault(&mut candidate, fault),
                    "unknown precheck scenario fault {fault:?}"
                );
            }
            let actual = blocker_types(&attach_reasons(candidate, true));
            let mut expected = scenario.expected_blockers;
            expected.sort();
            println!("precheck scenario {} => {actual:?}", scenario.name);
            assert_eq!(actual, expected, "scenario {:?}", scenario.name);
        }
        let expected_faults: HashSet<String> = [
            "active-bitlocker",
            "ambiguous-esp",
            "deferred-shrink",
            "dynamic-boot-disk",
            "efi-vars-locked",
            "incomplete-bitlocker",
            "incomplete-storage",
            "legacy-bios",
            "low-installed-ram",
            "low-usable-ram",
            "mbr-boot-disk",
            "missing-esp",
            "missing-linux-identity",
            "no-tpm",
            "not-elevated",
            "rst-boot-disk",
            "secondary-unsupported",
            "secure-boot",
            "shrink-too-small",
            "storage-spaces-boot-disk",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(
            exercised_faults, expected_faults,
            "every implemented scenario fault must remain exercised"
        );
    }

    #[test]
    fn independent_precheck_failures_compose_for_every_combination() {
        let independent = [
            ("not-elevated", "notElevated"),
            ("secure-boot", "secureBoot"),
            ("low-installed-ram", "ram"),
            ("mbr-boot-disk", "notGpt"),
            ("rst-boot-disk", "rst"),
            ("dynamic-boot-disk", "dynamic"),
            ("storage-spaces-boot-disk", "storageSpaces"),
        ];
        for mask in 0..(1usize << independent.len()) {
            let mut candidate = probe();
            let mut expected = Vec::new();
            for (index, (fault, blocker)) in independent.iter().enumerate() {
                if mask & (1 << index) != 0 {
                    assert!(apply_named_fault(&mut candidate, fault));
                    expected.push((*blocker).to_string());
                }
            }
            expected.sort();
            assert_eq!(
                blocker_types(&attach_reasons(candidate, true)),
                expected,
                "independent fault mask {mask:#09b}"
            );
        }
    }

    #[test]
    fn any_number_of_unsupported_secondary_disks_remains_nonblocking() {
        for count in 1..=16 {
            let mut candidate = probe();
            for number in 1..=count {
                let mut secondary = candidate.disks[0].clone();
                secondary.device_id = format!(r"\\.\PHYSICALDRIVE{number}");
                secondary.is_boot = false;
                secondary.is_rst = number & 1 != 0;
                secondary.is_dynamic = number & 2 != 0;
                secondary.is_storage_spaces = number & 4 != 0;
                secondary.partition_style = if number & 8 != 0 {
                    "mbr".into()
                } else {
                    "gpt".into()
                };
                candidate.disks.push(secondary);
            }
            assert_eq!(
                blocker_types(&attach_reasons(candidate, true)),
                Vec::<String>::new(),
                "{count} secondary disks"
            );
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
    fn deferred_shrink_capacity_does_not_block_precheck() {
        let mut p = probe();
        p.disks[0].max_shrink_bytes = None;
        let p = attach_reasons(p, true);
        assert!(p.blocking_reasons.is_empty(), "{:?}", p.blocking_reasons);
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
                assert_eq!(*need, INSTALLER_HOLE_BYTES + SHRINK_HEADROOM_BYTES);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn shrink_headroom_boundary() {
        let mut p = probe();
        p.disks[0].max_shrink_bytes = Some(INSTALLER_HOLE_BYTES + SHRINK_HEADROOM_BYTES - 1);
        assert!(attach_reasons(p.clone(), true)
            .blocking_reasons
            .iter()
            .any(|reason| matches!(reason, BlockingReason::ShrinkTooSmall { .. })));
        p.disks[0].max_shrink_bytes = Some(INSTALLER_HOLE_BYTES + SHRINK_HEADROOM_BYTES);
        assert!(!attach_reasons(p, true)
            .blocking_reasons
            .iter()
            .any(|reason| matches!(reason, BlockingReason::ShrinkTooSmall { .. })));
    }

    #[test]
    fn unsupported_storage_on_secondary_disk_does_not_block() {
        let mut p = probe();
        let mut secondary = p.disks[0].clone();
        secondary.device_id = r"\\.\PHYSICALDRIVE1".into();
        secondary.is_boot = false;
        secondary.is_rst = true;
        secondary.is_dynamic = true;
        secondary.is_storage_spaces = true;
        p.disks.push(secondary);
        assert!(attach_reasons(p, true).blocking_reasons.is_empty());
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
            detail: "access denied".into(),
        }];
        let p = attach_reasons(p, true);
        assert!(p.blocking_reasons.iter().any(|reason| matches!(
            reason,
            BlockingReason::ProbeIncomplete { component, .. } if component == "BitLocker WMI"
        )));
        assert!(!p
            .blocking_reasons
            .iter()
            .any(|reason| matches!(reason, BlockingReason::MissingEsp { .. })));
    }

    #[test]
    fn incomplete_probe_reports_causal_detail_before_mutation() {
        let mut p = probe();
        p.blocking_reasons = vec![BlockingReason::ProbeIncomplete {
            component: "Windows storage inventory".into(),
            detail: "timed out twice".into(),
        }];

        let error = require_complete(&p).unwrap_err().to_string();
        assert!(error.contains("Windows storage inventory"), "{error}");
        assert!(error.contains("timed out twice"), "{error}");
        assert!(error.contains("No disk changes were made"), "{error}");
    }
}
