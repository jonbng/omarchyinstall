//! Official ISO location. Version is resolved at runtime, not compiled in.
//! Trust is the `.sha256` sidecar + pinned GPG key, not GitHub.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

pub const ISO_ORIGIN: &str = "https://iso.omarchy.org";
pub const GITHUB_LATEST_RELEASE: &str =
    "https://api.github.com/repos/omacom/omarchy/releases/latest";
pub const OMARCHY_ISO_SIGNING_FPR: &str = "40DFB630FF42BCFFB047046CF0134EE680CAC571";
pub const OMARCHY_GPG: &str = include_str!("../omarchy.gpg");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IsoRelease {
    pub version: String,
    pub filename: String,
    pub url: String,
}

impl IsoRelease {
    pub fn from_version(version: &str) -> Self {
        let version = version_from_tag(version);
        let filename = format!("omarchy-{version}.iso");
        Self {
            url: format!("{ISO_ORIGIN}/{filename}"),
            filename,
            version,
        }
    }

    pub fn sha256_url(&self) -> String {
        format!("{}.sha256", self.url)
    }

    pub fn sig_url(&self) -> String {
        format!("{}.sig", self.url)
    }
}

pub fn version_from_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('v').to_string()
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
}

pub fn parse_github_latest_tag(json: &str) -> Result<String> {
    let rel: GithubRelease =
        serde_json::from_str(json).map_err(|e| Error::Message(format!("GitHub latest: {e}")))?;
    if rel.tag_name.trim().is_empty() {
        return Err(Error::Message(
            "GitHub latest release has empty tag_name".into(),
        ));
    }
    Ok(rel.tag_name)
}

pub fn normalize_fingerprint(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_becomes_iso_url_without_compiled_pin() {
        let rel = IsoRelease::from_version("v9.9.9");
        assert_eq!(rel.version, "9.9.9");
        assert_eq!(rel.filename, "omarchy-9.9.9.iso");
        assert_eq!(rel.url, "https://iso.omarchy.org/omarchy-9.9.9.iso");
        assert_eq!(
            rel.sha256_url(),
            "https://iso.omarchy.org/omarchy-9.9.9.iso.sha256"
        );
        assert_eq!(
            rel.sig_url(),
            "https://iso.omarchy.org/omarchy-9.9.9.iso.sig"
        );
    }

    #[test]
    fn parses_github_latest_json() {
        let json = r#"{"tag_name":"v1.2.3","name":"ignored"}"#;
        assert_eq!(parse_github_latest_tag(json).unwrap(), "v1.2.3");
        let rel = IsoRelease::from_version(&parse_github_latest_tag(json).unwrap());
        assert_eq!(rel.filename, "omarchy-1.2.3.iso");
        assert!(!rel.url.contains("4.0.2"));
    }

    #[test]
    fn fingerprint_normalizes() {
        assert_eq!(
            normalize_fingerprint("40DF B630 FF42 BCFF B047 046C F013 4EE6 80CA C571"),
            OMARCHY_ISO_SIGNING_FPR
        );
    }
}
