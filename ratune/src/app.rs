use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::style::Color;
use tokio::sync::mpsc;

use ratune_player::{spawn_player, PlayerCommand, PlayerEvent, SampleBuffer};
use ratune_subsonic::SubsonicClient;

use serde::{Deserialize, Serialize};

use crate::action::{Action, Direction};
use crate::color::{extract_accent, lerp_color};
use crate::config::{AlbumArtBackend, BrowseMode, Config};
use crate::history::PlayRecord;
use crate::keybinds::Keybinds;
use crate::state::{
    folder_left_default_row, folder_preview_rows, ConfirmAction, DirectoryListing,
    FolderBrowseState, FolderPreviewRow, GlobalConfirm, LibraryState, LoadingState, PlaybackState,
    PlaylistFocus, PlaylistInputMode, PlaylistOverlay, QueueState,
};
use crate::theme::Theme;
use image::{imageops::FilterType, DynamicImage};
use ratatui_image::picker::ProtocolType;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};
use ratatui_image::Resize;
use ratune_subsonic::LyricLine;

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
            Tab::Home => Tab::Browser,
            Tab::Browser => Tab::NowPlaying,
            Tab::NowPlaying => Tab::Home,
        }
    }

    /// Cycle backward: Home → NowPlaying → Browser → Home
    pub fn prev(self) -> Self {
        match self {
            Tab::Home => Tab::NowPlaying,
            Tab::Browser => Tab::Home,
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

impl App {
    /// Filter string for a browser column, if search was confirmed while that column was focused.
    pub fn browser_column_filter(&self, column: BrowserColumn) -> Option<&str> {
        match (&self.search_filter, self.search_filter_column) {
            (Some(q), Some(col)) if col == column => Some(q.as_str()),
            _ => None,
        }
    }

    pub fn clear_browser_search(&mut self) {
        self.search_filter = None;
        self.search_filter_column = None;
    }
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
            HomeSection::TopArtists => HomeSection::Rediscover,
            HomeSection::Rediscover => HomeSection::RecentAlbums,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            HomeSection::RecentAlbums => HomeSection::Rediscover,
            HomeSection::RecentTracks => HomeSection::RecentAlbums,
            HomeSection::TopArtists => HomeSection::RecentAlbums,
            HomeSection::Rediscover => HomeSection::RecentTracks,
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
    Artists(Result<Vec<ratune_subsonic::Artist>, String>),
    Albums {
        artist_id: String,
        result: Result<Vec<ratune_subsonic::Album>, String>,
    },
    Tracks {
        album_id: String,
        result: Result<Vec<ratune_subsonic::Song>, String>,
    },
    /// All tracks across every album for an artist; carries whether playback
    /// should auto-start (true when the queue was empty at dispatch time).
    AllTracksForArtist {
        songs: Vec<ratune_subsonic::Song>,
        start_playing: bool,
        /// When true, insert all songs at the front of the queue (album / track order preserved).
        prepend: bool,
    },
    /// Raw image bytes for a cover art ID fetched from Navidrome.
    CoverArt {
        cover_id: String,
        bytes: Vec<u8>,
    },
    /// Lyrics fetched for a song; `lines` is empty when the track has no lyrics.
    Lyrics {
        song_id: String,
        lines: Vec<LyricLine>,
    },
    /// Track bytes downloaded for offline caching.
    CacheTrack {
        song_id: String,
        album_id: String,
        bytes: Vec<u8>,
    },
    /// Cover art fetched for a home-tab album strip thumbnail.
    HomeArt {
        album_id: String,
        bytes: Vec<u8>,
    },
    /// Home strip cover fetch failed — release loading slot so more fetches can run.
    HomeArtFetchFailed {
        album_id: String,
    },
    /// All playlists fetched from `getPlaylists`.
    Playlists(Vec<ratune_subsonic::Playlist>),
    /// Full track list for a single playlist fetched from `getPlaylist`.
    PlaylistTracks {
        playlist_id: String,
        songs: Vec<ratune_subsonic::Song>,
    },
    /// A new playlist was successfully created.
    PlaylistCreated(ratune_subsonic::Playlist),
    /// A playlist was successfully deleted (carries the deleted ID).
    PlaylistDeleted(String),
    /// A playlist was successfully renamed.
    PlaylistRenamed {
        id: String,
        new_name: String,
    },
    /// A track was successfully added to a playlist.
    PlaylistTrackAdded {
        _playlist_id: String,
        playlist_name: String,
    },
    /// A track was successfully removed from a playlist.
    PlaylistTrackRemoved {
        _playlist_id: String,
        index: usize,
    },
    /// Playlist list fetched for the picker (separate from the overlay's list).
    PlaylistsForPicker(Vec<ratune_subsonic::Playlist>),
    /// Full library metadata index finished refreshing (background task).
    /// Tuple:
    /// - Vec<Song>: fresh index contents
    /// - Option<String>: Navidrome `lastScan` token to persist when scan-skip is enabled.
    /// - bool: whether this refresh was explicitly forced by the user (e.g. Ctrl+g).
    LibraryIndexRefreshComplete {
        result: Result<(Vec<ratune_subsonic::Song>, Option<String>, bool), String>,
    },
    /// Full-library fetch (from server) finished for "append whole library" when index is disabled.
    LibraryServerAppendQueueComplete {
        result: Result<Vec<ratune_subsonic::Song>, String>,
    },
    /// Top-level folders from `getMusicFolders` (file browse mode).
    MusicFolders(Result<Vec<ratune_subsonic::MusicFolder>, String>),
    /// One directory listing from `getMusicDirectory`.
    MusicDirectory {
        id: String,
        result: Result<DirectoryListing, String>,
    },
}

#[derive(Clone, Copy)]
enum AddAllMode {
    Append,
    ReplaceAlbum,
    ReplaceArtist,
    Prepend,
}

// ── PlaylistPicker ────────────────────────────────────────────────────────────

/// Floating picker shown when the user wants to add a browser track to a
/// playlist.  Populated lazily from `getPlaylists`.
#[derive(Debug)]
pub struct PlaylistPicker {
    pub playlists: Vec<ratune_subsonic::Playlist>,
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
    pub folders: FolderBrowseState,
    pub queue: QueueState,
    pub playback: PlaybackState,
    pub config: Config,
    /// Effective Browse tab layout: toggled at runtime when folder navigation is enabled.
    pub browser_browse_mode: BrowseMode,
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
    /// Active filter applied to one browser column after a search confirm.
    /// `None` = show all items; `Some(q)` = show only items whose name contains `q`.
    pub search_filter: Option<String>,
    /// Column that owns `search_filter` (filters apply only to that column).
    pub search_filter_column: Option<BrowserColumn>,
    /// Filter active before `/` opened search input; restored on Esc cancel.
    search_filter_before_edit: Option<(BrowserColumn, String)>,
    /// Whether the running terminal supports the Kitty graphics protocol.
    /// Set once by `main` before the TUI loop starts.
    pub kitty_supported: bool,
    /// Whether ratune is running inside a tmux session.
    /// Set once by `main` at startup via `$TMUX` env var.
    pub in_tmux: bool,
    /// Row offset caused by a top-positioned tmux status bar (0 or 1).
    /// Used to correct Unicode placeholder cursor positioning in tmux mode.
    pub tmux_status_offset: u16,
    /// Terminal cell pixel dimensions `(width_px, height_px)`.
    /// Queried once at startup; `None` if unavailable (fallback: 8×16).
    pub cell_px: Option<(u16, u16)>,
    /// OSC 4 / OSC 10 readback for smooth visualizer gradients with indexed or reset colours.
    pub visualizer_gradient_rgb_cache: Option<crate::ui::terminal_palette::GradientRgbCache>,
    /// Cached cover art: `(cover_art_id, raw_image_bytes)`.
    /// Updated whenever a new track starts with a different cover ID.
    pub art_cache: Option<(String, Vec<u8>)>,
    /// FNV digest of `art_cache` bytes — stable across tracks that share the same image.
    pub art_cache_fingerprint: Option<u64>,
    /// Decoded `art_cache` image for the current fingerprint — avoids JPEG decode on every resize.
    pub art_cache_decoded: Option<(u64, DynamicImage)>,
    /// Home tab album art cache: `album_id → raw image bytes`.
    pub home_art_cache: HashMap<String, Vec<u8>>,
    /// Album IDs for which a home art fetch is currently in flight.
    pub home_art_loading: HashSet<String>,
    /// Reused resize+zlib for Home strip Kitty thumbnails (avoids re-decoding on every redraw).
    pub home_strip_thumb_prepared: HashMap<String, crate::ui::kitty_art::StripThumbPrepared>,
    /// Resolved keybindings (parsed from config.toml [keybinds]).
    pub keybinds: Keybinds,
    /// Resolved theme colours (parsed from config.toml [theme]).
    pub theme: Theme,
    /// Monotonically increasing counter sent with every play command (`PlayUrl` / `PlayCached`).
    /// The engine uses it to discard stale downloads from rapid skips.
    play_gen: u64,

    // ── Library metadata index (Milestone 2) ───────────────────────────────────
    /// Cached tracks for fzf (text only; persisted under `~/.cache/ratune/` by default).
    pub library_index_tracks: Vec<ratune_subsonic::Song>,
    library_index_by_id: HashMap<String, ratune_subsonic::Song>,
    /// Unix seconds when the index was last fully refreshed, if known.
    pub library_index_refreshed_at: Option<u64>,
    /// True while a background full-library fetch is running.
    pub library_index_refreshing: bool,
    /// When the current refresh started; drives status-bar animation until complete.
    pub library_index_refresh_started: Option<Instant>,
    /// True while a background full-library fetch is running for "append whole library"
    /// when the local index is disabled/empty.
    pub library_server_append_fetching: bool,
    /// When the current server fetch started; drives status-bar animation until complete.
    pub library_server_append_started: Option<Instant>,

    // ── Offline cache (Feature 5.3) ───────────────────────────────────────────
    /// Track file cache (LRU, persisted to `~/.cache/ratune/`).
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
    /// Last inner rect of the Recently Played block (updated each Home render); drives strip scroll math.
    pub home_recent_albums_inner: Option<Rect>,
    /// After terminal resize, strip caches are cleared only once this instant is reached (debounce).
    pub home_strip_resize_settle: Option<Instant>,
    /// Fingerprint of strip layout; used to avoid re-encoding when resize settles with same thumb grid.
    pub home_strip_layout_key: Option<u64>,

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
    /// Recent time-domain samples for waveform visualizer (roughly -1.0..1.0).
    pub waveform: Vec<f32>,
    /// Whether the spectrum visualizer overlay is currently visible.
    pub visualizer_visible: bool,
    /// FFT planner — cached across frames for efficiency.
    pub fft_planner: rustfft::FftPlanner<f32>,
    /// Last time we updated spectrum/waveform (FPS throttling).
    pub visualizer_last_tick: Option<Instant>,

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

    /// Last measured height (inner rows) of the queue list; used when clamping scroll outside render.
    pub queue_viewport_rows: usize,
    /// Last measured inner height of each Browse column list (page up/down).
    pub browser_list_viewport_rows: usize,
    /// Saw a lone `g`; next `g` dispatches go-to-top (vim-style `gg`).
    pub pending_gg: bool,
    /// Confirmation prompt for expensive actions (e.g. full library index refresh).
    pub pending_global_confirm: Option<GlobalConfirm>,
    /// First visible line index inside the help popup (scroll).
    pub help_scroll: usize,

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

    /// Set after alternate screen when `album_art_backend = ratatui-image` and the probe succeeds.
    pub art_picker: Option<ratatui_image::picker::Picker>,
    /// Now Playing album art — encode runs on `ratatui_resize` worker thread (`ThreadProtocol`).
    pub np_art_state: Option<ThreadProtocol>,
    /// Worker queue for `ResizeRequest` (Now Playing only; home strip stays on-thread for now).
    pub ratatui_resize_tx: Option<Sender<ResizeRequest>>,
    pub ratatui_resize_rx: Option<Receiver<Result<ResizeResponse, ratatui_image::errors::Errors>>>,
    /// `(bytes_digest, inner_w, inner_h)` — rebuild when pixels or art `Rect` change.
    pub np_art_prep_key: Option<(u64, u16, u16)>,
    /// Home art strip: one protocol state per `album_id`.
    pub home_strip_art: HashMap<String, ratatui_image::protocol::StatefulProtocol>,
    /// Last thumbnail cell size per album — rebuild strip slot when layout resizes.
    pub home_strip_last_cells: HashMap<String, (u16, u16)>,
}

impl App {
    pub fn new(config: Config) -> Result<Self> {
        let subsonic = SubsonicClient::new(
            &config.subsonic_url,
            &config.subsonic_user,
            &config.subsonic_pass,
        )?;
        let (library_tx, library_rx) = mpsc::channel(64);
        let (player_tx, player_rx, player_join, sample_buffer) = spawn_player();
        // Apply configured default volume immediately.
        let _ = player_tx.send(PlayerCommand::SetVolume(
            config.default_volume as f32 / 100.0,
        ));
        let keybinds = Keybinds::from_section(&config.keybinds);
        let theme = Theme::from_section(&config.theme);
        let static_accent = theme.accent;
        let lyrics_visible = config.lyrics_visible;
        let visualizer_visible = config.visualizer_visible;
        let track_cache =
            crate::cache::TrackCache::load(config.cache_enabled, config.cache_max_size_gb);
        let index_path = config.resolved_library_index_path();
        let (library_index_tracks, library_index_refreshed_at) =
            match crate::library_index::load(&index_path) {
                Some(f) => (f.tracks, Some(f.refreshed_at_unix)),
                None => (Vec::new(), None),
            };
        let library_index_by_id = crate::library_index::index_by_id(&library_index_tracks);
        let browser_browse_mode = match config.browse_mode {
            BrowseMode::Genre => BrowseMode::Genre,
            _ if !config.browse_folder_navigation => BrowseMode::Artists,
            BrowseMode::Files => BrowseMode::Files,
            BrowseMode::Artists => BrowseMode::Artists,
        };
        Ok(Self {
            active_tab: Tab::Home,
            browser_focus: BrowserColumn::Artists,
            library: LibraryState::default(),
            folders: FolderBrowseState::default(),
            queue: QueueState::default(),
            playback: PlaybackState::default(),
            subsonic: Arc::new(subsonic),
            library_rx,
            library_tx,
            player_tx,
            player_rx,
            player_join: Some(player_join),
            config,
            browser_browse_mode,
            should_quit: false,
            search_mode: SearchMode::default(),
            search_filter: None,
            search_filter_column: None,
            search_filter_before_edit: None,
            kitty_supported: false,
            in_tmux: false,
            tmux_status_offset: 0,
            cell_px: None,
            visualizer_gradient_rgb_cache: None,
            art_cache: None,
            art_cache_fingerprint: None,
            art_cache_decoded: None,
            home_art_cache: HashMap::new(),
            home_art_loading: HashSet::new(),
            home_strip_thumb_prepared: HashMap::new(),
            keybinds,
            theme,
            play_gen: 0,
            library_index_tracks,
            library_index_by_id,
            library_index_refreshed_at,
            library_index_refreshing: false,
            library_index_refresh_started: None,
            library_server_append_fetching: false,
            library_server_append_started: None,
            cache: track_cache,
            prefetch_gen: Arc::new(AtomicU64::new(0)),
            help_visible: false,
            home_art_needs_redraw: false,
            home_art_last_tmux_render: None,
            home_recent_albums_inner: None,
            home_strip_resize_settle: None,
            home_strip_layout_key: None,
            home: HomeState::default(),
            pending_artist_select: None,
            playlist_overlay: PlaylistOverlay::default(),
            playlist_picker: None,
            status_flash: None,
            queue_viewport_rows: 12,
            browser_list_viewport_rows: 12,
            pending_gg: false,
            pending_global_confirm: None,
            help_scroll: 0,
            history: crate::history::PlayHistory::default(),
            play_recorded: false,
            lyrics_visible,
            lyrics_cache: None,
            lyrics_scroll: 0,
            lyrics_loading: false,
            sample_buffer,
            spectrum_bands: vec![0.0; 32],
            waveform: Vec::new(),
            visualizer_visible,
            fft_planner: rustfft::FftPlanner::new(),
            visualizer_last_tick: None,
            dynamic_accent: None,
            accent_current: static_accent,
            accent_lerp_from: static_accent,
            accent_target: static_accent,
            accent_transition_start: None,
            art_picker: None,
            np_art_state: None,
            ratatui_resize_tx: None,
            ratatui_resize_rx: None,
            np_art_prep_key: None,
            home_strip_art: HashMap::new(),
            home_strip_last_cells: HashMap::new(),
            #[cfg(target_os = "linux")]
            mpris: None,
        })
    }

    /// `album_art_backend = "kitty-apc"`: Kitty APC post-draw path is active.
    pub fn kitty_apc_graphics_ready(&self) -> bool {
        matches!(self.config.album_art_backend, AlbumArtBackend::KittyApc) && self.kitty_supported
    }

    /// `ratatui-image` picker initialized (terminal query succeeded).
    pub fn ratatui_art_ready(&self) -> bool {
        matches!(self.config.album_art_backend, AlbumArtBackend::RatatuiImage)
            && self.art_picker.is_some()
    }

    /// Picker chose Kitty graphics — use the same post-draw APC path as `kitty_art`, not `StatefulImage`.
    ///
    /// ratatui-image's in-buffer Kitty backend can re-encode pathologically; our hand-rolled APC is
    /// battle-tested for this app.
    pub fn ratatui_uses_kitty_apc(&self) -> bool {
        self.ratatui_art_ready()
            && self
                .art_picker
                .as_ref()
                .is_some_and(|p| matches!(p.protocol_type(), ProtocolType::Kitty))
    }

    /// Any path that draws album art via Kitty APC **after** `terminal.draw` (`kitty-apc` or ratatui-image on Kitty).
    pub fn kitty_apc_overlay_active(&self) -> bool {
        self.kitty_apc_graphics_ready() || self.ratatui_uses_kitty_apc()
    }

    /// `Resize` mode for `ratatui-image` [`StatefulImage`] (Sixel / halfblocks / iTerm2 — not Kitty APC).
    ///
    /// [`Resize::Fit`] caps the raster to the **source** pixel size, then pads the cell area with the
    /// background colour — common Sixel symptom: empty bands on the bottom/right. [`Resize::Scale`]
    /// upscales after our `art_prepare` budget so the image fills the allocated cells (still
    /// letterboxed if aspect ratios differ, but much less dead space).
    pub fn ratatui_stateful_resize(&self) -> Resize {
        Resize::Scale(Some(FilterType::Triangle))
    }

    /// Home strip: bitmap is exact super-res cell size after crop (`prepare_art_image_for_strip`).
    pub fn ratatui_stateful_resize_strip(&self) -> Resize {
        Resize::Scale(Some(FilterType::Triangle))
    }

    /// Visible album slots in the Recently Played strip (fixed-size thumbs, 1–2 rows).
    pub fn home_album_strip_visible_count(&self) -> usize {
        use crate::ui::kitty_art::art_strip_layout;
        const FALLBACK: usize = 12;
        self.home_recent_albums_inner
            .map(|inner| {
                art_strip_layout(inner.width, inner.height)
                    .total_visible
                    .max(1)
            })
            .unwrap_or(FALLBACK)
    }

    /// Home strip should use graphics (either backend), and the help overlay is closed.
    pub fn home_strip_graphics_wanted(&self, help_visible: bool) -> bool {
        self.config.home_recent_albums_show_art
            && !help_visible
            && (self.kitty_apc_graphics_ready() || self.ratatui_art_ready())
    }

    /// Drop all `ratatui-image` protocol state (tab switch, help overlay, fzf suspend, …).
    pub fn clear_ratatui_art_state(&mut self) {
        self.np_art_state = None;
        self.np_art_prep_key = None;
        self.home_strip_art.clear();
        self.home_strip_last_cells.clear();
        self.home_strip_layout_key = None;
        self.home_strip_resize_settle = None;
    }

    /// Now Playing ratatui art only (terminal resize before debounced strip invalidation).
    pub fn clear_np_ratatui_art_state(&mut self) {
        self.np_art_state = None;
        self.np_art_prep_key = None;
    }

    pub fn schedule_home_strip_resize_invalidate(&mut self) {
        self.home_strip_resize_settle = Some(Instant::now() + Duration::from_millis(200));
    }

    /// Call after `terminal.draw` when Home may be visible. Debounces strip re-encode on resize.
    pub fn apply_home_strip_resize_settle(&mut self) {
        let Some(deadline) = self.home_strip_resize_settle else {
            return;
        };
        if Instant::now() < deadline {
            return;
        }
        if self.active_tab != Tab::Home {
            self.home_strip_resize_settle = None;
            return;
        }
        let Some(inner) = self.home_recent_albums_inner else {
            self.home_strip_resize_settle = None;
            return;
        };
        self.home_strip_resize_settle = None;
        use crate::ui::kitty_art::{art_strip_layout, strip_layout_key};
        let layout = art_strip_layout(inner.width, inner.height);
        let key = strip_layout_key(inner, &layout);
        if self.home_strip_layout_key == Some(key) {
            return;
        }
        let prev = self.home_strip_layout_key;
        self.home_strip_layout_key = Some(key);
        let need_clear = prev.map_or(
            !self.home_strip_art.is_empty() || !self.home_strip_thumb_prepared.is_empty(),
            |old| old != key,
        );
        if !need_clear {
            return;
        }
        self.home_strip_art.clear();
        self.home_strip_last_cells.clear();
        self.home_strip_thumb_prepared.clear();
        if self.kitty_apc_overlay_active() {
            let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
        }
        self.home_art_needs_redraw = true;
    }

    /// Match Kitty APC clears on tab navigation: NP overlay always; Home strip when leaving Home.
    ///
    /// For **non-Kitty** `ratatui-image` (Sixel, etc.) we **do not** drop `StatefulProtocol` /
    /// `ThreadProtocol` here: clearing forces a full re-encode on every tab visit and is very slow
    /// in Foot. The next frame’s ratatui draw covers those cells; if a terminal leaves stray
    /// graphics, resize/fzf/focus still clear state.
    fn clear_art_on_tab_switch(&mut self) {
        if self.kitty_apc_overlay_active() {
            let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
            if self.active_tab == Tab::Home {
                let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
            }
        }
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
        // FPS throttling: reuse last computed values if we're drawing faster than desired.
        let fps = self.config.visualizer_fps.max(1) as f32;
        let min_dt = Duration::from_secs_f32(1.0 / fps);
        if let Some(last) = self.visualizer_last_tick {
            if last.elapsed() < min_dt {
                return;
            }
        }
        self.visualizer_last_tick = Some(Instant::now());

        let samples = match self.sample_buffer.lock() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        let vtype = self.config.visualizer_type.trim().to_lowercase();
        let gain_db = self.config.visualizer_gain_db;
        match vtype.as_str() {
            "wave" => {
                // Take a recent slice (renderer bins/averages per column).
                let take = samples.len().clamp(16, 2048);
                let start = samples.len().saturating_sub(take);
                let amp = 10.0f32.powf(gain_db / 20.0);
                self.waveform = samples
                    .iter()
                    .skip(start)
                    .map(|&s| (s * amp).clamp(-1.0, 1.0))
                    .collect();
            }
            _ => {
                let new_bands = crate::visualizer::compute_bands(
                    &samples,
                    &mut self.fft_planner,
                    &self.spectrum_bands,
                    32,
                    Some(self.config.visualizer_fft_size),
                    gain_db,
                );
                self.spectrum_bands = new_bands;
            }
        }
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
        let old_album_ids: Vec<String> = self
            .home
            .recent_albums
            .iter()
            .map(|a| a.album_id.clone())
            .collect();

        // Recent albums: up to 20 unique albums for the art strip.
        self.home.recent_albums = self
            .history
            .recent_albums(20)
            .into_iter()
            .map(|(album_id, album_name, artist_name)| RecentAlbum {
                album_id,
                album_name,
                artist_name,
            })
            .collect();

        let new_album_ids: Vec<String> = self
            .home
            .recent_albums
            .iter()
            .map(|a| a.album_id.clone())
            .collect();

        let album_strip_unchanged = old_album_ids == new_album_ids;
        if album_strip_unchanged {
            // Same albums in the same order — keep scroll/selection and the Kitty
            // strip CPU cache (`home_strip_thumb_prepared`). Tab switches still clear
            // terminal placements, but redraw reuses zlib without re-decoding covers.
            let max_idx = self.home.recent_albums.len().saturating_sub(1);
            self.home.album_scroll_offset = self.home.album_scroll_offset.min(max_idx);
            self.home.album_selected_index = self.home.album_selected_index.min(max_idx);
        } else {
            self.home.album_scroll_offset = 0;
            self.home.album_selected_index = 0;
        }

        // Drop prepared thumbs for albums no longer in the strip (never blanket-clear:
        // that forced a full decode pipeline on every Home visit).
        let keep_ids: HashSet<String> = self
            .home
            .recent_albums
            .iter()
            .map(|a| a.album_id.clone())
            .collect();
        self.home_strip_thumb_prepared
            .retain(|id, _| keep_ids.contains(id));

        // Recent tracks: last 15 from history (most recent first).
        let total = self.history.records.len();
        let start = total.saturating_sub(15);
        self.home.recent_tracks = self.history.records[start..]
            .iter()
            .cloned()
            .rev()
            .collect();

        // Top 15 artists by play count.
        self.home.top_artists = self.history.top_artists(15);

        // Rediscover: build library artist pairs from loaded state.
        let library_artist_pairs: Vec<(String, String)> =
            if let LoadingState::Loaded(artists) = &self.library.artists {
                artists
                    .iter()
                    .map(|a| (a.id.clone(), a.name.clone()))
                    .collect()
            } else {
                Vec::new()
            };
        self.home.rediscover = self.history.rediscover_artists(15, &library_artist_pairs);

        if !album_strip_unchanged {
            self.home.selected_index = 0;
            self.home.active_section = HomeSection::RecentAlbums;
        } else {
            // Lists from history may have changed length while section stayed the same.
            match self.home.active_section {
                HomeSection::RecentTracks => {
                    if self.home.recent_tracks.is_empty() {
                        self.home.selected_index = 0;
                    } else {
                        let m = self.home.recent_tracks.len() - 1;
                        self.home.selected_index = self.home.selected_index.min(m);
                    }
                }
                HomeSection::Rediscover => {
                    if self.home.rediscover.is_empty() {
                        self.home.selected_index = 0;
                    } else {
                        let m = self.home.rediscover.len() - 1;
                        self.home.selected_index = self.home.selected_index.min(m);
                    }
                }
                HomeSection::TopArtists => {
                    if self.home.top_artists.is_empty() {
                        self.home.selected_index = 0;
                    } else {
                        let m = self.home.top_artists.len() - 1;
                        self.home.selected_index = self.home.selected_index.min(m);
                    }
                }
                HomeSection::RecentAlbums => {}
            }
        }

        // Kick off art fetches for any album not yet cached.
        self.spawn_pending_home_art_fetches();
    }

    /// Spawn home art fetch tasks for albums not yet cached or loading,
    /// up to a maximum of concurrent in-flight fetches (see `MAX_CONCURRENT` below).
    fn spawn_pending_home_art_fetches(&mut self) {
        const MAX_CONCURRENT: usize = 8;
        let album_ids: Vec<String> = self
            .home
            .recent_albums
            .iter()
            .map(|a| a.album_id.clone())
            .collect();
        let max_px = self.config.home_cover_fetch_max_px;
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
                let res = if max_px > 0 {
                    client.get_cover_art_sized(&album_id, max_px).await
                } else {
                    client.get_cover_art(&album_id).await
                };
                match res {
                    Ok(bytes) => {
                        let _ = tx.send(LibraryUpdate::HomeArt { album_id, bytes }).await;
                    }
                    Err(e) => {
                        eprintln!("home art fetch({album_id}): {e}");
                        let _ = tx
                            .send(LibraryUpdate::HomeArtFetchFailed { album_id })
                            .await;
                    }
                }
            });
        }
    }

    // ── Background fetch helpers ──────────────────────────────────────────────

    fn browse_files(&self) -> bool {
        self.browser_browse_mode == BrowseMode::Files
    }

    /// Spawn a task to fetch top-level music folders (file browse mode).
    pub fn fetch_music_folders(&self) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            let result = client.get_music_folders().await.map_err(|e| e.to_string());
            let _ = tx.send(LibraryUpdate::MusicFolders(result)).await;
        });
    }

    /// Spawn [`SubsonicClient::browse_folder_listing`] (indexes for library roots, directory API below).
    pub fn fetch_music_directory(&self, id: String) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            let result = client
                .browse_folder_listing(&id)
                .await
                .map(|dir| DirectoryListing {
                    name: dir.name,
                    directories: dir
                        .directories
                        .into_iter()
                        .map(|c| (c.id, c.title))
                        .collect(),
                    tracks: dir.songs,
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(LibraryUpdate::MusicDirectory { id, result }).await;
        });
    }

    /// Subsonic cache id for the folder shown in the preview pane from the current left-column selection.
    pub fn folder_preview_source_id(&self) -> Option<String> {
        if !self.browse_files() {
            return None;
        }
        if self.folders.path.is_empty() {
            let roots = match &self.folders.roots {
                LoadingState::Loaded(r) => r,
                _ => return None,
            };
            if roots.is_empty() {
                return None;
            }
            let indices: Vec<usize> =
                if let Some(q) = self.browser_column_filter(BrowserColumn::Artists) {
                    roots
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.name.to_lowercase().contains(q))
                        .map(|(i, _)| i)
                        .collect()
                } else {
                    (0..roots.len()).collect()
                };
            if indices.is_empty() {
                return None;
            }
            let pos = self
                .folders
                .selected_dir
                .unwrap_or(0)
                .min(indices.len() - 1);
            let slot = indices[pos];
            Some(ratune_subsonic::music_library_root_cache_key(
                &roots[slot].id,
            ))
        } else {
            let sel = self.folders.selected_dir.unwrap_or(0);
            if sel == 0 {
                // `..` is highlighted: preview **this** folder (tracks / dirs here), not the parent.
                return self.folders.current_dir_id().map(|id| id.to_string());
            }
            let listing = match self.folders.current_listing()? {
                LoadingState::Loaded(l) => l,
                _ => return None,
            };
            let dir_indices: Vec<usize> =
                if let Some(q) = self.browser_column_filter(BrowserColumn::Artists) {
                    listing
                        .directories
                        .iter()
                        .enumerate()
                        .filter(|(_, (_, name))| name.to_lowercase().contains(q))
                        .map(|(i, _)| i)
                        .collect()
                } else {
                    (0..listing.directories.len()).collect()
                };
            let dir_pos = sel.saturating_sub(1);
            if dir_pos >= dir_indices.len() {
                return None;
            }
            let orig_i = dir_indices[dir_pos];
            Some(listing.directories[orig_i].0.clone())
        }
    }

    pub fn sync_folder_preview_from_left(&mut self) {
        if !self.browse_files() {
            return;
        }
        let Some(pid) = self.folder_preview_source_id() else {
            self.folders.preview_dir_id = None;
            self.folders.preview_selected_row = 0;
            self.folders.tracks_scroll = 0;
            return;
        };
        if self.folders.preview_dir_id.as_deref() != Some(pid.as_str()) {
            self.folders.preview_dir_id = Some(pid.clone());
            self.folders.preview_selected_row = 0;
            self.folders.tracks_scroll = 0;
        }
        if !self.folders.listings.contains_key(&pid) {
            self.folders
                .listings
                .insert(pid.clone(), LoadingState::Loading);
            self.fetch_music_directory(pid);
        }
    }

    fn folder_display_name_for_cache_id(&self, cache_id: &str) -> Option<String> {
        if let Some(fid) = ratune_subsonic::parse_music_library_root_folder_id(cache_id) {
            if let LoadingState::Loaded(roots) = &self.folders.roots {
                if let Some(r) = roots.iter().find(|r| r.id == fid) {
                    return Some(r.name.clone());
                }
            }
        }
        if let Some(LoadingState::Loaded(li)) = self.folders.listings.get(cache_id) {
            let n = li.name.trim();
            if !n.is_empty() {
                return Some(n.to_string());
            }
        }
        None
    }

    fn ensure_left_at_preview_folder(&mut self, preview_key: &str) {
        let max_ops = self.folders.path.len().saturating_add(8).max(16);
        for _ in 0..max_ops {
            let cur = self.folders.current_dir_id().map(|s| s.to_string());
            if cur.as_deref() == Some(preview_key) {
                return;
            }
            match cur {
                None => {
                    let name = self
                        .folder_display_name_for_cache_id(preview_key)
                        .unwrap_or_else(|| "Library".into());
                    self.folder_enter(preview_key.to_string(), name);
                    return;
                }
                Some(cur_id) => {
                    if self.folders.path.len() >= 2 {
                        let parent_id = self.folders.path[self.folders.path.len() - 2].0.clone();
                        if parent_id == preview_key {
                            self.folder_go_up();
                            continue;
                        }
                    }
                    match self.folders.listings.get(&cur_id) {
                        Some(LoadingState::Loaded(listing)) => {
                            if let Some((_, name)) =
                                listing.directories.iter().find(|(id, _)| id == preview_key)
                            {
                                self.folder_enter(preview_key.to_string(), name.clone());
                                continue;
                            }
                        }
                        Some(LoadingState::Loading) => return,
                        Some(LoadingState::NotLoaded) | None => {
                            self.folders
                                .listings
                                .insert(cur_id.clone(), LoadingState::Loading);
                            self.fetch_music_directory(cur_id.clone());
                            return;
                        }
                        Some(LoadingState::Error(_)) => {}
                    }
                    if self.folders.path.len() > 1 {
                        self.folder_go_up();
                        continue;
                    }
                    let name = self
                        .folder_display_name_for_cache_id(preview_key)
                        .unwrap_or_else(|| "...".into());
                    self.folder_enter(preview_key.to_string(), name);
                    return;
                }
            }
        }
    }

    fn folder_enter_preview_child(&mut self, child_id: String, child_name: String) {
        let Some(ref preview_key) = self.folders.preview_dir_id.clone() else {
            return;
        };
        let pk = preview_key.clone();
        self.ensure_left_at_preview_folder(&pk);
        self.folder_enter(child_id, child_name);
    }

    /// Activate the highlighted preview row (enter subdirectory or enqueue track).
    pub fn folder_activate_preview_selection(&mut self) {
        let Some(ref preview_key) = self.folders.preview_dir_id.clone() else {
            return;
        };
        let listing = match self.folders.listings.get(preview_key) {
            Some(LoadingState::Loaded(l)) => l,
            _ => return,
        };
        let rows = folder_preview_rows(listing, self.browser_column_filter(BrowserColumn::Tracks));
        if rows.is_empty() {
            return;
        }
        let row_ix = self
            .folders
            .preview_selected_row
            .min(rows.len().saturating_sub(1));
        match rows[row_ix] {
            FolderPreviewRow::Dir(i) => {
                let (id, name) = listing.directories[i].clone();
                self.folder_enter_preview_child(id, name);
                if self.active_tab == Tab::Browser && self.browse_files() {
                    self.browser_focus = BrowserColumn::Artists;
                }
            }
            FolderPreviewRow::Track(i) => {
                let song = listing.tracks[i].clone();
                let was_empty = self.queue.songs.is_empty();
                self.queue.push(song);
                if was_empty {
                    self.queue.cursor = 0;
                    self.play_current();
                }
            }
        }
    }

    /// Tracks from the folder preview pane for queue bulk-add (`A` / Ctrl+r).
    fn folder_preview_songs_for_queue(&mut self) -> Option<Vec<ratune_subsonic::Song>> {
        self.sync_folder_preview_from_left();
        let id = match self.folders.preview_dir_id.clone() {
            Some(id) => id,
            None => {
                self.flash_status("Select a folder to preview");
                return None;
            }
        };
        let listing = match self.folders.listings.get(&id) {
            Some(LoadingState::Loaded(l)) => l,
            Some(LoadingState::Loading) => {
                self.flash_status_secs("Folder still loading…", 3);
                return None;
            }
            _ => {
                self.flash_status("No tracks in this folder");
                return None;
            }
        };
        let filter = self.browser_column_filter(BrowserColumn::Tracks);
        let mut songs: Vec<ratune_subsonic::Song> = if filter.is_some() {
            folder_preview_rows(listing, filter)
                .into_iter()
                .filter_map(|row| match row {
                    FolderPreviewRow::Track(i) => listing.tracks.get(i).cloned(),
                    FolderPreviewRow::Dir(_) => None,
                })
                .collect()
        } else {
            listing.tracks.clone()
        };
        if songs.is_empty() {
            self.flash_status(if filter.is_some() {
                "No matching tracks"
            } else {
                "No tracks in this folder"
            });
            return None;
        }
        songs.sort_by_key(|s| (s.disc_number.unwrap_or(1), s.track.unwrap_or(0)));
        Some(songs)
    }

    fn flash_queue_bulk_add(&mut self, n: usize, replaced: bool) {
        if replaced {
            if n == 1 {
                self.flash_status_secs("Replaced queue with 1 track", 3);
            } else {
                self.flash_status_secs(format!("Replaced queue with {n} tracks"), 3);
            }
        } else if n == 1 {
            self.flash_status_secs("Added 1 track to queue", 3);
        } else {
            self.flash_status_secs(format!("Added {n} tracks to queue"), 3);
        }
    }

    /// Open the **music folder row** using its API `id` (scoped via `getIndexes?musicFolderId=`).
    pub fn folder_enter_music_library_root(
        &mut self,
        music_folder_id: String,
        display_name: String,
    ) {
        let cache_id = ratune_subsonic::music_library_root_cache_key(&music_folder_id);
        self.folder_enter(cache_id, display_name);
    }

    pub fn folder_enter(&mut self, id: String, name: String) {
        self.folders.path.push((id.clone(), name));
        self.folders.dirs_scroll = 0;
        self.folders.tracks_scroll = 0;

        let mut need_fetch = false;
        match self.folders.listings.get(&id) {
            Some(LoadingState::Loaded(listing)) => {
                self.folders.folder_default_row_pending = false;
                let row = folder_left_default_row(
                    listing,
                    self.browser_column_filter(BrowserColumn::Artists),
                );
                self.folders.selected_dir = Some(row);
            }
            Some(LoadingState::Loading) => {
                self.folders.folder_default_row_pending = true;
                self.folders.selected_dir = Some(0);
            }
            Some(LoadingState::NotLoaded) | None => {
                self.folders.folder_default_row_pending = true;
                self.folders.selected_dir = Some(0);
                need_fetch = true;
            }
            Some(LoadingState::Error(_)) => {
                self.folders.folder_default_row_pending = true;
                self.folders.selected_dir = Some(0);
                need_fetch = true;
            }
        }
        if need_fetch {
            self.folders
                .listings
                .insert(id.clone(), LoadingState::Loading);
            self.fetch_music_directory(id);
        }
        self.sync_folder_preview_from_left();
    }

    pub fn folder_go_up(&mut self) {
        if !self.folders.path.pop().is_some() {
            return;
        }
        self.folders.dirs_scroll = 0;
        self.folders.tracks_scroll = 0;

        if self.folders.path.is_empty() {
            self.folders.folder_default_row_pending = false;
            self.sync_folder_preview_from_left();
            return;
        }

        let Some(cur) = self.folders.current_dir_id().map(|s| s.to_string()) else {
            self.folders.folder_default_row_pending = false;
            self.sync_folder_preview_from_left();
            return;
        };

        let mut need_fetch = false;
        match self.folders.listings.get(&cur) {
            Some(LoadingState::Loaded(listing)) => {
                self.folders.folder_default_row_pending = false;
                let row = folder_left_default_row(
                    listing,
                    self.browser_column_filter(BrowserColumn::Artists),
                );
                self.folders.selected_dir = Some(row);
            }
            Some(LoadingState::Loading) => {
                self.folders.folder_default_row_pending = true;
                self.folders.selected_dir = Some(0);
            }
            Some(LoadingState::NotLoaded) | None => {
                self.folders.folder_default_row_pending = true;
                self.folders.selected_dir = Some(0);
                need_fetch = true;
            }
            Some(LoadingState::Error(_)) => {
                self.folders.folder_default_row_pending = true;
                self.folders.selected_dir = Some(0);
                need_fetch = true;
            }
        }
        if need_fetch {
            self.folders
                .listings
                .insert(cur.clone(), LoadingState::Loading);
            self.fetch_music_directory(cur);
        }
        self.sync_folder_preview_from_left();
    }

    /// Enter the highlighted row in the folders column (root, parent, or subfolder).
    pub fn folder_activate_selected_dir(&mut self) {
        if self.folders.path.is_empty() {
            let roots = match &self.folders.roots {
                LoadingState::Loaded(r) => r,
                _ => return,
            };
            let indices: Vec<usize> =
                if let Some(q) = self.browser_column_filter(BrowserColumn::Artists) {
                    roots
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.name.to_lowercase().contains(q))
                        .map(|(i, _)| i)
                        .collect()
                } else {
                    (0..roots.len()).collect()
                };
            if indices.is_empty() {
                return;
            }
            let pos = self
                .folders
                .selected_dir
                .unwrap_or(0)
                .min(indices.len() - 1);
            let slot = indices[pos];
            let root = &roots[slot];
            self.folder_enter_music_library_root(root.id.clone(), root.name.clone());
            return;
        }

        let listing = match self.folders.current_listing() {
            Some(LoadingState::Loaded(l)) => l,
            _ => return,
        };
        let sel = self.folders.selected_dir.unwrap_or(0);
        if sel == 0 {
            self.folder_go_up();
            return;
        }
        let dir_indices: Vec<usize> =
            if let Some(q) = self.browser_column_filter(BrowserColumn::Artists) {
                listing
                    .directories
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, name))| name.to_lowercase().contains(q))
                    .map(|(i, _)| i)
                    .collect()
            } else {
                (0..listing.directories.len()).collect()
            };
        let dir_pos = sel.saturating_sub(1);
        if dir_pos >= dir_indices.len() {
            return;
        }
        let (id, name) = listing.directories[dir_indices[dir_pos]].clone();
        self.folder_enter(id, name);
    }

    /// Spawn a task to fetch the artist list.
    pub fn fetch_artists(&self) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            let result = ratune_subsonic::fetch_library(&client)
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
        if !force && !self.library_index_tracks.is_empty() {
            if let Some(refreshed) = self.library_index_refreshed_at {
                let stale = self.config.library_index_max_age_secs == 0
                    || now.saturating_sub(refreshed) > self.config.library_index_max_age_secs;
                if !stale {
                    return;
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
            let result: Result<(Vec<ratune_subsonic::Song>, Option<String>, bool), String> =
                async {
                    if nav_skip && !force {
                        if let Some(file) = crate::library_index::load(&index_path) {
                            if let Some(ref tok) = file.navidrome_last_scan {
                                if let Ok(status) = client.get_scan_status().await {
                                    if !status.scanning
                                        && status.last_scan.as_deref() == Some(tok.as_str())
                                    {
                                        return Ok((file.tracks.clone(), Some(tok.clone()), false));
                                    }
                                }
                            }
                        }
                    }
                    let opts = ratune_subsonic::FetchLibraryOptions {
                        album_parallelism: album_p,
                        artist_parallelism: artist_p,
                    };
                    let tracks =
                        ratune_subsonic::fetch_all_library_songs_with_options(&client, opts)
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
            let _ = tx
                .send(LibraryUpdate::LibraryIndexRefreshComplete { result })
                .await;
        });
    }

    /// Fetch the full library from the server (without requiring the on-disk index)
    /// and append it to the queue.
    pub fn spawn_library_server_append_queue(&mut self) {
        if self.library_server_append_fetching {
            self.flash_status_secs("Library fetch already in progress", 8);
            return;
        }
        self.library_server_append_fetching = true;
        self.library_server_append_started = Some(Instant::now());

        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        let album_p = self.config.library_fetch_album_parallelism;
        let artist_p = self.config.library_fetch_artist_parallelism;
        tokio::spawn(async move {
            let opts = ratune_subsonic::FetchLibraryOptions {
                album_parallelism: album_p,
                artist_parallelism: artist_p,
            };
            let result = ratune_subsonic::fetch_all_library_songs_with_options(&client, opts)
                .await
                .map_err(|e| e.to_string());
            let _ = tx
                .send(LibraryUpdate::LibraryServerAppendQueueComplete { result })
                .await;
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
    pub fn fetch_lyrics(&mut self, song_id: String, artist: String, title: String, album: String) {
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
            .map(|(_, lines)| !lines.is_empty() && lines.iter().all(|l| l.time.is_none()))
            .unwrap_or(false)
    }

    /// Spawn a task to fetch all tracks for an album, then replace the queue
    /// and start playing (or just append, depending on `replace`).
    pub fn fetch_and_replace_queue_with_album(&self, album_id: String, start_playing: bool) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.get_album(&album_id).await {
                Ok(album) => {
                    let mut songs = album.song;
                    songs.sort_by_key(|s| (s.disc_number.unwrap_or(1), s.track.unwrap_or(0)));
                    let _ = tx
                        .send(LibraryUpdate::AllTracksForArtist {
                            songs,
                            start_playing,
                            prepend: false,
                        })
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
                        .send(LibraryUpdate::AllTracksForArtist {
                            songs,
                            start_playing: false,
                            prepend: false,
                        })
                        .await;
                }
                Err(e) => eprintln!("fetch_and_append_album_to_queue({album_id}): {e}"),
            }
        });
    }

    /// Spawn a task that fetches every album + every track for the given artist,
    /// then delivers them as a flat sorted `AllTracksForArtist` update.
    pub fn fetch_all_tracks_for_artist(
        &self,
        artist_id: String,
        start_playing: bool,
        prepend: bool,
    ) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            let mut artist = match client.get_artist(&artist_id).await {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("fetch_all_tracks_for_artist({}): {e}", artist_id);
                    return;
                }
            };
            artist.album.sort_by(|a, b| match (a.year, b.year) {
                (Some(ya), Some(yb)) => ya.cmp(&yb).then_with(|| a.name.cmp(&b.name)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.name.cmp(&b.name),
            });
            let mut songs = Vec::new();
            for album in &artist.album {
                match client.get_album(&album.id).await {
                    Ok(a) => {
                        let mut s = a.song;
                        s.sort_by_key(|t| (t.disc_number.unwrap_or(1), t.track.unwrap_or(0)));
                        songs.extend(s);
                    }
                    Err(e) => eprintln!("get_album({}): {e}", album.id),
                }
            }
            let _ = tx
                .send(LibraryUpdate::AllTracksForArtist {
                    songs,
                    start_playing,
                    prepend,
                })
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
                            let idx = self.library.selected_artist.unwrap().min(artists.len() - 1);
                            self.library.selected_artist = Some(idx);
                            // Fetch albums for the selected (or restored) artist.
                            let artist_id = artists[idx].id.clone();
                            if !self.library.albums.contains_key(&artist_id) {
                                self.library
                                    .albums
                                    .insert(artist_id.clone(), LoadingState::Loading);
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
                let is_selected_artist = self
                    .library
                    .selected_artist
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
                                let idx =
                                    self.library.selected_album.unwrap().min(albums.len() - 1);
                                self.library.selected_album = Some(idx);
                                let album_id = albums[idx].id.clone();
                                if !self.library.tracks.contains_key(&album_id) {
                                    self.library
                                        .tracks
                                        .insert(album_id.clone(), LoadingState::Loading);
                                    self.fetch_tracks(album_id);
                                }
                            } else {
                                // Background prefetch: fetch tracks for first album.
                                if self.library.selected_album.is_none() {
                                    self.library.selected_album = Some(0);
                                }
                                let first_id = albums[0].id.clone();
                                if !self.library.tracks.contains_key(&first_id) {
                                    self.library
                                        .tracks
                                        .insert(first_id.clone(), LoadingState::Loading);
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
                let is_current_album = self
                    .library
                    .current_album()
                    .map(|a| a.id == album_id)
                    .unwrap_or(false);
                let loaded = match result {
                    Ok(songs) => {
                        if is_current_album && !songs.is_empty() {
                            // Clamp restored index (or default to 0) to actual song count.
                            if self.library.selected_track.is_none() {
                                self.library.selected_track = Some(0);
                            }
                            let idx = self.library.selected_track.unwrap().min(songs.len() - 1);
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
            LibraryUpdate::AllTracksForArtist {
                mut songs,
                start_playing,
                prepend,
            } => {
                let was_empty = self.queue.songs.is_empty();
                songs.sort_by_key(|s| {
                    (
                        s.year.unwrap_or(0),
                        s.album_id.clone().unwrap_or_default(),
                        s.disc_number.unwrap_or(1),
                        s.track.unwrap_or(0),
                    )
                });
                if prepend {
                    self.queue.prepend_songs(songs);
                } else {
                    for song in songs {
                        self.queue.push(song);
                    }
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
                let fp = crate::ui::art_prepare::art_bytes_fingerprint(&bytes);
                self.art_cache = Some((cover_id, bytes));
                self.art_cache_fingerprint = Some(fp);
                self.art_cache_decoded = None;
                self.apply_dynamic_accent(accent);
                #[cfg(target_os = "linux")]
                self.mpris_emit_props();
            }
            LibraryUpdate::Lyrics { song_id, lines } => {
                self.lyrics_loading = false;
                self.lyrics_cache = Some((song_id, lines));
                self.lyrics_scroll = 0;
            }
            LibraryUpdate::CacheTrack {
                song_id,
                album_id,
                bytes,
            } => {
                let _ = self.cache.put(&song_id, &album_id, &bytes);
            }
            LibraryUpdate::HomeArt { album_id, bytes } => {
                self.home_art_loading.remove(&album_id);
                self.home_strip_thumb_prepared.remove(&album_id);
                self.home_strip_art.remove(&album_id);
                self.home_strip_last_cells.remove(&album_id);
                self.home_art_cache.insert(album_id, bytes);
                // A fetch slot opened up — check if more albums need fetching.
                self.spawn_pending_home_art_fetches();
                if self.active_tab == Tab::Home {
                    if self.in_tmux {
                        // tmux: full strip re-transmit blinks; batch until idle or 500 ms fallback.
                        let elapsed = self
                            .home_art_last_tmux_render
                            .map(|t| t.elapsed().as_millis())
                            .unwrap_or(u128::MAX);
                        if self.home_art_loading.is_empty() || elapsed >= 500 {
                            self.home_art_needs_redraw = true;
                        }
                    } else {
                        // Outside tmux: redraw each arrival — decode/zlib is cached in
                        // `home_strip_thumb_prepared` so this is mostly base64 + Kitty I/O.
                        self.home_art_needs_redraw = true;
                    }
                }
            }
            LibraryUpdate::HomeArtFetchFailed { album_id } => {
                self.home_art_loading.remove(&album_id);
                self.spawn_pending_home_art_fetches();
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
            LibraryUpdate::PlaylistTrackRemoved {
                _playlist_id: _,
                index,
            } => {
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
            LibraryUpdate::MusicFolders(result) => {
                self.folders.roots = match result {
                    Ok(roots) => {
                        if self.folders.selected_dir.is_none() && !roots.is_empty() {
                            self.folders.selected_dir = Some(0);
                        }
                        if roots.len() == 1 && self.folders.path.is_empty() {
                            let id = roots[0].id.clone();
                            let name = roots[0].name.clone();
                            self.folder_enter_music_library_root(id, name);
                        }
                        LoadingState::Loaded(roots)
                    }
                    Err(e) => LoadingState::Error(e),
                };
                if self.browse_files() {
                    self.sync_folder_preview_from_left();
                }
            }
            LibraryUpdate::MusicDirectory { id, result } => {
                let loaded = match result {
                    Ok(listing) => LoadingState::Loaded(listing),
                    Err(e) => LoadingState::Error(e),
                };
                self.folders.listings.insert(id.clone(), loaded);

                match self.folders.listings.get(&id) {
                    Some(LoadingState::Loaded(listing)) => {
                        if self.browse_files() && self.folders.current_dir_id() == Some(id.as_str())
                        {
                            if self.folders.folder_default_row_pending {
                                let row = folder_left_default_row(
                                    listing,
                                    self.browser_column_filter(BrowserColumn::Artists),
                                );
                                self.folders.selected_dir = Some(row);
                                self.folders.folder_default_row_pending = false;
                            } else if self.folders.selected_dir.is_none() {
                                let row = folder_left_default_row(
                                    listing,
                                    self.browser_column_filter(BrowserColumn::Artists),
                                );
                                self.folders.selected_dir = Some(row);
                            }
                        }
                    }
                    Some(LoadingState::Error(_)) => {
                        if self.browse_files() && self.folders.current_dir_id() == Some(id.as_str())
                        {
                            self.folders.folder_default_row_pending = false;
                        }
                    }
                    _ => {}
                }

                if self.browse_files() {
                    self.sync_folder_preview_from_left();
                }

                if let Some(LoadingState::Loaded(listing)) = self.folders.listings.get(&id) {
                    if self.browse_files()
                        && self.folders.preview_dir_id.as_deref() == Some(id.as_str())
                    {
                        let rows = folder_preview_rows(
                            listing,
                            self.browser_column_filter(BrowserColumn::Tracks),
                        );
                        let max_r = rows.len().saturating_sub(1);
                        self.folders.preview_selected_row =
                            self.folders.preview_selected_row.min(max_r);
                    }
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
                        if let Err(e) = crate::library_index::save(&path, &tracks, now, scan_save) {
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
                        self.status_flash =
                            Some((msg.into(), Instant::now() + Duration::from_secs(2)));
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
            LibraryUpdate::LibraryServerAppendQueueComplete { result } => {
                self.library_server_append_fetching = false;
                self.library_server_append_started = None;
                match result {
                    Ok(tracks) => {
                        let n = tracks.len();
                        if n == 0 {
                            self.flash_status_secs("No tracks found in library", 5);
                            return;
                        }
                        let was_empty = self.queue.songs.is_empty();
                        for s in tracks {
                            self.queue.push(s);
                        }
                        if was_empty && !self.queue.songs.is_empty() {
                            self.queue.cursor = 0;
                            self.queue.scroll = 0;
                            self.play_current();
                        }
                        self.flash_status_secs(format!("Added {n} tracks to queue"), 3);
                    }
                    Err(e) => {
                        eprintln!("library fetch for append queue: {e}");
                        self.flash_status_secs(format!("Library fetch failed: {e}"), 6);
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
                        let needs_fetch = self
                            .art_cache
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
                    let cached_for_song = self
                        .lyrics_cache
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
                            to_download
                                .push((song.id.clone(), song.album_id.clone().unwrap_or_default()));
                        }
                        // Next 2 tracks.
                        let cursor = self.queue.cursor;
                        for offset in 1..=2usize {
                            let idx = cursor + offset;
                            if idx < self.queue.songs.len() {
                                let s_id = self.queue.songs[idx].id.clone();
                                let a_id =
                                    self.queue.songs[idx].album_id.clone().unwrap_or_default();
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
                                duration_secs: song.duration.map(|d| d as u64).unwrap_or(0),
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
                    let duration = next
                        .duration
                        .map(|s| std::time::Duration::from_secs(u64::from(s)));
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
                        let needs_fetch = self
                            .art_cache
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
                    let cached_for_song = self
                        .lyrics_cache
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
            SearchStart
                | SearchInput(_)
                | SearchBackspace
                | SearchConfirm
                | SearchCancel
                | HelpScrollUp
                | HelpScrollDown
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
                let _ = self.player_tx.send(PlayerCommand::SetVolume(
                    self.config.default_volume as f32 / 100.0,
                ));
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
            let duration = song
                .duration
                .map(|s| std::time::Duration::from_secs(u64::from(s)));
            let resolved = self.resolve_playback(&song);
            self.playback.current_song = Some(song);
            self.playback.player_loaded = true;
            let gen = self.play_gen;
            match resolved {
                ResolvedPlayback::Cached(path) => {
                    let _ = self.player_tx.send(PlayerCommand::PlayCached {
                        path,
                        duration,
                        gen,
                    });
                }
                ResolvedPlayback::Url(url) => {
                    let _ = self
                        .player_tx
                        .send(PlayerCommand::PlayUrl { url, duration, gen });
                }
            }
        }
    }

    /// Resolve a Subsonic stream URL or a finished on-disk cache file for `song`.
    fn resolve_playback(&mut self, song: &ratune_subsonic::Song) -> ResolvedPlayback {
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
        let song_id = song_id.to_string();
        let album_id = album_id.to_string();
        let tx = self.library_tx.clone();
        let gen_arc = self.prefetch_gen.clone();
        let expected = gen_arc.load(Ordering::Acquire);
        tokio::spawn(async move {
            if let Ok(resp) = reqwest::Client::new().get(&url).send().await {
                if let Ok(bytes) = resp.bytes().await {
                    if gen_arc.load(Ordering::Acquire) == expected {
                        let _ = tx
                            .send(LibraryUpdate::CacheTrack {
                                song_id,
                                album_id,
                                bytes: bytes.to_vec(),
                            })
                            .await;
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
                self.pending_gg = false;
                let was_visible = self.help_visible;
                self.help_visible = !self.help_visible;
                if !was_visible && self.help_visible {
                    self.help_scroll = 0;
                }
                if self.active_tab == Tab::Home {
                    if self.kitty_apc_overlay_active() {
                        if !was_visible && self.help_visible {
                            // Opening help — clear Kitty strip so it doesn't bleed through.
                            let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                        } else if was_visible && !self.help_visible {
                            // Closing help — post-draw strip redraw.
                            self.home_art_needs_redraw = true;
                        }
                    }
                    if self.ratatui_art_ready()
                        && !self.ratatui_uses_kitty_apc()
                        && !was_visible
                        && self.help_visible
                    {
                        self.clear_ratatui_art_state();
                    }
                }
            }
            Action::HelpScrollUp => {
                self.help_scroll = self.help_scroll.saturating_sub(1);
            }
            Action::HelpScrollDown => {
                self.help_scroll = self.help_scroll.saturating_add(1);
            }
            Action::LibraryIndexRefresh => {
                if self.pending_global_confirm.is_some() {
                    self.flash_status("Already confirming — press y or n");
                } else {
                    self.pending_global_confirm = Some(GlobalConfirm::LibraryIndexRefresh);
                    self.flash_status_secs("Full library index refresh? (y/n)", 12);
                }
            }
            Action::ConfirmLibraryIndexRefresh => {
                if self.pending_global_confirm == Some(GlobalConfirm::LibraryIndexRefresh) {
                    self.pending_global_confirm = None;
                    self.spawn_library_index_refresh(true);
                }
            }
            Action::LibraryIndexAppendQueue => {
                if self.pending_global_confirm.is_some() {
                    self.flash_status("Already confirming — press y or n");
                } else if self.library_index_refreshing {
                    self.flash_status_secs(
                        "Library index refresh in progress — try again shortly",
                        8,
                    );
                } else if self.library_server_append_fetching {
                    self.flash_status_secs("Library fetch already in progress", 8);
                } else if self.config.library_index_enabled && !self.library_index_tracks.is_empty()
                {
                    // Fast path: local metadata index already loaded.
                    let n = self.library_index_tracks.len();
                    self.pending_global_confirm = Some(GlobalConfirm::LibraryIndexAppendQueue);
                    self.flash_status_secs(
                        format!("Append all {n} indexed tracks to queue? (y/n)"),
                        12,
                    );
                } else {
                    // Slow path: no local index available; fetch full library from server.
                    self.pending_global_confirm = Some(GlobalConfirm::LibraryServerAppendQueue);
                    self.flash_status_secs(
                        "Fetch full library from server and append to queue? (y/n)",
                        12,
                    );
                }
            }
            Action::ConfirmLibraryIndexAppendQueue => {
                if self.pending_global_confirm == Some(GlobalConfirm::LibraryIndexAppendQueue) {
                    self.pending_global_confirm = None;
                    self.handle_confirm_library_index_append_queue();
                }
            }
            Action::ConfirmLibraryServerAppendQueue => {
                if self.pending_global_confirm == Some(GlobalConfirm::LibraryServerAppendQueue) {
                    self.pending_global_confirm = None;
                    self.spawn_library_server_append_queue();
                }
            }
            Action::CancelGlobalConfirm => {
                if self.pending_global_confirm.take().is_some() {
                    self.flash_status("Cancelled");
                }
            }
            Action::LibraryFzfPicker => {}
            Action::Quit => self.should_quit = true,
            Action::SwitchTab => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                self.clear_art_on_tab_switch();
                self.active_tab = self.active_tab.next();
                self.clear_browser_search();
                if self.active_tab == Tab::Home {
                    self.refresh_home_data();
                    self.home_art_needs_redraw = true;
                }
            }
            Action::SwitchTabReverse => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                self.clear_art_on_tab_switch();
                self.active_tab = self.active_tab.prev();
                self.clear_browser_search();
                if self.active_tab == Tab::Home {
                    self.refresh_home_data();
                    self.home_art_needs_redraw = true;
                }
            }
            Action::GoToHome => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                if self.kitty_apc_overlay_active() {
                    let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                }
                self.active_tab = Tab::Home;
                self.clear_browser_search();
                self.refresh_home_data();
                self.home_art_needs_redraw = true;
            }
            Action::GoToBrowser => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                self.clear_art_on_tab_switch();
                self.active_tab = Tab::Browser;
                self.clear_browser_search();
                self.apply_pending_artist_select();
            }
            Action::ToggleBrowserFolder => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                self.clear_art_on_tab_switch();
                self.pending_gg = false;
                if !self.config.browse_folder_navigation {
                    self.flash_status_secs(
                        "Folder browse disabled — set [ui.browsetab] folder_navigation = true",
                        5,
                    );
                } else if self.browser_browse_mode == BrowseMode::Genre {
                    self.flash_status("Genre browse is not implemented (change mode in config)");
                } else {
                    self.browser_browse_mode = match self.browser_browse_mode {
                        BrowseMode::Files => BrowseMode::Artists,
                        _ => BrowseMode::Files,
                    };
                    self.active_tab = Tab::Browser;
                    self.clear_browser_search();
                    if self.browser_browse_mode == BrowseMode::Files {
                        if matches!(
                            &self.folders.roots,
                            LoadingState::NotLoaded | LoadingState::Error(_)
                        ) {
                            self.fetch_music_folders();
                        }
                        self.sync_folder_preview_from_left();
                    } else if matches!(
                        &self.library.artists,
                        LoadingState::NotLoaded | LoadingState::Error(_)
                    ) {
                        // Cold start in `files` mode never called `fetch_artists`; load when toggling here.
                        self.fetch_artists();
                    }
                    self.flash_status(match self.browser_browse_mode {
                        BrowseMode::Files => "Browse: folders",
                        _ => "Browse: artists / albums / tracks",
                    });
                }
            }
            Action::GoToNowPlaying => {
                self.playlist_overlay.visible = false;
                self.playlist_picker = None;
                self.clear_art_on_tab_switch();
                self.active_tab = Tab::NowPlaying;
                self.clear_browser_search();
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
                        Direction::PageUp => {
                            self.lyrics_scroll = self.lyrics_scroll.saturating_sub(16);
                        }
                        Direction::PageDown => {
                            self.lyrics_scroll = self.lyrics_scroll.saturating_add(16);
                        }
                    }
                } else {
                    self.handle_navigate(dir);
                }
            }
            Action::Select => self.handle_select(),
            Action::Back => self.handle_focus_left(),
            Action::AddToQueue => self.handle_add_to_queue(),
            Action::AddAllToQueue => self.handle_add_all_to_queue(AddAllMode::Append),
            Action::AddAllToQueueReplaceAlbum => {
                self.handle_add_all_to_queue(AddAllMode::ReplaceAlbum)
            }
            Action::AddAllToQueueReplaceArtist => {
                self.handle_add_all_to_queue(AddAllMode::ReplaceArtist)
            }
            Action::AddAllToQueuePrepend => self.handle_add_all_to_queue(AddAllMode::Prepend),
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
                let _ = self.player_tx.send(PlayerCommand::SetVolume(
                    self.config.default_volume as f32 / 100.0,
                ));
            }
            Action::VolumeDown => {
                self.config.default_volume = self.config.default_volume.saturating_sub(5);
                let _ = self.player_tx.send(PlayerCommand::SetVolume(
                    self.config.default_volume as f32 / 100.0,
                ));
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
                let new_pos = self
                    .playback
                    .elapsed
                    .saturating_sub(std::time::Duration::from_secs(10));
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
                self.pending_gg = false;
                self.search_filter_before_edit =
                    match (&self.search_filter, self.search_filter_column) {
                        (Some(q), Some(col)) => Some((col, q.clone())),
                        _ => None,
                    };
                self.search_mode.active = true;
                self.search_mode.query.clear();
                self.search_mode.selected = 0;
                // Starting a new search clears the previous filter while typing.
                self.clear_browser_search();
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
                        self.clear_browser_search();
                    } else {
                        self.search_filter = Some(q);
                        self.search_filter_column = Some(self.browser_focus);
                    }
                    self.handle_search_confirm();
                    self.search_mode.active = false;
                    self.search_mode.query.clear();
                    self.search_filter_before_edit = None;
                }
            }
            Action::SearchCancel => {
                self.pending_gg = false;
                self.search_mode.active = false;
                self.search_mode.query.clear();
                self.search_mode.selected = 0;
                if let Some((col, q)) = self.search_filter_before_edit.take() {
                    self.search_filter = Some(q);
                    self.search_filter_column = Some(col);
                } else {
                    let had_filter = self.search_filter.is_some();
                    self.clear_browser_search();
                    if had_filter {
                        self.flash_status_secs("Search cleared", 2);
                    }
                }
            }
            Action::ToggleLyrics => {
                // Only active on NowPlaying tab; silently ignored on Browser.
                if self.active_tab == Tab::NowPlaying && self.config.lyrics_enabled {
                    self.lyrics_visible = !self.lyrics_visible;
                    // Trigger a lyrics fetch if we just enabled the overlay and
                    // nothing is cached for the current song yet.
                    if self.lyrics_visible {
                        if let Some(song) = self.playback.current_song.clone() {
                            let cached = self
                                .lyrics_cache
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
                if self.active_tab == Tab::NowPlaying && self.config.visualizer_enabled {
                    self.visualizer_visible = !self.visualizer_visible;
                    if !self.visualizer_visible {
                        self.spectrum_bands = vec![0.0; 32];
                        self.waveform.clear();
                        self.visualizer_last_tick = None;
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
                        if !self.config.home_recent_albums_show_art {
                            // No-art list mode: h = up.
                            self.home.album_selected_index =
                                self.home.album_selected_index.saturating_sub(1);
                            if self.home.album_selected_index < self.home.album_scroll_offset {
                                self.home.album_scroll_offset = self.home.album_selected_index;
                            }
                            return;
                        }
                        if self.home.recent_albums.is_empty() {
                            return;
                        }
                        let max_idx = self.home.recent_albums.len().saturating_sub(1);
                        let per_row = self
                            .home_recent_albums_inner
                            .map(|inner| {
                                crate::ui::kitty_art::art_strip_layout(inner.width, inner.height)
                                    .per_row
                            })
                            .unwrap_or(1)
                            .max(1);

                        // Only move within the current visible row (no wrapping).
                        let rel = self
                            .home
                            .album_selected_index
                            .saturating_sub(self.home.album_scroll_offset);
                        let col = rel % per_row;
                        if col == 0 {
                            return;
                        }
                        self.home.album_selected_index =
                            self.home.album_selected_index.saturating_sub(1);

                        // Ensure still visible (row-step scroll).
                        if self.home.album_selected_index < self.home.album_scroll_offset {
                            let diff =
                                self.home.album_scroll_offset - self.home.album_selected_index;
                            let rows = diff.div_ceil(per_row);
                            self.home.album_scroll_offset =
                                self.home.album_scroll_offset.saturating_sub(rows * per_row);
                        }
                        self.home.album_scroll_offset = self.home.album_scroll_offset.min(max_idx);

                        if !self.in_tmux {
                            self.home_art_needs_redraw = true;
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
                        if !self.config.home_recent_albums_show_art {
                            // No-art list mode: l = down.
                            let max_idx = self.home.recent_albums.len().saturating_sub(1);
                            self.home.album_selected_index =
                                (self.home.album_selected_index + 1).min(max_idx);
                            let visible_rows = self
                                .home_recent_albums_inner
                                .map(|r| r.height as usize)
                                .unwrap_or(8)
                                .max(1);
                            let scroll_end =
                                self.home.album_scroll_offset + visible_rows.saturating_sub(1);
                            if self.home.album_selected_index > scroll_end {
                                self.home.album_scroll_offset =
                                    self.home.album_scroll_offset.saturating_add(1);
                            }
                            return;
                        }
                        let max_idx = self.home.recent_albums.len().saturating_sub(1);
                        if self.home.recent_albums.is_empty() {
                            return;
                        }
                        let per_row = self
                            .home_recent_albums_inner
                            .map(|inner| {
                                crate::ui::kitty_art::art_strip_layout(inner.width, inner.height)
                                    .per_row
                            })
                            .unwrap_or(1)
                            .max(1);
                        let visible_count = self.home_album_strip_visible_count().max(1);

                        let rel = self
                            .home
                            .album_selected_index
                            .saturating_sub(self.home.album_scroll_offset);
                        let row = rel / per_row;
                        let col = rel % per_row;
                        let row_end_rel = row * per_row + (per_row - 1);
                        // Can't move right past the row end or past data end.
                        if col + 1 >= per_row || self.home.album_selected_index >= max_idx {
                            return;
                        }
                        // Also avoid stepping into a non-visible slot in a short last row.
                        let candidate_rel = rel + 1;
                        if candidate_rel > row_end_rel {
                            return;
                        }

                        self.home.album_selected_index += 1;

                        // Ensure still visible (row-step scroll).
                        let end = self.home.album_scroll_offset + visible_count.saturating_sub(1);
                        if self.home.album_selected_index > end {
                            let diff = self.home.album_selected_index - end;
                            let rows = diff.div_ceil(per_row);
                            self.home.album_scroll_offset += rows * per_row;
                        }
                        self.home.album_scroll_offset = self.home.album_scroll_offset.min(max_idx);

                        if !self.in_tmux {
                            self.home_art_needs_redraw = true;
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
                if matches!(self.theme.preset, crate::config::ThemePreset::Terminal) {
                    // In terminal/OS mode we inherit colors from the terminal; don't override.
                    return;
                }
                if self.theme.dynamic {
                    // Disable: instant snap back to static accent.
                    self.theme.dynamic = false;
                    self.accent_current = self.theme.accent;
                    self.accent_target = self.theme.accent;
                    self.accent_transition_start = None;
                } else {
                    // Enable: start transition from current to dynamic accent (if any).
                    self.theme.dynamic = true;
                    let target = self.dynamic_accent.unwrap_or(self.theme.accent);
                    self.accent_lerp_from = self.accent_current;
                    self.accent_target = target;
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
                        self.library
                            .albums
                            .insert(artist_id.clone(), LoadingState::Loading);
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
        if self.browse_files() {
            match self.browser_focus {
                BrowserColumn::Artists => {
                    self.browser_focus = BrowserColumn::Tracks;
                    self.sync_folder_preview_from_left();
                }
                BrowserColumn::Albums | BrowserColumn::Tracks => {}
            }
            return;
        }
        match self.browser_focus {
            BrowserColumn::Artists => {
                if let Some(artist) = self.library.current_artist() {
                    let artist_id = artist.id.clone();
                    if !self.library.albums.contains_key(&artist_id) {
                        self.library
                            .albums
                            .insert(artist_id.clone(), LoadingState::Loading);
                        self.fetch_albums(artist_id);
                    }
                }
                self.browser_focus = BrowserColumn::Albums;
            }
            BrowserColumn::Albums => {
                if let Some(album) = self.library.current_album() {
                    let album_id = album.id.clone();
                    if !self.library.tracks.contains_key(&album_id) {
                        self.library
                            .tracks
                            .insert(album_id.clone(), LoadingState::Loading);
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
        if self.browse_files() {
            match self.browser_focus {
                BrowserColumn::Tracks => {
                    self.browser_focus = BrowserColumn::Artists;
                    self.sync_folder_preview_from_left();
                }
                BrowserColumn::Artists | BrowserColumn::Albums => {}
            }
            return;
        }
        self.browser_focus = self.browser_focus.left();
    }

    // ── Navigation ────────────────────────────────────────────────────────────

    fn handle_navigate(&mut self, dir: Direction) {
        match self.active_tab {
            Tab::Home => self.handle_navigate_home(dir),
            Tab::Browser => self.handle_navigate_browser(dir),
            Tab::NowPlaying => self.handle_navigate_queue(dir),
        }
    }

    fn handle_navigate_home(&mut self, dir: Direction) {
        if self.home.active_section == HomeSection::RecentAlbums {
            if !self.config.home_recent_albums_show_art {
                // No-art list mode: j/k navigate the album list vertically.
                if self.home.recent_albums.is_empty() {
                    return;
                }
                let max_idx = self.home.recent_albums.len().saturating_sub(1);
                let visible_rows = self
                    .home_recent_albums_inner
                    .map(|r| r.height as usize)
                    .unwrap_or(8)
                    .max(1);
                match dir {
                    Direction::Up | Direction::Top => {
                        self.home.album_selected_index =
                            self.home.album_selected_index.saturating_sub(1);
                        if self.home.album_selected_index < self.home.album_scroll_offset {
                            self.home.album_scroll_offset = self.home.album_selected_index;
                        }
                    }
                    Direction::Down | Direction::Bottom => {
                        self.home.album_selected_index =
                            (self.home.album_selected_index + 1).min(max_idx);
                        let scroll_end =
                            self.home.album_scroll_offset + visible_rows.saturating_sub(1);
                        if self.home.album_selected_index > scroll_end {
                            self.home.album_scroll_offset =
                                self.home.album_scroll_offset.saturating_add(1);
                        }
                    }
                    Direction::PageUp => {
                        self.home.album_selected_index =
                            self.home.album_selected_index.saturating_sub(visible_rows);
                        self.home.album_scroll_offset = self
                            .home
                            .album_scroll_offset
                            .min(self.home.album_selected_index);
                    }
                    Direction::PageDown => {
                        self.home.album_selected_index =
                            (self.home.album_selected_index + visible_rows).min(max_idx);
                        let scroll_end =
                            self.home.album_scroll_offset + visible_rows.saturating_sub(1);
                        if self.home.album_selected_index > scroll_end {
                            self.home.album_scroll_offset = self
                                .home
                                .album_selected_index
                                .saturating_sub(visible_rows.saturating_sub(1));
                        }
                    }
                }
                return;
            }

            // Art-strip mode: 2D-ish navigation on the visible grid.
            // - j/k: move up/down one thumbnail row (±per_row)
            // - h/l: move left/right one column (handled by HomeAlbumLeft/Right)
            let max_idx = self.home.recent_albums.len().saturating_sub(1);
            if self.home.recent_albums.is_empty() {
                return;
            }
            let per_row = self
                .home_recent_albums_inner
                .map(|inner| {
                    crate::ui::kitty_art::art_strip_layout(inner.width, inner.height).per_row
                })
                .unwrap_or(1)
                .max(1);
            let visible_count = self.home_album_strip_visible_count().max(1);

            let step = match dir {
                Direction::PageUp | Direction::PageDown => visible_count,
                _ => per_row,
            };

            let mut new_sel = self.home.album_selected_index;
            match dir {
                Direction::Up | Direction::Top | Direction::PageUp => {
                    new_sel = new_sel.saturating_sub(step);
                }
                Direction::Down | Direction::Bottom | Direction::PageDown => {
                    new_sel = (new_sel + step).min(max_idx);
                }
            }
            self.home.album_selected_index = new_sel;

            // Keep selection visible by adjusting scroll in row-sized steps.
            let vis = visible_count;
            let mut off = self.home.album_scroll_offset;
            if self.home.album_selected_index < off {
                let diff = off - self.home.album_selected_index;
                let rows = diff.div_ceil(per_row);
                off = off.saturating_sub(rows * per_row);
            } else {
                let end = off + vis.saturating_sub(1);
                if self.home.album_selected_index > end {
                    let diff = self.home.album_selected_index - end;
                    let rows = diff.div_ceil(per_row);
                    off = off.saturating_add(rows * per_row);
                }
            }
            self.home.album_scroll_offset = off.min(max_idx);

            if !self.in_tmux {
                self.home_art_needs_redraw = true;
            }
            return;
        }
        let section_len = match self.home.active_section {
            HomeSection::RecentAlbums => 0,
            HomeSection::RecentTracks => self.home.recent_tracks.len(),
            HomeSection::TopArtists => self.home.top_artists.len(),
            HomeSection::Rediscover => self.home.rediscover.len(),
        };
        if section_len == 0 {
            return;
        }
        const HOME_PAGE: usize = 8;
        self.home.selected_index = match dir {
            Direction::Up | Direction::Top => self.home.selected_index.saturating_sub(1),
            Direction::Down | Direction::Bottom => {
                (self.home.selected_index + 1).min(section_len - 1)
            }
            Direction::PageUp => self.home.selected_index.saturating_sub(HOME_PAGE),
            Direction::PageDown => (self.home.selected_index + HOME_PAGE).min(section_len - 1),
        };
    }

    fn handle_navigate_browser(&mut self, dir: Direction) {
        if self.browse_files() {
            self.handle_navigate_browser_files(dir);
            return;
        }
        match self.browser_focus {
            BrowserColumn::Artists => {
                let result = if let LoadingState::Loaded(artists) = &self.library.artists {
                    // Build navigable index set — filtered or full.
                    let indices: Vec<usize> =
                        if let Some(q) = self.browser_column_filter(BrowserColumn::Artists) {
                            artists
                                .iter()
                                .enumerate()
                                .filter(|(_, a)| a.name.to_lowercase().contains(q))
                                .map(|(i, _)| i)
                                .collect()
                        } else {
                            (0..artists.len()).collect()
                        };
                    if indices.is_empty() {
                        return;
                    }
                    let cur_pos = self
                        .library
                        .selected_artist
                        .and_then(|sel| indices.iter().position(|&i| i == sel))
                        .unwrap_or(0);
                    let page = self.browser_list_viewport_rows.max(1);
                    let new_pos = match dir {
                        Direction::Up => cur_pos.saturating_sub(1),
                        Direction::Down => (cur_pos + 1).min(indices.len() - 1),
                        Direction::Top => 0,
                        Direction::Bottom => indices.len() - 1,
                        Direction::PageUp => cur_pos.saturating_sub(page),
                        Direction::PageDown => (cur_pos + page).min(indices.len() - 1),
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
                        self.library
                            .albums
                            .insert(artist_id.clone(), LoadingState::Loading);
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
                    if let Some(LoadingState::Loaded(albums)) = self.library.albums.get(&artist_id)
                    {
                        let indices: Vec<usize> =
                            if let Some(q) = self.browser_column_filter(BrowserColumn::Albums) {
                                albums
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, a)| a.name.to_lowercase().contains(q))
                                    .map(|(i, _)| i)
                                    .collect()
                            } else {
                                (0..albums.len()).collect()
                            };
                        if indices.is_empty() {
                            return;
                        }
                        let cur_pos = self
                            .library
                            .selected_album
                            .and_then(|sel| indices.iter().position(|&i| i == sel))
                            .unwrap_or(0);
                        let page = self.browser_list_viewport_rows.max(1);
                        let new_pos = match dir {
                            Direction::Up => cur_pos.saturating_sub(1),
                            Direction::Down => (cur_pos + 1).min(indices.len() - 1),
                            Direction::Top => 0,
                            Direction::Bottom => indices.len() - 1,
                            Direction::PageUp => cur_pos.saturating_sub(page),
                            Direction::PageDown => (cur_pos + page).min(indices.len() - 1),
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
                        self.library
                            .tracks
                            .insert(album_id.clone(), LoadingState::Loading);
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
                    let indices: Vec<usize> =
                        if let Some(q) = self.browser_column_filter(BrowserColumn::Tracks) {
                            songs
                                .iter()
                                .enumerate()
                                .filter(|(_, s)| s.title.to_lowercase().contains(q))
                                .map(|(i, _)| i)
                                .collect()
                        } else {
                            (0..songs.len()).collect()
                        };
                    if indices.is_empty() {
                        return;
                    }
                    let cur_pos = self
                        .library
                        .selected_track
                        .and_then(|sel| indices.iter().position(|&i| i == sel))
                        .unwrap_or(0);
                    let page = self.browser_list_viewport_rows.max(1);
                    let new_pos = match dir {
                        Direction::Up => cur_pos.saturating_sub(1),
                        Direction::Down => (cur_pos + 1).min(indices.len() - 1),
                        Direction::Top => 0,
                        Direction::Bottom => indices.len() - 1,
                        Direction::PageUp => cur_pos.saturating_sub(page),
                        Direction::PageDown => (cur_pos + page).min(indices.len() - 1),
                    };
                    self.library.selected_track = Some(indices[new_pos]);
                }
            }
        }
    }

    fn handle_navigate_browser_files(&mut self, dir: Direction) {
        let page = self.browser_list_viewport_rows.max(1);
        match self.browser_focus {
            BrowserColumn::Artists | BrowserColumn::Albums => {
                let len = if self.folders.path.is_empty() {
                    match &self.folders.roots {
                        LoadingState::Loaded(roots) => {
                            if let Some(q) = self.browser_column_filter(BrowserColumn::Artists) {
                                roots
                                    .iter()
                                    .filter(|r| r.name.to_lowercase().contains(q))
                                    .count()
                            } else {
                                roots.len()
                            }
                        }
                        _ => return,
                    }
                } else {
                    match self.folders.current_listing() {
                        Some(LoadingState::Loaded(listing)) => {
                            let sub = if let Some(q) =
                                self.browser_column_filter(BrowserColumn::Artists)
                            {
                                listing
                                    .directories
                                    .iter()
                                    .filter(|(_, name)| name.to_lowercase().contains(q))
                                    .count()
                            } else {
                                listing.directories.len()
                            };
                            1 + sub
                        }
                        _ => return,
                    }
                };
                if len == 0 {
                    return;
                }
                let cur = self.folders.selected_dir.unwrap_or(0).min(len - 1);
                let new_pos = match dir {
                    Direction::Up => cur.saturating_sub(1),
                    Direction::Down => (cur + 1).min(len - 1),
                    Direction::Top => 0,
                    Direction::Bottom => len - 1,
                    Direction::PageUp => cur.saturating_sub(page),
                    Direction::PageDown => (cur + page).min(len - 1),
                };
                self.folders.selected_dir = Some(new_pos);
                self.folders.folder_default_row_pending = false;
                self.sync_folder_preview_from_left();
            }
            BrowserColumn::Tracks => {
                let Some(ref pid) = self.folders.preview_dir_id.clone() else {
                    return;
                };
                if let Some(LoadingState::Loaded(listing)) = self.folders.listings.get(pid) {
                    let rows = folder_preview_rows(
                        listing,
                        self.browser_column_filter(BrowserColumn::Tracks),
                    );
                    if rows.is_empty() {
                        return;
                    }
                    let cur_pos = self.folders.preview_selected_row.min(rows.len() - 1);
                    let new_pos = match dir {
                        Direction::Up => cur_pos.saturating_sub(1),
                        Direction::Down => (cur_pos + 1).min(rows.len() - 1),
                        Direction::Top => 0,
                        Direction::Bottom => rows.len() - 1,
                        Direction::PageUp => cur_pos.saturating_sub(page),
                        Direction::PageDown => (cur_pos + page).min(rows.len() - 1),
                    };
                    self.folders.preview_selected_row = new_pos;
                }
            }
        }
    }

    fn handle_navigate_queue(&mut self, dir: Direction) {
        let len = self.queue.songs.len();
        if len == 0 {
            return;
        }
        let page = self.queue_viewport_rows.max(1);
        self.queue.cursor = match dir {
            Direction::Up => self.queue.cursor.saturating_sub(1),
            Direction::Down => (self.queue.cursor + 1).min(len - 1),
            Direction::Top => 0,
            Direction::Bottom => len - 1,
            Direction::PageUp => self.queue.cursor.saturating_sub(page),
            Direction::PageDown => (self.queue.cursor + page).min(len - 1),
        };
    }

    // ── Select ────────────────────────────────────────────────────────────────

    fn handle_select(&mut self) {
        match self.active_tab {
            Tab::Home => self.handle_select_home(),
            Tab::Browser => {
                if self.browse_files() {
                    match self.browser_focus {
                        BrowserColumn::Artists | BrowserColumn::Albums => {
                            self.folder_activate_selected_dir()
                        }
                        BrowserColumn::Tracks => self.folder_activate_preview_selection(),
                    }
                } else {
                    match self.browser_focus {
                        BrowserColumn::Artists | BrowserColumn::Albums => self.handle_focus_right(),
                        BrowserColumn::Tracks => self.handle_add_to_queue(),
                    }
                }
            }
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
                    if self.kitty_apc_overlay_active() {
                        let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                        let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                    }
                    if self.ratatui_art_ready() && !self.ratatui_uses_kitty_apc() {
                        self.clear_ratatui_art_state();
                    }
                    self.pending_artist_select = Some(artist_name);
                    self.active_tab = Tab::Browser;
                    self.clear_browser_search();
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
                    let song = ratune_subsonic::Song {
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
                            let _ = self
                                .player_tx
                                .send(ratune_player::PlayerCommand::PlayCached {
                                    path,
                                    duration: dur,
                                    gen,
                                });
                        }
                        ResolvedPlayback::Url(url) => {
                            let _ = self.player_tx.send(ratune_player::PlayerCommand::PlayUrl {
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
                if self.kitty_apc_overlay_active() {
                    let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                    let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                }
                if self.ratatui_art_ready() && !self.ratatui_uses_kitty_apc() {
                    self.clear_ratatui_art_state();
                }
                self.active_tab = Tab::Browser;
                self.clear_browser_search();
            }
            HomeSection::Rediscover => {
                // Pre-select the chosen artist in the Browser, then switch.
                if let Some((_, artist_name)) = self.home.rediscover.get(self.home.selected_index) {
                    self.pending_artist_select = Some(artist_name.clone());
                }
                if self.kitty_apc_overlay_active() {
                    let _ = crate::ui::kitty_art::clear_image(self.in_tmux);
                    let _ = crate::ui::kitty_art::clear_art_strip(self.in_tmux);
                }
                if self.ratatui_art_ready() && !self.ratatui_uses_kitty_apc() {
                    self.clear_ratatui_art_state();
                }
                self.active_tab = Tab::Browser;
                self.apply_pending_artist_select();
                self.clear_browser_search();
            }
        }
    }

    // ── Queue helpers ─────────────────────────────────────────────────────────

    fn handle_add_to_queue(&mut self) {
        if self.browse_files() {
            if let Some(song) = self
                .folders
                .current_preview_track(self.browser_column_filter(BrowserColumn::Tracks))
                .cloned()
            {
                let was_empty = self.queue.songs.is_empty();
                self.queue.push(song);
                if was_empty {
                    self.queue.cursor = 0;
                    self.play_current();
                }
            }
            return;
        }
        if let Some(song) = self.library.current_track().cloned() {
            let was_empty = self.queue.songs.is_empty();
            self.queue.push(song);
            if was_empty {
                self.queue.cursor = 0;
                self.play_current();
            }
        }
    }

    /// Append every track from the on-disk metadata index to the queue (after y/n confirm).
    fn handle_confirm_library_index_append_queue(&mut self) {
        if !self.config.library_index_enabled {
            self.flash_status("Library index is disabled in config");
            return;
        }
        if self.library_index_tracks.is_empty() {
            self.flash_status("Library index empty");
            return;
        }
        let n = self.library_index_tracks.len();
        let was_empty = self.queue.songs.is_empty();
        for song in self.library_index_tracks.iter().cloned() {
            self.queue.push(song);
        }
        if was_empty && !self.queue.songs.is_empty() {
            self.queue.cursor = 0;
            self.queue.scroll = 0;
            self.play_current();
        }
        if n == 1 {
            self.flash_status_secs("Added 1 track to queue", 3);
        } else {
            self.flash_status_secs(format!("Added {n} tracks to queue"), 3);
        }
    }

    fn handle_add_all_to_queue(&mut self, mode: AddAllMode) {
        if self.browse_files() {
            let Some(songs) = self.folder_preview_songs_for_queue() else {
                return;
            };
            let n = songs.len();
            let prepend = matches!(mode, AddAllMode::Prepend);
            match mode {
                AddAllMode::ReplaceAlbum | AddAllMode::ReplaceArtist => {
                    self.queue.songs = songs;
                    self.queue.cursor = 0;
                    self.queue.scroll = 0;
                    self.queue.pre_shuffle_order = None;
                    let _ = self.player_tx.send(PlayerCommand::Stop);
                    self.playback.current_song = None;
                    self.playback.elapsed = std::time::Duration::ZERO;
                    self.playback.paused = false;
                    self.playback.player_loaded = false;
                    if !self.queue.songs.is_empty() {
                        self.play_current();
                    }
                    self.flash_queue_bulk_add(n, true);
                }
                AddAllMode::Append | AddAllMode::Prepend => {
                    let was_empty = self.queue.songs.is_empty();
                    if prepend {
                        self.queue.prepend_songs(songs);
                    } else {
                        for song in songs {
                            self.queue.push(song);
                        }
                    }
                    if was_empty && !self.queue.songs.is_empty() {
                        self.queue.cursor = 0;
                        self.play_current();
                    }
                    self.flash_queue_bulk_add(n, false);
                }
            }
            return;
        }
        match mode {
            AddAllMode::ReplaceAlbum => self.handle_replace_queue_with_current_album(),
            AddAllMode::ReplaceArtist => self.handle_replace_queue_with_current_artist(),
            AddAllMode::Append | AddAllMode::Prepend => {
                let prepend = matches!(mode, AddAllMode::Prepend);
                match self.browser_focus {
                    BrowserColumn::Artists | BrowserColumn::Albums => {
                        if let Some(artist) = self.library.current_artist() {
                            let artist_id = artist.id.clone();
                            let start_playing = self.queue.songs.is_empty();
                            self.fetch_all_tracks_for_artist(artist_id, start_playing, prepend);
                        }
                    }
                    BrowserColumn::Tracks => {
                        let album_id = match self.library.current_album() {
                            Some(a) => a.id.clone(),
                            None => return,
                        };
                        if let Some(LoadingState::Loaded(songs)) =
                            self.library.tracks.get(&album_id)
                        {
                            let mut sorted = songs.clone();
                            sorted.sort_by_key(|s| {
                                (s.disc_number.unwrap_or(1), s.track.unwrap_or(0))
                            });
                            let was_empty = self.queue.songs.is_empty();
                            if prepend {
                                self.queue.prepend_songs(sorted);
                            } else {
                                for song in sorted {
                                    self.queue.push(song);
                                }
                            }
                            if was_empty && !self.queue.songs.is_empty() {
                                self.queue.cursor = 0;
                                self.play_current();
                            }
                        }
                    }
                }
            }
        }
    }

    /// Replace the queue with the current album's tracks (from cache), or start a fetch.
    fn handle_replace_queue_with_current_album(&mut self) {
        let album_id = match self.library.current_album() {
            Some(a) => a.id.clone(),
            None => {
                self.flash_status("Select an album");
                return;
            }
        };
        if let Some(LoadingState::Loaded(songs)) = self.library.tracks.get(&album_id) {
            let mut sorted = songs.clone();
            sorted.sort_by_key(|s| (s.disc_number.unwrap_or(1), s.track.unwrap_or(0)));
            self.handle_clear_queue();
            for song in sorted {
                self.queue.push(song);
            }
            if !self.queue.songs.is_empty() {
                self.queue.cursor = 0;
                self.queue.scroll = 0;
                self.play_current();
            }
        } else {
            let needs_fetch = matches!(
                self.library.tracks.get(&album_id),
                None | Some(LoadingState::NotLoaded | LoadingState::Error(_))
            );
            if needs_fetch {
                self.library
                    .tracks
                    .insert(album_id.clone(), LoadingState::Loading);
                self.fetch_tracks(album_id);
            }
            self.flash_status("Loading album tracks… press again when ready");
        }
    }

    /// Replace the queue with all tracks for the current artist (background fetch).
    fn handle_replace_queue_with_current_artist(&mut self) {
        let Some(artist) = self.library.current_artist() else {
            self.flash_status("Select an artist");
            return;
        };
        let artist_id = artist.id.clone();
        self.handle_clear_queue();
        self.fetch_all_tracks_for_artist(artist_id, true, false);
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
        let mut rng = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);

        let next_lcg = |state: &mut u64| -> usize {
            *state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
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
            Tab::Browser => {
                if self.browse_files() {
                    match self.browser_focus {
                        BrowserColumn::Artists | BrowserColumn::Albums => {
                            if self.folders.path.is_empty() {
                                if let LoadingState::Loaded(roots) = &self.folders.roots {
                                    if let Some(visible_pos) = roots
                                        .iter()
                                        .position(|r| r.name.to_lowercase().contains(&q))
                                    {
                                        self.folders.selected_dir = Some(visible_pos);
                                    }
                                }
                            } else if let Some(LoadingState::Loaded(listing)) =
                                self.folders.current_listing()
                            {
                                if let Some(dp) = listing
                                    .directories
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, (_, n))| n.to_lowercase().contains(&q))
                                    .map(|(i, _)| i)
                                    .enumerate()
                                    .map(|(visible, _)| visible)
                                    .next()
                                {
                                    self.folders.selected_dir = Some(1 + dp);
                                }
                            }
                            self.folders.folder_default_row_pending = false;
                            self.folders.preview_selected_row = 0;
                            self.sync_folder_preview_from_left();
                        }
                        BrowserColumn::Tracks => {
                            if let Some(ref pid) = self.folders.preview_dir_id.clone() {
                                if let Some(LoadingState::Loaded(listing)) =
                                    self.folders.listings.get(pid)
                                {
                                    let rows = folder_preview_rows(
                                        listing,
                                        self.browser_column_filter(BrowserColumn::Tracks),
                                    );
                                    if let Some(ix) = rows.iter().position(|row| match row {
                                        FolderPreviewRow::Dir(i) => {
                                            listing.directories[*i].1.to_lowercase().contains(&q)
                                        }
                                        FolderPreviewRow::Track(i) => {
                                            listing.tracks[*i].title.to_lowercase().contains(&q)
                                        }
                                    }) {
                                        self.folders.preview_selected_row = ix;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    match self.browser_focus {
                        BrowserColumn::Artists => {
                            if let crate::state::LoadingState::Loaded(artists) =
                                &self.library.artists
                            {
                                if let Some(idx) = artists
                                    .iter()
                                    .position(|a| a.name.to_lowercase().contains(&q))
                                {
                                    self.library.selected_artist = Some(idx);
                                    self.library.selected_album = Some(0);
                                    self.library.selected_track = Some(0);
                                    let artist_id = artists[idx].id.clone();
                                    if !self.library.albums.contains_key(&artist_id) {
                                        self.library
                                            .albums
                                            .insert(artist_id.clone(), LoadingState::Loading);
                                        self.fetch_albums(artist_id);
                                    }
                                }
                            }
                        }
                        BrowserColumn::Albums => {
                            let artist_id = match self.library.current_artist() {
                                Some(a) => a.id.clone(),
                                None => return,
                            };
                            if let Some(crate::state::LoadingState::Loaded(albums)) =
                                self.library.albums.get(&artist_id)
                            {
                                if let Some(idx) = albums
                                    .iter()
                                    .position(|a| a.name.to_lowercase().contains(&q))
                                {
                                    self.library.selected_album = Some(idx);
                                    self.library.selected_track = Some(0);
                                    let album_id = albums[idx].id.clone();
                                    if !self.library.tracks.contains_key(&album_id) {
                                        self.library
                                            .tracks
                                            .insert(album_id.clone(), LoadingState::Loading);
                                        self.fetch_tracks(album_id);
                                    }
                                }
                            }
                        }
                        BrowserColumn::Tracks => {
                            let album_id = match self.library.current_album() {
                                Some(a) => a.id.clone(),
                                None => return,
                            };
                            if let Some(crate::state::LoadingState::Loaded(songs)) =
                                self.library.tracks.get(&album_id)
                            {
                                if let Some(idx) = songs
                                    .iter()
                                    .position(|s| s.title.to_lowercase().contains(&q))
                                {
                                    self.library.selected_track = Some(idx);
                                }
                            }
                        }
                    }
                }
            }
            Tab::NowPlaying => {
                if let Some(idx) = self
                    .queue
                    .songs
                    .iter()
                    .position(|s| s.title.to_lowercase().contains(&q))
                {
                    self.queue.cursor = idx;
                    self.queue
                        .scroll_clamp_cursor_visible(self.queue_viewport_rows.max(1));
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
                self.queue
                    .scroll_clamp_cursor_visible(self.queue_viewport_rows.max(1));
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
            self.library
                .albums
                .insert(artist_id.clone(), LoadingState::Loading);
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
            self.library
                .tracks
                .insert(album_id.clone(), LoadingState::Loading);
            self.fetch_tracks(album_id);
        }
    }

    pub fn click_folder_dir(&mut self, visible_pos: usize) {
        self.folders.folder_default_row_pending = false;
        self.folders.selected_dir = Some(visible_pos);
        self.folder_activate_selected_dir();
    }

    pub fn click_folder_preview_row(&mut self, visible_row: usize) {
        let Some(ref pid) = self.folders.preview_dir_id.clone() else {
            return;
        };
        let listing = match self.folders.listings.get(pid) {
            Some(LoadingState::Loaded(l)) => l,
            _ => return,
        };
        let rows = folder_preview_rows(listing, self.browser_column_filter(BrowserColumn::Tracks));
        if rows.is_empty() {
            return;
        }
        let vh = self.browser_list_viewport_rows.max(1);
        let sel_pos = self
            .folders
            .preview_selected_row
            .min(rows.len().saturating_sub(1));
        FolderBrowseState::clamp_scroll(&mut self.folders.tracks_scroll, sel_pos, rows.len(), vh);
        let scroll = self.folders.tracks_scroll;
        let clicked = scroll + visible_row;
        if clicked < rows.len() {
            self.folders.preview_selected_row = clicked;
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
            self.queue
                .scroll_clamp_cursor_visible(self.queue_viewport_rows.max(1));
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
                Err(e) => eprintln!("ratune: get_playlists failed — {e}"),
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
                Err(e) => eprintln!("ratune: get_playlist({playlist_id}) failed — {e}"),
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
                    self.playlist_overlay.selected_playlist_index = self
                        .playlist_overlay
                        .selected_playlist_index
                        .saturating_sub(1);
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
                    if let Some(song) = songs
                        .get(self.playlist_overlay.selected_track_index)
                        .cloned()
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
                    if let Some(song) = songs
                        .get(self.playlist_overlay.selected_track_index)
                        .cloned()
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
                self.playlist_overlay.input_mode = PlaylistInputMode::Creating {
                    buffer: String::new(),
                };
            }
            Action::PlaylistRename => {
                if let LoadingState::Loaded(ref playlists) = self.playlist_overlay.playlists {
                    if let Some(p) = playlists.get(self.playlist_overlay.selected_playlist_index) {
                        self.playlist_overlay.input_mode = PlaylistInputMode::Renaming {
                            buffer: p.name.clone(),
                            playlist_id: p.id.clone(),
                        };
                    }
                }
            }
            Action::PlaylistDelete => {
                if let LoadingState::Loaded(ref playlists) = self.playlist_overlay.playlists {
                    if let Some(p) = playlists.get(self.playlist_overlay.selected_playlist_index) {
                        self.playlist_overlay.input_mode = PlaylistInputMode::Confirming {
                            action: ConfirmAction::DeletePlaylist {
                                id: p.id.clone(),
                                name: p.name.clone(),
                            },
                        };
                    }
                }
            }
            Action::PlaylistInputChar(c) => match &mut self.playlist_overlay.input_mode {
                PlaylistInputMode::Creating { buffer }
                | PlaylistInputMode::Renaming { buffer, .. } => {
                    if c == '\x08' {
                        buffer.pop();
                    } else {
                        buffer.push(c);
                    }
                }
                _ => {}
            },
            Action::PlaylistInputConfirm => {
                match self.playlist_overlay.input_mode.clone() {
                    PlaylistInputMode::Creating { buffer } if !buffer.is_empty() => {
                        self.spawn_create_playlist(buffer);
                    }
                    PlaylistInputMode::Renaming {
                        buffer,
                        playlist_id,
                    } if !buffer.is_empty() => {
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
                let song = if self.browse_files() {
                    self.folders
                        .current_preview_track(self.browser_column_filter(BrowserColumn::Tracks))
                } else {
                    self.library.current_track()
                };
                if let Some(song) = song {
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
                        let playlist_id = playlist.id.clone();
                        let playlist_name = playlist.name.clone();
                        let song_id = picker.song_id.clone();
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
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.create_playlist(&name).await {
                Ok(p) => {
                    let _ = tx.send(LibraryUpdate::PlaylistCreated(p)).await;
                }
                Err(e) => eprintln!("create_playlist failed: {e}"),
            }
        });
    }

    fn spawn_rename_playlist(&self, playlist_id: String, new_name: String) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.rename_playlist(&playlist_id, &new_name).await {
                Ok(()) => {
                    let _ = tx
                        .send(LibraryUpdate::PlaylistRenamed {
                            id: playlist_id,
                            new_name,
                        })
                        .await;
                }
                Err(e) => eprintln!("rename_playlist failed: {e}"),
            }
        });
    }

    fn spawn_delete_playlist(&self, id: String) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.delete_playlist(&id).await {
                Ok(()) => {
                    let _ = tx.send(LibraryUpdate::PlaylistDeleted(id)).await;
                }
                Err(e) => eprintln!("delete_playlist failed: {e}"),
            }
        });
    }

    fn spawn_remove_track(&self, playlist_id: String, index: usize) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.remove_track_from_playlist(&playlist_id, index).await {
                Ok(()) => {
                    let _ = tx
                        .send(LibraryUpdate::PlaylistTrackRemoved {
                            _playlist_id: playlist_id,
                            index,
                        })
                        .await;
                }
                Err(e) => eprintln!("remove_track_from_playlist failed: {e}"),
            }
        });
    }

    fn spawn_fetch_playlists_for_picker(&self) {
        let client = self.subsonic.clone();
        let tx = self.library_tx.clone();
        tokio::spawn(async move {
            match client.get_playlists().await {
                Ok(list) => {
                    let _ = tx.send(LibraryUpdate::PlaylistsForPicker(list)).await;
                }
                Err(e) => eprintln!("get_playlists for picker failed: {e}"),
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
        let tx = self.library_tx.clone();
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
