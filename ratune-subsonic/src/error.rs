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

/// Subsonic API error code for invalid username or password.
pub const AUTH_ERROR_CODE: u32 = 40;

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

/// Whether `err` (or its source chain) is a Subsonic credential failure (code 40 or equivalent message).
pub fn is_auth_failure(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur = Some(err);
    while let Some(e) = cur {
        if let Some(se) = e.downcast_ref::<SubsonicError>() {
            if se.code == AUTH_ERROR_CODE {
                return true;
            }
        }
        let msg = e.to_string().to_ascii_lowercase();
        if msg.contains("wrong username")
            || msg.contains("wrong password")
            || msg.contains("invalid username")
            || msg.contains("invalid password")
            || msg.contains("bad credentials")
        {
            return true;
        }
        cur = e.source();
    }
    false
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

    #[test]
    fn is_auth_failure_detects_code_40() {
        let err = SubsonicError {
            code: super::AUTH_ERROR_CODE,
            message: "Wrong username or password.".into(),
        };
        let wrapped = anyhow::anyhow!(err);
        assert!(super::is_auth_failure(wrapped.as_ref()));
    }

    #[test]
    fn is_auth_failure_ignores_other_codes() {
        let err = SubsonicError {
            code: 70,
            message: "not found".into(),
        };
        let wrapped = anyhow::anyhow!(err);
        assert!(!super::is_auth_failure(wrapped.as_ref()));
    }
}
