//! Read-only classification of an interrupted installer run.

use crate::error::{Error, Result};
use crate::platform::{JournalStep, MachineProbe, PendingOperation, StateJournal};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryDisposition {
    Fresh,
    SafeToResume,
    SafeToRetry,
    RollbackRequired,
    ManualReconciliationRequired,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryAssessment {
    pub disposition: RecoveryDisposition,
    pub reason: String,
    pub operation_id: Option<String>,
    pub step: Option<JournalStep>,
    pub pending_operation: Option<PendingOperation>,
}

impl RecoveryAssessment {
    pub fn permits_execution(&self) -> bool {
        matches!(
            self.disposition,
            RecoveryDisposition::Fresh
                | RecoveryDisposition::SafeToResume
                | RecoveryDisposition::SafeToRetry
        )
    }
}

pub fn needs_probe(journal: Option<&StateJournal>) -> bool {
    journal.is_some_and(|journal| {
        matches!(
            journal.pending_operation,
            Some(
                PendingOperation::ShrinkWindows
                    | PendingOperation::CreateOmarchyPartition
                    | PendingOperation::CreateCidataPartition
            )
        )
    })
}

pub fn assess(
    journal: Option<&StateJournal>,
    probe: Option<&MachineProbe>,
) -> Result<RecoveryAssessment> {
    let Some(journal) = journal else {
        return Ok(assessment(
            RecoveryDisposition::Fresh,
            "No previous installation operation is recorded.",
            None,
        ));
    };

    let disposition = match journal.pending_operation {
        None if matches!(journal.step, JournalStep::BootNextSet) => {
            return Ok(assessment(
                RecoveryDisposition::Completed,
                "Staging completed and the one-time UEFI boot target is set.",
                Some(journal),
            ));
        }
        None if journal.plan_digest.is_none()
            && matches!(
                journal.step,
                JournalStep::PowerPrepared
                    | JournalStep::WindowsShrunk
                    | JournalStep::OmarchyPartitionCreated
            ) =>
        {
            RecoveryDisposition::ManualReconciliationRequired
        }
        None => RecoveryDisposition::SafeToResume,
        Some(PendingOperation::Rollback) => RecoveryDisposition::RollbackRequired,
        Some(PendingOperation::DisablePower)
        | Some(PendingOperation::StageEsp)
        | Some(PendingOperation::WriteCidata)
        | Some(PendingOperation::CreateBootEntry)
        | Some(PendingOperation::SetBootNext) => RecoveryDisposition::SafeToRetry,
        Some(PendingOperation::ShrinkWindows) => assess_shrink(journal, require_probe(probe)?)
            .unwrap_or(RecoveryDisposition::ManualReconciliationRequired),
        Some(PendingOperation::CreateOmarchyPartition) => assess_partition(
            journal,
            require_probe(probe)?,
            journal.omarchyinst_offset_bytes,
            journal.omarchyinst_size_bytes,
            journal.omarchyinst_partuuid.as_deref(),
            "OMARCHYINST",
            "NTFS",
        )
        .unwrap_or(RecoveryDisposition::ManualReconciliationRequired),
        Some(PendingOperation::CreateCidataPartition) => assess_partition(
            journal,
            require_probe(probe)?,
            journal.cidata_offset_bytes,
            journal.cidata_size_bytes,
            journal.cidata_partuuid.as_deref(),
            "cidata",
            "FAT32",
        )
        .unwrap_or(RecoveryDisposition::ManualReconciliationRequired),
    };

    let reason = match disposition {
        RecoveryDisposition::SafeToResume => {
            "The last durable checkpoint is complete; execution can continue from it."
        }
        RecoveryDisposition::SafeToRetry => {
            "The interrupted operation is idempotent or its observed disk state matches a safe retry."
        }
        RecoveryDisposition::RollbackRequired => {
            "A rollback was interrupted; finish rollback before starting or resuming installation."
        }
        RecoveryDisposition::ManualReconciliationRequired => {
            "Observed disk state does not prove that retry or resume is safe. Preserve the journal and reconcile the machine manually."
        }
        RecoveryDisposition::Fresh | RecoveryDisposition::Completed => unreachable!(),
    };
    Ok(assessment(disposition, reason, Some(journal)))
}

fn assessment(
    disposition: RecoveryDisposition,
    reason: &str,
    journal: Option<&StateJournal>,
) -> RecoveryAssessment {
    RecoveryAssessment {
        disposition,
        reason: reason.into(),
        operation_id: journal.map(|journal| journal.operation_id.clone()),
        step: journal.map(|journal| journal.step.clone()),
        pending_operation: journal.and_then(|journal| journal.pending_operation.clone()),
    }
}

fn require_probe(probe: Option<&MachineProbe>) -> Result<&MachineProbe> {
    probe.ok_or_else(|| Error::Message("recovery assessment requires a fresh disk probe".into()))
}

fn target_disk<'a>(
    journal: &StateJournal,
    probe: &'a MachineProbe,
) -> Result<&'a crate::platform::DiskMap> {
    let esp = probe.target_esp.as_ref().ok_or_else(|| {
        Error::Message("cannot reconcile recovery because the target ESP is missing".into())
    })?;
    if journal.target_disk_number != Some(esp.disk_number)
        || !same_guid(journal.target_disk_guid.as_deref(), Some(&esp.disk_guid))
    {
        return Err(Error::Message(
            "cannot reconcile recovery because the target disk identity changed".into(),
        ));
    }
    probe
        .disks
        .iter()
        .find(|disk| disk.device_id.eq_ignore_ascii_case(&esp.disk_id))
        .ok_or_else(|| {
            Error::Message("journaled target disk is absent from the fresh probe".into())
        })
}

