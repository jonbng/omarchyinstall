//! Authoritative backend sequencing for the Windows staging workflow.

use crate::cidata::CidataIdentity;
use crate::error::{Error, Result};
use crate::platform::{self, JournalStep, StateJournal};
use crate::recovery::{RecoveryAssessment, RecoveryDisposition};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InstallProgressStatus {
    Started,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub sequence: u64,
    pub phase: String,
    pub status: InstallProgressStatus,
    pub operation_id: Option<String>,
    pub plan_digest: Option<String>,
    pub detail: Option<String>,
}

pub struct InstallRequest {
    pub identity: CidataIdentity,
    pub allow_bitlocker: bool,
    pub iso_sha256: String,
    pub approved_plan_digest: String,
}

pub fn execute(
    request: InstallRequest,
    mut progress: impl FnMut(InstallProgress),
    cancellation_requested: impl Fn() -> bool,
) -> Result<StateJournal> {
    require_not_cancelled(&cancellation_requested)?;
    let assessment = platform::assess_install_recovery()?;
    require_executable(&assessment)?;
    let mut sequence = 0u64;
    let mut identity = Some(request.identity);

    let journal = platform::load_install_state()?;
    if journal.as_ref().is_some_and(|journal| {
        matches!(journal.step, JournalStep::BootNextSet) && journal.pending_operation.is_none()
    }) {
        return journal.ok_or_else(|| Error::Message("completed journal disappeared".into()));
    }

    if journal.as_ref().map(needs_prepare).unwrap_or(true) {
        require_not_cancelled(&cancellation_requested)?;
        run_phase("prepare", &mut sequence, &mut progress, || {
            let saved_iso = journal
                .as_ref()
                .and_then(|journal| journal.iso_sha256.clone())
                .unwrap_or_else(|| request.iso_sha256.clone());
            let saved_plan = journal
                .as_ref()
                .and_then(|journal| journal.plan_digest.clone())
                .unwrap_or_else(|| request.approved_plan_digest.clone());
            platform::prepare_installer_partition(request.allow_bitlocker, saved_iso, saved_plan)
                .map(|_| ())
        })?;
    }

    let journal = required_journal()?;
    require_not_cancelled(&cancellation_requested)?;
    if matches!(journal.step, JournalStep::CidataPartitionCreated)
        || matches!(
            journal.pending_operation,
            Some(crate::platform::PendingOperation::StageEsp)
        )
    {
        run_phase("stage", &mut sequence, &mut progress, || {
            platform::stage_bootloader().map(|_| ())
        })?;
    }

    let journal = required_journal()?;
    require_not_cancelled(&cancellation_requested)?;
    if matches!(journal.step, JournalStep::Staged)
        || matches!(
            journal.pending_operation,
            Some(crate::platform::PendingOperation::WriteCidata)
        )
    {
        let next_identity = identity.take().ok_or_else(|| {
            Error::Message("installation identity is unavailable for cidata recovery".into())
        })?;
        if next_identity.password.is_empty() {
            return Err(Error::Message(
                "re-enter the user password before resuming cidata writing".into(),
            ));
        }
        run_phase("cidata", &mut sequence, &mut progress, || {
            platform::write_cidata(next_identity).map(|_| ())
        })?;
    }

    let journal = required_journal()?;
    require_not_cancelled(&cancellation_requested)?;
    if matches!(
        journal.step,
        JournalStep::CidataWritten | JournalStep::BootEntryCreated
    ) || matches!(
        journal.pending_operation,
        Some(
            crate::platform::PendingOperation::CreateBootEntry
                | crate::platform::PendingOperation::SetBootNext
        )
    ) {
        run_phase("bootnext", &mut sequence, &mut progress, || {
            platform::set_boot_next().map(|_| ())
        })?;
    }

    let final_journal = required_journal()?;
    if !matches!(final_journal.step, JournalStep::BootNextSet)
        || final_journal.pending_operation.is_some()
    {
        return Err(Error::Message(
            "backend workflow stopped without reaching the BootNext checkpoint".into(),
        ));
    }
    Ok(final_journal)
}

fn require_not_cancelled(cancellation_requested: &impl Fn() -> bool) -> Result<()> {
    if cancellation_requested() {
        Err(Error::Message(
            "installation cancellation requested; rollback is required before another run".into(),
        ))
    } else {
        Ok(())
    }
}

fn require_executable(assessment: &RecoveryAssessment) -> Result<()> {
    if assessment.permits_execution()
        || matches!(assessment.disposition, RecoveryDisposition::Completed)
    {
        return Ok(());
    }
    Err(Error::Message(format!(
        "{}: {}",
        match assessment.disposition {
            RecoveryDisposition::RollbackRequired => "rollback required",
            RecoveryDisposition::ManualReconciliationRequired => {
                "manual reconciliation required"
            }
            _ => "execution refused",
        },
        assessment.reason
    )))
}

fn needs_prepare(journal: &StateJournal) -> bool {
    matches!(
        journal.step,
        JournalStep::Planned
            | JournalStep::PowerPrepared
            | JournalStep::WindowsShrunk
            | JournalStep::OmarchyPartitionCreated
    )
}

fn required_journal() -> Result<StateJournal> {
    platform::load_install_state()?.ok_or_else(|| {
        Error::Message("installation journal disappeared during backend execution".into())
    })
}

fn run_phase<T>(
    phase: &str,
    sequence: &mut u64,
    progress: &mut impl FnMut(InstallProgress),
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    emit(
        progress,
        sequence,
        phase,
        InstallProgressStatus::Started,
        None,
    );
    match work() {
        Ok(value) => {
            emit(
                progress,
                sequence,
                phase,
                InstallProgressStatus::Completed,
                None,
            );
            Ok(value)
        }
        Err(error) => {
            emit(
                progress,
                sequence,
                phase,
                InstallProgressStatus::Failed,
                Some(error.to_string()),
            );
            Err(error)
        }
    }
}

fn emit(
    progress: &mut impl FnMut(InstallProgress),
    sequence: &mut u64,
    phase: &str,
    status: InstallProgressStatus,
    detail: Option<String>,
) {
    *sequence += 1;
    let journal = platform::load_install_state().ok().flatten();
    progress(InstallProgress {
        sequence: *sequence,
        phase: phase.into(),
        status,
        operation_id: journal.as_ref().map(|journal| journal.operation_id.clone()),
        plan_digest: journal.and_then(|journal| journal.plan_digest),
        detail,
    });
}
