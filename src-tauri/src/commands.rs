use crate::error::Result;
use crate::platform::{self, HostInfo};

#[tauri::command]
pub fn host_info() -> Result<HostInfo> {
    platform::host_info()
}
