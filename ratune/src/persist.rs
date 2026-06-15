use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use ratune_player::PlayerCommand;
use ratune_subsonic::Song;

use crate::app::{App, BrowserColumn, Tab};

// ── Saved state ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
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

// ── Public API ────────────────────────────────────────────────────────────────

/// Serialize current UI state to `~/.config/ratune/state.json`.
pub fn save_state(app: &App) -> Result<()> {
    let state = SavedState {
        active_tab: app.active_tab,
        browser_focus: app.browser_focus,
        selected_artist: app.library.selected_artist,
        selected_album: app.library.selected_album,
        selected_track: app.library.selected_track,
        queue: app.queue.songs.clone(),
        queue_cursor: app.queue.cursor,
        player_volume: Some(app.config.default_volume),
    };
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
    app.queue.songs = state.queue;
    app.queue.cursor = state
        .queue_cursor
        .min(app.queue.songs.len().saturating_sub(1));
    app.queue.scroll = app.queue.cursor;
    app.queue.adopt_current_order_as_shuffle_baseline();

    if let Some(vol) = state.player_volume {
        let v = vol.min(100);
        app.config.default_volume = v;
        let _ = app
            .player_tx
            .send(PlayerCommand::SetVolume(v as f32 / 100.0));
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
