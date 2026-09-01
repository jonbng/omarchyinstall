use crate::cidata::CidataIdentity;
use crate::download::{self, LocalIsoSelection, VerifyResult};
use crate::error::Result;
use crate::platform::{
    self, BootNextResult, CidataResult, HostInfo, MachineProbe, PrepareResult, RollbackResult,
    StageResult, StateJournal,
};
use tauri::Emitter;

#[tauri::command]
pub fn exit_app(app: tauri::AppHandle) {
    // Let the IPC response reach the frontend before terminating the process.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        app.exit(0);
    });
}

#[tauri::command]
pub fn host_info() -> Result<HostInfo> {
    platform::host_info()
}

#[tauri::command]
pub fn probe_machine() -> Result<MachineProbe> {
    platform::probe_machine()
}

#[tauri::command]
pub fn relaunch_elevated() -> Result<()> {
    platform::relaunch_elevated()
}

#[tauri::command]
pub fn reboot_to_firmware() -> Result<()> {
    platform::reboot_to_firmware()
}

#[tauri::command]
pub fn load_install_state() -> Result<Option<StateJournal>> {
    platform::load_install_state()
}

#[tauri::command]
pub async fn download_iso(app: tauri::AppHandle) -> Result<()> {
    let emit = |progress| {
        let _ = app.emit("iso://progress", &progress);
    };
    if download::stub_skips_iso() {
        download::skip_iso_download(emit).await?;
    } else {
        download::download_iso_files(emit).await?;
    }
    Ok(())
}

#[tauri::command]
pub fn pick_local_iso() -> Result<Option<std::path::PathBuf>> {
    platform::pick_local_iso()
}

#[tauri::command]
pub async fn prepare_local_iso(path: std::path::PathBuf) -> Result<LocalIsoSelection> {
    download::prepare_local_iso(&path).await
}

#[tauri::command]
pub fn verify_iso(app: tauri::AppHandle) -> Result<VerifyResult> {
    let emit = |progress| {
        let _ = app.emit("iso://progress", &progress);
    };
    let result = if download::stub_skips_iso() {
        download::skip_iso_verify(emit)?
    } else {
        download::verify_iso_files(emit)?
    };
    let _ = app.emit("iso://verified", &result);
    Ok(result)
}

#[tauri::command]
pub fn prepare_installer_partition(allow_bitlocker: bool) -> Result<PrepareResult> {
    platform::prepare_installer_partition(allow_bitlocker)
}

#[tauri::command]
pub fn stage_bootloader() -> Result<StageResult> {
    platform::stage_bootloader()
}

#[tauri::command]
pub fn write_cidata(identity: CidataIdentity) -> Result<CidataResult> {
    platform::write_cidata(identity)
}

#[tauri::command]
pub fn set_boot_next() -> Result<BootNextResult> {
    platform::set_boot_next()
}

#[tauri::command]
pub fn reboot_to_installer() -> Result<()> {
    platform::reboot_to_installer()
}

#[tauri::command]
pub fn abort_and_rollback() -> Result<RollbackResult> {
    platform::abort_and_rollback()
}

#[tauri::command]
pub fn export_support_bundle() -> Result<std::path::PathBuf> {
    platform::export_support_bundle()
}
