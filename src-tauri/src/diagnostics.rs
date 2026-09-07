//! Persistent, bounded diagnostics for intermittent machine-probe failures.

use crate::platform::MachineProbe;
use serde_json::{json, Value};
use std::fs;

const MAX_PROBE_ATTEMPTS: usize = 20;
pub const PROBE_HISTORY_FILE: &str = "probe-history.json";

pub fn record_probe_attempt(
    started_unix_ms: u128,
    elapsed_ms: u128,
    result: &crate::error::Result<MachineProbe>,
) {
    if let Err(error) = record_probe_attempt_inner(started_unix_ms, elapsed_ms, result) {
        log::warn!("could not persist probe diagnostics: {error}");
    }
}

fn record_probe_attempt_inner(
    started_unix_ms: u128,
    elapsed_ms: u128,
    result: &crate::error::Result<MachineProbe>,
) -> crate::error::Result<()> {
    let path = crate::paths::install_data_dir()?.join(PROBE_HISTORY_FILE);
    let mut attempts: Vec<Value> = fs::read_to_string(&path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default();
    let outcome = match result {
        Ok(probe) => json!({
            "status": "completed",
            "blockingReasonCount": probe.blocking_reasons.len(),
            "probe": probe,
        }),
        Err(error) => json!({
            "status": "failed",
            "error": error.to_string(),
        }),
    };
    attempts.push(json!({
        "startedUnixMs": started_unix_ms,
        "elapsedMs": elapsed_ms,
        "appVersion": env!("CARGO_PKG_VERSION"),
        "outcome": outcome,
    }));
    if attempts.len() > MAX_PROBE_ATTEMPTS {
        attempts.drain(..attempts.len() - MAX_PROBE_ATTEMPTS);
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&attempts)
            .map_err(|error| crate::error::Error::Message(error.to_string()))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_bounded() {
        let mut attempts: Vec<Value> = (0..=MAX_PROBE_ATTEMPTS).map(|n| json!(n)).collect();
        if attempts.len() > MAX_PROBE_ATTEMPTS {
            attempts.drain(..attempts.len() - MAX_PROBE_ATTEMPTS);
        }
        assert_eq!(attempts.len(), MAX_PROBE_ATTEMPTS);
        assert_eq!(attempts[0], json!(1));
    }
}
