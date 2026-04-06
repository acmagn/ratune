use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, mpsc as std_mpsc};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::style::Color;
use tokio::sync::mpsc;

use playterm_player::{PlayerCommand, PlayerEvent, SampleBuffer, spawn_player};
use playterm_subsonic::SubsonicClient;

use serde::{Deserialize, Serialize};

use crate::action::{Action, Direction};
use crate::color::{extract_accent, lerp_color};
use crate::config::Config;
use crate::history::PlayRecord;
use crate::keybinds::Keybinds;
use crate::state::{ConfirmAction, LibraryState, LoadingState, PlaybackState, PlaylistFocus, PlaylistInputMode, PlaylistOverlay, QueueState};
use crate::theme::Theme;
use playterm_subsonic::LyricLine;

// ── Sizing helper (used in dispatch for scroll clamping) ──────────────────────

/// Map raw engine/network errors to a short status-bar line (no multiline chains).
fn humanize_playback_error(message: &str) -> String {
    let raw = message
        .strip_prefix("playback error: ")
        .or_else(|| message.strip_prefix("enqueue error: "))
        .unwrap_or(message);
    let lower = raw.to_lowercase();
    if lower.contains("decoding response body") || lower.contains("error decoding") {
        return "Playback failed: stream interrupted or invalid (retry)".to_string();
    }
    if lower.contains("stream http") || lower.contains("http 4") || lower.contains("http 5") {
        return "Playback failed: server error or denied".to_string();
    }
    if lower.contains("connecting to stream")
        || lower.contains("connection refused")
        || lower.contains("error sending request")
        || lower.contains("timeout")
        || lower.contains("dns")
    {
        return "Playback failed: network error".to_string();
    }
    if lower.contains("enqueue error") {
        return "Next track: download failed".to_string();
    }
    if lower.contains("reading stream body") {
        return "Playback failed: stream interrupted (retry)".to_string();
    }
    if lower.contains("reading cached track") {
        return "Playback failed: offline cache unreadable (try disabling cache)".to_string();
    }
    let one_line: String = raw
        .lines()
        .next()
        .unwrap_or(raw)
        .trim()
        .chars()
        .take(70)
        .collect();
    if one_line.is_empty() {
        return "Playback failed".to_string();
    }
    if one_line.chars().count() == 70 {
        format!("{one_line}…")
    } else {
        one_line
    }
}

/// Subsonic stream URL vs on-disk offline cache (see [`App::resolve_playback`]).
enum ResolvedPlayback {
    Url(String),
    Cached(PathBuf),
}

/// Approximate number of visible album thumbnails given cell pixel dimensions.
/// Assumes the strip is ~80 columns wide (fallback when terminal size unavailable).
fn compute_visible_count(cell_px: Option<(u16, u16)>, fallback: usize) -> usize {
    use crate::ui::kitty_art::{art_strip_thumbnail_size, visible_thumbnail_count};
    let (thumb_cols, _) = art_strip_thumbnail_size(cell_px, 8);
    let approx_term_cols = 80u16;
    let count = visible_thumbnail_count(approx_term_cols, thumb_cols, 1);
    if count < 1 { fallback } else { count }
}

// ── Tab ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    #[default]
    Home,
    Browser,
    NowPlaying,
}

impl Tab {
    /// Cycle forward: Home → Browser → NowPlaying → Home
    pub fn next(self) -> Self {
        match self {
            Tab::Home       => Tab::Browser,
            Tab::Browser    => Tab::NowPlaying,
            Tab::NowPlaying => Tab::Home,
        }
    }

    /// Cycle backward: Home → NowPlaying → Browser → Home
    pub fn prev(self) -> Self {
        match self {
            Tab::Home       => Tab::NowPlaying,
            Tab::Browser    => Tab::Home,
            Tab::NowPlaying => Tab::Browser,
        }
    }

    #[allow(dead_code)]
    pub fn toggle(self) -> Self {
        self.next()
    }
}

// ── BrowserColumn ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserColumn {
    #[default]
    Artists,
    Albums,
    Tracks,
}

impl BrowserColumn {
    pub fn left(self) -> Self {
        match self {
            BrowserColumn::Artists => BrowserColumn::Artists,
            BrowserColumn::Albums => BrowserColumn::Artists,
            BrowserColumn::Tracks => BrowserColumn::Albums,
        }
    }

    #[allow(dead_code)]
    pub fn right(self) -> Self {
        match self {
            BrowserColumn::Artists => BrowserColumn::Albums,
            BrowserColumn::Albums => BrowserColumn::Tracks,
            BrowserColumn::Tracks => BrowserColumn::Tracks,
        }
    }
}

// ── SearchMode ────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct SearchMode {
    pub active: bool,
    pub query: String,
    /// Index within the filtered result list that is currently selected.
    pub selected: usize,
}

// ── HomeSection / HomeState ───────────────────────────────────────────────────

/// A recently-played album entry for the Home tab art strip.
#[derive(Debug, Clone)]
pub struct RecentAlbum {
    pub album_id: String,
    pub album_name: String,
    pub artist_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HomeSection {
    #[default]
    RecentAlbums,
    RecentTracks,
    #[allow(dead_code)]
    TopArtists,
    Rediscover,
}

impl HomeSection {
    pub fn next(self) -> Self {
        match self {
            HomeSection::RecentAlbums => HomeSection::RecentTracks,
            HomeSection::RecentTracks => HomeSection::Rediscover,
            HomeSection::TopArtists  => HomeSection::Rediscover,
            HomeSection::Rediscover  => HomeSection::RecentAlbums,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            HomeSection::RecentAlbums => HomeSection::Rediscover,
            HomeSection::RecentTracks => HomeSection::RecentAlbums,
            HomeSection::TopArtists  => HomeSection::RecentAlbums,
            HomeSection::Rediscover  => HomeSection::RecentTracks,
        }
    }
}

#[derive(Debug, Default)]
pub struct HomeState {
    pub active_section: HomeSection,
    /// Up to 20 recently-played unique albums (most recent first) for the art strip.
    pub recent_albums: Vec<RecentAlbum>,
    /// Scroll offset for the art strip (horizontal).
    pub album_scroll_offset: usize,
    /// Selected album index within `recent_albums`.
    pub album_selected_index: usize,
    /// Last 10 plays from history, most recent first.
    pub recent_tracks: Vec<PlayRecord>,
    /// Top artists: (artist_id, artist_name, play_count).
    pub top_artists: Vec<(String, String, u64)>,
    /// Rediscover suggestions: (artist_id, artist_name).
    pub rediscover: Vec<(String, String)>,
    /// Cursor within the active section.
    pub selected_index: usize,
}

// ── LibraryUpdate — messages sent back from background fetch tasks ─────────────

#[derive(Debug)]
pub enum LibraryUpdate {
    Artists(Result<Vec<playterm_subsonic::Artist>, String>),
    Albums {
        artist_id: String,
        result: Result<Vec<playterm_subsonic::Album>, String>,
    },
    Tracks {
        album_id: String,
        result: Result<Vec<playterm_subsonic::Song>, String>,
    },
    /// All tracks across every album for an artist; carries whether playback
    /// should auto-start (true when the queue was empty at dispatch time).
    AllTracksForArtist {
        songs: Vec<playterm_subsonic::Song>,
        start_playing: bool,
    },
    /// Raw image bytes for a cover art ID fetched from Navidrome.
    CoverArt { cover_id: String, bytes: Vec<u8> },
    /// Lyrics fetched for a song; `lines` is empty when the track has no lyrics.
    Lyrics { song_id: String, lines: Vec<LyricLine> },
    /// Track bytes downloaded for offline caching.
    CacheTrack { song_id: String, album_id: String, bytes: Vec<u8> },
    /// Cover art fetched for a home-tab album strip thumbnail.
    HomeArt { album_id: String, bytes: Vec<u8> },
    /// All playlists fetched from `getPlaylists`.
    Playlists(Vec<playterm_subsonic::Playlist>),
    /// Full track list for a single playlist fetched from `getPlaylist`.
    PlaylistTracks { playlist_id: String, songs: Vec<playterm_subsonic::Song> },
    /// A new playlist was successfully created.
    PlaylistCreated(playterm_subsonic::Playlist),
    /// A playlist was successfully deleted (carries the deleted ID).
    PlaylistDeleted(String),
    /// A playlist was successfully renamed.
    PlaylistRenamed { id: String, new_name: String },
    /// A track was successfully added to a playlist.
    PlaylistTrackAdded { _playlist_id: String, playlist_name: String },
    /// A track was successfully removed from a playlist.
    PlaylistTrackRemoved { _playlist_id: String, index: usize },
    /// Playlist list fetched for the picker (separate from the overlay's list).
    PlaylistsForPicker(Vec<playterm_subsonic::Playlist>),
    /// Full library metadata index finished refreshing (background task).
    /// Tuple:
    /// - Vec<Song>: fresh index contents
    /// - Option<String>: Navidrome `lastScan` token to persist when scan-skip is enabled.
    /// - bool: whether this refresh was explicitly forced by the user (Ctrl+r).
    LibraryIndexRefreshComplete {
        result: Result<(Vec<playterm_subsonic::Song>, Option<String>, bool), String>,
    },
}

// ── PlaylistPicker ────────────────────────────────────────────────────────────

/// Floating picker shown when the user wants to add a browser track to a
/// playlist.  Populated lazily from `getPlaylists`.
#[derive(Debug)]
pub struct PlaylistPicker {
    pub playlists: Vec<playterm_subsonic::Playlist>,
    pub selected_index: usize,
    /// The song ID to be added to whichever playlist the user selects.
    pub song_id: String,
    /// `true` while a `getPlaylists` fetch is in flight.
    pub loading: bool,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    pub active_tab: Tab,
    pub browser_focus: BrowserColumn,
    pub library: LibraryState,
    pub queue: QueueState,
    pub playback: PlaybackState,
    pub config: Config,
    pub subsonic: Arc<SubsonicClient>,
    /// Receives library data from background tokio tasks.
    pub library_rx: mpsc::Receiver<LibraryUpdate>,
    library_tx: mpsc::Sender<LibraryUpdate>,
    /// Send commands to the audio engine thread.
    pub player_tx: std_mpsc::Sender<PlayerCommand>,
    /// Receive events from the audio engine thread.
    pub player_rx: std_mpsc::Receiver<PlayerEvent>,
    /// Join handle for the audio engine thread; taken on shutdown.
    pub player_join: Option<std::thread::JoinHandle<()>>,
    pub should_quit: bool,
    pub search_mode: SearchMode,
    /// Active filter applied to the current browser column after a search confirm.
    /// `None` = show all items; `Some(q)` = show only items whose name contains `q`.
    pub search_filter: Option<String>,
    /// Whether the running terminal supports the Kitty graphics protocol.
    /// Set once by `main` before the TUI loop starts.
    pub kitty_supported: bool,
    /// Whether playterm is running inside a tmux session.
    /// Set once by `main` at startup via `$TMUX` env var.
    pub in_tmux: bool,
    /// Row offset caused by a top-positioned tmux status bar (0 or 1).
    /// Used to correct Unicode placeholder cursor positioning in tmux mode.
    pub tmux_status_offset: u16,
    /// Terminal cell pixel dimensions `(width_px, height_px)`.
    /// Queried once at startup; `None` if unavailable (fallback: 8×16).
    pub cell_px: Option<(u16, u16)>,
    /// Cached cover art: `(cover_art_id, raw_image_bytes)`.
    /// Updated whenever a new track starts with a different cover ID.
    pub art_cache: Option<(String, Vec<u8>)>,
    /// Home tab album art cache: `album_id → raw image bytes`.
    pub home_art_cache: HashMap<String, Vec<u8>>,
    /// Album IDs for which a home art fetch is currently in flight.
    pub home_art_loading: HashSet<String>,
    /// Resolved keybindings (parsed from config.toml [keybinds]).
    pub keybinds: Keybinds,
    /// Resolved theme colours (parsed from config.toml [theme]).
    pub theme: Theme,
    /// Monotonically increasing counter sent with every play command (`PlayUrl` / `PlayCached`).
    /// The engine uses it to discard stale downloads from rapid skips.
    play_gen: u64,

    // ── Library metadata index (Milestone 2) ───────────────────────────────────
    /// Cached tracks for fzf (text only; persisted under `~/.cache/playterm/` by default).
    pub library_index_tracks: Vec<playterm_subsonic::Song>,
    library_index_by_id: HashMap<String, playterm_subsonic::Song>,
    /// Unix seconds when the index was last fully refreshed, if known.
    pub library_index_refreshed_at: Option<u64>,
    /// True while a background full-library fetch is running.
    pub library_index_refreshing: bool,
    /// When the current refresh started; drives status-bar animation until complete.
    pub library_index_refresh_started: Option<Instant>,

    // ── Offline cache (Feature 5.3) ───────────────────────────────────────────
    /// Track file cache (LRU, persisted to `~/.cache/playterm/`).
    pub cache: crate::cache::TrackCache,
    /// Monotonically increasing counter for background download tasks.
    /// Incremented on every `play_current()` call. Background tasks discard
    /// their result if the gen has advanced since they were spawned.
    prefetch_gen: Arc<AtomicU64>,

