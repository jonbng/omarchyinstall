// Read-only storage inventory built directly from Win32 volume and disk APIs.

use super::process::output_with_timeout_in_job;
use crate::error::{Error, Result};
use crate::platform::windows::vds;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    io::Write,
    mem,
    os::windows::{ffi::OsStrExt, process::CommandExt},
    process::Command,
    time::{Duration, Instant},
};
use windows::{
    core::{GUID, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_NO_MORE_FILES,
            GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{
            CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetVolumeInformationW,
            GetVolumeNameForVolumeMountPointW, GetVolumePathNamesForVolumeNameW, QueryDosDeviceW,
            FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            OPEN_EXISTING,
        },
        System::{
            Ioctl::{
                PropertyStandardQuery, StorageDeviceProperty, DRIVE_LAYOUT_INFORMATION_EX,
                GET_LENGTH_INFORMATION, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, IOCTL_DISK_GET_LENGTH_INFO,
                IOCTL_STORAGE_QUERY_PROPERTY, PARTITION_INFORMATION_EX, PARTITION_STYLE_GPT,
                PARTITION_STYLE_MBR, PARTITION_STYLE_RAW, STORAGE_DESCRIPTOR_HEADER,
                STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY, VOLUME_DISK_EXTENTS,
            },
            SystemInformation::GetWindowsDirectoryW,
            IO::DeviceIoControl,
        },
    },
};

