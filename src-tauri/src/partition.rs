//! Installer-partition sizing. OMARCHYINST is NTFS; cidata is a small FAT32 volume.

use crate::error::{Error, Result};
use crate::probe::{GIB, MIB};

/// Floor for the ISO payload partition.
pub const OMARCHYINST_MIN: u64 = 8 * GIB;
/// Second volume for `omarchy-cidata-load` (`/dev/disk/by-label/cidata`).
pub const CIDATA_BYTES: u64 = 64 * MIB;

/// `max(8 GiB, iso_size * 1.2 + 512 MiB)`.
pub fn omarchyinst_bytes(iso_size: u64) -> u64 {
    let from_iso = iso_size.saturating_mul(12) / 10 + 512 * MIB;
    align_up(from_iso.max(OMARCHYINST_MIN), MIB)
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.saturating_add(alignment - 1) / alignment * alignment
}

pub fn installer_hole_bytes(iso_size: u64) -> u64 {
    omarchyinst_bytes(iso_size).saturating_add(CIDATA_BYTES)
}

/// Fail if the verified ISO plus 15% does not fit the chosen payload partition.
pub fn iso_fits_omarchyinst(iso_size: u64, partition_bytes: u64) -> Result<()> {
    let need = iso_size.saturating_mul(115) / 100;
    if partition_bytes < need {
        return Err(Error::Message(format!(
            "OMARCHYINST is {partition_bytes} bytes; ISO {iso_size} plus 15% needs {need}"
        )));
    }
    Ok(())
}

/// Payload partition must be NTFS. FAT32 cannot hold a 6 GiB ISO file.
pub fn require_omarchyinst_fs(fs: &str) -> Result<()> {
    let n = fs.trim().to_ascii_lowercase();
    if n == "fat32" || n == "fat" || n == "vfat" || n == "exfat" {
        return Err(Error::Message(format!(
            "OMARCHYINST cannot be {fs}: FAT32/exFAT cannot hold the ~6 GiB ISO (4 GiB file cap)"
        )));
    }
    if n != "ntfs" {
        return Err(Error::Message(format!(
            "OMARCHYINST must be NTFS, not {fs}"
        )));
    }
    Ok(())
}

pub fn require_cidata_fs(fs: &str) -> Result<()> {
    let n = fs.trim().to_ascii_lowercase();
    if n != "fat32" && n != "fat" && n != "vfat" {
        return Err(Error::Message(format!(
            "cidata volume must be FAT32, not {fs}"
        )));
    }
    Ok(())
}

pub fn copytoram_size_spec(iso_bytes: u64) -> String {
    let iso_gib = iso_bytes.div_ceil(GIB);
    let n = iso_gib.saturating_add(2).max(8);
    format!("{n}G")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_is_8_gib_for_small_isos() {
        assert_eq!(omarchyinst_bytes(1 * GIB), 8 * GIB);
        assert_eq!(installer_hole_bytes(1 * GIB), 8 * GIB + 64 * MIB);
    }

    #[test]
    fn grows_with_iso() {
        let iso = 10 * GIB;
        let part = omarchyinst_bytes(iso);
        assert!(part > 8 * GIB);
        assert_eq!(part, iso * 12 / 10 + 512 * MIB);
    }

    #[test]
    fn fat32_payload_is_rejected() {
        let err = require_omarchyinst_fs("FAT32").unwrap_err().to_string();
        assert!(err.contains("FAT32"), "{err}");
        assert!(require_omarchyinst_fs("ntfs").is_ok());
        assert!(require_omarchyinst_fs("NTFS").is_ok());
        assert!(require_cidata_fs("fat32").is_ok());
        assert!(iso_fits_omarchyinst(6 * GIB, 8 * GIB).is_ok());
        assert!(iso_fits_omarchyinst(8 * GIB, 8 * GIB).is_err());
    }

    #[test]
    fn copytoram_is_absolute_gib() {
        assert_eq!(copytoram_size_spec(6 * GIB), "8G");
        assert_eq!(copytoram_size_spec(9 * GIB), "11G");
    }
}