    // ── Help popup (Feature 5.2.1) ────────────────────────────────────────────
    /// Whether the keybind reference popup is open.
    pub help_visible: bool,
    /// Set to `true` when the `i` popup is closed while on the Home tab so the
    /// art strip can be re-rendered on the next frame (same pattern as tab-switch
    /// art restoration).
    pub home_art_needs_redraw: bool,
    /// Timestamp of the last tmux art-strip render; used to batch HomeArt
    /// arrivals so each individual fetch doesn't trigger a full re-transmit.
    pub home_art_last_tmux_render: Option<Instant>,

    // ── Lyrics (Feature 5.2) ──────────────────────────────────────────────────
    /// Whether the lyrics overlay is currently visible (NowPlaying tab only).
    pub lyrics_visible: bool,
    /// Cached lyrics: `(song_id, lines)`. Empty `lines` = server has no lyrics.
    /// `None` = not yet fetched for the current song.
    pub lyrics_cache: Option<(String, Vec<LyricLine>)>,
    /// Scroll offset for unsynced lyrics (manual j/k scrolling).
    pub lyrics_scroll: usize,
    /// True while an async lyrics fetch is in flight.
    pub lyrics_loading: bool,

    // ── Visualizer (Phase 7) ──────────────────────────────────────────────────
    /// Shared ring buffer of the latest decoded f32 audio samples (max 4096).
    /// Written by the audio thread via SampleTap; read here to drive the FFT.
    pub sample_buffer: SampleBuffer,
    /// Smoothed, normalized frequency bands for the visualizer (0.0–1.0, len=32).
    pub spectrum_bands: Vec<f32>,
    /// Whether the spectrum visualizer overlay is currently visible.
    pub visualizer_visible: bool,
    /// FFT planner — cached across frames for efficiency.
    pub fft_planner: rustfft::FftPlanner<f32>,

    // ── Home tab state (Phase 6.3) ───────────────────────────────────────────
    /// Cached display data for the Home tab; refreshed on tab entry.
    pub home: HomeState,
    /// When `Some(name)`, the Browser tab will pre-select the artist with this
    /// name on the next render pass, then clear the field.
    pub pending_artist_select: Option<String>,

    // ── Playlist overlay (Phase 8) ────────────────────────────────────────────
    pub playlist_overlay: PlaylistOverlay,
    /// Floating picker for "add track to playlist" (None when not open).
    pub playlist_picker: Option<PlaylistPicker>,
    /// Transient status message shown in the status bar. Second element is when
    /// the message should be cleared (`Instant::now() >= deadline`).
    pub status_flash: Option<(String, Instant)>,

    // ── Play history (Phase 6.1) ──────────────────────────────────────────────
    /// Persistent play history (loaded on startup, saved on quit).
    pub history: crate::history::PlayHistory,
    /// Reset to `false` on every `TrackStarted`; set to `true` once the
    /// scrobble threshold (50% or 30 s, whichever is shorter) is crossed.
    pub play_recorded: bool,

    // ── Dynamic accent (Feature 5.1) ──────────────────────────────────────────
    /// Dominant colour extracted from the current track's album art.
    /// `None` = no art / no suitable colour found.
    pub dynamic_accent: Option<Color>,
    /// Currently displayed accent — interpolates toward `dynamic_accent` over 400 ms.
    /// Initialised to `theme.accent`; updated each render tick.
    pub accent_current: Color,
    /// Accent value at the start of the current transition.
    accent_lerp_from: Color,
    /// Target accent for the current transition.
    accent_target: Color,
    /// When the current colour transition started. `None` = no active transition.
    pub accent_transition_start: Option<Instant>,

    /// Linux: MPRIS D-Bus session registration and shared playback snapshot.
    #[cfg(target_os = "linux")]
    pub mpris: Option<crate::mpris::MprisLink>,
}

impl App {
    pub fn new(config: Config) -> Result<Self> {
        let subsonic =
            SubsonicClient::new(&config.subsonic_url, &config.subsonic_user, &config.subsonic_pass)?;
        let (library_tx, library_rx) = mpsc::channel(64);
        let (player_tx, player_rx, player_join, sample_buffer) = spawn_player();
        // Apply configured default volume immediately.
        let _ = player_tx.send(PlayerCommand::SetVolume(config.default_volume as f32 / 100.0));
        let keybinds = Keybinds::from_section(&config.keybinds);
        let theme    = Theme::from_section(&config.theme);
        let static_accent = theme.accent;
        let lyrics_visible = config.lyrics_visible;
        let track_cache = crate::cache::TrackCache::load(config.cache_enabled, config.cache_max_size_gb);
        let index_path = config.resolved_library_index_path();
        let (library_index_tracks, library_index_refreshed_at) =
            match crate::library_index::load(&index_path) {
                Some(f) => (f.tracks, Some(f.refreshed_at_unix)),
                None => (Vec::new(), None),
            };
        let library_index_by_id = crate::library_index::index_by_id(&library_index_tracks);
        Ok(Self {
            active_tab: Tab::Home,
            browser_focus: BrowserColumn::Artists,
            library: LibraryState::default(),
            queue: QueueState::default(),
            playback: PlaybackState::default(),
            subsonic: Arc::new(subsonic),
            library_rx,
            library_tx,
            player_tx,
            player_rx,
            player_join: Some(player_join),
            config,
            should_quit: false,
            search_mode: SearchMode::default(),
            search_filter: None,
            kitty_supported: false,
            in_tmux: false,
            tmux_status_offset: 0,
            cell_px: None,
            art_cache: None,
            home_art_cache: HashMap::new(),
            home_art_loading: HashSet::new(),
            keybinds,
            theme,
            play_gen: 0,
            library_index_tracks,
            library_index_by_id,
            library_index_refreshed_at,
            library_index_refreshing: false,
            library_index_refresh_started: None,
            cache: track_cache,
            prefetch_gen: Arc::new(AtomicU64::new(0)),
            help_visible: false,
            home_art_needs_redraw: false,
            home_art_last_tmux_render: None,
            home: HomeState::default(),
            pending_artist_select: None,
            playlist_overlay: PlaylistOverlay::default(),
            playlist_picker: None,
            status_flash: None,
            history: crate::history::PlayHistory::default(),
            play_recorded: false,
            lyrics_visible,
            lyrics_cache: None,
            lyrics_scroll: 0,
            lyrics_loading: false,
            sample_buffer,
            spectrum_bands: vec![0.0; 32],
            visualizer_visible: false,
            fft_planner: rustfft::FftPlanner::new(),
            dynamic_accent: None,
            accent_current: static_accent,
            accent_lerp_from: static_accent,
            accent_target: static_accent,
            accent_transition_start: None,
            #[cfg(target_os = "linux")]
            mpris: None,
        })
    }

    // ── Accent colour helpers ─────────────────────────────────────────────────

    /// The accent colour to use at render time.
    /// Returns `accent_current` (the OKLab-interpolated value) when dynamic
    /// mode is on, otherwise the static configured accent.
    pub fn accent(&self) -> Color {
        // Pass `accent_current` as the dynamic value — `effective_accent`
        // uses it when `theme.dynamic` is true, else falls back to static accent.
        self.theme.effective_accent(if self.theme.dynamic {
            Some(self.accent_current)
        } else {
            None
        })
    }

    /// Read from the sample buffer and compute FFT bands if the visualizer is on.
    /// Call once per render tick before `terminal.draw()`.
    pub fn tick_visualizer(&mut self) {
        if !self.visualizer_visible {
            return;
        }
        let samples = match self.sample_buffer.lock() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        let new_bands = crate::visualizer::compute_bands(
            &samples,
            &mut self.fft_planner,
            &self.spectrum_bands,
            32,
        );
        self.spectrum_bands = new_bands;
    }

    /// Returns true while a colour transition is in progress.
    pub fn accent_transition_active(&self) -> bool {
        self.accent_transition_start.is_some()
    }

    /// Call once per render tick to advance the colour interpolation.
    pub fn tick_accent_transition(&mut self) {
        if let Some(start) = self.accent_transition_start {
            let t = start.elapsed().as_secs_f32() / 0.4;
            if t >= 1.0 {
                self.accent_current = self.accent_target;
                self.accent_transition_start = None;
            } else {
                self.accent_current = lerp_color(self.accent_lerp_from, self.accent_target, t);
            }
        }
    }

    /// Set the dynamic accent, kicking off a transition if dynamic mode is on.
    fn apply_dynamic_accent(&mut self, color: Option<Color>) {
        self.dynamic_accent = color;
        if self.theme.dynamic {
            let target = color.unwrap_or(self.theme.accent);
            if target != self.accent_target {
                self.accent_lerp_from = self.accent_current;
                self.accent_target = target;
                self.accent_transition_start = Some(Instant::now());
            }
        }
    }

    // ── Home tab data refresh ─────────────────────────────────────────────────

    /// Populate `self.home` from play history.  Called on every entry to the
    /// Home tab (GoToHome, SwitchTab landing, SwitchTabReverse landing).
    pub fn refresh_home_data(&mut self) {
        // Recent albums: up to 20 unique albums for the art strip.
        self.home.recent_albums = self.history.recent_albums(20)
            .into_iter()
            .map(|(album_id, album_name, artist_name)| RecentAlbum {
                album_id,
                album_name,
                artist_name,
            })
            .collect();
        self.home.album_scroll_offset = 0;
        self.home.album_selected_index = 0;

        // Recent tracks: last 15 from history (most recent first).
        let total = self.history.records.len();
        let start = total.saturating_sub(15);
        self.home.recent_tracks = self.history.records[start..].iter().cloned().rev().collect();

        // Top 15 artists by play count.
        self.home.top_artists = self.history.top_artists(15);

        // Rediscover: build library artist pairs from loaded state.
        let library_artist_pairs: Vec<(String, String)> =
            if let LoadingState::Loaded(artists) = &self.library.artists {
                artists.iter().map(|a| (a.id.clone(), a.name.clone())).collect()
            } else {
                Vec::new()
            };
        self.home.rediscover = self.history.rediscover_artists(15, &library_artist_pairs);

        // Reset cursor / section.
        self.home.selected_index = 0;
        self.home.active_section = HomeSection::RecentAlbums;

        // Kick off art fetches for any album not yet cached.
        // Limit to 3 concurrent in-flight fetches to avoid overwhelming the connection.
        self.spawn_pending_home_art_fetches();
    }

    /// Spawn home art fetch tasks for albums not yet cached or loading,
    /// up to a maximum of 3 concurrent in-flight fetches.
    fn spawn_pending_home_art_fetches(&mut self) {
        const MAX_CONCURRENT: usize = 3;
        let album_ids: Vec<String> = self.home.recent_albums
            .iter()
            .map(|a| a.album_id.clone())
            .collect();
        for album_id in album_ids {
            if self.home_art_loading.len() >= MAX_CONCURRENT {
                break;
            }
            if self.home_art_cache.contains_key(&album_id) {
                continue;
            }
            if self.home_art_loading.contains(&album_id) {
                continue;
            }
            self.home_art_loading.insert(album_id.clone());
            let client = self.subsonic.clone();
            let tx = self.library_tx.clone();
            tokio::spawn(async move {
                match client.get_cover_art(&album_id).await {
                    Ok(bytes) => {
                        let _ = tx.send(LibraryUpdate::HomeArt { album_id, bytes }).await;
                    }
                    Err(e) => eprintln!("home art fetch({album_id}): {e}"),
                }
            });
        }
    }

    // ── Background fetch helpers ──────────────────────────────────────────────

