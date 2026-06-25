//! Mouse double-click detection (two clicks on the same target within a short window).

use std::time::{Duration, Instant};

use crate::app::App;

/// Maximum interval between two clicks that counts as a double-click.
pub const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(450);

/// Identifies a clickable list/item target for double-click pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseClickTarget {
    BrowserArtist(usize),
    BrowserAlbum(usize),
    BrowserTrack(usize),
    FolderDir(usize),
    FolderPreview(usize),
    HomeRecentAlbum(usize),
    HomeRecentTrack(usize),
    HomeRediscover(usize),
    PlaylistList(usize),
    PlaylistTrack(usize),
    FavoritesCategory(usize),
    FavoritesItem(usize),
    QueueRow(usize),
}

/// Returns `true` when this click is the second half of a double-click on `target`.
pub fn is_double_click(app: &mut App, target: MouseClickTarget) -> bool {
    let now = Instant::now();
    let is_double = app
        .last_mouse_click
        .is_some_and(|(t, prev)| prev == target && now.duration_since(t) <= DOUBLE_CLICK_INTERVAL);
    if is_double {
        app.last_mouse_click = None;
    } else {
        app.last_mouse_click = Some((now, target));
    }
    is_double
}

/// Clear pending single-click state (e.g. after a non-list click).
pub fn clear_pending_click(app: &mut App) {
    app.last_mouse_click = None;
}
