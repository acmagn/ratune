//! Install the platform credential store for [`keyring_core`] before any [`keyring_core::Entry`]
//! is created. See the [keyring ecosystem docs](https://github.com/open-source-cooperative/keyring-rs/wiki/Keyring).

/// Select and register the OS-native store ([`keyring_core::set_default_store`]).
/// On failure, logs a warning and leaves no default store (credential helpers in `config` fall
/// back to a session-only password prompt).
pub fn install_default_keyring_store() {
    #[cfg(target_os = "linux")]
    {
        match linux_keyutils_keyring_store::Store::new() {
            Ok(store) => keyring_core::set_default_store(store),
            Err(e) => {
                eprintln!(
                    "warning: could not open Linux kernel keyutils store: {e}\n\
                     Keyring storage disabled for this run — use password_command, a session password, [server].password, or SUBSONIC_PASS."
                );
            }
        }
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
        return;
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
        return;
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        eprintln!(
            "warning: no keyring backend is bundled for this OS; use [server].password_command, [server].password, or SUBSONIC_PASS."
        );
    }
}
