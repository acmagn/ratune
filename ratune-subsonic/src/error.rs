use anyhow::{bail, Result};
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_status_ok() {
        assert!(check_status("ok", None).is_ok());
    }

    #[test]
    fn check_status_failed_with_error() {
        let err = SubsonicError {
            code: 40,
            message: "not found".into(),
        };
        let r = check_status("failed", Some(&err));
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("40"));
    }

    #[test]
    fn check_status_failed_without_error_bails() {
        let r = check_status("failed", None);
        assert!(r.is_err());
    }

    #[test]
    fn display_formats_code_and_message() {
        let e = SubsonicError {
            code: 0,
            message: "x".into(),
        };
        assert_eq!(e.to_string(), "Subsonic error 0: x");
    }
}
