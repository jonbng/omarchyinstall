//! Verified, crash-safe publication of installer-owned artifacts.

#![cfg_attr(not(windows), allow(dead_code))]

use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static PENDING_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Materialization {
    ReusedVerified,
    AtomicRename,
}

pub fn require_sha256(value: &str) -> Result<&str> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(value)
    } else {
        Err(Error::Message(
            "expected a 64-character SHA-256 digest".into(),
        ))
    }
}

pub fn normalize_sha256(value: &str) -> Result<String> {
    require_sha256(value)?;
    Ok(value.to_ascii_lowercase())
}

pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| Error::Message("artifact size overflow".into()))?;
    }
    Ok((format!("{:x}", hasher.finalize()), size))
}

pub fn verified_copy(
    source: &Path,
    destination: &Path,
    expected_sha256: &str,
) -> Result<Materialization> {
    let expected_sha256 = require_sha256(expected_sha256)?;
    let (source_sha256, source_size) = sha256_file(source)?;
    if source_sha256 != expected_sha256 {
        return Err(Error::Message(format!(
            "source artifact digest changed before staging: expected {expected_sha256}, got {source_sha256}"
        )));
    }

    if destination.exists() {
        return verify_existing(destination, expected_sha256, source_size);
    }

    let pending = pending_path(destination)?;
    if pending.exists() {
        fs::remove_file(&pending)?;
    }
    let result = (|| {
        let copied = fs::copy(source, &pending)?;
        if copied != source_size {
            return Err(Error::Message(format!(
                "artifact copy was truncated: expected {source_size} bytes, copied {copied}"
            )));
        }
        File::options().write(true).open(&pending)?.sync_all()?;
        verify_path(&pending, expected_sha256, source_size)?;
        match fs::rename(&pending, destination) {
            Ok(()) => Ok(Materialization::AtomicRename),
            Err(rename_error) if destination.exists() => {
                let outcome = verify_existing(destination, expected_sha256, source_size);
                if outcome.is_err() {
                    return Err(Error::Message(format!(
                        "could not publish verified artifact: {rename_error}"
                    )));
                }
                outcome
            }
            Err(error) => Err(error.into()),
        }
    })();
    if pending.exists() {
        let _ = fs::remove_file(&pending);
    }
    result
}

pub fn verified_write(destination: &Path, content: &[u8]) -> Result<(Materialization, String)> {
    let expected_sha256 = format!("{:x}", Sha256::digest(content));
    if destination.exists() {
        let outcome = verify_existing(destination, &expected_sha256, content.len() as u64)?;
        return Ok((outcome, expected_sha256));
    }

    let pending = pending_path(destination)?;
    if pending.exists() {
        fs::remove_file(&pending)?;
    }
    let result = (|| {
        let mut file = File::options()
            .create_new(true)
            .write(true)
            .open(&pending)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        verify_path(&pending, &expected_sha256, content.len() as u64)?;
        match fs::rename(&pending, destination) {
            Ok(()) => Ok((Materialization::AtomicRename, expected_sha256.clone())),
            Err(rename_error) if destination.exists() => {
                let outcome = verify_existing(destination, &expected_sha256, content.len() as u64);
                if outcome.is_err() {
                    return Err(Error::Message(format!(
                        "could not publish verified artifact: {rename_error}"
                    )));
                }
                outcome.map(|materialization| (materialization, expected_sha256.clone()))
            }
            Err(error) => Err(error.into()),
        }
    })();
    if pending.exists() {
        let _ = fs::remove_file(&pending);
    }
    result
}

/// Durably replace a mutable installer-owned file, then independently hash the
/// published path. Immutable artifacts must continue to use `verified_write`.
pub fn verified_replace(destination: &Path, content: &[u8]) -> Result<String> {
    let expected_sha256 = format!("{:x}", Sha256::digest(content));
    let pending = pending_path(destination)?;
    if pending.exists() {
        fs::remove_file(&pending)?;
    }
    let result = (|| {
        let mut file = File::options()
            .create_new(true)
            .write(true)
            .open(&pending)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        verify_path(&pending, &expected_sha256, content.len() as u64)?;
        atomic_replace(&pending, destination)?;
        verify_path(destination, &expected_sha256, content.len() as u64)?;
        Ok(expected_sha256.clone())
    })();
    if pending.exists() {
        let _ = fs::remove_file(&pending);
    }
    result
}

#[cfg(windows)]
fn atomic_replace(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
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
fn atomic_replace(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to)?;
    if let Some(parent) = to.parent() {
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

fn verify_existing(
    path: &Path,
    expected_sha256: &str,
    expected_size: u64,
) -> Result<Materialization> {
    verify_path(path, expected_sha256, expected_size).map(|_| Materialization::ReusedVerified)
}

fn verify_path(path: &Path, expected_sha256: &str, expected_size: u64) -> Result<()> {
    let (actual_sha256, actual_size) = sha256_file(path)?;
    if actual_size != expected_size || actual_sha256 != expected_sha256 {
        return Err(Error::Message(format!(
            "staged artifact conflict at {}: expected {expected_size} bytes and {expected_sha256}, got {actual_size} bytes and {actual_sha256}",
            path.display()
        )));
    }
    Ok(())
}

fn pending_path(destination: &Path) -> Result<PathBuf> {
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Message("artifact destination has no valid filename".into()))?;
    let id = PENDING_ID.fetch_add(1, Ordering::Relaxed);
    Ok(destination.with_file_name(format!(
        ".{file_name}.{}.{}.pending",
        std::process::id(),
        id
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "omarchy-artifact-{name}-{}-{}",
            std::process::id(),
            PENDING_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn publishes_and_reuses_only_verified_bytes() {
        let dir = test_dir("publish");
        let source = dir.join("source");
        let destination = dir.join("destination");
        fs::write(&source, b"verified payload").unwrap();
        let digest = sha256_file(&source).unwrap().0;
        assert_eq!(
            verified_copy(&source, &destination, &digest).unwrap(),
            Materialization::AtomicRename
        );
        assert_eq!(
            verified_copy(&source, &destination, &digest).unwrap(),
            Materialization::ReusedVerified
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refuses_an_existing_conflicting_destination() {
        let dir = test_dir("conflict");
        let source = dir.join("source");
        let destination = dir.join("destination");
        fs::write(&source, b"verified payload").unwrap();
        fs::write(&destination, b"partial").unwrap();
        let digest = sha256_file(&source).unwrap().0;
        let error = verified_copy(&source, &destination, &digest)
            .unwrap_err()
            .to_string();
        assert!(error.contains("staged artifact conflict"), "{error}");
        assert_eq!(fs::read(&destination).unwrap(), b"partial");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn verified_write_never_accepts_different_existing_content() {
        let dir = test_dir("write");
        let destination = dir.join("grub.cfg");
        verified_write(&destination, b"first").unwrap();
        assert!(verified_write(&destination, b"second").is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"first");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn verified_replace_publishes_and_replaces_exact_content() {
        let root = test_dir("replace");
        let destination = root.join("cidata.json");

        let first = verified_replace(&destination, b"first").unwrap();
        assert_eq!(first, format!("{:x}", Sha256::digest(b"first")));
        let second = verified_replace(&destination, b"second").unwrap();
        assert_eq!(second, format!("{:x}", Sha256::digest(b"second")));
        assert_eq!(fs::read(&destination).unwrap(), b"second");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn digest_input_is_canonicalized() {
        let upper = "AB".repeat(32);
        assert_eq!(normalize_sha256(&upper).unwrap(), "ab".repeat(32));
        assert!(normalize_sha256("not-a-digest").is_err());
    }
}
