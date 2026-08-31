//! Official ISO fetch + sha256 + pinned GPG. OS-agnostic; dest is a cache
//! directory until the installer partition exists.

use crate::error::{Error, Result};
use crate::iso::{
    self, normalize_fingerprint, IsoRelease, GITHUB_LATEST_RELEASE, OMARCHY_GPG,
    OMARCHY_ISO_SIGNING_FPR,
};
use sequoia_openpgp::parse::stream::{
    DetachedVerifierBuilder, MessageLayer, MessageStructure, VerificationHelper,
};
use sequoia_openpgp::parse::Parse;
use sequoia_openpgp::policy::StandardPolicy;
use sequoia_openpgp::Cert;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::sync::Mutex;

fn download_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

const CHUNK: usize = 1024 * 1024;
const UA: &str = concat!("OmarchyInstall/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IsoProgress {
    pub phase: &'static str,
    pub bytes: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    pub sha256: String,
    pub bytes: u64,
}

pub struct IsoPaths {
    pub iso: PathBuf,
    pub sha256: PathBuf,
    pub sig: PathBuf,
}

pub fn iso_cache_dir() -> Result<PathBuf> {
    let base = if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| Error::Message("LOCALAPPDATA is unset".into()))?;
        PathBuf::from(local).join("OmarchyInstall")
    } else {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .ok_or_else(|| Error::Message("HOME is unset".into()))?;
        cache.join("omarchy-install")
    };
    let dir = base.join("iso");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn resolved_path(dir: &Path) -> PathBuf {
    dir.join("resolved.json")
}

pub fn load_resolved_release() -> Result<Option<IsoRelease>> {
    let path = resolved_path(&iso_cache_dir()?);
    match fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(body) => {
            Ok(Some(serde_json::from_str(&body).map_err(|e| {
                Error::Message(format!("resolved.json: {e}"))
            })?))
        }
    }
}

pub fn save_resolved_release(rel: &IsoRelease) -> Result<()> {
    let dir = iso_cache_dir()?;
    fs::write(
        resolved_path(&dir),
        serde_json::to_vec_pretty(rel).map_err(|e| Error::Message(e.to_string()))?,
    )?;
    Ok(())
}

pub fn iso_paths_for(rel: &IsoRelease) -> Result<IsoPaths> {
    let dir = iso_cache_dir()?;
    Ok(IsoPaths {
        iso: dir.join(&rel.filename),
        sha256: dir.join(format!("{}.sha256", rel.filename)),
        sig: dir.join(format!("{}.sig", rel.filename)),
    })
}

pub fn iso_paths() -> Result<IsoPaths> {
    let rel = load_resolved_release()?.ok_or_else(|| {
        Error::Message("no resolved ISO yet; download_iso first (latest GitHub release)".into())
    })?;
    iso_paths_for(&rel)
}

pub async fn resolve_iso_release(client: &reqwest::Client) -> Result<IsoRelease> {
    if let Ok(url) = std::env::var("OMARCHY_ISO_URL") {
        let filename = url.rsplit('/').next().unwrap_or("omarchy.iso").to_string();
        let version = filename
            .strip_prefix("omarchy-")
            .and_then(|s| s.strip_suffix(".iso"))
            .unwrap_or("unknown")
            .to_string();
        return Ok(IsoRelease {
            version,
            filename,
            url,
        });
    }
    if let Ok(ver) = std::env::var("OMARCHY_ISO_VERSION") {
        return Ok(IsoRelease::from_version(&ver));
    }
    if let Some(cached) = load_resolved_release()? {
        if cached.filename.starts_with("omarchy-") && !cached.url.is_empty() {
            log::info!("using cached ISO release {}", cached.version);
            return Ok(cached);
        }
    }
    let body = client
        .get(GITHUB_LATEST_RELEASE)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| Error::Message(format!("GitHub latest release: {e}")))?
        .error_for_status()
        .map_err(|e| Error::Message(format!("GitHub latest release: {e}")))?
        .text()
        .await
        .map_err(|e| Error::Message(e.to_string()))?;
    let tag = iso::parse_github_latest_tag(&body)?;
    let rel = IsoRelease::from_version(&tag);
    log::info!("resolved official ISO {} -> {}", rel.version, rel.url);
    save_resolved_release(&rel)?;
    Ok(rel)
}

/// GNU coreutils `sha256sum` line: `<hash>  <filename>` (two spaces) or `hash *file`.
pub fn parse_sha256_sidecar(text: &str) -> Result<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .ok_or_else(|| Error::Message("sha256 sidecar is empty".into()))?;
    let hash = line
        .split_whitespace()
        .next()
        .ok_or_else(|| Error::Message("sha256 sidecar has no hash".into()))?;
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Message(
            "sha256 sidecar is not a 64-char hex digest".into(),
        ));
    }
    Ok(hash.to_ascii_lowercase())
}

pub fn hash_file(path: &Path, mut on_progress: impl FnMut(u64, u64)) -> Result<(String, u64)> {
    let total = fs::metadata(path)?.len();
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(CHUNK, file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut bytes = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        bytes += n as u64;
        on_progress(bytes, total);
    }
    if bytes != total {
        return Err(Error::Message(format!(
            "read {bytes} bytes but file is {total}"
        )));
    }
    Ok((hex_lower(&hasher.finalize()), bytes))
}

