use crate::cidata::CidataIdentity;
use crate::download::{self, LocalIsoSelection, VerifyResult};
use crate::error::Result;
use crate::operation::OperationGate;
use crate::platform::{
    self, BootNextResult, CidataResult, HostInfo, MachineProbe, PrepareResult, RollbackResult,
    StageResult, StateJournal,
};
use tauri::{Emitter, State};

async fn run_blocking<T, F>(name: &'static str, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let started = std::time::Instant::now();
    log::info!("{name} started");
    let result = match tauri::async_runtime::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => Err(crate::error::Error::Message(format!(
            "{name} task failed: {error}"
        ))),
    };
    match &result {
        Ok(_) => log::info!("{name} completed in {} ms", started.elapsed().as_millis()),
        Err(error) => log::error!(
            "{name} failed after {} ms: {error}",
            started.elapsed().as_millis()
        ),
    }
    result
}

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
pub async fn probe_machine(operation: State<'_, OperationGate>) -> Result<MachineProbe> {
    let _operation = operation.lock().await;
    let started = std::time::SystemTime::now();
    let started_unix_ms = started
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let timer = std::time::Instant::now();
    let result = run_blocking("machine probe", platform::probe_machine).await;
    crate::diagnostics::record_probe_attempt(started_unix_ms, timer.elapsed().as_millis(), &result);
    result
}

#[tauri::command]
pub async fn relaunch_elevated(operation: State<'_, OperationGate>) -> Result<()> {
    let _operation = operation.lock().await;
    run_blocking("elevated relaunch", platform::relaunch_elevated).await
}

#[tauri::command]
pub async fn reboot_to_firmware(operation: State<'_, OperationGate>) -> Result<()> {
    let _operation = operation.lock().await;
    run_blocking("firmware reboot", platform::reboot_to_firmware).await
}

#[tauri::command]
pub fn load_install_state() -> Result<Option<StateJournal>> {
    platform::load_install_state()
}

#[tauri::command]
pub async fn download_iso(
    app: tauri::AppHandle,
    operation: State<'_, OperationGate>,
) -> Result<()> {
    let _operation = operation.lock().await;
    let started = std::time::Instant::now();
    log::info!("ISO download started");
    let emit = |progress| {
        let _ = app.emit("iso://progress", &progress);
    };
    let result = if download::stub_skips_iso() {
        download::skip_iso_download(emit).await
    } else {
        download::download_iso_files(emit).await.map(|_| ())
    };
    match &result {
        Ok(_) => log::info!(
            "ISO download completed in {} ms",
            started.elapsed().as_millis()
        ),
        Err(error) => log::error!(
            "ISO download failed after {} ms: {error}",
            started.elapsed().as_millis()
        ),
    }
    result
}

#[tauri::command]
pub async fn pick_local_iso(
    window: tauri::WebviewWindow,
    operation: State<'_, OperationGate>,
) -> Result<Option<std::path::PathBuf>> {
    let _operation = operation.lock().await;
    #[cfg(windows)]
    let owner = Some(
        window
            .hwnd()
            .map_err(|error| crate::error::Error::Message(error.to_string()))?
            .0 as isize,
    );
    #[cfg(not(windows))]
    let owner = {
        let _ = window;
        None
    };
    run_blocking("ISO file picker", move || platform::pick_local_iso(owner)).await
}

#[tauri::command]
pub async fn prepare_local_iso(
    path: std::path::PathBuf,
    operation: State<'_, OperationGate>,
) -> Result<LocalIsoSelection> {
    let _operation = operation.lock().await;
    let started = std::time::Instant::now();
    log::info!("local ISO preparation started");
    let result = download::prepare_local_iso(&path).await;
    match &result {
        Ok(_) => log::info!(
            "local ISO preparation completed in {} ms",
            started.elapsed().as_millis()
        ),
        Err(error) => log::error!(
            "local ISO preparation failed after {} ms: {error}",
            started.elapsed().as_millis()
        ),
    }
    result
}

#[tauri::command]
pub async fn verify_iso(
    app: tauri::AppHandle,
    operation: State<'_, OperationGate>,
) -> Result<VerifyResult> {
    let _operation = operation.lock().await;
    let progress_app = app.clone();
    let result = run_blocking("ISO verification", move || {
        let emit = move |progress| {
            let _ = progress_app.emit("iso://progress", &progress);
        };
        if download::stub_skips_iso() {
            download::skip_iso_verify(emit)
        } else {
            download::verify_iso_files(emit)
        }
    })
    .await?;
    let _ = app.emit("iso://verified", &result);
    Ok(result)
}

#[tauri::command]
pub async fn prepare_installer_partition(
    allow_bitlocker: bool,
    operation: State<'_, OperationGate>,
) -> Result<PrepareResult> {
    let _operation = operation.lock().await;
    run_blocking("installer partition preparation", move || {
        platform::prepare_installer_partition(allow_bitlocker)
    })
    .await
}

#[tauri::command]
pub async fn stage_bootloader(operation: State<'_, OperationGate>) -> Result<StageResult> {
    let _operation = operation.lock().await;
    run_blocking("bootloader staging", platform::stage_bootloader).await
}

#[tauri::command]
pub async fn write_cidata(
    identity: CidataIdentity,
    operation: State<'_, OperationGate>,
) -> Result<CidataResult> {
    let _operation = operation.lock().await;
    run_blocking("cidata writing", move || platform::write_cidata(identity)).await
}

#[tauri::command]
pub async fn set_boot_next(operation: State<'_, OperationGate>) -> Result<BootNextResult> {
    let _operation = operation.lock().await;
    run_blocking("BootNext configuration", platform::set_boot_next).await
}

#[tauri::command]
pub async fn reboot_to_installer(operation: State<'_, OperationGate>) -> Result<()> {
    let _operation = operation.lock().await;
    run_blocking("installer reboot", platform::reboot_to_installer).await
}

#[tauri::command]
pub async fn abort_and_rollback(operation: State<'_, OperationGate>) -> Result<RollbackResult> {
    let _operation = operation.lock().await;
    run_blocking("installer rollback", platform::abort_and_rollback).await
}

#[tauri::command]
pub async fn export_support_bundle(
    operation: State<'_, OperationGate>,
) -> Result<std::path::PathBuf> {
    let _operation = operation.lock().await;
    run_blocking("support bundle export", platform::export_support_bundle).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_task_panics_are_reported() {
        let error = tauri::async_runtime::block_on(run_blocking("test", || -> Result<()> {
            panic!("intentional test panic")
        }))
        .unwrap_err();
        assert!(error.to_string().contains("test task failed"));
    }
}
