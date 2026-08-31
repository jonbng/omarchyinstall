//! Windows volume UniqueId (`\\?\Volume{…}\`) vs GPT partition GUID (Linux PARTUUID).
//! Pure string conversion — no Win32.

use crate::error::{Error, Result};

/// Build `\\?\Volume{guid}\` for CreateFile / `fs::write`.
/// Get-Volume UniqueId is already this form; do not wrap it again.
pub fn windows_volume_path(unique_id_or_guid: &str) -> Result<String> {
    let s = unique_id_or_guid.trim();
    if let Some(inner) = volume_unique_id_guid(s) {
        return Ok(format!(r"\\?\Volume{{{inner}}}\"));
    }
    let guid = parse_raw_guid(s)?;
    Ok(format!(r"\\?\Volume{{{guid}}}\"))
}

/// Linux `PARTUUID=` / `img_dev=PARTUUID=` is the GPT **partition** GUID
/// (`Get-Partition.Guid`), never Get-Volume UniqueId.
pub fn gpt_partuuid(partition_guid: &str) -> Result<String> {
    if volume_unique_id_guid(partition_guid).is_some() {
        return Err(Error::Message(
            "Get-Volume UniqueId is not a GPT PARTUUID; use Get-Partition.Guid".into(),
        ));
    }
    parse_raw_guid(partition_guid)
}

fn volume_unique_id_guid(s: &str) -> Option<String> {
    let n = s.trim();
    let rest = n
        .strip_prefix(r"\\?\Volume{")
        .or_else(|| n.strip_prefix(r"\\?\volume{"))?;
    let guid = rest.trim_end_matches('\\').trim_end_matches('}');
    parse_raw_guid(guid).ok()
}

fn parse_raw_guid(s: &str) -> Result<String> {
    let g = s
        .trim()
        .trim_matches(|c| c == '{' || c == '}' || c == '\\')
        .to_ascii_lowercase();
    // 8-4-4-4-12
    let hex: String = g.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Message(format!("not a GPT GUID: {s}")));
    }
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPT: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    const UNIQUE: &str = r"\\?\Volume{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\";

    #[test]
    fn unique_id_is_not_rewrapped() {
        assert_eq!(windows_volume_path(UNIQUE).unwrap(), UNIQUE);
        assert_eq!(
            windows_volume_path(r"\\?\Volume{AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE}").unwrap(),
            UNIQUE
        );
    }

    #[test]
    fn bare_guid_becomes_volume_path() {
        assert_eq!(windows_volume_path(&format!("{{{GPT}}}")).unwrap(), UNIQUE);
        assert_eq!(windows_volume_path(GPT).unwrap(), UNIQUE);
    }

    #[test]
    fn partuuid_from_partition_guid() {
        assert_eq!(gpt_partuuid(&format!("{{{GPT}}}")).unwrap(), GPT);
        assert_eq!(gpt_partuuid(GPT).unwrap(), GPT);
    }

    #[test]
    fn unique_id_is_rejected_as_partuuid() {
        let err = gpt_partuuid(UNIQUE).unwrap_err().to_string();
        assert!(err.contains("PARTUUID"), "{err}");
        assert!(err.contains("UniqueId"), "{err}");
    }

    #[test]
    fn emit_grub_uses_gpt_guid_not_volume_path() {
        let partuuid = gpt_partuuid(&format!("{{{GPT}}}")).unwrap();
        let cfg = crate::grub::emit_grub_cfg(&partuuid, 6 * 1024 * 1024 * 1024);
        assert!(cfg.contains(&format!("img_dev=PARTUUID={GPT}")));
        assert!(!cfg.contains(r"\\?\Volume"), "{cfg}");
        assert!(gpt_partuuid(UNIQUE).is_err());
    }
}