fn assess_shrink(journal: &StateJournal, probe: &MachineProbe) -> Result<RecoveryDisposition> {
    let disk = target_disk(journal, probe)?;
    let windows = disk.partitions.iter().find(|partition| {
        same_guid(
            partition.gpt_guid.as_deref(),
            journal.windows_partition_guid.as_deref(),
        )
    });
    let Some(windows) = windows else {
        return Ok(RecoveryDisposition::ManualReconciliationRequired);
    };
    if Some(windows.offset_bytes) != journal.windows_partition_offset_bytes {
        return Ok(RecoveryDisposition::ManualReconciliationRequired);
    }
    if Some(windows.size_bytes) == journal.old_c_size_bytes
        || Some(windows.size_bytes) == journal.new_c_size_bytes
    {
        Ok(RecoveryDisposition::SafeToRetry)
    } else {
        Ok(RecoveryDisposition::ManualReconciliationRequired)
    }
}

#[allow(clippy::too_many_arguments)]
fn assess_partition(
    journal: &StateJournal,
    probe: &MachineProbe,
    expected_offset: Option<u64>,
    expected_size: Option<u64>,
    expected_guid: Option<&str>,
    expected_label: &str,
    expected_fs: &str,
) -> Result<RecoveryDisposition> {
    let disk = target_disk(journal, probe)?;
    let (Some(offset), Some(size)) = (expected_offset, expected_size) else {
        return Ok(RecoveryDisposition::ManualReconciliationRequired);
    };
    let matches = disk
        .partitions
        .iter()
        .filter(|partition| partition.offset_bytes == offset)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Ok(RecoveryDisposition::SafeToRetry);
    }
    if matches.len() != 1 {
        return Ok(RecoveryDisposition::ManualReconciliationRequired);
    }
    let partition = matches[0];
    if partition.size_bytes != size {
        return Ok(RecoveryDisposition::ManualReconciliationRequired);
    }
    let identity_matches =
        expected_guid.is_some_and(|guid| same_guid(partition.gpt_guid.as_deref(), Some(guid)));
    let volume_matches = partition
        .label
        .as_deref()
        .is_some_and(|label| label.eq_ignore_ascii_case(expected_label))
        && partition
            .fs
            .as_deref()
            .is_some_and(|fs| fs.eq_ignore_ascii_case(expected_fs));
    Ok(if identity_matches || volume_matches {
        RecoveryDisposition::SafeToRetry
    } else {
        RecoveryDisposition::ManualReconciliationRequired
    })
}

