mod browser;
mod cidata;
mod commands;
mod download;
mod error;
mod grub;
mod iso;
mod journal;
mod partition;
mod paths;
mod platform;
mod probe;
mod winvol;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin({
            let mut log = tauri_plugin_log::Builder::new()
                .level(if cfg!(debug_assertions) {
                    log::LevelFilter::Debug
                } else {
                    log::LevelFilter::Info
                })
                .timezone_strategy(tauri_plugin_log::TimezoneStrategy::UseLocal)
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stdout,
                ));
            if let Ok(dir) = paths::install_logs_dir() {
                log = log.target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Folder {
                        path: dir,
                        file_name: Some("omarchy-install".into()),
                    },
                ));
            }
            log.build()
        })
        .invoke_handler(tauri::generate_handler![
            commands::exit_app,
            commands::host_info,
            commands::probe_machine,
            commands::relaunch_elevated,
            commands::reboot_to_firmware,
            commands::load_install_state,
            commands::download_iso,
            commands::verify_iso,
            commands::prepare_installer_partition,
            commands::stage_bootloader,
            commands::write_cidata,
            commands::set_boot_next,
            commands::reboot_to_installer,
            commands::abort_and_rollback,
            commands::export_support_bundle,
        ])
        .setup(|app| {
            log::info!(
                "starting (os={}, native_windows={})",
                std::env::consts::OS,
                cfg!(windows)
            );

            let force_browser = std::env::args().any(|arg| arg == "--browser");
            let window_result = if force_browser {
                Err("browser mode requested".to_string())
            } else {
                let config = app
                    .config()
                    .app
                    .windows
                    .iter()
                    .find(|window| window.label == "main")
                    .cloned()
                    .ok_or_else(|| "main window configuration is missing".to_string());
                config.and_then(|config| {
                    tauri::WebviewWindowBuilder::from_config(app, &config)
                        .map_err(|e| e.to_string())?
                        .build()
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                })
            };

            if let Err(error) = window_result {
                if let Err(fallback_error) = browser::launch(app.handle(), &error) {
                    show_startup_error(&fallback_error);
                    return Err(std::io::Error::other(fallback_error).into());
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(windows)]
fn show_startup_error(message: &str) {
    use windows::{
        core::{w, PCWSTR},
        Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
    };
    let message: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            w!("Omarchy Installer could not start"),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_startup_error(message: &str) {
    eprintln!("Omarchy Installer could not start: {message}");
}

#[cfg(test)]
mod lib_calls {
    use crate::cidata::{build_cidata_files, check_sha512_crypt, sha512_crypt, CidataIdentity};
    use crate::grub::{discover_search_filename, emit_grub_cfg};
    use crate::journal::{parse_journal, serialize_journal};
    use crate::partition::require_omarchyinst_fs;
    use crate::platform;

    #[test]
    fn shipped_grub_cidata_journal_partition() {
        let cfg = emit_grub_cfg(
            "11111111-2222-3333-4444-555555555555",
            6 * 1024 * 1024 * 1024,
        );
        println!("grub.cfg:\n{cfg}");
        assert!(cfg.contains("copytoram=y"));
        assert!(cfg.contains("img_loop="));
        assert!(cfg.contains("img_dev=PARTUUID=11111111-2222-3333-4444-555555555555"));
        let bait = discover_search_filename(["/boot/cafef00d.uuid"]).unwrap();
        println!("bait={bait}");
        assert_eq!(bait, "/boot/cafef00d.uuid");
        assert!(!bait.contains(".disk"));

        let hash = sha512_crypt("vector-pass").unwrap();
        println!("crypt={hash}");
        assert!(hash.starts_with("$6$"), "{hash}");
        check_sha512_crypt("vector-pass", &hash).unwrap();

        let files = build_cidata_files(
            &CidataIdentity {
                username: "omarchy".into(),
                password: "vector-pass".into(),
                hostname: "box".into(),
                timezone: "UTC".into(),
                keyboard: "us".into(),
                encrypt: false,
                full_name: None,
                email: None,
            },
            "/dev/disk/by-id/nvme-TEST",
            256 * 1024 * 1024 * 1024,
        )
        .unwrap();
        println!("user_configuration.json:\n{}", files.user_configuration);
        assert!(
            files.user_configuration.contains("\"wipe\": true")
                || files.user_configuration.contains("\"wipe\":true")
        );
        assert!(files.user_configuration.contains("full_disk"));
        assert!(files
            .user_configuration
            .contains("/dev/disk/by-id/nvme-TEST"));
        assert!(files.password_hash.starts_with("$6$"));

        let j = crate::journal::empty_journal();
        let s = serialize_journal(&j).unwrap();
        assert!(!s.contains("password"));
        parse_journal(&s).unwrap();

        assert!(require_omarchyinst_fs("FAT32").is_err());
        assert_eq!(platform::is_stub_host(), !cfg!(windows));

        let unique = r"\\?\Volume{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}\";
        assert!(crate::winvol::gpt_partuuid(unique).is_err());
        let partuuid =
            crate::winvol::gpt_partuuid("{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}").unwrap();
        assert_eq!(partuuid, "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee");
        assert_eq!(crate::winvol::windows_volume_path(unique).unwrap(), unique);
        let bait = crate::grub::esp_rollback_relpaths(Some("/boot/deadbeef.uuid")).unwrap();
        assert!(bait.iter().any(|p| p == "boot/deadbeef.uuid"));
        assert!(crate::journal::interpret_rollback_output(false, "ok", 1, true).is_err());
    }
}
