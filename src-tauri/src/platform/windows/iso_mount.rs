use crate::error::{Error, Result};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE},
        Storage::{
            FileSystem::{
                CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
                FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
                OPEN_EXISTING,
            },
            Vhd::{
                AttachVirtualDisk, DetachVirtualDisk, GetVirtualDiskPhysicalPath, OpenVirtualDisk,
                ATTACH_VIRTUAL_DISK_FLAG_NO_DRIVE_LETTER, ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY,
                ATTACH_VIRTUAL_DISK_PARAMETERS, ATTACH_VIRTUAL_DISK_VERSION_1,
                DETACH_VIRTUAL_DISK_FLAG_NONE, OPEN_VIRTUAL_DISK_FLAG_NONE,
                OPEN_VIRTUAL_DISK_PARAMETERS, OPEN_VIRTUAL_DISK_VERSION_1,
                VIRTUAL_DISK_ACCESS_ATTACH_RO, VIRTUAL_STORAGE_TYPE,
                VIRTUAL_STORAGE_TYPE_DEVICE_ISO, VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
            },
        },
        System::{
            Ioctl::{IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER},
            IO::DeviceIoControl,
        },
    },
};

pub struct MountedIso {
    handle: HANDLE,
    root: PathBuf,
}

impl MountedIso {
    pub fn attach(path: &Path) -> Result<Self> {
        let absolute = path.canonicalize()?;
        let wide = wide_null(absolute.as_os_str());
        let storage_type = VIRTUAL_STORAGE_TYPE {
            DeviceId: VIRTUAL_STORAGE_TYPE_DEVICE_ISO,
            VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
        };
        let open_parameters = OPEN_VIRTUAL_DISK_PARAMETERS {
            Version: OPEN_VIRTUAL_DISK_VERSION_1,
            ..Default::default()
        };
        let mut handle = HANDLE::default();
        unsafe {
            OpenVirtualDisk(
                &storage_type,
                PCWSTR(wide.as_ptr()),
                VIRTUAL_DISK_ACCESS_ATTACH_RO,
                OPEN_VIRTUAL_DISK_FLAG_NONE,
                Some(&open_parameters),
                &mut handle,
            )
            .ok()?;
        }

        let attach_parameters = ATTACH_VIRTUAL_DISK_PARAMETERS {
            Version: ATTACH_VIRTUAL_DISK_VERSION_1,
            ..Default::default()
        };
        if let Err(error) = unsafe {
            AttachVirtualDisk(
                handle,
                None,
                ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY | ATTACH_VIRTUAL_DISK_FLAG_NO_DRIVE_LETTER,
                0,
                Some(&attach_parameters),
                None,
            )
            .ok()
        } {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(error.into());
        }

        let root = match wait_for_volume(handle) {
            Ok(root) => root,
            Err(error) => {
                unsafe {
                    let detached = DetachVirtualDisk(handle, DETACH_VIRTUAL_DISK_FLAG_NONE, 0);
                    if detached.is_err() {
                        log::warn!(
                            "failed to detach ISO after attach error: Windows error {}",
                            detached.0
                        );
                    }
                    let _ = CloseHandle(handle);
                }
                return Err(error);
            }
        };
        Ok(Self { handle, root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for MountedIso {
    fn drop(&mut self) {
        unsafe {
            let detached = DetachVirtualDisk(self.handle, DETACH_VIRTUAL_DISK_FLAG_NONE, 0);
            if detached.is_err() {
                log::warn!("failed to detach ISO: Windows error {}", detached.0);
            }
            let _ = CloseHandle(self.handle);
        }
    }
}

fn wait_for_volume(virtual_disk: HANDLE) -> Result<PathBuf> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_error = None;
    loop {
        match virtual_disk_physical_path(virtual_disk)
            .and_then(|path| storage_device_number(&path))
            .and_then(find_volume)
        {
            Ok(Some(root)) => return Ok(root),
            Ok(None) => {}
            Err(error) => last_error = Some(error),
        }
        if Instant::now() >= deadline {
            let detail = last_error
                .map(|error| format!(": {error}"))
                .unwrap_or_default();
            return Err(Error::Message(format!(
                "Windows attached the ISO but did not expose its volume{detail}"
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn virtual_disk_physical_path(handle: HANDLE) -> Result<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let mut bytes = (buffer.len() * std::mem::size_of::<u16>()) as u32;
    unsafe { GetVirtualDiskPhysicalPath(handle, &mut bytes, PWSTR(buffer.as_mut_ptr())).ok()? };
    let len = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    if len == 0 {
        return Err(Error::Message(
            "Windows returned an empty ISO physical path".into(),
        ));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
}

fn find_volume(expected: STORAGE_DEVICE_NUMBER) -> Result<Option<PathBuf>> {
    let mut buffer = vec![0u16; 32_768];
    let find = match unsafe { FindFirstVolumeW(&mut buffer) } {
        Ok(handle) => handle,
        Err(error) => return Err(error.into()),
    };
    let result = loop {
        let len = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        let root = PathBuf::from(String::from_utf16_lossy(&buffer[..len]));
        let device_path = PathBuf::from(root.to_string_lossy().trim_end_matches('\\'));
        if storage_device_number(&device_path).is_ok_and(|actual| same_device(actual, expected)) {
            break Ok(Some(root));
        }
        buffer.fill(0);
        if let Err(error) = unsafe { FindNextVolumeW(find, &mut buffer) } {
            if error.code() == ERROR_NO_MORE_FILES.to_hresult() {
                break Ok(None);
            }
            break Err(error.into());
        }
    };
    unsafe {
        let _ = FindVolumeClose(find);
    }
    result
}

fn same_device(left: STORAGE_DEVICE_NUMBER, right: STORAGE_DEVICE_NUMBER) -> bool {
    left.DeviceType == right.DeviceType && left.DeviceNumber == right.DeviceNumber
}

fn storage_device_number(path: &Path) -> Result<STORAGE_DEVICE_NUMBER> {
    let wide = wide_null(path.as_os_str());
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )?
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(Error::Message(format!(
            "could not open mounted ISO device {}",
            path.display()
        )));
    }
    let mut number = STORAGE_DEVICE_NUMBER::default();
    let mut returned = 0u32;
    let result = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            None,
            0,
            Some((&mut number as *mut STORAGE_DEVICE_NUMBER).cast()),
            std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            Some(&mut returned),
            None,
        )
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    result?;
    Ok(number)
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_matching_ignores_partition_number() {
        let disk = STORAGE_DEVICE_NUMBER {
            DeviceType: 2,
            DeviceNumber: 7,
            PartitionNumber: 0,
        };
        let volume = STORAGE_DEVICE_NUMBER {
            PartitionNumber: u32::MAX,
            ..disk
        };
        assert!(same_device(disk, volume));
    }
}
