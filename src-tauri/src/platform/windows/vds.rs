// Isolated, read-only VDS shrink-capacity query.

use super::process::output_with_timeout_in_job;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    os::windows::process::CommandExt,
    process::Command,
    time::{Duration, Instant},
};
use windows::{
    core::{Interface, PCWSTR, PWSTR},
    Win32::{
        Storage::VirtualDiskService::{
            CLSID_VdsLoader, IEnumVdsObject, IVdsPack, IVdsServiceLoader, IVdsSwProvider,
            IVdsVolume, IVdsVolumeMF, IVdsVolumeShrink, VDS_QUERY_SOFTWARE_PROVIDERS,
        },
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
                CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED,
            },
            Threading::CREATE_NO_WINDOW,
        },
    },
};

const HELPER_ARG: &str = "--storage-helper";
const PROTOCOL_VERSION: u32 = 2;
const VDS_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShrinkReport {
    pub protocol_version: u32,
    pub volume_guid: String,
    pub mount_path: String,
    pub elapsed_ms: u128,
    pub max_reclaimable_bytes: Option<u64>,
    pub error: Option<String>,
}

pub(crate) fn query_max_reclaimable(volume_guid: &str, mount_path: &str) -> Result<ShrinkReport> {
    let report = query_report(volume_guid, mount_path)?;
    if let Some(error) = &report.error {
        return Err(Error::Message(format!(
            "VDS shrink-capacity query failed: {error}"
        )));
    }
    if report.max_reclaimable_bytes.is_none() {
        return Err(Error::Message(
            "VDS helper returned no shrink capacity".into(),
        ));
    }
    Ok(report)
}

