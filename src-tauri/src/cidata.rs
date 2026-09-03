//! Full-disk autoinstall files for `omarchy-cidata-load`.

use crate::error::{Error, Result};
use crate::probe::{GIB, MIB};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha_crypt::{sha512_simple, Sha512Params};
use zeroize::Zeroize;

pub const ESP_OBJ_ID: &str = "ea21d3f2-82bb-49cc-ab5d-6f81ae94e18d";
pub const ROOT_OBJ_ID: &str = "8c2c2b92-1070-455d-b76a-56263bab24aa";
/// Temporary, deliberately narrow workaround while omarchy-iso#142 is pending.
pub const WINDOWS_VM_BY_ID: &str = "/dev/disk/by-id/ata-QEMU_HARDDISK_QM00001";
pub const WINDOWS_VM_ARCHINSTALL_DEVICE: &str = "/dev/sda";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CidataIdentity {
    pub username: String,
    pub password: String,
    pub hostname: String,
    pub timezone: String,
    pub keyboard: String,
    pub encrypt: bool,
    pub full_name: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug)]
pub struct CidataFiles {
    pub user_configuration: String,
    pub user_credentials: String,
    pub user_full_name: Option<String>,
    pub user_email: Option<String>,
    pub user_encrypt_installation: String,
    pub password_hash: String,
}

/// SHA-512 crypt (`$6$`), same family as `openssl passwd -6`.
pub fn sha512_crypt(password: &str) -> Result<String> {
    let params = Sha512Params::new(5_000)
        .map_err(|e| Error::Message(format!("sha512 crypt params: {e:?}")))?;
    sha512_simple(password, &params).map_err(|e| Error::Message(format!("sha512 crypt: {e:?}")))
}

pub fn check_sha512_crypt(password: &str, hash: &str) -> Result<()> {
    sha_crypt::sha512_check(password, hash)
        .map_err(|_| Error::Message("sha512 crypt verify failed".into()))
}

pub fn assert_linux_by_id(device: &str) -> Result<()> {
    if !device.starts_with("/dev/disk/by-id/") {
        return Err(Error::Message(format!(
            "cidata device must be /dev/disk/by-id/…, got {device}"
        )));
    }
    if device.to_ascii_lowercase().contains("physicaldrive") {
        return Err(Error::Message(format!(
            "cidata device must not be a Windows PhysicalDrive path: {device}"
        )));
    }
    Ok(())
}

/// Archinstall 4.4 drops `/dev/disk/by-id` device keys.  Only the measured
/// Windows/QEMU fixture is allowed to use its known canonical kernel name.
pub fn windows_vm_archinstall_device(device: &str) -> Result<&'static str> {
    assert_linux_by_id(device)?;
    if device != WINDOWS_VM_BY_ID {
        return Err(Error::Message(format!(
            "temporary VM build only supports {WINDOWS_VM_BY_ID}; probed {device}"
        )));
    }
    Ok(WINDOWS_VM_ARCHINSTALL_DEVICE)
}

pub const MIN_PASSWORD: usize = 6;
const RESERVED_USERNAMES: &[&str] = &[
    "root",
    "bin",
    "daemon",
    "mail",
    "ftp",
    "http",
    "nobody",
    "dbus",
    "git",
    "alpm",
    "avahi",
    "brltty",
    "cups",
    "cups-browsed",
    "gluster",
    "libvirt-qemu",
    "lp",
    "nvidia-persistenced",
    "pcscd",
    "polkitd",
    "qemu",
    "rpc",
    "rtkit",
    "sddm",
    "_talkd",
    "systemd-coredump",
    "systemd-journal-remote",
    "systemd-network",
    "systemd-oom",
    "systemd-resolve",
    "systemd-timesync",
    "tss",
    "uuidd",
];
const KEYBOARD_LAYOUTS: &[&str] = &[
    "us",
    "uk",
    "dvorak",
    "colemak",
    "azerty",
    "by",
    "be-latin1",
    "bg-cp1251",
    "croat",
    "cz",
    "dk-latin1",
    "nl",
    "et",
    "fi",
    "fr",
    "cf",
    "fr_CH",
    "ge",
    "de",
    "de_CH-latin1",
    "gr",
    "il",
    "hu",
    "is-latin1",
    "ie",
    "it",
    "jp106",
    "kazakh",
    "kyrgyz",
    "la-latin1",
    "lv",
    "lt",
    "mk-utf",
    "no-latin1",
    "pl",
    "pt-latin1",
    "br-abnt2",
    "ro",
    "ru",
    "sr-latin",
    "sk-qwertz",
    "slovene",
    "es",
    "sv-latin1",
    "tj_alt-UTF8",
    "trq",
    "ua",
];