pub fn verify_detached_signature(cert_armored: &str, sig: &[u8], data: &Path) -> Result<String> {
    let cert = Cert::from_bytes(cert_armored.as_bytes())
        .map_err(|e| Error::Message(format!("embedded Omarchy public key: {e}")))?;
    let fpr = normalize_fingerprint(&cert.fingerprint().to_string());
    if fpr != OMARCHY_ISO_SIGNING_FPR {
        return Err(Error::Message(format!(
            "embedded key fingerprint {fpr} != pinned {OMARCHY_ISO_SIGNING_FPR}"
        )));
    }

    let policy = StandardPolicy::new();
    let helper = Helper { cert, good: false };
    let mut verifier = DetachedVerifierBuilder::from_bytes(sig)
        .map_err(|e| Error::Message(format!("ISO signature: {e}")))?
        .with_policy(&policy, None, helper)
        .map_err(|e| Error::Message(format!("ISO signature policy: {e}")))?;
    verifier
        .verify_file(data)
        .map_err(|e| Error::Message(format!("ISO GPG verification failed: {e}")))?;
    Ok(fpr)
}

struct Helper {
    cert: Cert,
    good: bool,
}

impl VerificationHelper for Helper {
    fn get_certs(
        &mut self,
        _ids: &[sequoia_openpgp::KeyHandle],
    ) -> sequoia_openpgp::Result<Vec<Cert>> {
        Ok(vec![self.cert.clone()])
    }

    fn check(&mut self, structure: MessageStructure) -> sequoia_openpgp::Result<()> {
        for layer in structure {
            if let MessageLayer::SignatureGroup { results } = layer {
                self.good = results.iter().any(|r| r.is_ok());
            }
        }
        if self.good {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "signature is not from the pinned Omarchy key"
            ))
        }
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(|e| Error::Message(e.to_string()))
}

async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Message(e.to_string()))?
        .error_for_status()
        .map_err(|e| Error::Message(e.to_string()))?;
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| Error::Message(e.to_string()))
}

pub async fn download_iso_files(mut on_progress: impl FnMut(IsoProgress)) -> Result<IsoPaths> {
    let _guard = download_lock().lock().await;
    let client = client()?;
    let rel = resolve_iso_release(&client).await?;
    save_resolved_release(&rel)?;
    let paths = iso_paths_for(&rel)?;

    let sha = fetch_bytes(&client, &rel.sha256_url()).await?;
    fs::write(&paths.sha256, &sha)?;
    let sig = fetch_bytes(&client, &rel.sig_url()).await?;
    if sig.is_empty() {
        return Err(Error::Message("ISO .sig was empty".into()));
    }
    fs::write(&paths.sig, &sig)?;

    let head = client
        .head(&rel.url)
        .send()
        .await
        .map_err(|e| Error::Message(e.to_string()))?;
    let total = head
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .or(head.content_length());

    let mut have = if paths.iso.exists() {
        fs::metadata(&paths.iso)?.len()
    } else {
        0
    };
    if let Some(t) = total {
        if have > t {
            fs::remove_file(&paths.iso)?;
            have = 0;
        } else if have == t {
            on_progress(IsoProgress {
                phase: "download",
                bytes: have,
                total,
            });
            return Ok(paths);
        }
    }

    let mut req = client.get(&rel.url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| Error::Message(e.to_string()))?
        .error_for_status()
        .map_err(|e| Error::Message(e.to_string()))?;

    let status = resp.status();
    let append = have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
    if have > 0 && !append {
        fs::remove_file(&paths.iso)?;
        have = 0;
    }

    let total = total.or(resp.content_length().map(|n| n + have));
    let mut opts = fs::OpenOptions::new();
    opts.create(true).write(true);
    if append {
        opts.append(true);
    } else {
        opts.truncate(true);
    }
    let mut file = opts.open(&paths.iso)?;

    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut bytes = have;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Error::Message(e.to_string()))?;
        file.write_all(&chunk)?;
        bytes += chunk.len() as u64;
        on_progress(IsoProgress {
            phase: "download",
            bytes,
            total,
        });
    }
    file.flush()?;
    Ok(paths)
}

