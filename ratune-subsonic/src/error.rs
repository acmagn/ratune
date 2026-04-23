use anyhow::{Result, bail};
use serde::Deserialize;

/// An application-level error returned by the Subsonic server (HTTP 200, status `"failed"`).
#[derive(Debug, Clone, Deserialize)]
pub struct SubsonicError {
    /// Subsonic error code (see API docs for the full list).
    pub code: u32,
    /// Human-readable error message.
    pub message: String,
}

impl std::fmt::Display for SubsonicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Subsonic error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for SubsonicError {}

/// Check a raw `status`/`error` pair from any Subsonic response body.
pub(crate) fn check_status(status: &str, error: Option<&SubsonicError>) -> Result<()> {
    if status == "ok" {
        return Ok(());
    }
    if let Some(e) = error {
        bail!("{e}");
    }
    bail!("Subsonic returned non-ok status: {status}");
}
