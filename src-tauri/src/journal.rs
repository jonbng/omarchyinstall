//! Install journal: rollback state. Never stores the user password.

use crate::error::{Error, Result};
use crate::platform::{JournalStep, RollbackResult, StateJournal};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub const JOURNAL_VERSION: u32 = 2;

fn journal_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub fn parse_journal(json: &str) -> Result<StateJournal> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| Error::Message(format!("state.json: {e}")))?;
    refuse_password_field(&value)?;
    let version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version != JOURNAL_VERSION as u64 {
        return Err(Error::Message(format!(
            "unsupported state.json schema {version}; automatic rollback is disabled because this development journal lacks exact disk identifiers"
        )));
    }
    serde_json::from_value(value).map_err(|e| Error::Message(format!("state.json: {e}")))
}

pub fn serialize_journal(journal: &StateJournal) -> Result<String> {
    if journal.version != JOURNAL_VERSION {
        return Err(Error::Message(format!(
            "refusing to write state.json schema {} (expected {JOURNAL_VERSION})",
            journal.version
        )));
    }
    let json = serde_json::to_string_pretty(journal).map_err(|e| Error::Message(e.to_string()))?;
    refuse_password_field(&serde_json::from_str(&json).unwrap())?;
    Ok(json)
}

/// Durably replace the journal. The temporary file lives beside the target so
/// the final rename cannot cross filesystems.
pub fn save_atomic(path: &Path, journal: &StateJournal) -> Result<()> {
    let _guard = journal_lock()
        .lock()
        .map_err(|_| Error::Message("journal lock poisoned".into()))?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::Message("state.json has no parent directory".into()))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".state.json.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)?;
    file.write_all(serialize_journal(journal)?.as_bytes())?;
    file.sync_all()?;
    drop(file);
    atomic_replace(&temp, path)
}

#[cfg(windows)]
fn atomic_replace(temp: &Path, path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace(temp: &Path, path: &Path) -> Result<()> {
    fs::rename(temp, path)?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

pub fn redact_journal_json(json: &str) -> Result<String> {
    let mut value: Value =
        serde_json::from_str(json).map_err(|e| Error::Message(format!("state.json: {e}")))?;
    strip_secrets(&mut value);
    serde_json::to_string_pretty(&value).map_err(|e| Error::Message(e.to_string()))
}

pub fn refuse_password_field(value: &Value) -> Result<()> {
    if has_password_key(value) {
        return Err(Error::Message(
            "state.json must not contain a password field".into(),
        ));
    }
    Ok(())
}

fn has_password_key(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.keys().any(|k| {
                let k = k.to_ascii_lowercase();
                k == "password" || k == "encryption_password"
            }) || map.values().any(has_password_key)
        }
        Value::Array(items) => items.iter().any(has_password_key),
        _ => false,
    }
}

fn strip_secrets(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|k, _| {
                let k = k.to_ascii_lowercase();
                k != "password" && k != "encryption_password"
            });
            for v in map.values_mut() {
                strip_secrets(v);
            }
        }
        Value::Array(items) => {
            for v in items {
                strip_secrets(v);
            }
        }
        _ => {}
    }
}

pub fn empty_journal() -> StateJournal {
    StateJournal {
        version: JOURNAL_VERSION,
        operation_id: random_operation_id(),
        step: JournalStep::Planned,
        pending_operation: None,
        target_disk_guid: None,
        target_disk_number: None,
        windows_partition_guid: None,
        windows_partition_offset_bytes: None,
        new_c_size_bytes: None,
        omarchyinst_offset_bytes: None,
        omarchyinst_size_bytes: None,
        omarchyinst_guid: None,
        cidata_guid: None,
        cidata_partuuid: None,
        cidata_offset_bytes: None,
        cidata_size_bytes: None,
        esp_partition_guid: None,
        esp_volume_guid: None,
        linux_device: None,
        old_c_size_bytes: None,
        iso_sha256: None,
        search_filename: None,
        boot_id: None,
        boot_description: None,
        hiberboot_was: None,
        hibernation_disabled_by_us: false,
        omarchyinst_partuuid: None,
    }
}

fn random_operation_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        bytes[..4].copy_from_slice(&std::process::id().to_le_bytes());
        bytes[8..].copy_from_slice(
            &(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64)
                .to_le_bytes(),
        );
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Map rollback process output to a result. Failed undo must not report success.
pub fn interpret_rollback_output(
    powershell_ok: bool,
    stdout: &str,
    old_c_size_bytes: u64,
    restore_hiber: bool,
) -> Result<RollbackResult> {
    if !powershell_ok {
        return Err(Error::Message(
            "rollback powershell failed; Windows may still have staging partitions".into(),
        ));
    }
    if !stdout.lines().any(|l| l.trim() == "ok") {
        return Err(Error::Message(format!(
            "rollback did not confirm ok: {stdout}"
        )));
    }
    Ok(RollbackResult {
        removed_partition: true,
        extended_ntfs: old_c_size_bytes > 0,
        restored_power_settings: restore_hiber,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut j = empty_journal();
        j.omarchyinst_guid = Some("{aaaa}".into());
        j.linux_device = Some("/dev/disk/by-id/nvme-x".into());
        j.search_filename = Some("/boot/deadbeef.uuid".into());
        let s = serialize_journal(&j).unwrap();
        assert!(!s.to_ascii_lowercase().contains("password"));
        let back = parse_journal(&s).unwrap();
        assert_eq!(back, j);
    }

    #[test]
    fn refuses_plaintext_password_field() {
        let json = r#"{"version":1,"step":"probeDone","password":"secret"}"#;
        let err = parse_journal(json).unwrap_err().to_string();
        assert!(err.contains("password"), "{err}");
    }

    #[test]
    fn redact_strips_password() {
        let json = r#"{"version":1,"step":"probeDone","nested":{"encryption_password":"x"}}"#;
        let red = redact_journal_json(json).unwrap();
        assert!(!red.contains("encryption_password"));
        assert!(!red.contains("\"x\""));
    }

    #[test]
    fn failed_rollback_is_not_success() {
        let err = interpret_rollback_output(false, "ok", 100, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("failed"), "{err}");
        let err = interpret_rollback_output(true, "partial", 100, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ok"), "{err}");
        let ok = interpret_rollback_output(true, "removed\nok\n", 100, true).unwrap();
        assert!(ok.removed_partition);
        assert!(ok.extended_ntfs);
        assert!(ok.restored_power_settings);
    }

    #[test]
    fn refuses_schema_one_without_guessing() {
        let err = parse_journal(r#"{"version":1,"step":"probeDone"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unsupported state.json schema 1"), "{err}");
        assert!(err.contains("automatic rollback is disabled"), "{err}");
    }

    #[test]
    fn atomic_save_replaces_complete_journal() {
        let dir = std::env::temp_dir().join(format!(
            "omarchy-journal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let mut first = empty_journal();
        first.operation_id = "first".into();
        save_atomic(&path, &first).unwrap();
        let mut second = first.clone();
        second.operation_id = "second".into();
        second.step = JournalStep::PowerPrepared;
        save_atomic(&path, &second).unwrap();
        assert_eq!(
            parse_journal(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            second
        );
        assert!(!dir
            .join(format!(".state.json.{}.tmp", std::process::id()))
            .exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