const MAX_IOCTL_BYTES: usize = 1024 * 1024;
const HELPER_ARG: &str = "--storage-helper";
const SELF_TEST_ARG: &str = "--storage-self-test";
const PROTOCOL_VERSION: u32 = 2;
const INVENTORY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InventoryEnvelope {
    pub protocol_version: u32,
    pub elapsed_ms: u128,
    pub report: Option<NativeStorageReport>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageIssue {
    pub stage: String,
    pub scope: String,
    pub disk_number: Option<u32>,
    pub volume_guid: Option<String>,
    pub blocking: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StageTiming {
    pub stage: String,
    pub elapsed_ms: u128,
    pub succeeded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeStorageReport {
    pub elapsed_ms: u128,
    pub boot_volume_guid: String,
    pub boot_disk_number: u32,
    pub disks: Vec<NativeDisk>,
    pub volumes: Vec<NativeVolume>,
    pub warnings: Vec<String>,
    pub issues: Vec<StorageIssue>,
    pub timings: Vec<StageTiming>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeDisk {
    pub number: u32,
    pub device_id: String,
    pub size_bytes: u64,
    pub partition_style: String,
    pub disk_guid: Option<String>,
    pub bus: String,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub partitions: Vec<NativePartition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativePartition {
    pub number: u32,
    pub offset_bytes: u64,
    pub size_bytes: u64,
    pub gpt_guid: Option<String>,
    pub type_guid: Option<String>,
    pub volume_guid: Option<String>,
    pub mount_paths: Vec<String>,
    pub label: Option<String>,
    pub fs: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeVolume {
    pub volume_guid: String,
    pub mount_paths: Vec<String>,
    pub label: Option<String>,
    pub fs: Option<String>,
    pub extents: Vec<NativeExtent>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeExtent {
    pub disk_number: u32,
    pub offset_bytes: u64,
    pub length_bytes: u64,
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub(crate) fn inventory() -> Result<NativeStorageReport> {
    let envelope = inventory_report()?;
    if let Some(error) = envelope.error {
        return Err(Error::Message(format!(
            "native storage inventory helper failed: {error}"
        )));
    }
    envelope
        .report
        .ok_or_else(|| Error::Message("native storage inventory helper returned no report".into()))
}

pub(crate) fn inventory_report() -> Result<InventoryEnvelope> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .args([HELPER_ARG, "inventory"])
        .creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0);
    let output =
        output_with_timeout_in_job(&mut command, INVENTORY_TIMEOUT, "native storage inventory")?;
    if !output.status.success() {
        return Err(Error::Message(format!(
            "storage inventory helper exited with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    parse_inventory_envelope(&output.stdout)
}

fn parse_inventory_envelope(bytes: &[u8]) -> Result<InventoryEnvelope> {
    let envelope: InventoryEnvelope = serde_json::from_slice(bytes).map_err(|error| {
        Error::Message(format!(
            "storage inventory helper returned invalid JSON: {error}"
        ))
    })?;
    if envelope.protocol_version != PROTOCOL_VERSION {
        return Err(Error::Message(format!(
            "storage inventory helper protocol {} is unsupported",
            envelope.protocol_version
        )));
    }
    Ok(envelope)
}

fn inventory_in_process() -> Result<NativeStorageReport> {
    let started = Instant::now();
    let mut warnings = Vec::new();
    let mut issues = Vec::new();
    let mut timings = Vec::new();

    let stage = Instant::now();
    let boot_volume_guid = windows_volume_guid()?;
    timings.push(StageTiming {
        stage: "windows-volume".into(),
        elapsed_ms: stage.elapsed().as_millis(),
        succeeded: true,
    });
    let stage = Instant::now();
    let volumes = enumerate_volumes(&boot_volume_guid, &mut warnings, &mut issues)?;
    timings.push(StageTiming {
        stage: "volumes".into(),
        elapsed_ms: stage.elapsed().as_millis(),
        succeeded: true,
    });
    let boot_volume = volumes
        .iter()
        .find(|v| v.volume_guid.eq_ignore_ascii_case(&boot_volume_guid))
        .ok_or_else(|| {
            Error::Message(format!(
                "native volume inventory did not contain Windows system volume {boot_volume_guid}"
            ))
        })?;
    if boot_volume.extents.len() != 1 {
        return Err(Error::Message(format!(
            "Windows system volume {} has {} physical extents; exactly one is required",
            boot_volume.volume_guid,
            boot_volume.extents.len()
        )));
    }
    let boot_disk_number = boot_volume.extents[0].disk_number;
    let stage = Instant::now();
    let mut numbers = enumerate_disk_numbers()?;
    if !numbers.contains(&boot_disk_number) {
        return Err(Error::Message(format!(
            "Windows system volume mapped to missing PhysicalDrive{boot_disk_number}"
        )));
    }
    numbers.sort_by_key(|number| (*number != boot_disk_number, *number));
    let mut disks = collect_disks(
        numbers,
        boot_disk_number,
        &mut warnings,
        &mut issues,
        read_disk,
    )?;
    timings.push(StageTiming {
        stage: "disks".into(),
        elapsed_ms: stage.elapsed().as_millis(),
        succeeded: true,
    });

    for volume in &volumes {
        if volume.extents.len() != 1 {
            warnings.push(format!(
                "volume {} has {} disk extents; it cannot be associated with one GPT partition",
                volume.volume_guid,
                volume.extents.len()
            ));
            continue;
        }
        let extent = volume.extents[0];
        let Some(disk) = disks.iter_mut().find(|d| d.number == extent.disk_number) else {
            warnings.push(format!(
                "volume {} refers to missing PhysicalDrive{}",
                volume.volume_guid, extent.disk_number
            ));
            continue;
        };
        let matches: Vec<_> = disk
            .partitions
            .iter_mut()
            .filter(|p| p.offset_bytes == extent.offset_bytes)
            .collect();
        if matches.len() != 1 {
            warnings.push(format!(
                "volume {} extent at disk {} offset {} matched {} partitions",
                volume.volume_guid,
                extent.disk_number,
                extent.offset_bytes,
                matches.len()
            ));
            continue;
        }
        let partition = matches.into_iter().next().expect("one match");
        partition.volume_guid = Some(volume.volume_guid.clone());
        partition.mount_paths = volume.mount_paths.clone();
        partition.label = volume.label.clone();
        partition.fs = volume.fs.clone();
    }

    Ok(NativeStorageReport {
        elapsed_ms: started.elapsed().as_millis(),
        boot_volume_guid,
        boot_disk_number,
        disks,
        volumes,
        warnings,
        issues,
        timings,
    })
}

fn collect_disks<F>(
    numbers: Vec<u32>,
    boot_disk_number: u32,
    warnings: &mut Vec<String>,
    issues: &mut Vec<StorageIssue>,
    mut reader: F,
) -> Result<Vec<NativeDisk>>
where
    F: FnMut(u32) -> Result<NativeDisk>,
{
    let mut disks = Vec::new();
    for number in numbers {
        match reader(number) {
            Ok(disk) => disks.push(disk),
            Err(error) if number == boot_disk_number => {
                return Err(Error::Message(format!(
                    "target PhysicalDrive{number} native inventory failed: {error}"
                )));
            }
            Err(error) => {
                let detail = format!("PhysicalDrive{number} native inventory failed: {error}");
                warnings.push(detail.clone());
                issues.push(StorageIssue {
                    stage: "disk-inventory".into(),
                    scope: "secondary-disk".into(),
                    disk_number: Some(number),
                    volume_guid: None,
                    blocking: false,
                    detail,
                });
            }
        }
    }
    Ok(disks)
}

fn enumerate_disk_numbers() -> Result<Vec<u32>> {
    let mut names = vec![0u16; 65_536];
    let written = unsafe { QueryDosDeviceW(PCWSTR::null(), Some(&mut names)) } as usize;
    if written == 0 {
        return Err(windows::core::Error::from_win32().into());
    }
    let mut numbers: Vec<u32> = split_multisz(&names[..written])
        .filter_map(|name| {
            let suffix = name
                .strip_prefix("PhysicalDrive")
                .or_else(|| name.strip_prefix("PHYSICALDRIVE"))?;
            suffix.parse().ok()
        })
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    if numbers.is_empty() {
        return Err(Error::Message(
            "QueryDosDeviceW returned no PhysicalDrive devices".into(),
        ));
    }
    Ok(numbers)
}

fn read_disk(number: u32) -> Result<NativeDisk> {
    let device_id = format!(r"\\.\PHYSICALDRIVE{number}");
    let handle = open_device(&device_id, GENERIC_READ.0)?;
    let length: GET_LENGTH_INFORMATION = ioctl_fixed(handle.0, IOCTL_DISK_GET_LENGTH_INFO, None)?;
    let layout_bytes = ioctl_growing(handle.0, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, None, 4096)?;
    let (style, disk_guid, partitions) = parse_layout(&layout_bytes)?;
    let descriptor = storage_descriptor(handle.0)?;
    Ok(NativeDisk {
        number,
        device_id,
        size_bytes: nonnegative(length.Length, "disk length")?,
        partition_style: style,
        disk_guid,
        bus: bus_name(descriptor.bus_type),
        model: descriptor.model,
        serial: descriptor.serial,
        partitions,
    })
}

struct Descriptor {
    bus_type: i32,
    model: Option<String>,
    serial: Option<String>,
}

fn storage_descriptor(handle: HANDLE) -> Result<Descriptor> {
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let header: STORAGE_DESCRIPTOR_HEADER =
        ioctl_fixed(handle, IOCTL_STORAGE_QUERY_PROPERTY, Some(as_bytes(&query)))?;
    let size = usize::try_from(header.Size).unwrap_or(0);
    if size < mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() || size > MAX_IOCTL_BYTES {
        return Err(Error::Message(format!(
            "invalid storage descriptor size {size}"
        )));
    }
    let bytes = ioctl_sized(
        handle,
        IOCTL_STORAGE_QUERY_PROPERTY,
        Some(as_bytes(&query)),
        size,
    )?;
    let descriptor = read_struct::<STORAGE_DEVICE_DESCRIPTOR>(&bytes, 0, "storage descriptor")?;
    Ok(Descriptor {
        bus_type: descriptor.BusType.0,
        model: descriptor_string(&bytes, descriptor.ProductIdOffset),
        serial: descriptor_string(&bytes, descriptor.SerialNumberOffset),
    })
}

fn parse_layout(bytes: &[u8]) -> Result<(String, Option<String>, Vec<NativePartition>)> {
    let layout = read_struct::<DRIVE_LAYOUT_INFORMATION_EX>(bytes, 0, "drive layout")?;
    let style = match layout.PartitionStyle {
        n if n == PARTITION_STYLE_GPT.0 as u32 => "gpt",
        n if n == PARTITION_STYLE_MBR.0 as u32 => "mbr",
        n if n == PARTITION_STYLE_RAW.0 as u32 => "raw",
        _ => "unknown",
    }
    .to_string();
    let disk_guid = if style == "gpt" {
        Some(guid_string(unsafe { layout.Anonymous.Gpt.DiskId }))
    } else {
        None
    };
    let first = mem::offset_of!(DRIVE_LAYOUT_INFORMATION_EX, PartitionEntry);
    let entry_size = mem::size_of::<PARTITION_INFORMATION_EX>();
    let count = usize::try_from(layout.PartitionCount)
        .map_err(|_| Error::Message("partition count overflow".into()))?;
    let need = first
        .checked_add(
            count
                .checked_mul(entry_size)
                .ok_or_else(|| Error::Message("partition layout overflow".into()))?,
        )
        .ok_or_else(|| Error::Message("partition layout overflow".into()))?;
    if need > bytes.len() {
        return Err(Error::Message(format!(
            "truncated drive layout: {count} entries require {need} bytes, got {}",
            bytes.len()
        )));
    }
    let mut partitions = Vec::with_capacity(count);
    for index in 0..count {
        let p = read_struct::<PARTITION_INFORMATION_EX>(
            bytes,
            first + index * entry_size,
            "partition entry",
        )?;
        if p.PartitionLength <= 0 {
            continue;
        }
        let (gpt_guid, type_guid) = if p.PartitionStyle == PARTITION_STYLE_GPT {
            let gpt = unsafe { p.Anonymous.Gpt };
            (
                Some(guid_string(gpt.PartitionId)),
                Some(guid_string(gpt.PartitionType)),
            )
        } else {
            (None, None)
        };
        partitions.push(NativePartition {
            number: p.PartitionNumber,
            offset_bytes: nonnegative(p.StartingOffset, "partition offset")?,
            size_bytes: nonnegative(p.PartitionLength, "partition length")?,
            gpt_guid,
            type_guid,
            volume_guid: None,
            mount_paths: Vec::new(),
            label: None,
            fs: None,
        });
    }
    Ok((style, disk_guid, partitions))
}

fn enumerate_volumes(
    boot_volume_guid: &str,
    warnings: &mut Vec<String>,
    issues: &mut Vec<StorageIssue>,
) -> Result<Vec<NativeVolume>> {
    let mut name = vec![0u16; 1024];
    let find = unsafe { FindFirstVolumeW(&mut name) }?;
    let mut volumes = Vec::new();
    let result = loop {
        let volume_guid = wide_buffer_string(&name);
        match read_volume(&volume_guid) {
            Ok(volume) => volumes.push(volume),
            Err(error) => {
                let detail = format!("volume {volume_guid}: {error}");
                let blocking = volume_guid.eq_ignore_ascii_case(boot_volume_guid);
                warnings.push(detail.clone());
                issues.push(StorageIssue {
                    stage: "volume-inventory".into(),
                    scope: if volume_guid.eq_ignore_ascii_case(boot_volume_guid) {
                        "windows-volume".into()
                    } else {
                        "secondary-volume".into()
                    },
                    disk_number: None,
                    volume_guid: Some(volume_guid),
                    blocking,
                    detail,
                });
            }
        }
        name.fill(0);
        if let Err(error) = unsafe { FindNextVolumeW(find, &mut name) } {
            if error.code() == ERROR_NO_MORE_FILES.to_hresult() {
                break Ok(volumes);
            }
            break Err(error.into());
        }
    };
    unsafe {
        let _ = FindVolumeClose(find);
    }
    result
}

fn read_volume(volume_guid: &str) -> Result<NativeVolume> {
    let device = volume_guid.trim_end_matches('\\');
    let handle = open_device(device, 0)?;
    let extent_bytes = ioctl_growing(
        handle.0,
        windows::Win32::Storage::FileSystem::IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        None,
        256,
    )?;
    let extents = parse_extents(&extent_bytes)?;
    let mount_paths = volume_mount_paths(volume_guid).unwrap_or_default();
    let (label, fs) = volume_information(volume_guid).unwrap_or((None, None));
    Ok(NativeVolume {
        volume_guid: volume_guid.to_string(),
        mount_paths,
        label,
        fs,
        extents,
    })
}

fn parse_extents(bytes: &[u8]) -> Result<Vec<NativeExtent>> {
    let header = read_struct::<VOLUME_DISK_EXTENTS>(bytes, 0, "volume extents")?;
    let first = mem::offset_of!(VOLUME_DISK_EXTENTS, Extents);
    let entry_size = mem::size_of::<windows::Win32::System::Ioctl::DISK_EXTENT>();
    let count = header.NumberOfDiskExtents as usize;
    let need = first
        .checked_add(
            count
                .checked_mul(entry_size)
                .ok_or_else(|| Error::Message("extent count overflow".into()))?,
        )
        .ok_or_else(|| Error::Message("extent buffer overflow".into()))?;
    if need > bytes.len() {
        return Err(Error::Message(format!(
            "truncated volume extents: need {need}, got {}",
            bytes.len()
        )));
    }
    (0..count)
        .map(|index| {
            let e = read_struct::<windows::Win32::System::Ioctl::DISK_EXTENT>(
                bytes,
                first + index * entry_size,
                "disk extent",
            )?;
            Ok(NativeExtent {
                disk_number: e.DiskNumber,
                offset_bytes: nonnegative(e.StartingOffset, "extent offset")?,
                length_bytes: nonnegative(e.ExtentLength, "extent length")?,
            })
        })
        .collect()
}

fn windows_volume_guid() -> Result<String> {
    let mut windows_dir = vec![0u16; 32_768];
    let len = unsafe { GetWindowsDirectoryW(Some(&mut windows_dir)) } as usize;
    if len < 3 || len >= windows_dir.len() {
        return Err(Error::Message(
            "GetWindowsDirectoryW returned an invalid path".into(),
        ));
    }
    let path = String::from_utf16_lossy(&windows_dir[..len]);
    let root = path
        .get(..3)
        .ok_or_else(|| Error::Message("Windows directory has no drive root".into()))?;
    let root_wide = wide(root);
    let mut volume = vec![0u16; 1024];
    unsafe {
        GetVolumeNameForVolumeMountPointW(PCWSTR(root_wide.as_ptr()), &mut volume)?;
    }
    Ok(wide_buffer_string(&volume))
}

fn volume_mount_paths(volume: &str) -> Result<Vec<String>> {
    let volume_wide = wide(volume);
    let mut required = 0u32;
    let mut buffer = vec![0u16; 512];
    let first = unsafe {
        GetVolumePathNamesForVolumeNameW(
            PCWSTR(volume_wide.as_ptr()),
            Some(&mut buffer),
            &mut required,
        )
    };
    if let Err(error) = first {
        if error.code() != ERROR_MORE_DATA.to_hresult() {
            return Err(error.into());
        }
        let size = usize::try_from(required)
            .map_err(|_| Error::Message("mount path buffer overflow".into()))?;
        if size > MAX_IOCTL_BYTES / 2 {
            return Err(Error::Message(format!(
                "mount path buffer too large: {size}"
            )));
        }
        buffer.resize(size, 0);
        unsafe {
            GetVolumePathNamesForVolumeNameW(
                PCWSTR(volume_wide.as_ptr()),
                Some(&mut buffer),
                &mut required,
            )?;
        }
    }
    Ok(split_multisz(&buffer).filter(|s| !s.is_empty()).collect())
}

fn volume_information(volume: &str) -> Result<(Option<String>, Option<String>)> {
    let volume_wide = wide(volume);
    let mut label = vec![0u16; 1024];
    let mut fs = vec![0u16; 128];
    unsafe {
        GetVolumeInformationW(
            PCWSTR(volume_wide.as_ptr()),
            Some(&mut label),
            None,
            None,
            None,
            Some(&mut fs),
        )?;
    }
    Ok((
        nonempty(wide_buffer_string(&label)),
        nonempty(wide_buffer_string(&fs)),
    ))
}

fn open_device(path: &str, access: u32) -> Result<OwnedHandle> {
    let path_wide = wide(path);
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path_wide.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )?
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(Error::Message(format!("could not open {path}")));
    }
    Ok(OwnedHandle(handle))
}

fn ioctl_fixed<T: Copy + Default>(handle: HANDLE, code: u32, input: Option<&[u8]>) -> Result<T> {
    let mut output = T::default();
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle,
            code,
            input.map(|v| v.as_ptr().cast()),
            input.map_or(0, |v| v.len() as u32),
            Some((&mut output as *mut T).cast()),
            mem::size_of::<T>() as u32,
            Some(&mut returned),
            None,
        )?;
    }
    if returned < mem::size_of::<T>() as u32 {
        return Err(Error::Message(format!(
            "IOCTL {code:#x} returned only {returned} bytes"
        )));
    }
    Ok(output)
}

fn ioctl_sized(handle: HANDLE, code: u32, input: Option<&[u8]>, size: usize) -> Result<Vec<u8>> {
    let mut words = vec![0usize; size.div_ceil(mem::size_of::<usize>())];
    let capacity = words.len() * mem::size_of::<usize>();
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle,
            code,
            input.map(|v| v.as_ptr().cast()),
            input.map_or(0, |v| v.len() as u32),
            Some(words.as_mut_ptr().cast()),
            capacity as u32,
            Some(&mut returned),
            None,
        )?;
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), returned as usize) };
    Ok(bytes.to_vec())
}

fn ioctl_growing(
    handle: HANDLE,
    code: u32,
    input: Option<&[u8]>,
    initial: usize,
) -> Result<Vec<u8>> {
    let mut size = initial;
    loop {
        match ioctl_sized(handle, code, input, size) {
            Ok(bytes) => return Ok(bytes),
            Err(Error::Windows(error))
                if error.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult()
                    || error.code() == ERROR_MORE_DATA.to_hresult() =>
            {
                size = size
                    .checked_mul(2)
                    .ok_or_else(|| Error::Message("IOCTL buffer overflow".into()))?;
                if size > MAX_IOCTL_BYTES {
                    return Err(Error::Message(format!(
                        "IOCTL {code:#x} exceeded {MAX_IOCTL_BYTES} bytes"
                    )));
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn read_struct<T: Copy>(bytes: &[u8], offset: usize, description: &str) -> Result<T> {
    let end = offset
        .checked_add(mem::size_of::<T>())
        .ok_or_else(|| Error::Message(format!("{description} offset overflow")))?;
    if end > bytes.len() {
        return Err(Error::Message(format!(
            "truncated {description}: need {end}, got {}",
            bytes.len()
        )));
    }
    Ok(unsafe { std::ptr::read_unaligned(bytes.as_ptr().add(offset).cast::<T>()) })
}

fn descriptor_string(bytes: &[u8], offset: u32) -> Option<String> {
    let start = usize::try_from(offset).ok()?;
    if start == 0 || start >= bytes.len() {
        return None;
    }
    let end = bytes[start..]
        .iter()
        .position(|b| *b == 0)
        .map(|n| start + n)?;
    nonempty(
        String::from_utf8_lossy(&bytes[start..end])
            .trim()
            .to_string(),
    )
}

fn as_bytes<T>(value: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts((value as *const T).cast(), mem::size_of::<T>()) }
}

fn nonnegative(value: i64, name: &str) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| Error::Message(format!("Windows returned negative {name}: {value}")))
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn wide_buffer_string(buffer: &[u16]) -> String {
    let len = buffer.iter().position(|u| *u == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

fn split_multisz(buffer: &[u16]) -> impl Iterator<Item = String> + '_ {
    buffer
        .split(|u| *u == 0)
        .take_while(|part| !part.is_empty())
        .map(String::from_utf16_lossy)
}

fn guid_string(guid: GUID) -> String {
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

fn bus_name(value: i32) -> String {
    match value {
        1 => "SCSI",
        2 => "ATAPI",
        3 => "ATA",
        4 => "1394",
        5 => "SSA",
        6 => "Fibre",
        7 => "USB",
        8 => "RAID",
        9 => "iSCSI",
        10 => "SAS",
        11 => "SATA",
        12 => "SD",
        13 => "MMC",
        14 => "Virtual",
        15 => "FileBackedVirtual",
        16 => "StorageSpaces",
        17 => "NVMe",
        18 => "SCM",
        19 => "UFS",
        _ => "Unknown",
    }
    .to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageSelfTestEnvelope {
    protocol_version: u32,
    inventory: Option<InventoryEnvelope>,
    inventory_error: Option<String>,
    shrink: Option<vds::ShrinkReport>,
    shrink_error: Option<String>,
}

/// Handles the private storage helper protocol before the GUI runtime starts.
pub fn run_helper_if_requested() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let is_helper = args.first().map(String::as_str) == Some(HELPER_ARG);
    let is_self_test = args.first().map(String::as_str) == Some(SELF_TEST_ARG);
    if !is_helper && !is_self_test {
        return false;
    }

    let output = if is_self_test {
        let (inventory, inventory_error) = match inventory_report() {
            Ok(envelope) => (Some(envelope), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let (shrink, shrink_error) = match inventory
            .as_ref()
            .and_then(|envelope| envelope.report.as_ref())
        {
            Some(report) => match vds::query_report(&report.boot_volume_guid, "C:\\") {
                Ok(report) => (Some(report), None),
                Err(error) => (None, Some(error.to_string())),
            },
            None => (None, None),
        };
        serde_json::to_vec(&StorageSelfTestEnvelope {
            protocol_version: PROTOCOL_VERSION,
            inventory,
            inventory_error,
            shrink,
            shrink_error,
        })
    } else {
        match args.get(1).map(String::as_str) {
            Some("inventory") if args.len() == 2 => {
                serde_json::to_vec(&inventory_envelope_in_process())
            }
            Some("shrink") if args.len() == 4 => {
                serde_json::to_vec(&vds::query_report_in_process(&args[2], &args[3]))
            }
            _ => serde_json::to_vec(&InventoryEnvelope {
                protocol_version: PROTOCOL_VERSION,
                elapsed_ms: 0,
                report: None,
                error: Some("invalid storage helper command".into()),
            }),
        }
    }
    .unwrap_or_else(|_| {
        br#"{"protocolVersion":2,"elapsedMs":0,"report":null,"error":"serialization failed"}"#
            .to_vec()
    });
    let _ = std::io::stdout().write_all(&output);
    true
}

fn inventory_envelope_in_process() -> InventoryEnvelope {
    let started = Instant::now();
    match inventory_in_process() {
        Ok(report) => InventoryEnvelope {
            protocol_version: PROTOCOL_VERSION,
            elapsed_ms: started.elapsed().as_millis(),
            report: Some(report),
            error: None,
        },
        Err(error) => InventoryEnvelope {
            protocol_version: PROTOCOL_VERSION,
            elapsed_ms: started.elapsed().as_millis(),
            report: None,
            error: Some(error.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_flexible_arrays() {
        let mut bytes = vec![0u8; mem::size_of::<VOLUME_DISK_EXTENTS>()];
        bytes[..4].copy_from_slice(&2u32.to_ne_bytes());
        assert!(parse_extents(&bytes)
            .unwrap_err()
            .to_string()
            .contains("truncated"));
    }

    #[test]
    fn descriptor_offsets_are_bounds_checked() {
        assert_eq!(descriptor_string(b"abcd\0", 1), Some("bcd".into()));
        assert_eq!(descriptor_string(b"abcd", 9), None);
        assert_eq!(descriptor_string(b"abcd", 1), None);
    }

    #[test]
    fn helper_protocol_rejects_malformed_json_and_versions() {
        assert!(parse_inventory_envelope(b"not json").is_err());
        let json = br#"{"protocolVersion":999,"elapsedMs":0,"report":null,"error":"x"}"#;
        assert!(parse_inventory_envelope(json)
            .unwrap_err()
            .to_string()
            .contains("unsupported"));
    }

    fn fake_disk(number: u32) -> NativeDisk {
        NativeDisk {
            number,
            device_id: format!(r"\\.\PHYSICALDRIVE{number}"),
            size_bytes: 1,
            partition_style: "gpt".into(),
            disk_guid: Some("test".into()),
            bus: "NVMe".into(),
            model: Some("test".into()),
            serial: Some("test".into()),
            partitions: Vec::new(),
        }
    }

    #[test]
    fn secondary_disk_failure_is_a_structured_warning() {
        let mut warnings = Vec::new();
        let mut issues = Vec::new();
        let disks = collect_disks(vec![0, 1], 0, &mut warnings, &mut issues, |number| {
            if number == 1 {
                Err(Error::Message("access denied".into()))
            } else {
                Ok(fake_disk(number))
            }
        })
        .unwrap();
        assert_eq!(disks.len(), 1);
        assert_eq!(issues.len(), 1);
        assert!(!issues[0].blocking);
        assert_eq!(issues[0].disk_number, Some(1));
    }

    #[test]
    fn target_disk_failure_is_fatal() {
        let error = collect_disks(vec![0, 1], 0, &mut Vec::new(), &mut Vec::new(), |number| {
            if number == 0 {
                Err(Error::Message("access denied".into()))
            } else {
                Ok(fake_disk(number))
            }
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("target PhysicalDrive0"), "{error}");
    }
}
