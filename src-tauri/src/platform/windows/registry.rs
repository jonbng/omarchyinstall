use crate::error::{Error, Result};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            RegGetValueW, RegSetKeyValueW, HKEY_LOCAL_MACHINE, REG_DWORD, RRF_RT_REG_DWORD,
        },
    },
};

pub fn get_hklm_dword(subkey: PCWSTR, value: PCWSTR) -> Option<u32> {
    unsafe {
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        );
        (status == ERROR_SUCCESS).then_some(data)
    }
}

pub fn set_hklm_dword(subkey: PCWSTR, value: PCWSTR, data: u32) -> Result<()> {
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            subkey,
            value,
            REG_DWORD.0,
            Some((&data as *const u32).cast()),
            std::mem::size_of::<u32>() as u32,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(Error::Message(format!(
            "registry DWORD write failed with Windows error {}",
            status.0
        )));
    }
    Ok(())
}
