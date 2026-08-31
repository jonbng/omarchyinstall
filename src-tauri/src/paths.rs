//! App data and log directories. Windows: `%LOCALAPPDATA%\OmarchyInstall`.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

pub fn install_data_dir_from_base(base: &Path) -> PathBuf {
    base.join("OmarchyInstall")
}

pub fn install_logs_dir_from_base(base: &Path) -> PathBuf {
    install_data_dir_from_base(base).join("logs")
}

pub fn install_data_dir() -> Result<PathBuf> {
    let base = if cfg!(windows) {
        PathBuf::from(
            std::env::var_os("LOCALAPPDATA")
                .ok_or_else(|| Error::Message("LOCALAPPDATA is unset".into()))?,
        )
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
            .ok_or_else(|| Error::Message("HOME is unset".into()))?
    };
    let dir = install_data_dir_from_base(&base);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn install_logs_dir() -> Result<PathBuf> {
    let dir = install_data_dir()?.join("logs");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_localappdata_layout() {
        let base = Path::new("AppData").join("Local");
        let logs = install_logs_dir_from_base(&base);
        assert_eq!(logs.file_name().unwrap(), "logs");
        assert_eq!(
            logs.parent().unwrap().file_name().unwrap(),
            "OmarchyInstall"
        );
        assert!(logs.ends_with(Path::new("OmarchyInstall").join("logs")));
    }
}
