//! Install platform credential stores for [`keyring_core`] and build entries for a chosen backend.
//! See the [keyring ecosystem docs](https://github.com/open-source-cooperative/keyring-rs/wiki/Keyring).

use std::sync::{Arc, OnceLock};

use anyhow::{bail, Result};
use keyring_core::api::CredentialStoreApi;
use keyring_core::{Entry, Error as KeyringError};

/// Linux keyring backend. macOS and Windows always use the native system store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyringBackend {
    /// Linux kernel keyutils (session-ish; may not survive reboot).
    Keyutils,
    /// Linux Secret Service (gnome-keyring, KWallet, etc.).
    SecretService,
}

impl KeyringBackend {
    /// Backend for scrobble API secrets and session keys.
    pub fn scrobble() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::SecretService
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::Keyutils
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Keyutils => "kernel keyutils",
            Self::SecretService => "Secret Service (gnome-keyring, KWallet, …)",
        }
    }
}

/// Parse `[server].password_keyring` (`keyutils` or `secret-service`).
pub fn parse_password_keyring(raw: &str) -> Result<KeyringBackend> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "keyutils" | "kernel" => Ok(KeyringBackend::Keyutils),
        "secret-service" | "secret_service" | "libsecret" | "gnome-keyring" | "gnome_keyring" => {
            Ok(KeyringBackend::SecretService)
        }
        other => bail!(
            "unknown [server].password_keyring value {other:?} — use \"keyutils\" or \"secret-service\""
        ),
    }
}

#[cfg(target_os = "linux")]
static KEYUTILS_STORE: OnceLock<Option<Arc<linux_keyutils_keyring_store::Store>>> = OnceLock::new();

#[cfg(target_os = "linux")]
static SECRET_SERVICE_STORE: OnceLock<Option<Arc<dbus_secret_service_keyring_store::Store>>> =
    OnceLock::new();

#[cfg(target_os = "linux")]
fn linux_keyutils_store() -> Option<Arc<linux_keyutils_keyring_store::Store>> {
    KEYUTILS_STORE
        .get_or_init(|| linux_keyutils_keyring_store::Store::new().ok())
        .clone()
}

#[cfg(target_os = "linux")]
fn linux_secret_service_store() -> Option<Arc<dbus_secret_service_keyring_store::Store>> {
    SECRET_SERVICE_STORE
        .get_or_init(|| dbus_secret_service_keyring_store::Store::new().ok())
        .clone()
}

/// Create a keyring entry using the given backend.
pub fn keyring_entry(
    service: &str,
    user: &str,
    backend: KeyringBackend,
) -> Result<Entry, KeyringError> {
    #[cfg(target_os = "linux")]
    {
        let store: Arc<dyn CredentialStoreApi> = match backend {
            KeyringBackend::Keyutils => linux_keyutils_store()
                .map(|s| s as Arc<dyn CredentialStoreApi>)
                .ok_or(KeyringError::NoDefaultStore)?,
            KeyringBackend::SecretService => linux_secret_service_store()
                .map(|s| s as Arc<dyn CredentialStoreApi>)
                .ok_or(KeyringError::NoDefaultStore)?,
        };
        store.build(service, user, None)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = backend;
        Entry::new(service, user)
    }
}

/// Select and register the default credential store ([`keyring_core::set_default_store`]).
/// On Linux this is keyutils for backward compatibility with `[server].password_keyring = "keyutils"`.
pub fn install_default_keyring_store() {
    #[cfg(target_os = "linux")]
    {
        match linux_keyutils_store() {
            Some(store) => keyring_core::set_default_store(store),
            None => {
                eprintln!(
                    "warning: could not open Linux kernel keyutils store.\n\
                     Keyutils keyring disabled — set [server].password_keyring = \"secret-service\", \
                     use password_command, a session password, [server].password, or SUBSONIC_PASS."
                );
            }
        }
        // Touch the Secret Service store so scrobble helpers fail fast if unavailable.
        let _ = linux_secret_service_store();
    }

    #[cfg(target_os = "macos")]
    {
        match apple_native_keyring_store::keychain::Store::new() {
            Ok(store) => keyring_core::set_default_store(store),
            Err(e) => {
                eprintln!(
                    "warning: could not open macOS keychain store: {e}\n\
                     Keyring storage disabled for this run."
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        match windows_native_keyring_store::Store::new() {
            Ok(store) => keyring_core::set_default_store(store),
            Err(e) => {
                eprintln!(
                    "warning: could not open Windows Credential Manager store: {e}\n\
                     Keyring storage disabled for this run."
                );
            }
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        eprintln!(
            "warning: no keyring backend is bundled for this OS; use [server].password_command, [server].password, or SUBSONIC_PASS."
        );
    }
}