fn same_guid(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left
            .trim()
            .trim_matches(|character| character == '{' || character == '}')
            .eq_ignore_ascii_case(
                right
                    .trim()
                    .trim_matches(|character| character == '{' || character == '}'),
            ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::empty_journal;
    use crate::platform::{DiskMap, HostInfo, PartitionMap, TargetEsp};

    #[test]
    fn fresh_completed_and_clean_checkpoint_are_distinct() {
        assert_eq!(
            assess(None, None).unwrap().disposition,
            RecoveryDisposition::Fresh
        );
        let mut journal = empty_journal();
        assert_eq!(
            assess(Some(&journal), None).unwrap().disposition,
            RecoveryDisposition::SafeToResume
        );
        journal.step = JournalStep::BootNextSet;
        assert_eq!(
            assess(Some(&journal), None).unwrap().disposition,
            RecoveryDisposition::Completed
        );
    }

    #[test]
    fn rollback_never_becomes_an_install_retry() {
        let mut journal = empty_journal();
        journal.pending_operation = Some(PendingOperation::Rollback);
        let result = assess(Some(&journal), None).unwrap();
        assert_eq!(result.disposition, RecoveryDisposition::RollbackRequired);
        assert!(!result.permits_execution());
    }

    #[test]
    fn uncertain_shrink_requires_the_exact_old_or_new_geometry() {
        let (mut journal, mut probe) = disk_fixture();
        journal.pending_operation = Some(PendingOperation::ShrinkWindows);
        assert_eq!(
            assess(Some(&journal), Some(&probe)).unwrap().disposition,
            RecoveryDisposition::SafeToRetry
        );
        probe.disks[0].partitions[0].size_bytes = journal.new_c_size_bytes.unwrap();
        assert_eq!(
            assess(Some(&journal), Some(&probe)).unwrap().disposition,
            RecoveryDisposition::SafeToRetry
        );
        probe.disks[0].partitions[0].size_bytes -= 1;
        assert_eq!(
            assess(Some(&journal), Some(&probe)).unwrap().disposition,
            RecoveryDisposition::ManualReconciliationRequired
        );
    }

    #[test]
    fn an_unowned_partition_at_the_planned_offset_is_not_reformatted() {
        let (mut journal, mut probe) = disk_fixture();
        journal.pending_operation = Some(PendingOperation::CreateOmarchyPartition);
        probe.disks[0].partitions.push(PartitionMap {
            gpt_guid: Some("foreign".into()),
            type_guid: None,
            letter: None,
            label: None,
            size_bytes: journal.omarchyinst_size_bytes.unwrap(),
            offset_bytes: journal.omarchyinst_offset_bytes.unwrap(),
            fs: None,
        });
        assert_eq!(
            assess(Some(&journal), Some(&probe)).unwrap().disposition,
            RecoveryDisposition::ManualReconciliationRequired
        );
        probe.disks[0].partitions[1].label = Some("OMARCHYINST".into());
        probe.disks[0].partitions[1].fs = Some("NTFS".into());
        assert_eq!(
            assess(Some(&journal), Some(&probe)).unwrap().disposition,
            RecoveryDisposition::SafeToRetry
        );
    }

    fn disk_fixture() -> (StateJournal, MachineProbe) {
        let mut journal = empty_journal();
        journal.target_disk_guid = Some("disk-guid".into());
        journal.target_disk_number = Some(0);
        journal.windows_partition_guid = Some("windows-guid".into());
        journal.windows_partition_offset_bytes = Some(1024);
        journal.old_c_size_bytes = Some(10_000);
        journal.new_c_size_bytes = Some(8_000);
        journal.omarchyinst_offset_bytes = Some(9_024);
        journal.omarchyinst_size_bytes = Some(1_500);
        journal.cidata_offset_bytes = Some(10_524);
        journal.cidata_size_bytes = Some(500);
        journal.plan_digest = Some("a".repeat(64));
        let probe = MachineProbe {
            host: HostInfo {
                os: "windows".into(),
                arch: "x86_64".into(),
                elevated: true,
                native_windows: true,
                os_version: None,
            },
            uefi: true,
            secure_boot: false,
            efi_vars_writable: true,
            ram_installed_bytes: 16_000,
            ram_total_phys_bytes: 16_000,
            ram_avail_bytes: 8_000,
            ram_ok_for_copytoram: true,
            tpm_present: true,
            recommended_disk_id: Some("disk-0".into()),
            target_esp: Some(TargetEsp {
                disk_id: "disk-0".into(),
                disk_guid: "disk-guid".into(),
                disk_number: 0,
                partition_guid: "esp-guid".into(),
                volume_guid: "esp-volume".into(),
            }),
            linux_by_id: Some("/dev/disk/by-id/example".into()),
            bitlocker: Vec::new(),
            disks: vec![DiskMap {
                device_id: "disk-0".into(),
                size_bytes: 20_000,
                partition_style: "gpt".into(),
                bus: None,
                is_boot: true,
                is_rst: false,
                is_dynamic: false,
                is_storage_spaces: false,
                max_shrink_bytes: Some(5_000),
                partitions: vec![PartitionMap {
                    gpt_guid: Some("windows-guid".into()),
                    type_guid: None,
                    letter: Some("C:".into()),
                    label: None,
                    size_bytes: 10_000,
                    offset_bytes: 1024,
                    fs: Some("NTFS".into()),
                }],
            }],
            blocking_reasons: Vec::new(),
        };
        (journal, probe)
    }
}
