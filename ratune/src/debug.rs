//! Optional stderr diagnostics. Enable with `RATUNE_DEBUG=1` in the environment.

/// True when `RATUNE_DEBUG` is set (any value).
#[must_use]
pub fn enabled() -> bool {
    std::env::var_os("RATUNE_DEBUG").is_some()
}

/// Print to stderr when [`enabled`].
pub fn log(msg: impl std::fmt::Display) {
    if enabled() {
        eprintln!("ratune: {msg}");
    }
}