    /// Spawn a task to fetch the artist list.
    pub fn fetch_artists(&self) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            let result = playterm_subsonic::fetch_library(&client)
                .await
                .map(|lib| lib.artists)
                .map_err(|e| e.to_string());
            let _ = tx.send(LibraryUpdate::Artists(result)).await;
        });
    }

    /// Walk the full library (`getArtists` + `getAlbum` per album) and refresh the
    /// on-disk metadata index used by the fzf picker.
    pub fn spawn_library_index_refresh(&mut self, force: bool) {
        if !self.config.library_index_enabled {
            if force {
                self.flash_status("Library index is disabled in config");
            }
            return;
        }
        if self.library_index_refreshing {
            self.flash_status_secs("Library index refresh already in progress", 8);
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if !force {
            if !self.library_index_tracks.is_empty() {
                if let Some(refreshed) = self.library_index_refreshed_at {
                    let stale = self.config.library_index_max_age_secs == 0
                        || now.saturating_sub(refreshed) > self.config.library_index_max_age_secs;
                    if !stale {
                        return;
                    }
                }
            }
        }
        self.library_index_refreshing = true;
        self.library_index_refresh_started = Some(Instant::now());
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        let index_path = self.config.resolved_library_index_path();
        let nav_skip = self.config.library_navidrome_skip_unchanged_scan;
        let album_p = self.config.library_fetch_album_parallelism;
        let artist_p = self.config.library_fetch_artist_parallelism;
        tokio::spawn(async move {
            let result: Result<(Vec<playterm_subsonic::Song>, Option<String>, bool), String> = async {
                if nav_skip && !force {
                    if let Some(file) = crate::library_index::load(&index_path) {
                        if let Some(ref tok) = file.navidrome_last_scan {
                            if let Ok(status) = client.get_scan_status().await {
                                if !status.scanning {
                                    if status.last_scan.as_deref() == Some(tok.as_str()) {
                                        return Ok((file.tracks.clone(), Some(tok.clone()), false));
                                    }
                                }
                            }
                        }
                    }
                }
                let opts = playterm_subsonic::FetchLibraryOptions {
                    album_parallelism: album_p,
                    artist_parallelism: artist_p,
                };
                let tracks = playterm_subsonic::fetch_all_library_songs_with_options(&client, opts)
                    .await
                    .map_err(|e| e.to_string())?;
                let scan_tok = if nav_skip {
                    client
                        .get_scan_status()
                        .await
                        .ok()
                        .and_then(|s| s.last_scan)
                } else {
                    None
                };
                Ok((tracks, scan_tok, force))
            }
            .await;
            let _ = tx.send(LibraryUpdate::LibraryIndexRefreshComplete { result }).await;
        });
    }

    pub fn flash_status(&mut self, msg: impl Into<String>) {
        self.flash_status_secs(msg, 3);
    }

    pub fn flash_status_secs(&mut self, msg: impl Into<String>, secs: u64) {
        self.status_flash = Some((msg.into(), Instant::now() + Duration::from_secs(secs)));
    }

    /// Apply fzf picker selection: append to the queue, or replace the queue when
    /// `replace` is true (clears queue and stops playback first).
    pub fn apply_library_index_picks(&mut self, ids: &[String], replace: bool) -> usize {
        if ids.is_empty() {
            return 0;
        }
        if replace {
            self.queue.songs.clear();
            self.queue.cursor = 0;
            self.queue.scroll = 0;
            self.queue.pre_shuffle_order = None;
            let _ = self.player_tx.send(PlayerCommand::Stop);
            self.playback.current_song = None;
            self.playback.elapsed = std::time::Duration::ZERO;
            self.playback.paused = false;
            self.playback.player_loaded = false;
        }
        let was_empty = self.queue.songs.is_empty();
        let mut n = 0usize;
        for id in ids {
            if let Some(song) = self.library_index_by_id.get(id.as_str()).cloned() {
                self.queue.push(song);
                n += 1;
            }
        }
        if n == 0 {
            let msg = if ids.len() == 1 {
                "Selected track not found in index"
            } else {
                "Selected tracks not found in index"
            };
            self.flash_status(msg);
            return 0;
        }
        if was_empty && !self.queue.songs.is_empty() {
            self.queue.cursor = 0;
            self.play_current();
        }
        if replace {
            self.flash_status_secs(
                if n == 1 {
                    "Replaced queue (1 track)".into()
                } else {
                    format!("Replaced queue ({n} tracks)")
                },
                3,
            );
        } else if n > 1 {
            self.flash_status_secs(format!("Added {n} tracks to queue"), 3);
        }
        n
    }

    /// Spawn a task to fetch albums for the given artist.
    pub fn fetch_albums(&self, artist_id: String) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            let result = client
                .get_artist(&artist_id)
                .await
                .map(|a| a.album)
                .map_err(|e| e.to_string());
            let _ = tx.send(LibraryUpdate::Albums { artist_id, result }).await;
        });
    }

    /// Spawn a task to fetch the track list for the given album.
    pub fn fetch_tracks(&self, album_id: String) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            let result = client
                .get_album(&album_id)
                .await
                .map(|a| a.song)
                .map_err(|e| e.to_string());
            let _ = tx.send(LibraryUpdate::Tracks { album_id, result }).await;
        });
    }

    /// Spawn a task to fetch raw cover art bytes for the given cover art ID.
    pub fn fetch_cover_art(&self, cover_id: String) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.get_cover_art(&cover_id).await {
                Ok(bytes) => {
                    let _ = tx.send(LibraryUpdate::CoverArt { cover_id, bytes }).await;
                }
                Err(e) => eprintln!("fetch_cover_art({cover_id}): {e}"),
            }
        });
    }

    /// Spawn a task to fetch lyrics for a song from LRCLib.
    ///
    /// Soft-fails silently — on any error an empty `lines` vec is delivered so
    /// the UI shows "No lyrics available" rather than a loading spinner forever.
    pub fn fetch_lyrics(
        &mut self,
        song_id: String,
        artist: String,
        title: String,
        album: String,
    ) {
        self.lyrics_loading = true;
        self.lyrics_scroll = 0;
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            let lines = crate::lyrics::fetch_lyrics(&artist, &title, &album).await;
            let _ = tx.send(LibraryUpdate::Lyrics { song_id, lines }).await;
        });
    }

    /// Return `true` when lyrics are plain-text (no timestamps) and should scroll
    /// with j/k. Empty lines count as *not* unsynced so j/k still moves the queue
    /// when the lyrics pane is open but has nothing to scroll.
    pub fn lyrics_are_unsynced(&self) -> bool {
        self.lyrics_cache
            .as_ref()
            .map(|(_, lines)| {
                !lines.is_empty() && lines.iter().all(|l| l.time.is_none())
            })
            .unwrap_or(false)
    }

    /// Spawn a task to fetch all tracks for an album, then replace the queue
    /// and start playing (or just append, depending on `replace`).
    pub fn fetch_and_replace_queue_with_album(&self, album_id: String, _start_playing: bool) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.get_album(&album_id).await {
                Ok(album) => {
                    let mut songs = album.song;
                    songs.sort_by_key(|s| (s.disc_number.unwrap_or(1), s.track.unwrap_or(0)));
                    let _ = tx
                        .send(LibraryUpdate::AllTracksForArtist { songs, start_playing: true })
                        .await;
                }
                Err(e) => eprintln!("fetch_and_replace_queue_with_album({album_id}): {e}"),
            }
        });
    }

    /// Spawn a task to fetch all tracks for an album and append them to the queue.
    pub fn fetch_and_append_album_to_queue(&self, album_id: String) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.get_album(&album_id).await {
                Ok(album) => {
                    let mut songs = album.song;
                    songs.sort_by_key(|s| (s.disc_number.unwrap_or(1), s.track.unwrap_or(0)));
                    let _ = tx
                        .send(LibraryUpdate::AllTracksForArtist { songs, start_playing: false })
                        .await;
                }
                Err(e) => eprintln!("fetch_and_append_album_to_queue({album_id}): {e}"),
            }
        });
    }

    /// Spawn a task that fetches every album + every track for the given artist,
    /// then delivers them as a flat sorted `AllTracksForArtist` update.
    pub fn fetch_all_tracks_for_artist(&self, artist_id: String, start_playing: bool) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            let artist = match client.get_artist(&artist_id).await {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("fetch_all_tracks_for_artist({}): {e}", artist_id);
                    return;
                }
            };
            let mut songs = Vec::new();
            for album in &artist.album {
                match client.get_album(&album.id).await {
                    Ok(a) => songs.extend(a.song),
                    Err(e) => eprintln!("get_album({}): {e}", album.id),
                }
            }
            songs.sort_by_key(|s| (s.disc_number.unwrap_or(1), s.track.unwrap_or(0)));
            let _ = tx
                .send(LibraryUpdate::AllTracksForArtist { songs, start_playing })
                .await;
        });
    }

    // ── Library update ingestion ──────────────────────────────────────────────

    pub fn apply_library_update(&mut self, update: LibraryUpdate) {
        match update {
            LibraryUpdate::Artists(result) => {
                self.library.artists = match result {
                    Ok(artists) => {
                        if !artists.is_empty() {
                            // Default to 0 on fresh start; restore keeps whatever was saved.
                            if self.library.selected_artist.is_none() {
                                self.library.selected_artist = Some(0);
                            }
                            // Clamp restored index to actual list size.
                            let idx = self.library.selected_artist.unwrap()
                                .min(artists.len() - 1);
                            self.library.selected_artist = Some(idx);
                            // Fetch albums for the selected (or restored) artist.
                            let artist_id = artists[idx].id.clone();
                            if !self.library.albums.contains_key(&artist_id) {
                                self.library.albums.insert(artist_id.clone(), LoadingState::Loading);
                                self.fetch_albums(artist_id);
                            }
                        }
                        LoadingState::Loaded(artists)
                    }
                    Err(e) => LoadingState::Error(e),
                };
            }
            LibraryUpdate::Albums { artist_id, result } => {
                // Is this update for the currently-selected artist?
                let is_selected_artist = self.library.selected_artist
                    .and_then(|idx| {
                        if let LoadingState::Loaded(artists) = &self.library.artists {
                            artists.get(idx).map(|a| a.id == artist_id)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(false);

                self.library.albums.insert(
                    artist_id,
                    match result {
                        Ok(albums) if !albums.is_empty() => {
                            if is_selected_artist {
                                // Use restored selection (default 0), clamped to list size.
                                if self.library.selected_album.is_none() {
                                    self.library.selected_album = Some(0);
                                }
                                let idx = self.library.selected_album.unwrap()
                                    .min(albums.len() - 1);
                                self.library.selected_album = Some(idx);
                                let album_id = albums[idx].id.clone();
                                if !self.library.tracks.contains_key(&album_id) {
                                    self.library.tracks.insert(album_id.clone(), LoadingState::Loading);
                                    self.fetch_tracks(album_id);
                                }
                            } else {
                                // Background prefetch: fetch tracks for first album.
                                if self.library.selected_album.is_none() {
                                    self.library.selected_album = Some(0);
                                }
                                let first_id = albums[0].id.clone();
                                if !self.library.tracks.contains_key(&first_id) {
                                    self.library.tracks.insert(first_id.clone(), LoadingState::Loading);
                                    self.fetch_tracks(first_id);
                                }
                            }
                            LoadingState::Loaded(albums)
                        }
                        Ok(albums) => LoadingState::Loaded(albums),
                        Err(e) => LoadingState::Error(e),
                    },
                );
            }
            LibraryUpdate::Tracks { album_id, result } => {
                // Is this update for the currently-selected album?
                let is_current_album = self.library.current_album()
                    .map(|a| a.id == album_id)
                    .unwrap_or(false);
                let loaded = match result {
                    Ok(songs) => {
                        if is_current_album && !songs.is_empty() {
                            // Clamp restored index (or default to 0) to actual song count.
                            if self.library.selected_track.is_none() {
                                self.library.selected_track = Some(0);
                            }
                            let idx = self.library.selected_track.unwrap()
                                .min(songs.len() - 1);
                            self.library.selected_track = Some(idx);
                        } else if self.library.selected_track.is_none() {
                            self.library.selected_track = Some(0);
                        }
                        LoadingState::Loaded(songs)
                    }
                    Err(e) => LoadingState::Error(e),
                };
                self.library.tracks.insert(album_id, loaded);
            }
            LibraryUpdate::AllTracksForArtist { mut songs, start_playing } => {
                let was_empty = self.queue.songs.is_empty();
                songs.sort_by_key(|s| (
                    s.album_id.clone().unwrap_or_default(),
                    s.disc_number.unwrap_or(1),
                    s.track.unwrap_or(0),
                ));
                for song in songs {
                    self.queue.push(song);
                }
                if start_playing && was_empty && !self.queue.songs.is_empty() {
                    self.queue.cursor = 0;
                    self.queue.scroll = 0;
                    self.play_current();
                }
            }
            LibraryUpdate::CoverArt { cover_id, bytes } => {
                // Extract dynamic accent before storing (bytes are consumed here).
                let accent = extract_accent(&bytes);
                self.art_cache = Some((cover_id, bytes));
                self.apply_dynamic_accent(accent);
                #[cfg(target_os = "linux")]
                self.mpris_emit_props();
            }
            LibraryUpdate::Lyrics { song_id, lines } => {
                self.lyrics_loading = false;
                self.lyrics_cache = Some((song_id, lines));
                self.lyrics_scroll = 0;
            }
            LibraryUpdate::CacheTrack { song_id, album_id, bytes } => {
                let _ = self.cache.put(&song_id, &album_id, &bytes);
            }
            LibraryUpdate::HomeArt { album_id, bytes } => {
                self.home_art_loading.remove(&album_id);
                self.home_art_cache.insert(album_id, bytes);
                // A fetch slot opened up — check if more albums need fetching.
                self.spawn_pending_home_art_fetches();
                // Signal main loop to re-render the art strip now that a new
                // thumbnail is available in the cache.
                if self.active_tab == Tab::Home {
                    if self.in_tmux {
                        // In tmux mode, re-transmitting the full strip for every
                        // individual image arrival causes visible blinking.  Only
                        // trigger a render when all pending fetches are done, or
                        // when 500 ms have passed since the last render (fallback
                        // so partially-loaded strips still appear promptly).
                        let elapsed = self.home_art_last_tmux_render
                            .map(|t| t.elapsed().as_millis())
                            .unwrap_or(u128::MAX);
                        if self.home_art_loading.is_empty() || elapsed >= 500 {
                            self.home_art_needs_redraw = true;
                        }
                    } else {
                        self.home_art_needs_redraw = true;
                    }
                }
            }
            LibraryUpdate::Playlists(playlists) => {
                self.playlist_overlay.playlists = crate::state::LoadingState::Loaded(playlists);
                self.playlist_overlay.selected_playlist_index = 0;
            }
            LibraryUpdate::PlaylistTracks { playlist_id, songs } => {
                // Ignore stale results if the user navigated to a different playlist
                // while this fetch was in flight.
                if self.playlist_overlay.loaded_playlist_id.as_deref() == Some(&playlist_id) {
                    self.playlist_overlay.tracks = LoadingState::Loaded(songs);
                    self.playlist_overlay.selected_track_index = 0;
                }
            }
            LibraryUpdate::PlaylistCreated(p) => {
                // Append new playlist and select it.
                match &mut self.playlist_overlay.playlists {
                    LoadingState::Loaded(ref mut list) => {
                        self.playlist_overlay.selected_playlist_index = list.len();
                        list.push(p);
                    }
                    _ => {
                        self.playlist_overlay.playlists = LoadingState::Loaded(vec![p]);
                        self.playlist_overlay.selected_playlist_index = 0;
                    }
                }
            }
            LibraryUpdate::PlaylistDeleted(id) => {
                if let LoadingState::Loaded(ref mut list) = self.playlist_overlay.playlists {
                    list.retain(|p| p.id != id);
                    let max = list.len().saturating_sub(1);
                    self.playlist_overlay.selected_playlist_index =
                        self.playlist_overlay.selected_playlist_index.min(max);
                }
                if self.playlist_overlay.loaded_playlist_id.as_deref() == Some(&id) {
                    self.playlist_overlay.tracks = LoadingState::NotLoaded;
                    self.playlist_overlay.loaded_playlist_id = None;
                }
            }
            LibraryUpdate::PlaylistRenamed { id, new_name } => {
                if let LoadingState::Loaded(ref mut list) = self.playlist_overlay.playlists {
                    if let Some(p) = list.iter_mut().find(|p| p.id == id) {
                        p.name = new_name;
                    }
                }
            }
            LibraryUpdate::PlaylistTrackAdded { playlist_name, .. } => {
                self.status_flash = Some((
                    format!("Added to {}", playlist_name),
                    Instant::now() + Duration::from_secs(2),
                ));
            }
            LibraryUpdate::PlaylistTrackRemoved { _playlist_id: _, index } => {
                if let LoadingState::Loaded(ref mut songs) = self.playlist_overlay.tracks {
                    if index < songs.len() {
                        songs.remove(index);
                    }
                    let max = songs.len().saturating_sub(1);
                    self.playlist_overlay.selected_track_index =
                        self.playlist_overlay.selected_track_index.min(max);
                }
            }
            LibraryUpdate::PlaylistsForPicker(playlists) => {
                self.playlist_overlay.playlists = LoadingState::Loaded(playlists.clone());
                if let Some(ref mut picker) = self.playlist_picker {
                    picker.playlists = playlists;
                    picker.loading = false;
                }
            }
            LibraryUpdate::LibraryIndexRefreshComplete { result } => {
                self.library_index_refreshing = false;
                self.library_index_refresh_started = None;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                match result {
                    Ok((tracks, navidrome_last_scan, forced)) => {
                        let path = self.config.resolved_library_index_path();
                        let scan_save = navidrome_last_scan.as_deref();
                        if let Err(e) =
                            crate::library_index::save(&path, &tracks, now, scan_save)
                        {
                            eprintln!("library index save: {e}");
                        }
                        self.library_index_refreshed_at = Some(now);
                        self.library_index_tracks = tracks;
                        self.library_index_by_id =
                            crate::library_index::index_by_id(&self.library_index_tracks);
                        let msg = if forced {
                            "Library index refresh complete"
                        } else {
                            "Library index updated"
                        };
                        self.status_flash = Some((
                            msg.into(),
                            Instant::now() + Duration::from_secs(2),
                        ));
                        if forced && self.config.library_notify_on_forced_index_refresh {
                            crate::desktop_notify::spawn_forced_library_index_complete();
                        }
                    }
                    Err(e) => {
                        eprintln!("library index refresh: {e}");
                        self.status_flash = Some((
                            format!("Library index refresh failed: {e}"),
                            Instant::now() + Duration::from_secs(5),
                        ));
                    }
                }
            }
        }
    }

    // ── Player event ingestion ────────────────────────────────────────────────

    pub fn handle_player_event(&mut self, event: PlayerEvent) {
        let progress_only = matches!(&event, PlayerEvent::Progress { .. });
        match event {
            PlayerEvent::TrackStarted => {
                self.play_recorded = false;
                self.playback.paused = false;
                if let Some(song) = self.queue.current().cloned() {
                    // Fetch cover art when the track has one and it differs from cache.
                    let cover_id = song.cover_art.clone();
                    if let Some(ref cid) = cover_id {
                        let needs_fetch = self.art_cache
                            .as_ref()
                            .map(|(cached_id, _)| cached_id != cid)
                            .unwrap_or(true);
                        if needs_fetch {
                            // Art will arrive via CoverArt library update — accent
                            // is applied there.  Clear stale dynamic accent for now.
                            self.apply_dynamic_accent(None);
                            self.fetch_cover_art(cid.clone());
                        } else if let Some((_, ref bytes)) = self.art_cache.clone() {
                            // Art already cached for this cover_id — extract immediately.
                            let accent = extract_accent(bytes);
                            self.apply_dynamic_accent(accent);
                        }
                    } else {
                        // Track has no cover art.
                        self.apply_dynamic_accent(None);
                    }
                    // Fetch lyrics if not already cached for this song.
                    let cached_for_song = self.lyrics_cache
                        .as_ref()
                        .map(|(id, _)| id == &song.id)
                        .unwrap_or(false);
                    if !cached_for_song {
                        self.fetch_lyrics(
                            song.id.clone(),
                            song.artist.clone().unwrap_or_default(),
                            song.title.clone(),
                            song.album.clone().unwrap_or_default(),
                        );
                    }
                    // Background-cache current track + prefetch next 2.
                    if self.config.cache_enabled {
                        // Collect (song_id, album_id) pairs to download, then spawn.
                        // We read from queue and cache separately to satisfy the borrow checker.
                        let mut to_download: Vec<(String, String)> = Vec::new();
                        // Current track.
                        if !self.cache.get_const(&song.id) {
                            to_download.push((song.id.clone(), song.album_id.clone().unwrap_or_default()));
                        }
                        // Next 2 tracks.
                        let cursor = self.queue.cursor;
                        for offset in 1..=2usize {
                            let idx = cursor + offset;
                            if idx < self.queue.songs.len() {
                                let s_id = self.queue.songs[idx].id.clone();
                                let a_id = self.queue.songs[idx].album_id.clone().unwrap_or_default();
                                if !self.cache.get_const(&s_id) {
                                    to_download.push((s_id, a_id));
                                }
                            }
                        }
                        for (s_id, a_id) in to_download {
                            self.spawn_cache_download(&s_id, &a_id);
                        }
                    }
                    self.playback.current_song = Some(song);
                }
            }
            PlayerEvent::Progress { elapsed, total } => {
                self.playback.elapsed = elapsed;
                self.playback.total = total;

                // Record play once threshold is crossed (50% of duration OR 30 s).
                if !self.play_recorded {
                    let threshold = if let Some(dur) = total {
                        let half = dur.as_secs_f64() * 0.5;
                        std::time::Duration::from_secs_f64(half.min(30.0))
                    } else {
                        std::time::Duration::from_secs(30)
                    };
                    if elapsed >= threshold {
                        if let Some(song) = self.playback.current_song.as_ref() {
                            let record = crate::history::PlayRecord {
                                song_id: song.id.clone(),
                                album_id: song.album_id.clone().unwrap_or_default(),
                                artist_id: song.artist_id.clone().unwrap_or_default(),
                                artist_name: song.artist.clone().unwrap_or_default(),
                                album_name: song.album.clone().unwrap_or_default(),
                                track_name: song.title.clone(),
                                played_at: crate::history::PlayRecord::now_secs(),
                                duration_secs: song
                                    .duration
                                    .map(|d| d as u64)
                                    .unwrap_or(0),
                            };
                            self.history.record_play(record);
                        }
                        self.play_recorded = true;
                    }
                }
            }
            PlayerEvent::AboutToFinish => {
                // Pre-load the next track for gapless playback.
                if let Some(next) = self.queue.peek_next().cloned() {
                    let duration =
                        next.duration.map(|s| std::time::Duration::from_secs(u64::from(s)));
                    match self.resolve_playback(&next) {
                        ResolvedPlayback::Cached(path) => {
                            let _ = self
                                .player_tx
                                .send(PlayerCommand::EnqueueNextCached { path, duration });
                        }
                        ResolvedPlayback::Url(url) => {
                            let _ = self
                                .player_tx
                                .send(PlayerCommand::EnqueueNext { url, duration });
                        }
                    }
                }
            }
            PlayerEvent::TrackAdvanced => {
                // The gapless transition happened — advance the queue cursor.
                self.queue.next();
                self.playback.paused = false;
                self.playback.elapsed = std::time::Duration::ZERO;
                if let Some(song) = self.queue.current().cloned() {
                    let cover_id = song.cover_art.clone();
                    if let Some(ref cid) = cover_id {
                        let needs_fetch = self.art_cache
                            .as_ref()
                            .map(|(cached_id, _)| cached_id != cid)
                            .unwrap_or(true);
                        if needs_fetch {
                            self.apply_dynamic_accent(None);
                            self.fetch_cover_art(cid.clone());
                        } else if let Some((_, ref bytes)) = self.art_cache.clone() {
                            let accent = extract_accent(bytes);
                            self.apply_dynamic_accent(accent);
                        }
                    } else {
                        self.apply_dynamic_accent(None);
                    }
                    // Fetch lyrics if not already cached for this song.
                    let cached_for_song = self.lyrics_cache
                        .as_ref()
                        .map(|(id, _)| id == &song.id)
                        .unwrap_or(false);
                    if !cached_for_song {
                        self.fetch_lyrics(
                            song.id.clone(),
                            song.artist.clone().unwrap_or_default(),
                            song.title.clone(),
                            song.album.clone().unwrap_or_default(),
                        );
                    }
                    self.playback.current_song = Some(song);
                }
            }
            PlayerEvent::TrackEnded => {
                if self.queue.next() {
                    self.play_current();
                } else if !self.queue.songs.is_empty() {
                    // End of queue — loop back to the first track.
                    self.queue.cursor = 0;
                    self.queue.scroll = 0;
                    self.play_current();
                } else {
                    self.playback.current_song = None;
                    self.playback.elapsed = std::time::Duration::ZERO;
                }
            }
            PlayerEvent::Error(e) => {
                // Never eprintln here — stderr draws over the alternate-screen TUI.
                self.playback.player_loaded = false;
                self.status_flash = Some((
                    humanize_playback_error(&e),
                    Instant::now() + Duration::from_secs(10),
                ));
            }
        }
        #[cfg(target_os = "linux")]
        {
            if self.mpris.is_some() {
                if progress_only {
                    self.mpris_touch_snapshot_only();
                    // Spec: `Position` must not emit PropertiesChanged on tick. Many shells
                    // and widgets never poll `Get(Position)` and only resync on `Seeked` or
                    // `PlaybackStatus` — emit `Seeked` on each progress update (~500 ms) so
                    // the displayed time advances while playing.
                    if let Some(link) = &self.mpris {
                        link.notify_seeked(self.playback.elapsed);
                    }
                } else {
                    self.mpris_emit_props();
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn mpris_touch_snapshot_only(&mut self) {
        if let Some(link) = &self.mpris {
            crate::mpris::write_snapshot(self, &link.snapshot);
        }
    }

    #[cfg(target_os = "linux")]
    fn mpris_emit_props(&mut self) {
        if let Some(link) = &self.mpris {
            crate::mpris::write_snapshot(self, &link.snapshot);
            link.notify_refresh();
        }
    }

    /// Push current playback state to MPRIS (call after registering the link).
    #[cfg(target_os = "linux")]
    pub fn mpris_sync_now(&mut self) {
        self.mpris_emit_props();
    }

    #[cfg(target_os = "linux")]
    fn mpris_emit_seek(&mut self, pos: std::time::Duration) {
        if let Some(link) = &self.mpris {
            crate::mpris::write_snapshot(self, &link.snapshot);
            link.notify_seeked(pos);
            link.notify_refresh();
        }
    }

    #[cfg(target_os = "linux")]
    fn mpris_after_action(&mut self, action: &Action) {
        use crate::action::Action::*;
        // Search dispatches per keystroke — skip D-Bus work for those.
        if matches!(
            action,
            SearchStart | SearchInput(_) | SearchBackspace | SearchConfirm | SearchCancel
        ) {
            return;
        }
        let Some(link) = &self.mpris else {
            return;
        };
        crate::mpris::write_snapshot(self, &link.snapshot);
        match action {
            SeekForward | SeekBackward | SeekTo(_) => {
                link.notify_seeked(self.playback.elapsed);
                link.notify_refresh();
            }
            _ => {
                link.notify_refresh();
            }
        }
    }

    /// Handle D-Bus MPRIS remote control (Linux).
    #[cfg(target_os = "linux")]
    pub fn handle_mpris_control(&mut self, c: crate::mpris::MprisControl) {
        use crate::mpris::MprisControl::*;
        match c {
            PlayPause => {
                self.dispatch(Action::PlayPause);
                return;
            }
            Next => {
                self.dispatch(Action::NextTrack);
                return;
            }
            Previous => {
                self.dispatch(Action::PrevTrack);
                return;
            }
            Pause => {
                if self.playback.player_loaded && !self.playback.paused {
                    self.playback.paused = true;
                    let _ = self.player_tx.send(PlayerCommand::Pause);
                }
            }
            Play => {
                if !self.playback.player_loaded && self.queue.current().is_some() {
                    self.play_current();
                } else if self.playback.paused {
                    self.playback.paused = false;
                    let _ = self.player_tx.send(PlayerCommand::Resume);
                }
            }
            Stop => {
                let _ = self.player_tx.send(PlayerCommand::Stop);
                self.playback.player_loaded = false;
                self.playback.elapsed = std::time::Duration::ZERO;
                self.playback.paused = false;
            }
            SeekDelta(dt_micros) => {
                let cur = self.playback.elapsed.as_micros() as i128;
                let target = cur + i128::from(dt_micros);
                let clamped_low = target.max(0);
                let max_micros = self
                    .playback
                    .total
                    .map(|t| t.as_micros() as i128)
                    .unwrap_or(clamped_low);
                let final_micros = clamped_low.min(max_micros).max(0);
                let new_pos = std::time::Duration::from_micros(final_micros as u64);
                let _ = self.player_tx.send(PlayerCommand::Seek(new_pos));
                self.playback.elapsed = new_pos;
                self.mpris_emit_seek(new_pos);
                return;
            }
            SetPosition {
                track_path,
                position_micros,
            } => {
                let Some(song) = self.playback.current_song.as_ref() else {
                    return;
                };
                let expected = crate::mpris::dbus_track_path_for_song_id(&song.id);
                if track_path != expected {
                    return;
                }
                let mut new_pos = std::time::Duration::from_micros(position_micros.max(0) as u64);
                if let Some(total) = self.playback.total {
                    new_pos = new_pos.min(total);
                }
                let _ = self.player_tx.send(PlayerCommand::Seek(new_pos));
                self.playback.elapsed = new_pos;
                self.mpris_emit_seek(new_pos);
                return;
            }
            SetVolume(v) => {
                let pct = (v.clamp(0.0, 1.0) * 100.0).round() as u8;
                self.config.default_volume = pct;
                let _ = self
                    .player_tx
                    .send(PlayerCommand::SetVolume(self.config.default_volume as f32 / 100.0));
            }
            Quit => {
                self.dispatch(Action::Quit);
                return;
            }
        }
        self.mpris_emit_props();
    }

    #[cfg(not(target_os = "linux"))]
    pub fn handle_mpris_control(&mut self, _c: crate::mpris::MprisControl) {}

    /// Send a PlayUrl command for the song the queue cursor points at.
    fn play_current(&mut self) {
        if let Some(song) = self.queue.current().cloned() {
            self.play_gen += 1;
            // Advance the prefetch gen so stale background downloads are discarded.
            self.prefetch_gen.fetch_add(1, Ordering::Release);
            let duration = song.duration.map(|s| std::time::Duration::from_secs(u64::from(s)));
            let resolved = self.resolve_playback(&song);
            self.playback.current_song = Some(song);
            self.playback.player_loaded = true;
            let gen = self.play_gen;
            match resolved {
                ResolvedPlayback::Cached(path) => {
                    let _ = self.player_tx.send(PlayerCommand::PlayCached { path, duration, gen });
                }
                ResolvedPlayback::Url(url) => {
                    let _ = self.player_tx.send(PlayerCommand::PlayUrl { url, duration, gen });
                }
            }
        }
    }

    /// Resolve a Subsonic stream URL or a finished on-disk cache file for `song`.
    fn resolve_playback(&mut self, song: &playterm_subsonic::Song) -> ResolvedPlayback {
        if self.config.cache_enabled {
            if let Some(path) = self.cache.get(&song.id) {
                self.cache.touch(&song.id);
                return ResolvedPlayback::Cached(path);
            }
        }
        ResolvedPlayback::Url(self.subsonic.stream_url(&song.id, self.config.max_bit_rate))
    }

    /// Spawn a background task to download `song_id` for caching.
    ///
    /// Callers are responsible for checking `cache.get_const()` first.
    /// The task checks `prefetch_gen` after download and discards stale bytes
    /// (from rapid skips or queue changes since spawn time).
    fn spawn_cache_download(&self, song_id: &str, album_id: &str) {
        let url = self.subsonic.stream_url(song_id, self.config.max_bit_rate);
        let song_id  = song_id.to_string();
        let album_id = album_id.to_string();
        let tx       = self.library_tx.clone();
        let gen_arc  = self.prefetch_gen.clone();
        let expected = gen_arc.load(Ordering::Acquire);
        tokio::spawn(async move {
            if let Ok(resp) = reqwest::Client::new().get(&url).send().await {
                if let Ok(bytes) = resp.bytes().await {
                    if gen_arc.load(Ordering::Acquire) == expected {
                        let _ = tx.send(LibraryUpdate::CacheTrack {
                            song_id,
                            album_id,
                            bytes: bytes.to_vec(),
                        }).await;
                    }
                }
            }
        });
    }

    // ── Action dispatch ───────────────────────────────────────────────────────

    pub fn dispatch(&mut self, action: Action) {
        let mpris_action_hook = action.clone();
        match action {
            Action::ToggleHelp => {
                let was_visible = self.help_visible;
                self.help_visible = !self.help_visible;
                if self.kitty_supported && self.active_tab == Tab::Home {
                    if !was_visible {
                        // Opening popup on Home tab — clear art strip so it doesn't
                        // bleed through the popup overlay.
                        let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                    } else {
                        // Closing popup on Home tab — request art strip redraw on next frame.
                        self.home_art_needs_redraw = true;
                    }
                }
            }
            Action::LibraryIndexRefresh => self.spawn_library_index_refresh(true),
            Action::LibraryFzfPicker => {}
            Action::Quit => self.should_quit = true,
            Action::SwitchTab => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                if self.kitty_supported {
                    let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                    if self.active_tab == Tab::Home {
                        let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                    }
                }
                self.active_tab = self.active_tab.next();
                self.search_filter = None;
                if self.active_tab == Tab::Home {
                    self.refresh_home_data();
                    self.home_art_needs_redraw = true;
                }
            }
            Action::SwitchTabReverse => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                if self.kitty_supported {
                    let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                    if self.active_tab == Tab::Home {
                        let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                    }
                }
                self.active_tab = self.active_tab.prev();
                self.search_filter = None;
                if self.active_tab == Tab::Home {
                    self.refresh_home_data();
                    self.home_art_needs_redraw = true;
                }
            }
            Action::GoToHome => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                if self.kitty_supported { let _ = crate::ui::kitty_art::clear_image(self.in_tmux); }
                self.active_tab = Tab::Home;
                self.search_filter = None;
                self.refresh_home_data();
                self.home_art_needs_redraw = true;
            }
            Action::GoToBrowser => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                if self.kitty_supported {
                    let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                    if self.active_tab == Tab::Home {
                        let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                    }
                }
                self.active_tab = Tab::Browser;
                self.search_filter = None;
                self.apply_pending_artist_select();
            }
            Action::GoToNowPlaying => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                if self.kitty_supported {
                    let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                    if self.active_tab == Tab::Home {
                        let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                    }
                }
                self.active_tab = Tab::NowPlaying;
                self.search_filter = None;
            }
            Action::FocusLeft => self.handle_focus_left(),
            Action::FocusRight => self.handle_focus_right(),
            Action::Navigate(dir) => {
                // On NowPlaying tab with unsynced lyrics visible, j/k scroll
                // the lyrics pane instead of the queue.
                if self.active_tab == Tab::NowPlaying
                    && self.lyrics_visible
                    && self.lyrics_are_unsynced()
                {
                    match dir {
                        Direction::Up | Direction::Top => {
                            self.lyrics_scroll = self.lyrics_scroll.saturating_sub(1);
                        }
                        Direction::Down | Direction::Bottom => {
                            self.lyrics_scroll = self.lyrics_scroll.saturating_add(1);
                        }
                    }
                } else {
                    self.handle_navigate(dir);
                }
            }
            Action::Select => self.handle_select(),
            Action::Back => self.handle_focus_left(),
            Action::AddToQueue => self.handle_add_to_queue(),
            Action::AddAllToQueue => self.handle_add_all_to_queue(),
            Action::PlayPause => {
                if !self.playback.player_loaded && self.queue.current().is_some() {
                    // Restored queue: engine has no track yet — load and start playing.
                    self.play_current();
                } else if self.playback.paused {
                    self.playback.paused = false;
                    let _ = self.player_tx.send(PlayerCommand::Resume);
                } else {
                    self.playback.paused = true;
                    let _ = self.player_tx.send(PlayerCommand::Pause);
                }
            }
            Action::NextTrack => {
                if self.queue.next() {
                    self.play_current();
                }
            }
            Action::PrevTrack => {
                if self.queue.prev() {
                    self.play_current();
                }
            }
            Action::VolumeUp => {
                self.config.default_volume = self.config.default_volume.saturating_add(5).min(100);
                let _ = self.player_tx.send(PlayerCommand::SetVolume(self.config.default_volume as f32 / 100.0));
            }
            Action::VolumeDown => {
                self.config.default_volume = self.config.default_volume.saturating_sub(5);
                let _ = self.player_tx.send(PlayerCommand::SetVolume(self.config.default_volume as f32 / 100.0));
            }
            Action::ClearQueue => self.handle_clear_queue(),
            Action::Shuffle => self.handle_shuffle(),
            Action::Unshuffle => self.handle_unshuffle(),
            Action::SeekForward => {
                let new_pos = if let Some(total) = self.playback.total {
                    (self.playback.elapsed + std::time::Duration::from_secs(10)).min(total)
                } else {
                    self.playback.elapsed + std::time::Duration::from_secs(10)
                };
                let _ = self.player_tx.send(PlayerCommand::Seek(new_pos));
                self.playback.elapsed = new_pos;
            }
            Action::SeekBackward => {
                let new_pos = self.playback.elapsed.saturating_sub(std::time::Duration::from_secs(10));
                let _ = self.player_tx.send(PlayerCommand::Seek(new_pos));
                self.playback.elapsed = new_pos;
            }
            Action::SeekTo(pos) => {
                let new_pos = if let Some(total) = self.playback.total {
                    pos.min(total)
                } else {
                    pos
                };
                let _ = self.player_tx.send(PlayerCommand::Seek(new_pos));
                self.playback.elapsed = new_pos;
            }
            Action::SearchStart => {
                self.search_mode.active = true;
                self.search_mode.query.clear();
                self.search_mode.selected = 0;
                // Starting a new search clears the previous filter.
                self.search_filter = None;
            }
            Action::SearchInput(ch) => {
                if self.search_mode.active {
                    self.search_mode.query.push(ch);
                    self.search_mode.selected = 0;
                }
            }
            Action::SearchBackspace => {
                if self.search_mode.active {
                    self.search_mode.query.pop();
                    self.search_mode.selected = 0;
                }
            }
            Action::SearchConfirm => {
                if self.search_mode.active {
                    let q = self.search_mode.query.to_lowercase();
                    if q.is_empty() {
                        self.search_filter = None;
                    } else {
                        self.search_filter = Some(q);
                    }
                    self.handle_search_confirm();
                    self.search_mode.active = false;
                    self.search_mode.query.clear();
                }
            }
            Action::SearchCancel => {
                self.search_mode.active = false;
                self.search_mode.query.clear();
                self.search_mode.selected = 0;
                self.search_filter = None;
            }
            Action::ToggleLyrics => {
                // Only active on NowPlaying tab; silently ignored on Browser.
                if self.active_tab == Tab::NowPlaying {
                    self.lyrics_visible = !self.lyrics_visible;
                    // Trigger a lyrics fetch if we just enabled the overlay and
                    // nothing is cached for the current song yet.
                    if self.lyrics_visible {
                        if let Some(song) = self.playback.current_song.clone() {
                            let cached = self.lyrics_cache
                                .as_ref()
                                .map(|(id, _)| id == &song.id)
                                .unwrap_or(false);
                            if !cached {
                                self.fetch_lyrics(
                                    song.id.clone(),
                                    song.artist.clone().unwrap_or_default(),
                                    song.title.clone(),
                                    song.album.clone().unwrap_or_default(),
                                );
                            }
                        }
                    }
                }
            }
            Action::ToggleVisualizer => {
                if self.active_tab == Tab::NowPlaying {
                    self.visualizer_visible = !self.visualizer_visible;
                    if self.visualizer_visible {
                        self.lyrics_visible = false;
                    } else {
                        self.spectrum_bands = vec![0.0; 32];
                    }
                }
            }
            Action::HomeSectionNext => {
                if self.active_tab == Tab::Home {
                    self.home.active_section = self.home.active_section.next();
                    self.home.selected_index = 0;
                }
            }
            Action::HomeSectionPrev => {
                if self.active_tab == Tab::Home {
                    self.home.active_section = self.home.active_section.prev();
                    self.home.selected_index = 0;
                }
            }
            Action::HomeRefresh => {
                if self.active_tab == Tab::Home {
                    // Preserve the active section so the user stays in Rediscover
                    // after pressing r — the re-roll is visible immediately.
                    let saved_section = self.home.active_section;
                    self.refresh_home_data();
                    self.home.active_section = saved_section;
                    self.home_art_needs_redraw = true;
                }
            }
            Action::HomeAlbumLeft => {
                if self.active_tab == Tab::Home {
                    if self.home.active_section == HomeSection::RecentAlbums {
                        if self.home.album_selected_index > 0 {
                            self.home.album_selected_index -= 1;
                            if self.home.album_selected_index < self.home.album_scroll_offset {
                                self.home.album_scroll_offset = self.home.album_selected_index;
                            }
                            if !self.in_tmux {
                                self.home_art_needs_redraw = true;
                            }
                        }
                    } else {
                        // In bottom panes: h escapes to previous section.
                        self.home.active_section = self.home.active_section.prev();
                        self.home.selected_index = 0;
                    }
                }
            }
            Action::HomeAlbumRight => {
                if self.active_tab == Tab::Home {
                    if self.home.active_section == HomeSection::RecentAlbums {
                        let max_idx = self.home.recent_albums.len().saturating_sub(1);
                        if self.home.album_selected_index < max_idx {
                            self.home.album_selected_index += 1;
                            // visible_count: approximate from cell_px; assume 5 if unknown
                            let visible_count = compute_visible_count(self.cell_px, 5);
                            let scroll_end = self.home.album_scroll_offset + visible_count.saturating_sub(1);
                            if self.home.album_selected_index > scroll_end {
                                self.home.album_scroll_offset += 1;
                            }
                            if !self.in_tmux {
                                self.home_art_needs_redraw = true;
                            }
                        }
                    } else {
                        // In bottom panes: l escapes to next section.
                        self.home.active_section = self.home.active_section.next();
                        self.home.selected_index = 0;
                    }
                }
            }
            Action::HomeAlbumPlay => {
                if self.active_tab == Tab::Home {
                    let idx = self.home.album_selected_index;
                    if let Some(album) = self.home.recent_albums.get(idx) {
                        let album_id = album.album_id.clone();
                        // Clear queue first then fetch+play.
                        self.queue.songs.clear();
                        self.queue.cursor = 0;
                        self.queue.scroll = 0;
                        self.queue.pre_shuffle_order = None;
                        let _ = self.player_tx.send(PlayerCommand::Stop);
                        self.playback.current_song = None;
                        self.playback.elapsed = std::time::Duration::ZERO;
                        self.playback.paused = false;
                        self.playback.player_loaded = false;
                        self.fetch_and_replace_queue_with_album(album_id, true);
                    }
                }
            }
            Action::HomeAlbumAddToQueue => {
                if self.active_tab == Tab::Home {
                    let idx = self.home.album_selected_index;
                    if let Some(album) = self.home.recent_albums.get(idx) {
                        let album_id = album.album_id.clone();
                        self.fetch_and_append_album_to_queue(album_id);
                    }
                }
            }
            Action::ToggleDynamicTheme => {
                if self.theme.dynamic {
                    // Disable: instant snap back to static accent.
                    self.theme.dynamic = false;
                    self.accent_current = self.theme.accent;
                    self.accent_target  = self.theme.accent;
                    self.accent_transition_start = None;
                } else {
                    // Enable: start transition from current to dynamic accent (if any).
                    self.theme.dynamic = true;
                    let target = self.dynamic_accent.unwrap_or(self.theme.accent);
                    self.accent_lerp_from = self.accent_current;
                    self.accent_target    = target;
                    self.accent_transition_start = Some(Instant::now());
                }
            }
            Action::TogglePlaylistOverlay
            | Action::PlaylistScrollUp
            | Action::PlaylistScrollDown
            | Action::PlaylistFocusTracks
            | Action::PlaylistFocusList
            | Action::PlaylistPlayAll
            | Action::PlaylistAppendAll
            | Action::PlaylistPlayTrack
            | Action::PlaylistAppendTrack => {
                self.handle_playlist_action(action);
            }
            Action::PlaylistCreate
            | Action::PlaylistDelete
            | Action::PlaylistRename
            | Action::PlaylistRemoveTrack
            | Action::BrowserAddToPlaylist
            | Action::PlaylistPickerSelect
            | Action::PlaylistPickerCancel
            | Action::PlaylistPickerScrollUp
            | Action::PlaylistPickerScrollDown
            | Action::PlaylistInputConfirm
            | Action::PlaylistInputCancel
            | Action::PlaylistInputChar(_)
            | Action::PlaylistConfirmYes
            | Action::PlaylistConfirmNo => {
                self.handle_playlist_mutation(action);
            }
            Action::None => {}
        }
        #[cfg(target_os = "linux")]
        self.mpris_after_action(&mpris_action_hook);
    }

    // ── Pending artist pre-selection ──────────────────────────────────────────

    /// If `pending_artist_select` is set, find the artist by name in the loaded
    /// artist list, select it, and clear the pending value.
    /// Called whenever the Browser tab becomes active.
    pub fn apply_pending_artist_select(&mut self) {
        if let Some(name) = self.pending_artist_select.take() {
            if let LoadingState::Loaded(artists) = &self.library.artists {
                if let Some(idx) = artists.iter().position(|a| a.name == name) {
                    let artist_id = artists[idx].id.clone();
                    self.library.selected_artist = Some(idx);
                    self.library.selected_album = Some(0);
                    self.library.selected_track = Some(0);
                    self.browser_focus = BrowserColumn::Artists;
                    if !self.library.albums.contains_key(&artist_id) {
                        self.library.albums.insert(artist_id.clone(), LoadingState::Loading);
                        self.fetch_albums(artist_id);
                    }
                }
                // If artist not found, pending was taken (cleared) — switch Browser normally.
            }
            // If artists not yet loaded, pending was taken — no-op.
        }
    }

    // ── Focus movement ────────────────────────────────────────────────────────

    fn handle_focus_right(&mut self) {
        if self.active_tab != Tab::Browser {
            return;
        }
        self.search_filter = None;
        match self.browser_focus {
            BrowserColumn::Artists => {
                if let Some(artist) = self.library.current_artist() {
                    let artist_id = artist.id.clone();
                    if !self.library.albums.contains_key(&artist_id) {
                        self.library.albums.insert(artist_id.clone(), LoadingState::Loading);
                        self.fetch_albums(artist_id);
                    }
                }
                self.browser_focus = BrowserColumn::Albums;
            }
            BrowserColumn::Albums => {
                if let Some(album) = self.library.current_album() {
                    let album_id = album.id.clone();
                    if !self.library.tracks.contains_key(&album_id) {
                        self.library.tracks.insert(album_id.clone(), LoadingState::Loading);
                        self.fetch_tracks(album_id);
                    }
                }
                self.browser_focus = BrowserColumn::Tracks;
            }
            BrowserColumn::Tracks => {} // already rightmost
        }
    }

    fn handle_focus_left(&mut self) {
        if self.active_tab != Tab::Browser {
            return;
        }
        self.search_filter = None;
        self.browser_focus = self.browser_focus.left();
    }

    // ── Navigation ────────────────────────────────────────────────────────────

    fn handle_navigate(&mut self, dir: Direction) {
        match self.active_tab {
            Tab::Home       => self.handle_navigate_home(dir),
            Tab::Browser    => self.handle_navigate_browser(dir),
            Tab::NowPlaying => self.handle_navigate_queue(dir),
        }
    }

    fn handle_navigate_home(&mut self, dir: Direction) {
        if self.home.active_section == HomeSection::RecentAlbums {
            // j/k in the album strip: move to next/prev section.
            match dir {
                Direction::Down | Direction::Bottom => {
                    self.home.active_section = self.home.active_section.next();
                    self.home.selected_index = 0;
                }
                Direction::Up | Direction::Top => {
                    self.home.active_section = self.home.active_section.prev();
                    self.home.selected_index = 0;
                }
            }
            return;
        }
        let section_len = match self.home.active_section {
            HomeSection::RecentAlbums => 0,
            HomeSection::RecentTracks => self.home.recent_tracks.len(),
            HomeSection::TopArtists  => self.home.top_artists.len(),
            HomeSection::Rediscover  => self.home.rediscover.len(),
        };
        if section_len == 0 { return; }
        self.home.selected_index = match dir {
            Direction::Up | Direction::Top    => self.home.selected_index.saturating_sub(1),
            Direction::Down | Direction::Bottom => (self.home.selected_index + 1).min(section_len - 1),
        };
    }

    fn handle_navigate_browser(&mut self, dir: Direction) {
        match self.browser_focus {
            BrowserColumn::Artists => {
                let result = if let LoadingState::Loaded(artists) = &self.library.artists {
                    // Build navigable index set — filtered or full.
                    let indices: Vec<usize> = if let Some(q) = &self.search_filter {
                        artists.iter().enumerate()
                            .filter(|(_, a)| a.name.to_lowercase().contains(q.as_str()))
                            .map(|(i, _)| i)
                            .collect()
                    } else {
                        (0..artists.len()).collect()
                    };
                    if indices.is_empty() { return; }
                    let cur_pos = self.library.selected_artist
                        .and_then(|sel| indices.iter().position(|&i| i == sel))
                        .unwrap_or(0);
                    let new_pos = match dir {
                        Direction::Up => cur_pos.saturating_sub(1),
                        Direction::Down => (cur_pos + 1).min(indices.len() - 1),
                        Direction::Top => 0,
                        Direction::Bottom => indices.len() - 1,
                    };
                    let new_orig = indices[new_pos];
                    Some((new_orig, artists[new_orig].id.clone()))
                } else {
                    None
                };
                if let Some((new_idx, artist_id)) = result {
                    self.library.selected_artist = Some(new_idx);
                    self.library.selected_album = Some(0);
                    self.library.selected_track = Some(0);
                    if !self.library.albums.contains_key(&artist_id) {
                        self.library.albums.insert(artist_id.clone(), LoadingState::Loading);
                        self.fetch_albums(artist_id);
                    }
                }
            }
            BrowserColumn::Albums => {
                let result = {
                    let artist_id = match self.library.current_artist() {
                        Some(a) => a.id.clone(),
                        None => return,
                    };
                    if let Some(LoadingState::Loaded(albums)) = self.library.albums.get(&artist_id) {
                        let indices: Vec<usize> = if let Some(q) = &self.search_filter {
                            albums.iter().enumerate()
                                .filter(|(_, a)| a.name.to_lowercase().contains(q.as_str()))
                                .map(|(i, _)| i)
                                .collect()
                        } else {
                            (0..albums.len()).collect()
                        };
                        if indices.is_empty() { return; }
                        let cur_pos = self.library.selected_album
                            .and_then(|sel| indices.iter().position(|&i| i == sel))
                            .unwrap_or(0);
                        let new_pos = match dir {
                            Direction::Up => cur_pos.saturating_sub(1),
                            Direction::Down => (cur_pos + 1).min(indices.len() - 1),
                            Direction::Top => 0,
                            Direction::Bottom => indices.len() - 1,
                        };
                        let new_orig = indices[new_pos];
                        Some((new_orig, albums[new_orig].id.clone()))
                    } else {
                        None
                    }
                };
                if let Some((new_idx, album_id)) = result {
                    self.library.selected_album = Some(new_idx);
                    self.library.selected_track = Some(0);
                    if !self.library.tracks.contains_key(&album_id) {
                        self.library.tracks.insert(album_id.clone(), LoadingState::Loading);
                        self.fetch_tracks(album_id);
                    }
                }
            }
            BrowserColumn::Tracks => {
                let album_id = match self.library.current_album() {
                    Some(a) => a.id.clone(),
                    None => return,
                };
                if let Some(LoadingState::Loaded(songs)) = self.library.tracks.get(&album_id) {
                    let indices: Vec<usize> = if let Some(q) = &self.search_filter {
                        songs.iter().enumerate()
                            .filter(|(_, s)| s.title.to_lowercase().contains(q.as_str()))
                            .map(|(i, _)| i)
                            .collect()
                    } else {
                        (0..songs.len()).collect()
                    };
                    if indices.is_empty() { return; }
                    let cur_pos = self.library.selected_track
                        .and_then(|sel| indices.iter().position(|&i| i == sel))
                        .unwrap_or(0);
                    let new_pos = match dir {
                        Direction::Up => cur_pos.saturating_sub(1),
                        Direction::Down => (cur_pos + 1).min(indices.len() - 1),
                        Direction::Top => 0,
                        Direction::Bottom => indices.len() - 1,
                    };
                    self.library.selected_track = Some(indices[new_pos]);
                }
            }
        }
    }

    fn handle_navigate_queue(&mut self, dir: Direction) {
        let len = self.queue.songs.len();
        if len == 0 {
            return;
        }
        self.queue.cursor = match dir {
            Direction::Up => self.queue.cursor.saturating_sub(1),
            Direction::Down => (self.queue.cursor + 1).min(len - 1),
            Direction::Top => 0,
            Direction::Bottom => len - 1,
        };
        self.queue.scroll = self.queue.cursor;
    }

    // ── Select ────────────────────────────────────────────────────────────────

    fn handle_select(&mut self) {
        match self.active_tab {
            Tab::Home => self.handle_select_home(),
            Tab::Browser => match self.browser_focus {
                // Enter on Artists or Albums: same as pressing l
                BrowserColumn::Artists | BrowserColumn::Albums => self.handle_focus_right(),
                // Enter on Tracks: add the highlighted track to the queue
                BrowserColumn::Tracks => self.handle_add_to_queue(),
            },
            Tab::NowPlaying => {
                // Enter: jump playback to the highlighted queue row.
                if !self.queue.songs.is_empty() {
                    self.play_current();
                }
            }
        }
    }

    fn handle_select_home(&mut self) {
        match self.home.active_section {
            HomeSection::RecentAlbums => {
                // Enter on album strip: navigate to Browser with the album's artist pre-selected.
                let idx = self.home.album_selected_index;
                if let Some(album) = self.home.recent_albums.get(idx) {
                    let artist_name = album.artist_name.clone();
                    if self.kitty_supported {
                        let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                        let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                    }
                    self.pending_artist_select = Some(artist_name);
                    self.active_tab = Tab::Browser;
                    self.search_filter = None;
                    self.apply_pending_artist_select();
                }
            }
            HomeSection::RecentTracks => {
                let idx = self.home.selected_index;
                if let Some(record) = self.home.recent_tracks.get(idx).cloned() {
                    // We have a PlayRecord but need a Song. Find it in the queue or
                    // create a minimal Song so we can play it.  The simplest
                    // approach: push a synthetic Song into the queue, then resolve
                    // the stream URL (cache-aware, properly encoded query params).
                    let song_id = record.song_id.clone();
                    // Build a minimal Song for queue display.
                    let song = playterm_subsonic::Song {
                        id: song_id,
                        title: record.track_name.clone(),
                        artist: Some(record.artist_name.clone()),
                        artist_id: Some(record.artist_id.clone()),
                        album: Some(record.album_name.clone()),
                        album_id: Some(record.album_id.clone()),
                        duration: Some(record.duration_secs as u32),
                        track: None,
                        disc_number: None,
                        year: None,
                        genre: None,
                        cover_art: None,
                        path: None,
                        suffix: None,
                        content_type: None,
                        bit_rate: None,
                        size: None,
                        starred: None,
                    };
                    let was_empty = self.queue.songs.is_empty();
                    self.queue.push(song);
                    if was_empty {
                        self.queue.cursor = 0;
                    } else {
                        // Bring the new track to the cursor position.
                        self.queue.cursor = self.queue.songs.len() - 1;
                    }
                    self.play_gen += 1;
                    let duration = record.duration_secs;
                    let dur = if duration > 0 {
                        Some(std::time::Duration::from_secs(duration))
                    } else {
                        None
                    };
                    let sid = record.song_id.clone();
                    let resolved = match self.queue.current().cloned() {
                        Some(s) => self.resolve_playback(&s),
                        None => ResolvedPlayback::Url(
                            self.subsonic.stream_url(&sid, self.config.max_bit_rate),
                        ),
                    };
                    self.playback.player_loaded = true;
                    let gen = self.play_gen;
                    match resolved {
                        ResolvedPlayback::Cached(path) => {
                            let _ = self.player_tx.send(playterm_player::PlayerCommand::PlayCached {
                                path,
                                duration: dur,
                                gen,
                            });
                        }
                        ResolvedPlayback::Url(url) => {
                            let _ = self.player_tx.send(playterm_player::PlayerCommand::PlayUrl {
                                url,
                                duration: dur,
                                gen,
                            });
                        }
                    }
                }
            }
            HomeSection::TopArtists => {
                // Switch to Browser tab.
                if self.kitty_supported {
                    let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                    let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                }
                self.active_tab = Tab::Browser;
                self.search_filter = None;
            }
            HomeSection::Rediscover => {
                // Pre-select the chosen artist in the Browser, then switch.
                if let Some((_, artist_name)) = self.home.rediscover.get(self.home.selected_index) {
                    self.pending_artist_select = Some(artist_name.clone());
                }
                if self.kitty_supported {
                    let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                    let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                }
                self.active_tab = Tab::Browser;
                self.apply_pending_artist_select();
                self.search_filter = None;
            }
        }
    }

    // ── Queue helpers ─────────────────────────────────────────────────────────

    fn handle_add_to_queue(&mut self) {
        if let Some(song) = self.library.current_track().cloned() {
            let was_empty = self.queue.songs.is_empty();
            self.queue.push(song);
            if was_empty {
                self.queue.cursor = 0;
                self.play_current();
            }
        }
    }

    fn handle_add_all_to_queue(&mut self) {
        match self.browser_focus {
            BrowserColumn::Artists | BrowserColumn::Albums => {
                // Fetch every album and every track for the selected artist,
                // then push them all to the queue via AllTracksForArtist.
                if let Some(artist) = self.library.current_artist() {
                    let artist_id = artist.id.clone();
                    let start_playing = self.queue.songs.is_empty();
                    self.fetch_all_tracks_for_artist(artist_id, start_playing);
                }
            }
            BrowserColumn::Tracks => {
                // Add every track in the selected album to the queue.
                let album_id = match self.library.current_album() {
                    Some(a) => a.id.clone(),
                    None => return,
                };
                if let Some(LoadingState::Loaded(songs)) = self.library.tracks.get(&album_id) {
                    let was_empty = self.queue.songs.is_empty();
                    let mut sorted = songs.clone();
                    sorted.sort_by_key(|s| (s.disc_number.unwrap_or(1), s.track.unwrap_or(0)));
                    for song in sorted {
                        self.queue.push(song);
                    }
                    if was_empty && !self.queue.songs.is_empty() {
                        self.queue.cursor = 0;
                        self.play_current();
                    }
                }
                // If tracks not loaded yet: no-op; proactive loading makes this rare.
            }
        }
    }

    fn handle_shuffle(&mut self) {
        let len = self.queue.songs.len();
        if len < 2 {
            return;
        }
        // pre_shuffle_order is maintained by QueueState::push and must NOT be
        // overwritten here — it always holds the original add-order so Z can
        // revert regardless of how many times x is pressed.

        // LCG seeded from system time — no external crate needed.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(12345) as u64;
        let mut rng = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);

        let next_lcg = |state: &mut u64| -> usize {
            *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (*state >> 33) as usize
        };

        // If something is playing, pull the current track to index 0 first,
        // then Fisher-Yates shuffle indices 1..len.
        if self.playback.current_song.is_some() && self.queue.cursor < len {
            self.queue.songs.swap(0, self.queue.cursor);
            for i in (2..len).rev() {
                let j = next_lcg(&mut rng) % i + 1; // range [1, i]
                self.queue.songs.swap(i, j);
            }
        } else {
            // Nothing playing — shuffle the whole vec.
            for i in (1..len).rev() {
                let j = next_lcg(&mut rng) % (i + 1);
                self.queue.songs.swap(i, j);
            }
        }
        self.queue.cursor = 0;
        self.queue.scroll = 0;
    }

    /// Apply the current search: move selection to the first filtered result.
    fn handle_search_confirm(&mut self) {
        let q = self.search_mode.query.to_lowercase();
        if q.is_empty() {
            return;
        }
        match self.active_tab {
            Tab::Home => {} // no search targets yet
            Tab::Browser => match self.browser_focus {
                BrowserColumn::Artists => {
                    if let crate::state::LoadingState::Loaded(artists) = &self.library.artists {
                        if let Some(idx) = artists.iter().position(|a| a.name.to_lowercase().contains(&q)) {
                            self.library.selected_artist = Some(idx);
                        }
                    }
                }
                BrowserColumn::Albums => {
                    let artist_id = match self.library.current_artist() {
                        Some(a) => a.id.clone(),
                        None => return,
                    };
                    if let Some(crate::state::LoadingState::Loaded(albums)) = self.library.albums.get(&artist_id) {
                        if let Some(idx) = albums.iter().position(|a| a.name.to_lowercase().contains(&q)) {
                            self.library.selected_album = Some(idx);
                        }
                    }
                }
                BrowserColumn::Tracks => {
                    let album_id = match self.library.current_album() {
                        Some(a) => a.id.clone(),
                        None => return,
                    };
                    if let Some(crate::state::LoadingState::Loaded(songs)) = self.library.tracks.get(&album_id) {
                        if let Some(idx) = songs.iter().position(|s| s.title.to_lowercase().contains(&q)) {
                            self.library.selected_track = Some(idx);
                        }
                    }
                }
            },
            Tab::NowPlaying => {
                if let Some(idx) = self.queue.songs.iter().position(|s| s.title.to_lowercase().contains(&q)) {
                    self.queue.cursor = idx;
                    self.queue.scroll = idx;
                }
            }
        }
    }

    fn handle_clear_queue(&mut self) {
        self.queue.songs.clear();
        self.queue.cursor = 0;
        self.queue.scroll = 0;
        self.queue.pre_shuffle_order = None;
        let _ = self.player_tx.send(PlayerCommand::Stop);
        self.playback.current_song = None;
        self.playback.elapsed = std::time::Duration::ZERO;
        self.playback.paused = false;
        self.playback.player_loaded = false;
    }

    fn handle_unshuffle(&mut self) {
        let original = match &self.queue.pre_shuffle_order {
            Some(o) => o.clone(),
            None => return,
        };
        // Do NOT clear pre_shuffle_order — Z should always work, even after
        // reshuffling multiple times.
        let current_id = self.queue.current().map(|s| s.id.clone());
        self.queue.songs = original;
        if let Some(id) = current_id {
            if let Some(idx) = self.queue.songs.iter().position(|s| s.id == id) {
                self.queue.cursor = idx;
                self.queue.scroll = idx;
            }
        }
    }

    // ── Mouse-click helpers (called from main.rs event handler) ──────────────

    pub fn click_browser_artist(&mut self, orig_idx: usize) {
        if let LoadingState::Loaded(artists) = &self.library.artists {
            if orig_idx >= artists.len() {
                return;
            }
        } else {
            return;
        }
        self.library.selected_artist = Some(orig_idx);
        self.library.selected_album = Some(0);
        self.library.selected_track = Some(0);
        let artist_id = if let LoadingState::Loaded(artists) = &self.library.artists {
            artists[orig_idx].id.clone()
        } else {
            return;
        };
        if !self.library.albums.contains_key(&artist_id) {
            self.library.albums.insert(artist_id.clone(), LoadingState::Loading);
            self.fetch_albums(artist_id);
        }
    }

    pub fn click_browser_album(&mut self, orig_idx: usize) {
        let artist_id = match self.library.current_artist() {
            Some(a) => a.id.clone(),
            None => return,
        };
        let album_id = {
            let albums = match self.library.albums.get(&artist_id) {
                Some(LoadingState::Loaded(a)) => a,
                _ => return,
            };
            if orig_idx >= albums.len() {
                return;
            }
            albums[orig_idx].id.clone()
        };
        self.library.selected_album = Some(orig_idx);
        self.library.selected_track = Some(0);
        if !self.library.tracks.contains_key(&album_id) {
            self.library.tracks.insert(album_id.clone(), LoadingState::Loading);
            self.fetch_tracks(album_id);
        }
    }

    pub fn click_browser_track(&mut self, orig_idx: usize) {
        let album_id = match self.library.current_album() {
            Some(a) => a.id.clone(),
            None => return,
        };
        let valid = match self.library.tracks.get(&album_id) {
            Some(LoadingState::Loaded(songs)) => orig_idx < songs.len(),
            _ => false,
        };
        if valid {
            self.library.selected_track = Some(orig_idx);
        }
    }

    pub fn set_queue_cursor(&mut self, idx: usize) {
        if idx < self.queue.songs.len() {
            self.queue.cursor = idx;
            self.queue.scroll = idx;
        }
    }

    /// Clear an expired status flash (call once per frame in the main loop).
    pub fn tick_status_flash(&mut self) {
        if let Some((_, deadline)) = &self.status_flash {
            if Instant::now() >= *deadline {
                self.status_flash = None;
            }
        }
    }

    // ── Playlist overlay ──────────────────────────────────────────────────────

    /// Spawn a background task to fetch all playlists from the server.
    pub fn fetch_playlists(&self) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.get_playlists().await {
                Ok(playlists) => {
                    let _ = tx.send(LibraryUpdate::Playlists(playlists)).await;
                }
                Err(e) => eprintln!("playterm: get_playlists failed — {e}"),
            }
        });
    }

    /// Spawn a background task to fetch the track list for `playlist_id`.
    pub fn fetch_playlist_tracks(&self, playlist_id: String) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.get_playlist(&playlist_id).await {
                Ok(detail) => {
                    let _ = tx
                        .send(LibraryUpdate::PlaylistTracks {
                            playlist_id,
                            songs: detail.songs,
                        })
                        .await;
                }
                Err(e) => eprintln!("playterm: get_playlist({playlist_id}) failed — {e}"),
            }
        });
    }

    /// Handle an action directed at the playlist overlay.
    ///
    /// Called both from `dispatch()` (for `TogglePlaylistOverlay` when overlay is
    /// closed) and directly from the event loop (for all keys when overlay is open).
    pub fn handle_playlist_action(&mut self, action: Action) {
        match action {
            Action::TogglePlaylistOverlay => {
                if self.playlist_overlay.visible {
                    self.playlist_overlay.visible = false;
                } else {
                    self.playlist_overlay.visible = true;
                    if matches!(self.playlist_overlay.playlists, LoadingState::NotLoaded) {
                        self.playlist_overlay.playlists = LoadingState::Loading;
                        self.fetch_playlists();
                    }
                }
            }
            Action::PlaylistScrollUp => match self.playlist_overlay.focus {
                PlaylistFocus::List => {
                    self.playlist_overlay.selected_playlist_index =
                        self.playlist_overlay.selected_playlist_index.saturating_sub(1);
                }
                PlaylistFocus::Tracks => {
                    self.playlist_overlay.selected_track_index =
                        self.playlist_overlay.selected_track_index.saturating_sub(1);
                }
            },
            Action::PlaylistScrollDown => match self.playlist_overlay.focus {
                PlaylistFocus::List => {
                    if let LoadingState::Loaded(ref playlists) = self.playlist_overlay.playlists {
                        let max = playlists.len().saturating_sub(1);
                        self.playlist_overlay.selected_playlist_index =
                            (self.playlist_overlay.selected_playlist_index + 1).min(max);
                    }
                }
                PlaylistFocus::Tracks => {
                    if let LoadingState::Loaded(ref songs) = self.playlist_overlay.tracks {
                        let max = songs.len().saturating_sub(1);
                        self.playlist_overlay.selected_track_index =
                            (self.playlist_overlay.selected_track_index + 1).min(max);
                    }
                }
            },
            Action::PlaylistFocusTracks => {
                self.playlist_overlay.focus = PlaylistFocus::Tracks;
                // Fetch tracks if not already loaded for the currently selected playlist.
                if let LoadingState::Loaded(ref playlists) = self.playlist_overlay.playlists {
                    if let Some(playlist) =
                        playlists.get(self.playlist_overlay.selected_playlist_index)
                    {
                        let id = playlist.id.clone();
                        if self.playlist_overlay.loaded_playlist_id.as_deref() != Some(&id) {
                            self.playlist_overlay.tracks = LoadingState::Loading;
                            self.playlist_overlay.loaded_playlist_id = Some(id.clone());
                            self.fetch_playlist_tracks(id);
                        }
                    }
                }
            }
            Action::PlaylistFocusList => {
                self.playlist_overlay.focus = PlaylistFocus::List;
            }
            Action::PlaylistPlayAll => {
                if let LoadingState::Loaded(ref songs) = self.playlist_overlay.tracks {
                    let songs = songs.clone();
                    self.queue.songs.clear();
                    self.queue.cursor = 0;
                    self.queue.scroll = 0;
                    self.queue.pre_shuffle_order = None;
                    for song in songs {
                        self.queue.push(song);
                    }
                    if !self.queue.songs.is_empty() {
                        self.queue.cursor = 0;
                        self.queue.scroll = 0;
                        self.play_current();
                    }
                }
                self.playlist_overlay.visible = false;
            }
            Action::PlaylistAppendAll => {
                if let LoadingState::Loaded(ref songs) = self.playlist_overlay.tracks {
                    let was_empty = self.queue.songs.is_empty();
                    for song in songs.clone() {
                        self.queue.push(song);
                    }
                    if was_empty && !self.queue.songs.is_empty() {
                        self.queue.cursor = 0;
                        self.queue.scroll = 0;
                        self.play_current();
                    }
                }
                self.playlist_overlay.visible = false;
            }
            Action::PlaylistPlayTrack => {
                if let LoadingState::Loaded(ref songs) = self.playlist_overlay.tracks {
                    if let Some(song) =
                        songs.get(self.playlist_overlay.selected_track_index).cloned()
                    {
                        self.queue.songs.clear();
                        self.queue.cursor = 0;
                        self.queue.scroll = 0;
                        self.queue.pre_shuffle_order = None;
                        self.queue.push(song);
                        self.queue.cursor = 0;
                        self.queue.scroll = 0;
                        self.play_current();
                    }
                }
                self.playlist_overlay.visible = false;
            }
            Action::PlaylistAppendTrack => {
                if let LoadingState::Loaded(ref songs) = self.playlist_overlay.tracks {
                    if let Some(song) =
                        songs.get(self.playlist_overlay.selected_track_index).cloned()
                    {
                        let was_empty = self.queue.songs.is_empty();
                        self.queue.push(song);
                        if was_empty {
                            self.queue.cursor = 0;
                            self.queue.scroll = 0;
                            self.play_current();
                        }
                    }
                }
                self.playlist_overlay.visible = false;
            }
            _ => {}
        }
    }

    // ── Playlist mutation actions (Phase 8.2) ─────────────────────────────────

    pub fn handle_playlist_mutation(&mut self, action: Action) {
        match action {
            Action::PlaylistCreate => {
                self.playlist_overlay.input_mode =
                    PlaylistInputMode::Creating { buffer: String::new() };
            }
            Action::PlaylistRename => {
                if let LoadingState::Loaded(ref playlists) = self.playlist_overlay.playlists {
                    if let Some(p) =
                        playlists.get(self.playlist_overlay.selected_playlist_index)
                    {
                        self.playlist_overlay.input_mode = PlaylistInputMode::Renaming {
                            buffer: p.name.clone(),
                            playlist_id: p.id.clone(),
                        };
                    }
                }
            }
            Action::PlaylistDelete => {
                if let LoadingState::Loaded(ref playlists) = self.playlist_overlay.playlists {
                    if let Some(p) =
                        playlists.get(self.playlist_overlay.selected_playlist_index)
                    {
                        self.playlist_overlay.input_mode =
                            PlaylistInputMode::Confirming {
                                action: ConfirmAction::DeletePlaylist {
                                    id: p.id.clone(),
                                    name: p.name.clone(),
                                },
                            };
                    }
                }
            }
            Action::PlaylistInputChar(c) => {
                match &mut self.playlist_overlay.input_mode {
                    PlaylistInputMode::Creating { buffer }
                    | PlaylistInputMode::Renaming { buffer, .. } => {
                        if c == '\x08' {
                            buffer.pop();
                        } else {
                            buffer.push(c);
                        }
                    }
                    _ => {}
                }
            }
            Action::PlaylistInputConfirm => {
                match self.playlist_overlay.input_mode.clone() {
                    PlaylistInputMode::Creating { buffer } if !buffer.is_empty() => {
                        self.spawn_create_playlist(buffer);
                    }
                    PlaylistInputMode::Renaming { buffer, playlist_id }
                        if !buffer.is_empty() =>
                    {
                        self.spawn_rename_playlist(playlist_id, buffer);
                    }
                    _ => {}
                }
                self.playlist_overlay.input_mode = PlaylistInputMode::Normal;
            }
            Action::PlaylistInputCancel => {
                self.playlist_overlay.input_mode = PlaylistInputMode::Normal;
            }
            Action::PlaylistConfirmYes => {
                if let PlaylistInputMode::Confirming {
                    action: ConfirmAction::DeletePlaylist { id, .. },
                } = self.playlist_overlay.input_mode.clone()
                {
                    self.spawn_delete_playlist(id);
                    self.playlist_overlay.input_mode = PlaylistInputMode::Normal;
                }
            }
            Action::PlaylistConfirmNo => {
                self.playlist_overlay.input_mode = PlaylistInputMode::Normal;
            }
            Action::PlaylistRemoveTrack => {
                if let (Some(playlist_id), LoadingState::Loaded(ref songs)) = (
                    self.playlist_overlay.loaded_playlist_id.clone(),
                    &self.playlist_overlay.tracks,
                ) {
                    if !songs.is_empty() {
                        let index = self.playlist_overlay.selected_track_index;
                        self.spawn_remove_track(playlist_id, index);
                    }
                }
            }
            Action::BrowserAddToPlaylist => {
                if let Some(song) = self.library.current_track() {
                    let song_id = song.id.clone();
                    match &self.playlist_overlay.playlists {
                        LoadingState::Loaded(playlists) => {
                            self.playlist_picker = Some(PlaylistPicker {
                                playlists: playlists.clone(),
                                selected_index: 0,
                                song_id,
                                loading: false,
                            });
                        }
                        _ => {
                            self.playlist_picker = Some(PlaylistPicker {
                                playlists: vec![],
                                selected_index: 0,
                                song_id,
                                loading: true,
                            });
                            self.spawn_fetch_playlists_for_picker();
                        }
                    }
                }
            }
            Action::PlaylistPickerSelect => {
                if let Some(ref picker) = self.playlist_picker {
                    if let Some(playlist) = picker.playlists.get(picker.selected_index) {
                        let playlist_id   = playlist.id.clone();
                        let playlist_name = playlist.name.clone();
                        let song_id       = picker.song_id.clone();
                        self.spawn_add_track_to_playlist(playlist_id, playlist_name, song_id);
                    }
                }
                self.playlist_picker = None;
            }
            Action::PlaylistPickerCancel => {
                self.playlist_picker = None;
            }
            Action::PlaylistPickerScrollUp => {
                if let Some(ref mut picker) = self.playlist_picker {
                    picker.selected_index = picker.selected_index.saturating_sub(1);
                }
            }
            Action::PlaylistPickerScrollDown => {
                if let Some(ref mut picker) = self.playlist_picker {
                    let max = picker.playlists.len().saturating_sub(1);
                    picker.selected_index = (picker.selected_index + 1).min(max);
                }
            }
            _ => {}
        }
    }

    fn spawn_create_playlist(&self, name: String) {
        let client = self.subsonic.clone();
        let tx     = self.library_tx.clone();
        tokio::spawn(async move {
            match client.create_playlist(&name).await {
                Ok(p)  => { let _ = tx.send(LibraryUpdate::PlaylistCreated(p)).await; }
                Err(e) => eprintln!("create_playlist failed: {e}"),
            }
        });
    }

    fn spawn_rename_playlist(&self, playlist_id: String, new_name: String) {
        let client = self.subsonic.clone();
        let tx     = self.library_tx.clone();
        tokio::spawn(async move {
            match client.rename_playlist(&playlist_id, &new_name).await {
                Ok(()) => {
                    let _ = tx
                        .send(LibraryUpdate::PlaylistRenamed { id: playlist_id, new_name })
                        .await;
                }
                Err(e) => eprintln!("rename_playlist failed: {e}"),
            }
        });
    }

    fn spawn_delete_playlist(&self, id: String) {
        let client = self.subsonic.clone();
        let tx     = self.library_tx.clone();
        tokio::spawn(async move {
            match client.delete_playlist(&id).await {
                Ok(()) => { let _ = tx.send(LibraryUpdate::PlaylistDeleted(id)).await; }
                Err(e) => eprintln!("delete_playlist failed: {e}"),
            }
        });
    }

    fn spawn_remove_track(&self, playlist_id: String, index: usize) {
        let client = self.subsonic.clone();
        let tx     = self.library_tx.clone();
        tokio::spawn(async move {
            match client.remove_track_from_playlist(&playlist_id, index).await {
                Ok(()) => {
                    let _ = tx
                        .send(LibraryUpdate::PlaylistTrackRemoved { _playlist_id: playlist_id, index })
                        .await;
                }
                Err(e) => eprintln!("remove_track_from_playlist failed: {e}"),
            }
        });
    }

    fn spawn_fetch_playlists_for_picker(&self) {
        let client = self.subsonic.clone();
        let tx     = self.library_tx.clone();
        tokio::spawn(async move {
            match client.get_playlists().await {
                Ok(list) => { let _ = tx.send(LibraryUpdate::PlaylistsForPicker(list)).await; }
                Err(e)   => eprintln!("get_playlists for picker failed: {e}"),
            }
        });
    }

    fn spawn_add_track_to_playlist(
        &self,
        playlist_id: String,
        playlist_name: String,
        song_id: String,
    ) {
        let client = self.subsonic.clone();
        let tx     = self.library_tx.clone();
        tokio::spawn(async move {
            match client.add_track_to_playlist(&playlist_id, &song_id).await {
                Ok(()) => {
                    let _ = tx
                        .send(LibraryUpdate::PlaylistTrackAdded {
                            _playlist_id: playlist_id,
                            playlist_name,
                        })
                        .await;
                }
                Err(e) => eprintln!("add_track_to_playlist failed: {e}"),
            }
        });
    }
}