pub fn verify_iso_files(mut on_progress: impl FnMut(IsoProgress)) -> Result<VerifyResult> {
    let paths = iso_paths()?;
    if !paths.iso.exists() {
        return Err(Error::Message("ISO is not downloaded yet".into()));
    }
    if !paths.sha256.exists() {
        return Err(Error::Message("ISO .sha256 sidecar is missing".into()));
    }
    if !paths.sig.exists() {
        return Err(Error::Message("ISO .sig is missing".into()));
    }

    let expected = parse_sha256_sidecar(&fs::read_to_string(&paths.sha256)?)?;
    let (actual, bytes) = hash_file(&paths.iso, |done, total| {
        on_progress(IsoProgress {
            phase: "hash",
            bytes: done,
            total: Some(total),
        });
    })?;
    if actual != expected {
        return Err(Error::Message(format!(
            "ISO sha256 mismatch (got {actual}, expected {expected})"
        )));
    }

    let sig = fs::read(&paths.sig)?;
    if sig.is_empty() {
        return Err(Error::Message("ISO .sig is missing".into()));
    }
    on_progress(IsoProgress {
        phase: "signature",
        bytes,
        total: Some(bytes),
    });
    let fpr = verify_detached_signature(OMARCHY_GPG, &sig, &paths.iso)?;
    if fpr != OMARCHY_ISO_SIGNING_FPR {
        return Err(Error::Message(format!(
            "ISO signed by {fpr}, expected {OMARCHY_ISO_SIGNING_FPR}"
        )));
    }
    Ok(VerifyResult {
        sha256: actual,
        bytes,
    })
}

/// ~6 GiB, same order of magnitude as the official ISO. Progress only; no file.
pub const STUB_ISO_BYTES: u64 = 6 * 1024 * 1024 * 1024;

/// Linux/macOS `tauri dev` skips the real ISO unless `OMARCHY_STUB_REAL_ISO` is set.
pub fn stub_skips_iso() -> bool {
    crate::platform::is_stub_host() && std::env::var_os("OMARCHY_STUB_REAL_ISO").is_none()
}

pub fn stub_iso_sha256() -> String {
    hex_lower(&Sha256::digest(b"omarchy-install-dry-run"))
}

fn stub_tick_delay() -> std::time::Duration {
    if cfg!(test) {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_millis(70)
    }
}

/// Fake download progress. Does not write an ISO and does not hit the network.
pub async fn skip_iso_download(mut on_progress: impl FnMut(IsoProgress)) -> Result<()> {
    let total = STUB_ISO_BYTES;
    let steps = 8u64;
    let delay = stub_tick_delay();
    for i in 1..=steps {
        on_progress(IsoProgress {
            phase: "download",
            bytes: total * i / steps,
            total: Some(total),
        });
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    Ok(())
}

/// Fake sha256 + GPG phases. Does not read a file.
pub fn skip_iso_verify(mut on_progress: impl FnMut(IsoProgress)) -> Result<VerifyResult> {
    let bytes = STUB_ISO_BYTES;
    on_progress(IsoProgress {
        phase: "hash",
        bytes,
        total: Some(bytes),
    });
    on_progress(IsoProgress {
        phase: "signature",
        bytes,
        total: Some(bytes),
    });
    Ok(VerifyResult {
        sha256: stub_iso_sha256(),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso::OMARCHY_ISO_SIGNING_FPR;

    #[test]
    fn parses_coreutils_sidecar() {
        let text =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  omarchy-9.9.9.iso\n";
        assert_eq!(
            parse_sha256_sidecar(text).unwrap(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn rejects_short_sidecar() {
        assert!(parse_sha256_sidecar("deadbeef  file\n").is_err());
        assert!(parse_sha256_sidecar("").is_err());
    }

    #[test]
    fn hash_match_and_mismatch() {
        let dir = tempfile_dir();
        let path = dir.join("blob");
        fs::write(&path, b"omarchy").unwrap();
        let (digest, bytes) = hash_file(&path, |_, _| {}).unwrap();
        assert_eq!(bytes, 7);
        let expected = hex_lower(&Sha256::digest(b"omarchy"));
        assert_eq!(digest, expected);

        fs::write(&path, b"nope").unwrap();
        let (other, _) = hash_file(&path, |_, _| {}).unwrap();
        assert_ne!(other, expected);
    }

    #[test]
    fn missing_sig_fails_closed() {
        let prev = std::env::var_os("XDG_CACHE_HOME");
        let home = tempfile_dir();
        std::env::set_var("XDG_CACHE_HOME", &home);
        std::env::set_var("HOME", &home);
        let rel = IsoRelease::from_version("9.9.9");
        crate::download::save_resolved_release(&rel).unwrap();
        let paths = iso_paths_for(&rel).unwrap();
        fs::write(&paths.iso, b"iso").unwrap();
        fs::write(
            &paths.sha256,
            format!("{}  {}\n", hex_lower(&Sha256::digest(b"iso")), rel.filename),
        )
        .unwrap();
        let err = verify_iso_files(|_| {}).unwrap_err().to_string();
        assert!(err.contains(".sig"), "{err}");
        match prev {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
    }

    #[test]
    fn embedded_key_fingerprint_is_pinned() {
        let cert = Cert::from_bytes(OMARCHY_GPG.as_bytes()).unwrap();
        assert_eq!(
            normalize_fingerprint(&cert.fingerprint().to_string()),
            OMARCHY_ISO_SIGNING_FPR
        );
    }

    #[test]
    fn stub_verify_is_64_hex_and_six_gib() {
        let result = skip_iso_verify(|_| {}).unwrap();
        assert_eq!(result.bytes, STUB_ISO_BYTES);
        assert_eq!(result.sha256.len(), 64);
        assert!(result.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(result.sha256, stub_iso_sha256());
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omarchyinstall-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
