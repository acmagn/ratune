//! Desktop notifications (FreeDesktop `org.freedesktop.Notifications`, same as `notify-send`).

use notify_rust::{Notification, Timeout};

/// Fire-and-forget: show a system notification when a forced library index refresh finishes.
/// Runs on a background thread so the TUI thread never blocks on D-Bus.
pub fn spawn_forced_library_index_complete() {
    std::thread::spawn(|| {
        let _ = show_forced_library_index_complete();
    });
}

fn show_forced_library_index_complete() -> Result<(), notify_rust::error::Error> {
    Notification::new()
        .summary("Playterm")
        .body("Library index refresh complete")
        .timeout(Timeout::Milliseconds(8000))
        .show()?;
    Ok(())
}
