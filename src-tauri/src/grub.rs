//! Official-GRUB bait discovery and the ESP `boot/grub/grub.cfg` we emit.

use crate::error::{Error, Result};
use crate::partition::copytoram_size_spec;
use std::collections::BTreeSet;

/// Paths we may write on the ESP. Never `EFI/Microsoft` or `EFI/Boot/bootx64.efi`.
pub const ESP_GRUB_EFI: &str = "EFI/OmarchyInstall/BOOTX64.EFI";
pub const ESP_GRUB_CFG: &str = "boot/grub/grub.cfg";
pub const ISO_LOOP_PATH: &str = "/omarchy.iso";

pub fn assert_esp_write_allowed(relative: &str) -> Result<()> {
    let n = relative.replace('\\', "/").to_ascii_lowercase();
    let n = n.trim_start_matches('/');
    if n.starts_with("efi/microsoft/") || n == "efi/microsoft" {
        return Err(Error::Message(format!(
            "refusing ESP write to {relative}: never touch EFI/Microsoft"
        )));
    }
    if n == "efi/boot/bootx64.efi" {
        return Err(Error::Message(format!(
            "refusing ESP write to {relative}: never overwrite EFI/Boot/bootx64.efi"
        )));
    }
    Ok(())
}

/// Pick the baked `ARCHISO_SEARCH_FILENAME` from ISO/Joliet/RR paths.
/// Unique `*.uuid`; do not hardcode `/.disk/`. Prefer `/boot/` when several match.
pub fn discover_search_filename<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<String> {
    let mut uniq = BTreeSet::new();
    for raw in paths {
        let p = normalize_iso_path(raw);
        if p.ends_with(".uuid") {
            uniq.insert(p);
        }
    }
    select_uuid_path(uniq)
}

pub fn discover_search_filename_in_bytes(blob: &[u8]) -> Result<String> {
    if let Ok(from_env) = search_filename_from_grubenv(blob) {
        return Ok(from_env);
    }
    let mut uniq = BTreeSet::new();
    for candidate in ascii_uuid_paths(blob) {
        uniq.insert(candidate);
    }
    select_uuid_path(uniq)
}

pub fn search_filename_from_grubenv(blob: &[u8]) -> Result<String> {
    let text = String::from_utf8_lossy(blob);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("ARCHISO_SEARCH_FILENAME=") {
            let p = normalize_iso_path(rest.trim());
            if p.ends_with(".uuid") {
                return Ok(p);
            }
        }
    }
    Err(Error::Message(
        "grubenv has no ARCHISO_SEARCH_FILENAME".into(),
    ))
}

fn select_uuid_path(uniq: BTreeSet<String>) -> Result<String> {
    if uniq.len() == 1 {
        return Ok(uniq.into_iter().next().unwrap());
    }
    let boot: Vec<String> = uniq
        .iter()
        .filter(|p| p.starts_with("/boot/"))
        .cloned()
        .collect();
    if boot.len() == 1 {
        return Ok(boot.into_iter().next().unwrap());
    }
    if uniq.is_empty() {
        return Err(Error::Message("ISO has no *.uuid search bait".into()));
    }
    Err(Error::Message(format!(
        "ISO has {} *.uuid files; need a unique bait path",
        uniq.len()
    )))
}

fn normalize_iso_path(p: &str) -> String {
    let p = p.trim().replace('\\', "/");
    if p.starts_with('/') {
        p
    } else {
        format!("/{p}")
    }
}

