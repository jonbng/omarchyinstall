//! Immutable description of every disk identity and layout value approved by
//! the user before Windows is modified.

use crate::artifact;
use crate::error::{Error, Result};
use crate::partition;
use crate::platform::MachineProbe;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PLAN_VERSION: u32 = 1;
pub const ACKNOWLEDGEMENT_VERSION: u32 = 1;
pub const BOOT_CONTRACT_VERSION: u32 = 1;
pub const CIDATA_CONTRACT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlan {
    pub version: u32,
    pub target_disk_guid: String,
    pub target_disk_number: u32,
    pub target_disk_device_id: String,
    pub windows_partition_guid: String,
    pub windows_partition_offset_bytes: u64,
    pub old_c_size_bytes: u64,
    pub new_c_size_bytes: u64,
    pub esp_partition_guid: String,
    pub esp_volume_guid: String,
    pub omarchyinst_offset_bytes: u64,
    pub omarchyinst_size_bytes: u64,
    pub cidata_offset_bytes: u64,
    pub cidata_size_bytes: u64,
    pub linux_device: String,
    pub iso_sha256: String,
    pub iso_size_bytes: u64,
    pub boot_contract_version: u32,
    pub cidata_contract_version: u32,
    pub acknowledgement_version: u32,
    pub destructive_erase_acknowledged: bool,
    pub bitlocker_risk_acknowledged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlanPreview {
    pub plan: InstallPlan,
    pub digest: String,
}

pub fn build(
    probe: &MachineProbe,
    iso_sha256: &str,
    iso_size: u64,
    allow_bitlocker: bool,
) -> Result<InstallPlanPreview> {
    artifact::require_sha256(iso_sha256)?;
    if iso_size == 0 {
        return Err(Error::Message("verified ISO is empty".into()));
    }
    if !probe.blocking_reasons.is_empty() {
        return Err(Error::Message(
            "machine probe has blocking reasons; refuse to create an install plan".into(),
        ));
    }
    let esp = probe
        .target_esp
        .as_ref()
        .ok_or_else(|| Error::Message("target ESP identity missing".into()))?;
    let disk = probe
        .disks
        .iter()
        .find(|disk| disk.device_id == esp.disk_id)
        .ok_or_else(|| Error::Message("target boot disk disappeared".into()))?;
    let windows = disk
        .partitions
        .iter()
        .find(|partition| partition.letter.as_deref() == Some("C:"))
        .ok_or_else(|| Error::Message("C: partition is not on the target disk".into()))?;
    let windows_guid = windows
        .gpt_guid
        .clone()
        .ok_or_else(|| Error::Message("C: GPT partition GUID missing".into()))?;
    let linux_device = probe
        .linux_by_id
        .clone()
        .ok_or_else(|| Error::Message("linux /dev/disk/by-id path missing".into()))?;

    if probe.bitlocker.iter().any(|volume| !volume.fully_decrypted) && !allow_bitlocker {
        return Err(Error::Message(
            "BitLocker is still enabled. Turn it off (recommended), or explicitly accept the BitLocker recovery risk before continuing."
                .into(),
        ));
    }

    let omarchyinst_size = partition::omarchyinst_bytes(iso_size);
    partition::iso_fits_omarchyinst(iso_size, omarchyinst_size)?;
    let hole = partition::installer_hole_bytes(iso_size);
    let new_c_size = windows
        .size_bytes
        .checked_sub(hole)
        .ok_or_else(|| Error::Message("C: is smaller than the installer hole".into()))?;
    let omarchyinst_offset = windows
        .offset_bytes
        .checked_add(new_c_size)
        .ok_or_else(|| Error::Message("installer partition offset overflow".into()))?;
    let cidata_offset = omarchyinst_offset
        .checked_add(omarchyinst_size)
        .ok_or_else(|| Error::Message("cidata partition offset overflow".into()))?;

    let plan = InstallPlan {
        version: PLAN_VERSION,
        target_disk_guid: esp.disk_guid.clone(),
        target_disk_number: esp.disk_number,
        target_disk_device_id: disk.device_id.clone(),
        windows_partition_guid: windows_guid,
        windows_partition_offset_bytes: windows.offset_bytes,
        old_c_size_bytes: windows.size_bytes,
        new_c_size_bytes: new_c_size,
        esp_partition_guid: esp.partition_guid.clone(),
        esp_volume_guid: esp.volume_guid.clone(),
        omarchyinst_offset_bytes: omarchyinst_offset,
        omarchyinst_size_bytes: omarchyinst_size,
        cidata_offset_bytes: cidata_offset,
        cidata_size_bytes: partition::CIDATA_BYTES,
        linux_device,
        iso_sha256: iso_sha256.to_ascii_lowercase(),
        iso_size_bytes: iso_size,
        boot_contract_version: BOOT_CONTRACT_VERSION,
        cidata_contract_version: CIDATA_CONTRACT_VERSION,
        acknowledgement_version: ACKNOWLEDGEMENT_VERSION,
        destructive_erase_acknowledged: true,
        bitlocker_risk_acknowledged: allow_bitlocker,
    };
    let digest = digest(&plan)?;
    Ok(InstallPlanPreview { plan, digest })
}

pub fn digest(plan: &InstallPlan) -> Result<String> {
    let encoded = serde_json::to_vec(plan)
        .map_err(|error| Error::Message(format!("could not encode install plan: {error}")))?;
    let mut hasher = Sha256::new();
    hasher.update(b"omarchy-install-plan-v1\0");
    hasher.update(encoded);
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn require_digest(value: &str) -> Result<&str> {
    artifact::require_sha256(value)
        .map_err(|_| Error::Message("approved install-plan digest is invalid".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_deterministic_and_binds_every_field() {
        let mut plan = InstallPlan {
            version: 1,
            target_disk_guid: "disk".into(),
            target_disk_number: 0,
            target_disk_device_id: "disk-0".into(),
            windows_partition_guid: "windows".into(),
            windows_partition_offset_bytes: 1,
            old_c_size_bytes: 20,
            new_c_size_bytes: 10,
            esp_partition_guid: "esp".into(),
            esp_volume_guid: "volume".into(),
            omarchyinst_offset_bytes: 11,
            omarchyinst_size_bytes: 8,
            cidata_offset_bytes: 19,
            cidata_size_bytes: 1,
            linux_device: "/dev/disk/by-id/example".into(),
            iso_sha256: "a".repeat(64),
            iso_size_bytes: 7,
            boot_contract_version: 1,
            cidata_contract_version: 1,
            acknowledgement_version: 1,
            destructive_erase_acknowledged: true,
            bitlocker_risk_acknowledged: false,
        };
        let first = digest(&plan).unwrap();
        assert_eq!(first, digest(&plan).unwrap());
        plan.new_c_size_bytes += 1;
        assert_ne!(first, digest(&plan).unwrap());
    }
}