pub(crate) fn query_report(volume_guid: &str, mount_path: &str) -> Result<ShrinkReport> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .args([HELPER_ARG, "shrink", volume_guid, mount_path])
        .creation_flags(CREATE_NO_WINDOW.0);
    let output = output_with_timeout_in_job(
        &mut command,
        VDS_TIMEOUT,
        "native VDS shrink-capacity query",
    )?;
    if !output.status.success() {
        return Err(Error::Message(format!(
            "VDS helper exited with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let report: ShrinkReport = serde_json::from_slice(&output.stdout)
        .map_err(|error| Error::Message(format!("VDS helper returned invalid JSON: {error}")))?;
    validate_report(&report, volume_guid, mount_path)?;
    Ok(report)
}

fn validate_report(report: &ShrinkReport, volume_guid: &str, mount_path: &str) -> Result<()> {
    if report.protocol_version != PROTOCOL_VERSION {
        return Err(Error::Message(format!(
            "VDS helper protocol {} is unsupported",
            report.protocol_version
        )));
    }
    if !report.volume_guid.eq_ignore_ascii_case(volume_guid) {
        return Err(Error::Message(
            "VDS helper returned a result for the wrong volume".into(),
        ));
    }
    if !same_access_path(&report.mount_path, mount_path) {
        return Err(Error::Message(
            "VDS helper returned a result for the wrong access path".into(),
        ));
    }
    Ok(())
}

pub(crate) fn query_report_in_process(volume_guid: &str, mount_path: &str) -> ShrinkReport {
    let started = Instant::now();
    match query_in_process(volume_guid, mount_path) {
        Ok(bytes) => ShrinkReport {
            protocol_version: PROTOCOL_VERSION,
            volume_guid: volume_guid.into(),
            mount_path: mount_path.into(),
            elapsed_ms: started.elapsed().as_millis(),
            max_reclaimable_bytes: Some(bytes),
            error: None,
        },
        Err(error) => ShrinkReport {
            protocol_version: PROTOCOL_VERSION,
            volume_guid: volume_guid.into(),
            mount_path: mount_path.into(),
            elapsed_ms: started.elapsed().as_millis(),
            max_reclaimable_bytes: None,
            error: Some(error.to_string()),
        },
    }
}

fn query_in_process(target_volume: &str, target_mount: &str) -> Result<u64> {
    if target_volume.trim().is_empty() {
        return Err(Error::Message(
            "VDS helper received an empty volume GUID".into(),
        ));
    }
    if target_mount.trim().is_empty() {
        return Err(Error::Message(
            "VDS helper received an empty access path".into(),
        ));
    }
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    }
    let result = query_initialized(target_volume, target_mount);
    unsafe {
        CoUninitialize();
    }
    result
}

fn query_initialized(target_volume: &str, target_mount: &str) -> Result<u64> {
    unsafe {
        let loader: IVdsServiceLoader =
            CoCreateInstance(&CLSID_VdsLoader, None, CLSCTX_LOCAL_SERVER)?;
        let service = loader.LoadService(PCWSTR::null())?;
        service.WaitForServiceReady()?;
        let providers = service.QueryProviders(VDS_QUERY_SOFTWARE_PROVIDERS.0 as u32)?;
        while let Some(provider_unknown) = next_object(&providers)? {
            let provider: IVdsSwProvider = match provider_unknown.cast() {
                Ok(provider) => provider,
                Err(_) => continue,
            };
            let packs = provider.QueryPacks()?;
            while let Some(pack_unknown) = next_object(&packs)? {
                let pack: IVdsPack = match pack_unknown.cast() {
                    Ok(pack) => pack,
                    Err(_) => continue,
                };
                let volumes = pack.QueryVolumes()?;
                while let Some(volume_unknown) = next_object(&volumes)? {
                    let volume: IVdsVolume = match volume_unknown.cast() {
                        Ok(volume) => volume,
                        Err(_) => continue,
                    };
                    let mut properties =
                        windows::Win32::Storage::VirtualDiskService::VDS_VOLUME_PROP::default();
                    volume.GetProperties(&mut properties)?;
                    let name = if properties.pwszName.is_null() {
                        String::new()
                    } else {
                        properties.pwszName.to_string().unwrap_or_default()
                    };
                    if !properties.pwszName.is_null() {
                        CoTaskMemFree(Some(properties.pwszName.0.cast()));
                    }
                    let id_name = format!(r"\\?\Volume{{{}}}\", guid_string(properties.id));
                    let guid_matches =
                        same_volume(&name, target_volume) || same_volume(&id_name, target_volume);
                    let paths = volume
                        .cast::<IVdsVolumeMF>()
                        .ok()
                        .and_then(|volume| query_access_paths(&volume).ok())
                        .unwrap_or_default();
                    let access_path_matches = paths
                        .iter()
                        .any(|path| same_access_path(path, target_mount));
                    if guid_matches && access_path_matches {
                        let shrink: IVdsVolumeShrink = volume.cast().map_err(|error| {
                            Error::Message(format!(
                                "Windows volume does not expose IVdsVolumeShrink: {error}"
                            ))
                        })?;
                        return Ok(shrink.QueryMaxReclaimableBytes()?);
                    }
                }
            }
        }
    }
    Err(Error::Message(format!(
        "VDS did not return Windows volume {target_volume}"
    )))
}

unsafe fn next_object(enumerator: &IEnumVdsObject) -> Result<Option<windows::core::IUnknown>> {
    let mut values = [None];
    let mut fetched = 0u32;
    unsafe {
        enumerator.Next(&mut values, &mut fetched)?;
    }
    Ok(if fetched == 1 { values[0].take() } else { None })
}

fn same_volume(left: &str, right: &str) -> bool {
    left.trim_end_matches('\\')
        .eq_ignore_ascii_case(right.trim_end_matches('\\'))
}

fn same_access_path(left: &str, right: &str) -> bool {
    left.trim_end_matches(['\\', ':'])
        .eq_ignore_ascii_case(right.trim_end_matches(['\\', ':']))
}

fn query_access_paths(volume: &IVdsVolumeMF) -> Result<Vec<String>> {
    unsafe {
        let mut array: *mut PWSTR = std::ptr::null_mut();
        let mut count = 0i32;
        volume.QueryAccessPaths(&mut array, &mut count)?;
        if count < 0 || count > 4096 {
            CoTaskMemFree(Some(array.cast()));
            return Err(Error::Message(format!(
                "VDS returned invalid access path count {count}"
            )));
        }
        let pointers = if count == 0 || array.is_null() {
            &[][..]
        } else {
            std::slice::from_raw_parts(array, count as usize)
        };
        let mut output = Vec::with_capacity(pointers.len());
        for pointer in pointers {
            if !pointer.is_null() {
                output.push(pointer.to_string().unwrap_or_default());
                CoTaskMemFree(Some(pointer.0.cast()));
            }
        }
        if !array.is_null() {
            CoTaskMemFree(Some(array.cast()));
        }
        Ok(output)
    }
}

fn guid_string(guid: windows::core::GUID) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_names_ignore_case_and_trailing_slash() {
        assert!(same_volume(
            r"\\?\Volume{AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE}\",
            r"\\?\volume{aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee}"
        ));
    }

    #[test]
    fn access_paths_ignore_drive_colon_and_slash() {
        assert!(same_access_path("c:\\", "C:"));
        assert!(!same_access_path("D:\\", "C:\\"));
    }

    #[test]
    fn helper_result_must_match_guid_and_access_path() {
        let report = ShrinkReport {
            protocol_version: PROTOCOL_VERSION,
            volume_guid: r"\\?\Volume{aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa}\".into(),
            mount_path: "C:\\".into(),
            elapsed_ms: 1,
            max_reclaimable_bytes: Some(1),
            error: None,
        };
        assert!(validate_report(
            &report,
            r"\\?\Volume{bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb}\",
            "C:\\"
        )
        .is_err());
        assert!(validate_report(&report, &report.volume_guid, "D:\\").is_err());
        assert!(validate_report(&report, &report.volume_guid, "C:").is_ok());
    }
}
