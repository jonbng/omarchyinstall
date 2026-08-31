use serde::Serialize;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[allow(dead_code)]
    #[error("{0}")]
    Message(String),

    #[error("this operation is only available on Windows")]
    WindowsOnly,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[cfg(windows)]
    #[error(transparent)]
    Windows(#[from] windows::core::Error),
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_only_serializes_as_message() {
        let json = serde_json::to_string(&Error::WindowsOnly).unwrap();
        assert!(json.contains("Windows"));
    }
}
