use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use ratune_player::PlayerCommand;
use ratune_subsonic::Song;

use crate::app::{App, BrowserColumn, Tab};
use crate::state::NowPlayingPaneFocus;

// ── Saved state ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedState {
    #[serde(default)]
    pub active_tab: Tab,
    #[serde(default)]
    pub browser_focus: BrowserColumn,
    #[serde(default)]
    pub selected_artist: Option<usize>,
    #[serde(default)]
    pub selected_album: Option<usize>,
    #[serde(default)]
    pub selected_track: Option<usize>,
    #[serde(default)]
    pub queue: Vec<Song>,
    #[serde(default)]
    pub queue_cursor: usize,
    /// In-app playback volume 0–100 (software gain; independent of OS / game volume).
    /// Omitted in older state files: keep volume from `config.toml` on restore.
    #[serde(default)]
    pub player_volume: Option<u8>,
    /// Now Playing sidebar: library queue vs radio station list.
    #[serde(default)]
    pub np_pane_focus: NowPlayingPaneFocus,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn state_path() -> Result<PathBuf> {
    let dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("ratune")
    } else {
        let home = std::env::var("HOME").context("HOME env var not set")?;
        PathBuf::from(home).join(".config").join("ratune")
    };
    Ok(dir.join("state.json"))
}

fn read_existing_state() -> Option<SavedState> {
    let path = state_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Keep TUI navigation from `ui`; take live queue/volume/pane from `playback`.
fn merge_daemon_playback_into_saved(ui: SavedState, playback: SavedState) -> SavedState {
    SavedState {
        active_tab: ui.active_tab,
        browser_focus: ui.browser_focus,
        selected_artist: ui.selected_artist,
        selected_album: ui.selected_album,
        selected_track: ui.selected_track,
        queue: playback.queue,
        queue_cursor: playback.queue_cursor,
        player_volume: playback.player_volume,
        np_pane_focus: playback.np_pane_focus,
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Serialize current UI state to `~/.config/ratune/state.json`.
pub fn save_state(app: &App) -> Result<()> {
    let mut state = SavedState {
        active_tab: app.active_tab,
        browser_focus: app.browser_focus,
        selected_artist: app.library.selected_artist,
        selected_album: app.library.selected_album,
        selected_track: app.library.selected_track,
        queue: app.queue.songs.clone(),
        queue_cursor: app.queue.cursor,
        player_volume: Some(app.config.default_volume),
        np_pane_focus: app.np_pane_focus,
    };
    // The daemon never drives Browse/Home selection. Keep those fields from the
    // last TUI save so a 30s persist does not rewind the browser position.
    if app.is_player_daemon() {
        if let Some(existing) = read_existing_state() {
            state = merge_daemon_playback_into_saved(existing, state);
        }
    }
    let path = state_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating state dir {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(&state)?;
    std::fs::write(&path, json).with_context(|| format!("writing state to {}", path.display()))?;
    Ok(())
}

/// Restore previously saved state into `app`. Populates playback display state
/// (current_song, total, paused=true) so the now-playing bar renders immediately,
/// but does NOT send any command to the player engine — the track loads on first play.
pub fn restore_state(app: &mut App) -> Result<()> {
    let path = state_path()?;
    if !path.exists() {
        return Ok(());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let state: SavedState =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    app.active_tab = state.active_tab;
    app.browser_focus = state.browser_focus;
    app.library.selected_artist = state.selected_artist;
    app.library.selected_album = state.selected_album;
    app.library.selected_track = state.selected_track;

    // Client TUI takes live queue/playback from the daemon snapshot after connect.
    if app.is_player_client() {
        return Ok(());
    }

    app.queue.songs = state.queue;
    app.queue.cursor = state
        .queue_cursor
        .min(app.queue.songs.len().saturating_sub(1));
    app.queue.scroll = app.queue.cursor;
    app.queue.adopt_current_order_as_shuffle_baseline();

    app.np_pane_focus = if app.config.radio_enabled {
        state.np_pane_focus
    } else {
        NowPlayingPaneFocus::Queue
    };

    if let Some(vol) = state.player_volume {
        let v = vol.min(100);
        app.config.default_volume = v;
        app.send_player(PlayerCommand::SetVolume(v as f32 / 100.0));
    }

    // Populate display-only playback state so the now-playing bar shows the
    // restored track immediately. `player_loaded` stays false — the engine gets
    // the actual URL only when the user presses play for the first time.
    if let Some(song) = app.queue.current().cloned() {
        let duration = song
            .duration
            .map(|s| std::time::Duration::from_secs(u64::from(s)));
        // Prefetch album art so it's ready when the NowPlaying tab is shown.
        if let Some(cover_id) = &song.cover_art {
            app.fetch_cover_art(cover_id.clone());
        }
        app.playback.current_song = Some(song);
        app.playback.total = duration;
        app.playback.paused = true;
        // player_loaded remains false (default) — engine has no track yet.
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui_only() -> SavedState {
        SavedState {
            active_tab: Tab::Browser,
            browser_focus: BrowserColumn::Albums,
            selected_artist: Some(2),
            selected_album: Some(4),
            selected_track: Some(1),
            queue: Vec::new(),
            queue_cursor: 0,
            player_volume: Some(40),
            np_pane_focus: NowPlayingPaneFocus::Queue,
        }
    }

    fn playback_only() -> SavedState {
        SavedState {
            active_tab: Tab::Home,
            browser_focus: BrowserColumn::Artists,
            selected_artist: None,
            selected_album: None,
            selected_track: None,
            queue: Vec::new(),
            queue_cursor: 7,
            player_volume: Some(80),
            np_pane_focus: NowPlayingPaneFocus::Radio,
        }
    }

    #[test]
    fn daemon_save_keeps_tui_navigation() {
        let merged = merge_daemon_playback_into_saved(ui_only(), playback_only());
        assert_eq!(merged.active_tab, Tab::Browser);
        assert_eq!(merged.browser_focus, BrowserColumn::Albums);
        assert_eq!(merged.selected_artist, Some(2));
        assert_eq!(merged.selected_album, Some(4));
        assert_eq!(merged.selected_track, Some(1));
        assert_eq!(merged.queue_cursor, 7);
        assert_eq!(merged.player_volume, Some(80));
        assert_eq!(merged.np_pane_focus, NowPlayingPaneFocus::Radio);
    }
}