pub fn assert_username(username: &str) -> Result<()> {
    let username = username.trim();
    if username.is_empty() {
        return Err(Error::Message("username is required".into()));
    }
    if RESERVED_USERNAMES.contains(&username) {
        return Err(Error::Message("username is reserved by the system".into()));
    }
    let b = username.as_bytes();
    if b.len() > 32 {
        return Err(Error::Message("username is too long".into()));
    }
    let first = b[0];
    if !(first.is_ascii_lowercase() || first == b'_')
        || !b
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
    {
        return Err(Error::Message(
            "username must be lowercase letters, digits, _ or -, starting with a letter or _"
                .into(),
        ));
    }
    Ok(())
}

pub fn assert_hostname(hostname: &str) -> Result<()> {
    let hostname = hostname.trim();
    if hostname.is_empty() {
        return Err(Error::Message("hostname is required".into()));
    }
    let b = hostname.as_bytes();
    if b.len() > 63 {
        return Err(Error::Message("hostname is too long".into()));
    }
    let first = b[0];
    let last = b[b.len() - 1];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit())
        || last == b'-'
        || !b
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
    {
        return Err(Error::Message(
            "hostname must be a DNS label (lowercase letters, digits, hyphens)".into(),
        ));
    }
    Ok(())
}

pub fn assert_password(password: &str) -> Result<()> {
    if password.is_empty() {
        return Err(Error::Message("password is required".into()));
    }
    if password.len() < MIN_PASSWORD {
        return Err(Error::Message(format!(
            "password must be at least {MIN_PASSWORD} characters"
        )));
    }
    Ok(())
}

pub fn assert_keyboard(keyboard: &str) -> Result<()> {
    if !KEYBOARD_LAYOUTS.contains(&keyboard) {
        return Err(Error::Message("unsupported keyboard layout".into()));
    }
    Ok(())
}

pub fn assert_timezone(timezone: &str) -> Result<()> {
    let valid_chars = timezone
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'_' | b'-' | b'+'));
    if timezone.is_empty()
        || timezone.len() > 64
        || !valid_chars
        || timezone.starts_with('/')
        || timezone.ends_with('/')
        || timezone.contains("//")
    {
        return Err(Error::Message("invalid timezone".into()));
    }
    Ok(())
}

pub fn assert_git_identity(full_name: &Option<String>, email: &Option<String>) -> Result<()> {
    if let Some(name) = full_name {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
            return Err(Error::Message("invalid Git author name".into()));
        }
    }
    if let Some(email) = email {
        let valid = email.len() <= 254
            && !email.chars().any(char::is_whitespace)
            && email.split_once('@').is_some_and(|(local, domain)| {
                !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
            });
        if !valid {
            return Err(Error::Message("invalid Git author email".into()));
        }
    }
    Ok(())
}