fn ascii_uuid_paths(blob: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let needle = b".uuid";
    let mut i = 0;
    while i + needle.len() <= blob.len() {
        if &blob[i..i + needle.len()] == needle {
            let mut start = i;
            while start > 0 {
                let c = blob[start - 1];
                let pathish = c.is_ascii_alphanumeric() || matches!(c, b'/' | b'-' | b'_' | b'.');
                if !pathish {
                    break;
                }
                start -= 1;
                if i - start > 200 {
                    break;
                }
            }
            let s = String::from_utf8_lossy(&blob[start..i + needle.len()]).into_owned();
            if s.ends_with(".uuid") && (s.starts_with('/') || s.starts_with("boot/")) {
                out.push(normalize_iso_path(&s));
            }
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Destinations on the ESP for this staging run (relative, forward slashes).
pub fn esp_stage_destinations(search_filename: &str) -> Result<Vec<String>> {
    esp_rollback_relpaths(Some(search_filename))
}

/// Files rollback must delete on the ESP, including the journaled search bait.
pub fn esp_rollback_relpaths(search_filename: Option<&str>) -> Result<Vec<String>> {
    let mut dests = vec![ESP_GRUB_EFI.to_string(), ESP_GRUB_CFG.to_string()];
    if let Some(bait) = search_filename {
        dests.push(normalize_iso_path(bait).trim_start_matches('/').to_string());
    }
    for d in &dests {
        assert_esp_write_allowed(d)?;
    }
    Ok(dests)
}

/// Windows `Remove-Item` relative path for the journaled bait (backslashes).
pub fn esp_bait_windows_path(search_filename: &str) -> Result<String> {
    let rel = normalize_iso_path(search_filename)
        .trim_start_matches('/')
        .replace('/', "\\");
    assert_esp_write_allowed(&rel)?;
    Ok(rel)
}

pub fn emit_grub_cfg(partuuid: &str, iso_bytes: u64) -> String {
    let size = copytoram_size_spec(iso_bytes);
    let guid = partuuid.trim().trim_matches(|c| c == '{' || c == '}');
    format!(
        r#"insmod part_gpt
insmod ntfs
insmod ntfscomp
insmod iso9660
insmod loopback

search --no-floppy --set=img_part --file {ISO_LOOP_PATH}
set iso_path="{ISO_LOOP_PATH}"
export iso_path
loopback loop (${{img_part}})${{iso_path}}
set root=(loop)

set default=0
set timeout=0

menuentry "Omarchy Installer" --id 'archlinux' {{
    set gfxpayload=keep
    linux /arch/boot/x86_64/vmlinuz-linux-t2 \
        archisobasedir=arch \
        img_dev=PARTUUID={guid} \
        img_loop="${{iso_path}}" \
        copytoram=y \
        copytoram_size={size} \
        splash xe.enable_panel_replay=0 initramfs_async=0
    initrd /arch/boot/x86_64/initramfs-linux-t2.img
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grub_cfg_has_decision_e_strings() {
        let guid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let cfg = emit_grub_cfg(guid, 6 * 1024 * 1024 * 1024);
        assert!(cfg.contains("copytoram=y"), "{cfg}");
        assert!(!cfg.contains("copytoram=auto"), "{cfg}");
        assert!(cfg.contains("img_loop="), "{cfg}");
        assert!(cfg.contains(&format!("img_dev=PARTUUID={guid}")), "{cfg}");
        assert!(!cfg.contains("img_dev=UUID="), "{cfg}");
        assert!(!cfg.contains("/.disk/"), "{cfg}");
        assert!(!cfg.contains("quiet"), "{cfg}");
    }

    #[test]
    fn bait_discovery_uses_boot_uuid_not_dot_disk() {
        let path = discover_search_filename([
            "/EFI/BOOT/BOOTX64.EFI",
            "/boot/archiso.uuid",
            "/arch/boot/x86_64/vmlinuz-linux-t2",
        ])
        .unwrap();
        assert_eq!(path, "/boot/archiso.uuid");
        assert!(!path.contains(".disk"));
    }

    #[test]
    fn bait_discovery_does_not_hardcode_dot_disk() {
        let from_blob =
            discover_search_filename_in_bytes(b"noise /boot/cafef00d.uuid more /.disk/ignored.txt")
                .unwrap();
        assert_eq!(from_blob, "/boot/cafef00d.uuid");
        assert!(!from_blob.contains(".disk"));
    }

    #[test]
    fn grubenv_fallback() {
        let env = b"# GRUB Environment Block\nARCHISO_SEARCH_FILENAME=/boot/deadbeef.uuid\n";
        assert_eq!(
            search_filename_from_grubenv(env).unwrap(),
            "/boot/deadbeef.uuid"
        );
    }

    #[test]
    fn esp_plan_never_touches_microsoft_or_fallback() {
        let dests = esp_stage_destinations("/boot/deadbeef.uuid").unwrap();
        for d in &dests {
            assert_esp_write_allowed(d).unwrap();
            let n = d.to_ascii_lowercase();
            assert!(!n.contains("microsoft"), "{d}");
            assert!(!n.contains("boot/bootx64.efi"), "{d}");
        }
        assert!(dests.iter().any(|d| d == ESP_GRUB_CFG));
        assert!(dests.iter().any(|d| d == "boot/deadbeef.uuid"));
    }

    #[test]
    fn microsoft_write_is_rejected() {
        assert!(assert_esp_write_allowed("EFI/Microsoft/Boot/bootmgfw.efi").is_err());
        assert!(assert_esp_write_allowed("EFI/Boot/bootx64.efi").is_err());
    }

    #[test]
    fn rollback_list_includes_journaled_bait() {
        let files = esp_rollback_relpaths(Some("/boot/deadbeef.uuid")).unwrap();
        assert!(files.iter().any(|f| f == "boot/deadbeef.uuid"), "{files:?}");
        assert!(files.iter().any(|f| f == ESP_GRUB_CFG), "{files:?}");
        assert!(files.iter().any(|f| f == ESP_GRUB_EFI), "{files:?}");
        let win = esp_bait_windows_path("/boot/deadbeef.uuid").unwrap();
        assert_eq!(win, r"boot\deadbeef.uuid");
        assert!(!win.to_ascii_lowercase().contains("microsoft"));
    }
}
