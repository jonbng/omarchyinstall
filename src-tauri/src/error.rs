use serde::Serialize;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[allow(dead_code)]
    #[error("{0}")]
    Message(String),

    #[cfg_attr(not(windows), allow(dead_code))]
    #[error("{description} timed out after {seconds} seconds")]
    Timeout { description: String, seconds: u64 },

    #[error("this operation is only available on Windows")]
    WindowsOnly,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),

    #[cfg(windows)]
    #[error(transparent)]
    Windows(#[from] windows::core::Error),
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
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

    #[test]
    fn timeout_serializes_as_a_specific_message() {
        let json = serde_json::to_string(&Error::Timeout {
            description: "Windows storage inventory".into(),
            seconds: 45,
        })
        .unwrap();
        assert_eq!(
            json,
            r#""Windows storage inventory timed out after 45 seconds""#
        );
    }
}