pub fn build_cidata_files(
    identity: &CidataIdentity,
    install_device: &str,
    disk_bytes: u64,
) -> Result<CidataFiles> {
    if install_device != WINDOWS_VM_ARCHINSTALL_DEVICE {
        assert_linux_by_id(install_device)?;
    }
    assert_username(&identity.username)?;
    assert_hostname(&identity.hostname)?;
    assert_password(&identity.password)?;
    assert_keyboard(&identity.keyboard)?;
    assert_timezone(&identity.timezone)?;
    assert_git_identity(&identity.full_name, &identity.email)?;
    let mut password = identity.password.clone();
    let hash = match sha512_crypt(&password) {
        Ok(hash) => hash,
        Err(e) => {
            password.zeroize();
            return Err(e);
        }
    };
    if !hash.starts_with("$6$") {
        password.zeroize();
        return Err(Error::Message("hasher did not produce $6$".into()));
    }

    let boot_start = MIB;
    let boot_size = 2 * GIB;
    let main_start = boot_start + boot_size;
    let gpt_backup = MIB;
    let main_size = disk_bytes.saturating_sub(main_start + gpt_backup);

    let mut config = json!({
        "app_config": null,
        "archinstall-language": "English",
        "auth_config": {},
        "audio_config": { "audio": "pipewire" },
        "bootloader_config": { "bootloader": "Limine", "uki": false, "removable": false },
        "custom_commands": [],
        "omarchy_install": {
            "mode": "full_disk",
            "defer_provisioning": false,
            "target_mount": "/mnt",
            "boot": {
                "esp_mount": "/boot",
                "esp_path": "/EFI/limine",
                "efi_binary": "limine_x64.efi",
                "enable_fallback": true
            },
            "storage": { "kernel": "linux" }
        },
        "disk_config": {
            "config_type": "default_layout",
            "device_modifications": [{
                "device": install_device,
                "wipe": true,
                "partitions": [
                    {
                        "btrfs": [],
                        "dev_path": null,
                        "flags": ["boot", "esp"],
                        "fs_type": "fat32",
                        "mount_options": [],
                        "mountpoint": "/boot",
                        "obj_id": ESP_OBJ_ID,
                        "size": { "sector_size": { "unit": "B", "value": 512 }, "unit": "B", "value": boot_size },
                        "start": { "sector_size": { "unit": "B", "value": 512 }, "unit": "B", "value": boot_start },
                        "status": "create",
                        "type": "primary"
                    },
                    {
                        "btrfs": [
                            { "mountpoint": "/", "name": "@" },
                            { "mountpoint": "/home", "name": "@home" },
                            { "mountpoint": "/var/log", "name": "@log" },
                            { "mountpoint": "/var/cache/pacman/pkg", "name": "@pkg" }
                        ],
                        "dev_path": null,
                        "flags": [],
                        "fs_type": "btrfs",
                        "mount_options": ["compress=zstd"],
                        "mountpoint": null,
                        "obj_id": ROOT_OBJ_ID,
                        "size": { "sector_size": { "unit": "B", "value": 512 }, "unit": "B", "value": main_size },
                        "start": { "sector_size": { "unit": "B", "value": 512 }, "unit": "B", "value": main_start },
                        "status": "create",
                        "type": "primary"
                    }
                ]
            }]
        },
        "hostname": identity.hostname,
        "kernels": ["linux"],
        "network_config": { "type": "iso" },
        "ntp": true,
        "parallel_downloads": 8,
        "script": null,
        "services": [],
        "swap": true,
        "timezone": identity.timezone,
        "locale_config": {
            "kb_layout": identity.keyboard,
            "sys_enc": "UTF-8",
            "sys_lang": "en_US.UTF-8"
        }
    });

    if identity.encrypt {
        config["disk_config"]["disk_encryption"] = json!({
            "encryption_type": "luks",
            "lvm_volumes": [],
            "iter_time": 2000,
            "partitions": [ROOT_OBJ_ID],
            "encryption_password": password
        });
    }

    let mut creds = json!({
        "root_enc_password": hash,
        "users": [{
            "enc_password": hash,
            "groups": [],
            "sudo": true,
            "username": identity.username
        }]
    });
    if identity.encrypt {
        creds["encryption_password"] = Value::String(password.clone());
    }

    password.zeroize();

    Ok(CidataFiles {
        user_configuration: serde_json::to_string_pretty(&config)
            .map_err(|e| Error::Message(e.to_string()))?,
        user_credentials: serde_json::to_string_pretty(&creds)
            .map_err(|e| Error::Message(e.to_string()))?,
        user_full_name: identity.full_name.clone(),
        user_email: identity.email.clone(),
        user_encrypt_installation: if identity.encrypt {
            "true\n"
        } else {
            "false\n"
        }
        .into(),
        password_hash: hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident() -> CidataIdentity {
        CidataIdentity {
            username: "dhh".into(),
            password: "secret-pass".into(),
            hostname: "omarchy".into(),
            timezone: "UTC".into(),
            keyboard: "us".into(),
            encrypt: true,
            full_name: Some("DHH".into()),
            email: Some("dhh@example.com".into()),
        }
    }

    #[test]
    fn hash_is_sha512_crypt_prefix() {
        let hash = sha512_crypt("secret-pass").unwrap();
        assert!(hash.starts_with("$6$"), "{hash}");
        check_sha512_crypt("secret-pass", &hash).unwrap();
        assert!(check_sha512_crypt("wrong", &hash).is_err());
    }

    #[test]
    fn openssl_passwd_6_vector_when_present() {
        let out = std::process::Command::new("openssl")
            .args(["passwd", "-6", "-salt", "testsalt1234567", "secret-pass"])
            .output();
        let Ok(out) = out else { return };
        if !out.status.success() {
            return;
        }
        let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(hash.starts_with("$6$"), "{hash}");
        check_sha512_crypt("secret-pass", &hash)
            .unwrap_or_else(|e| panic!("shipped checker must accept openssl passwd -6: {e}"));
    }

    #[test]
    fn json_is_full_disk_wipe_by_id() {
        let files =
            build_cidata_files(&ident(), "/dev/disk/by-id/nvme-VENDOR_DISK_1234", 512 * GIB)
                .unwrap();
        let cfg: Value = serde_json::from_str(&files.user_configuration).unwrap();
        assert_eq!(cfg["omarchy_install"]["mode"], "full_disk");
        let disk = &cfg["disk_config"]["device_modifications"][0];
        assert_eq!(disk["wipe"], true);
        let device = disk["device"].as_str().unwrap();
        assert!(device.starts_with("/dev/disk/by-id/"), "{device}");
        assert!(!device.contains("PhysicalDrive"));
        assert!(files.password_hash.starts_with("$6$"));
        let creds: Value = serde_json::from_str(&files.user_credentials).unwrap();
        assert!(creds["root_enc_password"]
            .as_str()
            .unwrap()
            .starts_with("$6$"));
        assert_eq!(creds["encryption_password"], "secret-pass");
        assert_eq!(files.user_encrypt_installation.trim(), "true");
        assert_eq!(files.user_full_name.as_deref(), Some("DHH"));
        assert_eq!(files.user_email.as_deref(), Some("dhh@example.com"));
    }

    #[test]
    fn rejects_windows_physicaldrive() {
        let err = build_cidata_files(&ident(), r"\\.\PhysicalDrive0", 512 * GIB)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("PhysicalDrive") || err.contains("by-id"),
            "{err}"
        );
    }

    #[test]
    fn exact_windows_vm_uses_archinstall_canonical_device() {
        assert_eq!(
            windows_vm_archinstall_device(WINDOWS_VM_BY_ID).unwrap(),
            "/dev/sda"
        );
        assert!(windows_vm_archinstall_device("/dev/disk/by-id/ata-QEMU_HARDDISK_OTHER").is_err());
        let files = build_cidata_files(
            &ident(),
            windows_vm_archinstall_device(WINDOWS_VM_BY_ID).unwrap(),
            64 * GIB,
        )
        .unwrap();
        let cfg: Value = serde_json::from_str(&files.user_configuration).unwrap();
        assert_eq!(
            cfg["disk_config"]["device_modifications"][0]["device"],
            "/dev/sda"
        );
    }

    #[test]
    fn rejects_bad_identity() {
        let mut bad = ident();
        bad.username = "Root".into();
        assert!(build_cidata_files(&bad, "/dev/disk/by-id/nvme-x", 512 * GIB).is_err());
        bad = ident();
        bad.username = "root".into();
        assert!(build_cidata_files(&bad, "/dev/disk/by-id/nvme-x", 512 * GIB).is_err());
        bad = ident();
        bad.hostname = "-nope".into();
        assert!(build_cidata_files(&bad, "/dev/disk/by-id/nvme-x", 512 * GIB).is_err());
        bad = ident();
        bad.password = "12345".into();
        assert!(build_cidata_files(&bad, "/dev/disk/by-id/nvme-x", 512 * GIB).is_err());
        bad = ident();
        bad.keyboard = "not-a-keymap".into();
        assert!(build_cidata_files(&bad, "/dev/disk/by-id/nvme-x", 512 * GIB).is_err());
        bad = ident();
        bad.timezone = "../../etc/passwd".into();
        assert!(build_cidata_files(&bad, "/dev/disk/by-id/nvme-x", 512 * GIB).is_err());
        bad = ident();
        bad.full_name = Some("Bad\nName".into());
        assert!(build_cidata_files(&bad, "/dev/disk/by-id/nvme-x", 512 * GIB).is_err());
        bad = ident();
        bad.email = Some("not-an-email".into());
        assert!(build_cidata_files(&bad, "/dev/disk/by-id/nvme-x", 512 * GIB).is_err());
    }
}
