// Read-only Windows Storage cmdlet shrink-capacity query.

use super::process::run_storage_powershell_read_only;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

const PROTOCOL_VERSION: u32 = 3;
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShrinkReport {
    pub protocol_version: u32,
    pub source: String,
    pub volume_guid: String,
    pub mount_path: String,
    pub disk_number: u32,
    pub partition_number: u32,
    pub partition_guid: String,
    pub partition_size_bytes: u64,
    pub minimum_size_bytes: u64,
    pub maximum_size_bytes: u64,
    pub elapsed_ms: u128,
    pub max_reclaimable_bytes: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PowerShellShrinkOutput {
    disk_number: u32,
    partition_number: u32,
    partition_guid: String,
    access_paths: Vec<String>,
    partition_size_bytes: u64,
    size_min_bytes: u64,
    size_max_bytes: u64,
}

pub(crate) fn query_max_reclaimable(volume_guid: &str, mount_path: &str) -> Result<ShrinkReport> {
    let report = query_report(volume_guid, mount_path)?;
    if report.max_reclaimable_bytes.is_none() {
        return Err(Error::Message("Windows returned no shrink capacity".into()));
    }
    Ok(report)
}

pub(crate) fn query_report(volume_guid: &str, mount_path: &str) -> Result<ShrinkReport> {
    let drive_letter = parse_drive_letter(mount_path)?;
    validate_volume_guid(volume_guid)?;
    let started = Instant::now();

    // The inserted value is a validated ASCII drive letter. Returned volume
    // identity and size bounds are independently checked below.
    let script = format!(
        r#"$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; $WarningPreference='SilentlyContinue'; $partitions=@(Get-Partition -DriveLetter {drive_letter} -ErrorAction Stop); if ($partitions.Count -ne 1) {{ throw 'drive letter did not resolve to exactly one partition' }}; $p=$partitions[0]; $supported=Get-PartitionSupportedSize -DiskNumber $p.DiskNumber -PartitionNumber $p.PartitionNumber -ErrorAction Stop; [pscustomobject]@{{ diskNumber=[uint32]$p.DiskNumber; partitionNumber=[uint32]$p.PartitionNumber; partitionGuid=[string]$p.Guid; accessPaths=@($p.AccessPaths); partitionSizeBytes=[uint64]$p.Size; sizeMinBytes=[uint64]$supported.SizeMin; sizeMaxBytes=[uint64]$supported.SizeMax }} | ConvertTo-Json -Compress -Depth 3"#
    );
    let stdout =
        run_storage_powershell_read_only(&script, QUERY_TIMEOUT, "Windows shrink-capacity query")?;
    let output: PowerShellShrinkOutput = serde_json::from_str(&stdout).map_err(|error| {
        Error::Message(format!(
            "Windows shrink-capacity query returned invalid JSON: {error}"
        ))
    })?;
    validate_output(&output, volume_guid, mount_path)?;
    let max_reclaimable_bytes = output
        .partition_size_bytes
        .checked_sub(output.size_min_bytes)
        .ok_or_else(|| {
            Error::Message("Windows returned a minimum size larger than the partition".into())
        })?;

    Ok(ShrinkReport {
        protocol_version: PROTOCOL_VERSION,
        source: "Get-PartitionSupportedSize".into(),
        volume_guid: volume_guid.into(),
        mount_path: mount_path.into(),
        disk_number: output.disk_number,
        partition_number: output.partition_number,
        partition_guid: output.partition_guid,
        partition_size_bytes: output.partition_size_bytes,
        minimum_size_bytes: output.size_min_bytes,
        maximum_size_bytes: output.size_max_bytes,
        elapsed_ms: started.elapsed().as_millis(),
        max_reclaimable_bytes: Some(max_reclaimable_bytes),
        error: None,
    })
}

fn validate_output(
    output: &PowerShellShrinkOutput,
    volume_guid: &str,
    mount_path: &str,
) -> Result<()> {
    if !output
        .access_paths
        .iter()
        .any(|path| same_volume(path, volume_guid))
    {
        return Err(Error::Message(
            "Windows shrink-capacity query returned the wrong volume".into(),
        ));
    }
    if !output
        .access_paths
        .iter()
        .any(|path| same_access_path(path, mount_path))
    {
        return Err(Error::Message(
            "Windows shrink-capacity query returned the wrong access path".into(),
        ));
    }
    if output.size_min_bytes > output.partition_size_bytes {
        return Err(Error::Message(
            "Windows returned a minimum size larger than the partition".into(),
        ));
    }
    if output.size_max_bytes < output.partition_size_bytes {
        return Err(Error::Message(
            "Windows returned a maximum size smaller than the partition".into(),
        ));
    }
    Ok(())
}

fn parse_drive_letter(mount_path: &str) -> Result<char> {
    let bytes = mount_path.trim().as_bytes();
    let valid_suffix = bytes.len() == 2 || (bytes.len() == 3 && matches!(bytes[2], b'\\' | b'/'));
    if !valid_suffix || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return Err(Error::Message(format!(
            "invalid Windows drive access path: {mount_path}"
        )));
    }
    Ok((bytes[0] as char).to_ascii_uppercase())
}

fn validate_volume_guid(volume_guid: &str) -> Result<()> {
    let Some(value) = volume_guid
        .strip_prefix(r"\\?\Volume{")
        .and_then(|value| value.strip_suffix(r"}\"))
    else {
        return Err(Error::Message(format!(
            "invalid Windows volume GUID path: {volume_guid}"
        )));
    };
    if value.len() != 36
        || !value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        return Err(Error::Message(format!(
            "invalid Windows volume GUID path: {volume_guid}"
        )));
    }
    Ok(())
}

fn same_volume(left: &str, right: &str) -> bool {
    left.trim_end_matches('\\')
        .eq_ignore_ascii_case(right.trim_end_matches('\\'))
}

fn same_access_path(left: &str, right: &str) -> bool {
    left.trim_end_matches(['\\', ':'])
        .eq_ignore_ascii_case(right.trim_end_matches(['\\', ':']))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> PowerShellShrinkOutput {
        PowerShellShrinkOutput {
            disk_number: 0,
            partition_number: 3,
            partition_guid: "{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}".into(),
            access_paths: vec![
                "C:\\".into(),
                r"\\?\Volume{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\".into(),
            ],
            partition_size_bytes: 100,
            size_min_bytes: 40,
            size_max_bytes: 100,
        }
    }

    #[test]
    fn inputs_are_strictly_validated() {
        assert_eq!(parse_drive_letter("c:\\").unwrap(), 'C');
        assert!(parse_drive_letter("C:\\Windows").is_err());
        assert!(validate_volume_guid(r"\\?\Volume{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\").is_ok());
        assert!(validate_volume_guid("C:\\").is_err());
    }

    #[test]
    fn output_must_match_identity_and_size_bounds() {
        let mut value = output();
        assert!(validate_output(
            &value,
            r"\\?\Volume{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\",
            "C:\\"
        )
        .is_ok());
        assert!(validate_output(
            &value,
            r"\\?\Volume{bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee}\",
            "C:\\"
        )
        .is_err());
        assert!(validate_output(
            &value,
            r"\\?\Volume{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\",
            "D:\\"
        )
        .is_err());
        value.size_min_bytes = 101;
        assert!(validate_output(
            &value,
            r"\\?\Volume{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\",
            "C:\\"
        )
        .is_err());
    }
}
