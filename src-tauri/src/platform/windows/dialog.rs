use crate::error::{Error, Result};
use std::path::PathBuf;
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{ERROR_CANCELLED, HWND},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        },
        UI::Shell::{
            Common::COMDLG_FILTERSPEC, FileOpenDialog, IFileOpenDialog, FOS_FILEMUSTEXIST,
            FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, SIGDN_FILESYSPATH,
        },
    },
};

pub fn pick_local_iso(owner: Option<isize>) -> Result<Option<PathBuf>> {
    let thread = std::thread::Builder::new()
        .name("iso-file-dialog".into())
        .spawn(move || pick_on_sta(owner))?;
    thread
        .join()
        .map_err(|_| Error::Message("ISO file picker thread panicked".into()))?
}

fn pick_on_sta(owner: Option<isize>) -> Result<Option<PathBuf>> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok()?;
        let result = show_dialog(owner);
        CoUninitialize();
        result
    }
}

unsafe fn show_dialog(owner: Option<isize>) -> Result<Option<PathBuf>> {
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)? };
    let options = unsafe { dialog.GetOptions()? };
    unsafe {
        dialog.SetOptions(
            options | FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR,
        )?;
        dialog.SetTitle(w!("Select an already downloaded Omarchy ISO"))?;
        dialog.SetDefaultExtension(w!("iso"))?;
    }
    let filters = [
        COMDLG_FILTERSPEC {
            pszName: w!("Omarchy ISO (omarchy-*.iso)"),
            pszSpec: w!("omarchy-*.iso"),
        },
        COMDLG_FILTERSPEC {
            pszName: w!("ISO images (*.iso)"),
            pszSpec: w!("*.iso"),
        },
    ];
    unsafe { dialog.SetFileTypes(&filters)? };

    let owner = owner.map(|value| HWND(value as *mut core::ffi::c_void));
    if let Err(error) = unsafe { dialog.Show(owner) } {
        if error.code() == ERROR_CANCELLED.to_hresult() {
            return Ok(None);
        }
        return Err(error.into());
    }

    let item = unsafe { dialog.GetResult()? };
    let raw = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)? };
    let path = unsafe { PCWSTR(raw.0).to_string() }
        .map_err(|_| Error::Message("ISO file picker returned an invalid path".into()));
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    Ok(Some(PathBuf::from(path?)))
}
