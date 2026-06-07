use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use inquire::Password;
use keyring_core::Error as KeyringError;
use serde::{Deserialize, Serialize};

/// How album art is rendered in the terminal (Now Playing column + Home strip).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AlbumArtBackend {
    /// Multi-protocol rendering via `ratatui-image` (Kitty, Sixel, iTerm2, half-blocks, …).
    #[default]
    RatatuiImage,
    /// Kitty terminal: APC graphics + post-draw path (separate from `ratatui-image`’s Kitty path).
    KittyApc,
}

// ── File-level serde structs ──────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    server: ServerSection,
    #[serde(default)]
    player: PlayerSection,
    #[serde(default)]
    pub keybinds: KeybindsSection,
    #[serde(default)]
    pub theme: ThemeSection,
    #[serde(default)]
    pub ui: UiSection,
    #[serde(default)]
    pub cache: CacheSection,
    #[serde(default)]
    pub library: LibrarySection,
    #[serde(default)]
    pub scrobble: ScrobbleSection,
}

// ── [keybinds] ────────────────────────────────────────────────────────────────

/// Raw keybind strings from config.toml. Every field is `Option<String>`;
/// unset fields fall back to built-in defaults inside `Keybinds::from_section`.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct KeybindsSection {
    pub scroll_up: Option<String>,
    pub scroll_down: Option<String>,
    pub column_left: Option<String>,
    pub column_right: Option<String>,
    pub play_pause: Option<String>,
    pub next_track: Option<String>,
    pub prev_track: Option<String>,
    pub seek_forward: Option<String>,
    pub seek_backward: Option<String>,
    pub add_track: Option<String>,
    pub add_all: Option<String>,
    /// Replace queue with the **current album** (Browser). Default: Ctrl+r
    pub add_all_replace_album: Option<String>,
    /// Replace queue with **all tracks for the current artist** (Browser). Default: Ctrl+Shift+r
    pub add_all_replace_artist: Option<String>,
    /// Prepend artist/album tracks to the queue. Default: Ctrl+Shift+p
    pub add_all_prepend: Option<String>,
    pub shuffle: Option<String>,
    pub unshuffle: Option<String>,
    pub clear_queue: Option<String>,
    /// Remove highlighted track from queue (Now Playing tab). Default: d
    pub remove_from_queue: Option<String>,
    pub search: Option<String>,
    pub volume_up: Option<String>,
    pub volume_down: Option<String>,
    pub tab_switch: Option<String>,
    /// Reverse tab cycle (Backtick by default)
    pub tab_switch_reverse: Option<String>,
    /// Jump to Home tab (default: '1')
    pub go_to_home: Option<String>,
    /// Jump to Browser tab (default: '2')
    pub go_to_browser: Option<String>,
    /// Jump to NowPlaying tab (default: '3')
    pub go_to_nowplaying: Option<String>,
    pub quit: Option<String>,
    /// Fuzzy track picker (metadata index). Default: Ctrl+f
    pub library_fzf: Option<String>,
    /// Force library index refresh. Default: Ctrl+g
    pub library_refresh: Option<String>,
    /// Append all indexed tracks to the queue (y/n confirm). Default: Ctrl+a (`""` disables)
    pub library_index_append_queue: Option<String>,
    /// Toggle this help popup. Default: i
    pub toggle_help: Option<String>,
    /// Toggle dynamic accent from album art. Default: t
    pub toggle_dynamic_theme: Option<String>,
    /// Toggle lyrics overlay. Default: Shift+l (`L` in TOML is fine)
    pub toggle_lyrics: Option<String>,
    /// Toggle spectrum visualizer. Default: Shift+v (bare `V` still works in-app)
    pub toggle_visualizer: Option<String>,
    /// Browser: playlist overlay. Default: Shift+p
    pub playlist_overlay: Option<String>,
    /// Browser: add track to playlist. Default: >
    pub browser_add_to_playlist: Option<String>,
    /// Playlist overlay (tracks pane): remove highlighted track. Default: <
    pub remove_from_playlist: Option<String>,
    /// Home: next panel section. Default: Shift+j (`J` in TOML is fine)
    pub home_section_next: Option<String>,
    /// Home: previous panel section. Default: Shift+k
    pub home_section_prev: Option<String>,
    /// Home: re-roll / refresh. Default: r
    pub home_refresh: Option<String>,
    /// Browse: toggle folder navigation (requires `[ui.browsetab] folder_navigation`). Default: Ctrl+b
    pub toggle_folder_browse: Option<String>,
}

// ── [theme] ───────────────────────────────────────────────────────────────────

// ── [ui] ─────────────────────────────────────────────────────────────────────

// ── [cache] ───────────────────────────────────────────────────────────────────

/// Offline track cache settings from config.toml.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CacheSection {
    /// Whether the track cache is enabled. Default: true.
    #[serde(default = "default_cache_enabled")]
    pub enabled: bool,
    /// Maximum total cache size in gigabytes. Default: 2.0.
    #[serde(default = "default_cache_max_size_gb")]
    pub max_size_gb: f64,
}

fn default_cache_enabled() -> bool {
    true
}
fn default_cache_max_size_gb() -> f64 {
    2.0
}

impl Default for CacheSection {
    fn default() -> Self {
        Self {
            enabled: default_cache_enabled(),
            max_size_gb: default_cache_max_size_gb(),
        }
    }
}

// ── [library] — metadata index + fzf picker ───────────────────────────────────

/// Local library metadata index and fuzzy picker (Milestone 2).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LibrarySection {
    /// Build and use the on-disk index for fzf. Default: true.
    #[serde(default = "default_library_enabled")]
    pub enabled: bool,
    /// Path to `library_index.json`. Empty = `~/.cache/ratune/library_index.json`.
    #[serde(default)]
    pub index_path: String,
    /// Consider the index stale after this many seconds (full refresh in background).
    /// Default: 86400 (24 h). Set to 0 to always refresh at startup.
    #[serde(default = "default_library_max_age_secs")]
    pub max_age_secs: u64,
    /// Executable name or path for the fuzzy finder. Default: `fzf` (also works with `sk`).
    #[serde(default = "default_fzf_binary")]
    pub fzf_binary: String,
    /// Extra arguments passed to fzf after defaults (delimiter, columns).
    #[serde(default = "default_fzf_args")]
    pub fzf_args: Vec<String>,
    /// Concurrent `getAlbum` calls per artist during a full index refresh. Default: 12.
    #[serde(default = "default_library_fetch_album_parallelism")]
    pub fetch_album_parallelism: usize,
    /// Concurrent artists during a full index refresh. Default: 4.
    #[serde(default = "default_library_fetch_artist_parallelism")]
    pub fetch_artist_parallelism: usize,
    /// Navidrome only: if the on-disk index was built after the same library scan as
    /// `getScanStatus.lastScan`, skip the full API walk (still obeys forced index refresh).
    #[serde(default)]
    pub navidrome_skip_unchanged_scan: bool,
    /// After a forced index refresh, send a desktop notification (FreeDesktop
    /// `notify-send` protocol). Default: true.
    #[serde(default = "default_library_notify_on_forced_refresh")]
    pub notify_on_forced_index_refresh: bool,
}

fn default_library_enabled() -> bool {
    true
}

fn default_library_fetch_album_parallelism() -> usize {
    12
}

fn default_library_fetch_artist_parallelism() -> usize {
    4
}

fn default_library_notify_on_forced_refresh() -> bool {
    true
}

fn default_library_max_age_secs() -> u64 {
    86400
}

fn default_fzf_binary() -> String {
    "fzf".into()
}

fn default_fzf_args() -> Vec<String> {
    vec![
        "--delimiter=\t".into(),
        // Hide song id in the UI; only show artist–time.
        "--with-nth=2,3,4,5".into(),
        // After `--with-nth`, displayed field 1 = artist … field 4 = time. Search artist,
        // album, title only (duration is visible but not fuzzy-matched).
        "--nth=1,2,3".into(),
        "--multi".into(),
        // Enter = append to queue; Ctrl+R = replace queue (first stdout line is `ctrl-r`).
        "--expect=ctrl-r".into(),
        "--border=rounded".into(),
    ]
}

impl Default for LibrarySection {
    fn default() -> Self {
        Self {
            enabled: default_library_enabled(),
            index_path: String::new(),
            max_age_secs: default_library_max_age_secs(),
            fzf_binary: default_fzf_binary(),
            fzf_args: default_fzf_args(),
            fetch_album_parallelism: default_library_fetch_album_parallelism(),
            fetch_artist_parallelism: default_library_fetch_artist_parallelism(),
            navidrome_skip_unchanged_scan: false,
            notify_on_forced_index_refresh: default_library_notify_on_forced_refresh(),
        }
    }
}

// ── [scrobble] — Last.fm / Libre.fm + Subsonic play counts ───────────────────

/// Local listen threshold (history + Subsonic). Defaults: 50%, 30 s cap.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScrobbleLocalThresholdSection {
    /// Minimum fraction of track length before counting a listen (1–100). Default: 50.
    #[serde(default = "default_scrobble_min_percent")]
    pub min_percent: u8,
    /// Upper cap on listen time (seconds). Default: 30.
    #[serde(default = "default_local_max_listen_seconds")]
    pub max_listen_seconds: u32,
}

impl Default for ScrobbleLocalThresholdSection {
    fn default() -> Self {
        Self {
            min_percent: default_scrobble_min_percent(),
            max_listen_seconds: default_local_max_listen_seconds(),
        }
    }
}

impl ScrobbleLocalThresholdSection {
    pub fn resolve(&self) -> ratune_scrobble::ListenThreshold {
        ratune_scrobble::ListenThreshold {
            min_percent: self.min_percent.clamp(1, 100),
            max_listen: std::time::Duration::from_secs(self.max_listen_seconds.max(1) as u64),
        }
    }
}

/// Last.fm / Libre.fm listen rules. Defaults match Last.fm’s documented scrobbling rules.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScrobbleAudioscrobblerThresholdSection {
    #[serde(default = "default_scrobble_min_percent")]
    pub min_percent: u8,
    #[serde(default = "default_audioscrobbler_max_listen_seconds")]
    pub max_listen_seconds: u32,
    /// Tracks this long or shorter are not scrobbled. Default: 30.
    #[serde(default = "default_audioscrobbler_min_track_seconds")]
    pub min_track_seconds: u32,
}

impl Default for ScrobbleAudioscrobblerThresholdSection {
    fn default() -> Self {
        Self {
            min_percent: default_scrobble_min_percent(),
            max_listen_seconds: default_audioscrobbler_max_listen_seconds(),
            min_track_seconds: default_audioscrobbler_min_track_seconds(),
        }
    }
}

impl ScrobbleAudioscrobblerThresholdSection {
    pub fn resolve(&self) -> ratune_scrobble::AudioscrobblerRules {
        ratune_scrobble::AudioscrobblerRules {
            listen: ratune_scrobble::ListenThreshold {
                min_percent: self.min_percent.clamp(1, 100),
                max_listen: std::time::Duration::from_secs(self.max_listen_seconds.max(1) as u64),
            },
            min_track_length: std::time::Duration::from_secs(self.min_track_seconds as u64),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ScrobbleThresholdsSection {
    #[serde(default)]
    pub local: ScrobbleLocalThresholdSection,
    #[serde(default)]
    pub audioscrobbler: ScrobbleAudioscrobblerThresholdSection,
}

fn default_scrobble_min_percent() -> u8 {
    50
}

fn default_local_max_listen_seconds() -> u32 {
    30
}

fn default_audioscrobbler_max_listen_seconds() -> u32 {
    240
}

fn default_audioscrobbler_min_track_seconds() -> u32 {
    30
}

/// Audioscrobbler and server-side scrobbling settings from config.toml.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScrobbleSection {
    /// Enable scrobbling to Last.fm or Libre.fm. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// `lastfm` (default) or `librefm`.
    #[serde(default = "default_scrobble_service")]
    pub service: String,
    /// Application API key from your Last.fm / Libre.fm account.
    #[serde(default)]
    pub api_key: String,
    /// API shared secret (required to sign POST requests).
    #[serde(default)]
    pub api_secret: String,
    /// Shell command whose stdout is the API shared secret (trimmed).
    #[serde(default)]
    pub api_secret_command: String,
    /// Session key from `auth.getSession` (see docs). Prefer keyring / command over plaintext.
    #[serde(default)]
    pub session_key: String,
    /// Shell command whose stdout is the session key (trimmed).
    #[serde(default)]
    pub session_key_command: String,
    /// Call the Subsonic `/scrobble` endpoint when a listen is recorded. Default: true.
    #[serde(default = "default_scrobble_to_server")]
    pub scrobble_to_server: bool,
    /// Optional listen thresholds (defaults follow Last.fm conventions).
    #[serde(default)]
    pub thresholds: ScrobbleThresholdsSection,
}

fn default_scrobble_service() -> String {
    "lastfm".into()
}

fn default_scrobble_to_server() -> bool {
    true
}

impl Default for ScrobbleSection {
    fn default() -> Self {
        Self {
            enabled: false,
            service: default_scrobble_service(),
            api_key: String::new(),
            api_secret: String::new(),
            api_secret_command: String::new(),
            session_key: String::new(),
            session_key_command: String::new(),
            scrobble_to_server: default_scrobble_to_server(),
            thresholds: ScrobbleThresholdsSection::default(),
        }
    }
}

/// App-wide UI (all tabs): tab strip, and other cross-tab options.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiGeneralSection {
    /// Tab strip: `bottom` (default) or `top` (still above the 1-row status bar).
    #[serde(default)]
    pub tab_bar_position: Option<String>,
    /// When true (default), show in-app playback level (`N%`) on the bottom-right of the status bar.
    #[serde(default)]
    pub show_volume_indicator: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabArtSection {
    #[serde(default)]
    pub show: Option<bool>,
    #[serde(default)]
    pub position: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabLayoutSection {
    /// When the Now Playing tab is split into left/right columns, the left column width in percent (1–99).
    ///
    /// This replaces per-widget width percentages; it controls the left/right column split.
    #[serde(default)]
    pub left_width_percent: Option<u8>,
    /// When a single Now Playing column has exactly two stacked panes, this controls how much
    /// vertical space the **top** pane gets (1–99%). Default: 50.
    #[serde(default)]
    pub vertical_fill_top_percent: Option<u8>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabVisualizerSection {
    /// If false, the visualizer cannot be shown or toggled (`V`). Default: true.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Start with the spectrum visualizer overlay visible (toggle `V`).
    #[serde(default)]
    pub visible: Option<bool>,
    /// When open: `left` or `right`.
    #[serde(default)]
    pub location: Option<String>,
    /// Visualizer type: `spectrum` (default), `wave`.
    ///
    /// TOML key: `type` (alias: `visualizer_type`).
    #[serde(default, rename = "type", alias = "visualizer_type")]
    pub visualizer_type: Option<String>,
    /// Visualizer update rate (FPS) when visible. Higher = smoother, more CPU. Default: 30.
    #[serde(default)]
    pub fps: Option<u16>,
    /// FFT window size for spectrum. Supported: 1024, 2048 (default), 4096.
    #[serde(default)]
    pub fft_size: Option<usize>,
    /// Gain in dB applied before normalisation. Default: 0.0.
    #[serde(default)]
    pub gain_db: Option<f32>,
    /// Visualizer colors. Examples:
    /// - `["accent"]` (default)
    /// - `["#00ffff", "#00ff00", "#ffff00", "#ff0000"]` (gradient)
    /// - `["47", "83", "119"]` (256-color indices)
    #[serde(default)]
    pub colors: Option<Vec<String>>,
    /// Color mode: `accent` (default), `fixed`, `gradient_height`, `gradient_theme`.
    #[serde(default, alias = "visualizer_color_mode")]
    pub color_mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabQueueSection {
    /// Queue column side on the Now Playing tab: `left` or `right`.
    ///
    /// If omitted, it defaults to the opposite of `[ui.nptab.art].position` (so art + queue form
    /// two columns by default).
    #[serde(default)]
    pub position: Option<String>,
    /// One format string for **each queue row** (Now Playing tab queue list). Not the same as
    /// now-playing `lines` (`%` / `$` tags); this uses `{title}`, `{n}`, etc. (see `queue.rs`).
    ///
    /// Takes precedence over `[ui.nptab].queue_template` when set.
    #[serde(default)]
    pub queue_template: Option<String>,
}

/// Now Playing tab: overrides for the bottom strip + boxed pane (see `[ui.row_now_playing]` for shared defaults).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabNowPlayingSection {
    #[serde(default)]
    pub bar_height: Option<u16>,
    #[serde(default)]
    pub layout: Option<String>,
    /// When `layout` is `boxed`: dock the pane on `left` or `right`.
    #[serde(default)]
    pub box_location: Option<String>,
    #[serde(default)]
    pub show_controls: Option<bool>,
    #[serde(default)]
    pub show_progress: Option<bool>,
    #[serde(default)]
    pub box_include_controls: Option<bool>,
    #[serde(default)]
    pub box_include_progress: Option<bool>,
    #[serde(default)]
    pub progress_style: Option<String>,
    /// ncmpcpp-style lines for the **boxed** NP pane only; omit to reuse row strip templates.
    #[serde(default)]
    pub lines: Option<Vec<String>>,
}

/// Shared defaults for the bottom **now-playing strip** (used on Home, Browse, and Now Playing).
///
/// Precedence for overlapping keys: `[ui.nptab.now_playing]` wins, then this table, then built-in defaults.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiRowNowPlayingSection {
    #[serde(default)]
    pub bar_height: Option<u16>,
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub box_location: Option<String>,
    #[serde(default)]
    pub show_controls: Option<bool>,
    #[serde(default)]
    pub show_progress: Option<bool>,
    #[serde(default)]
    pub box_include_controls: Option<bool>,
    #[serde(default)]
    pub box_include_progress: Option<bool>,
    #[serde(default)]
    pub progress_style: Option<String>,
    #[serde(default)]
    pub lines: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiHomeRecentAlbumsSection {
    /// When false, Home uses text-only recently played (no Kitty art strip).
    #[serde(default)]
    pub show_art: Option<bool>,
    /// `getCoverArt` `size` (max edge px) for Home strip downloads. Smaller = faster network + decode.
    /// `0` = request full-size art (slowest). Default when omitted: 320.
    #[serde(default)]
    pub cover_fetch_max_px: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiHomeLayoutSection {
    /// Height of the top band as percent of the Home content area (25–75). Default: 50.
    #[serde(default)]
    pub top_height_percent: Option<u8>,
    /// Which panel sits where: `[top, bottom_left, bottom_right]`.
    /// Each value is `recent_albums`, `recent_tracks`, or `rediscover` (must be a permutation).
    #[serde(default)]
    pub panels: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiHomeTabSection {
    #[serde(default)]
    pub recent_albums: Option<UiHomeRecentAlbumsSection>,
    #[serde(default)]
    pub layout: Option<UiHomeLayoutSection>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiBrowseTabSection {
    /// When true, folder (`getMusicFolders` / `getMusicDirectory`) browsing is available and
    /// the configured key toggles between artist columns and folder view. Default: false.
    #[serde(default)]
    pub folder_navigation: Option<bool>,
    /// Default browse layout on startup: `artists` (default), `genre` (stub), or `files` (only
    /// applies when [`folder_navigation`](Self::folder_navigation) is true).
    #[serde(default)]
    pub mode: Option<String>,
    /// Browse tab: list rows to move per mouse wheel tick (default: 1).
    #[serde(default)]
    pub mouse_wheel_scroll_lines: Option<usize>,
}

/// Optional nested `[ui.nptab]` (Now Playing tab) table.
///
/// All fields are optional; when present, they override `[ui.row_now_playing]` and built-in defaults.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabSection {
    /// Queue row template (Now Playing tab queue column). Prefer `[ui.nptab.queue].queue_template`
    /// when grouping with `position`; that key overrides this one when set.
    #[serde(default)]
    pub queue_template: Option<String>,
    /// Empty-queue hint (only when library fzf is enabled).
    #[serde(default)]
    pub show_fzf_hint: Option<bool>,
    /// Album art settings for the Now Playing tab.
    #[serde(default)]
    pub art: Option<UiNpTabArtSection>,
    /// Layout settings (column widths, etc.) for the Now Playing tab.
    #[serde(default)]
    pub layout: Option<UiNpTabLayoutSection>,
    /// Queue column placement for the Now Playing tab.
    #[serde(default)]
    pub queue: Option<UiNpTabQueueSection>,
    /// Lyrics overlay: same shape as [`UiNpTabVisualizerSection`] (`enabled`, `visible`, `location`).
    #[serde(default)]
    pub lyrics_pane: Option<UiNpTabLyricsSection>,
    /// Visualizer: feature toggle, startup visibility, pane docking.
    #[serde(default)]
    pub visualizer_pane: Option<UiNpTabVisualizerSection>,
    /// Bottom strip layout + boxed pane text (overrides `row_now_playing`).
    #[serde(default)]
    pub now_playing: Option<UiNpTabNowPlayingSection>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabLyricsSection {
    /// If false, lyrics cannot be shown or toggled (`L`). Default: true.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Start with the lyrics pane visible (toggle `L`). Default: false.
    #[serde(default)]
    pub visible: Option<bool>,
    /// `left`, `right`, or `full` (full-width dock row). Omitted → same side as the queue column.
    #[serde(default)]
    pub location: Option<String>,
}

/// UI preferences from config.toml.
///
/// Layout and Now Playing behavior live under `[ui.general]`, `[ui.row_now_playing]`, and
/// `[ui.nptab]` (plus `[ui.hometab]` / `[ui.browsetab]`). Only `album_art_backend` remains on the
/// root `[ui]` table.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiSection {
    #[serde(default)]
    pub general: Option<UiGeneralSection>,
    /// `ratatui-image` (default) or `kitty-apc` (Kitty APC post-draw path).
    #[serde(default)]
    pub album_art_backend: AlbumArtBackend,
    #[serde(default)]
    pub nptab: Option<UiNpTabSection>,
    #[serde(default)]
    pub row_now_playing: Option<UiRowNowPlayingSection>,
    #[serde(default)]
    pub hometab: Option<UiHomeTabSection>,
    #[serde(default)]
    pub browsetab: Option<UiBrowseTabSection>,
}

fn default_ui_lyrics_enabled() -> bool {
    true
}
fn default_ui_visualizer_enabled() -> bool {
    true
}
fn default_ui_visualizer_type() -> String {
    "spectrum".into()
}
fn default_ui_visualizer_fps() -> u16 {
    30
}
fn default_ui_visualizer_fft_size() -> usize {
    2048
}
fn default_ui_visualizer_gain_db() -> f32 {
    0.0
}
fn default_ui_visualizer_colors() -> Vec<String> {
    vec!["accent".into()]
}
fn default_ui_visualizer_color_mode() -> String {
    "accent".into()
}
fn default_ui_progress_style() -> String {
    "██░".into()
}
fn default_ui_nowplaying_show_art() -> bool {
    true
}
fn default_ui_nowplaying_art_position() -> String {
    "left".into()
}
fn default_ui_nowplaying_left_width_percent() -> u8 {
    50
}
fn default_ui_show_fzf_hint() -> bool {
    false
}
fn default_ui_visualizer_location() -> String {
    "queue".into()
}
fn default_ui_tab_bar_position() -> String {
    "bottom".into()
}
fn default_ui_now_playing_bar_height() -> u16 {
    4
}
fn default_ui_now_playing_layout() -> String {
    "row".into()
}
fn default_ui_now_playing_box_location() -> String {
    "right".into()
}
fn default_ui_now_playing_show_controls() -> bool {
    true
}
fn default_ui_now_playing_show_progress() -> bool {
    true
}

fn default_now_playing_lines() -> Vec<String> {
    vec!["$b%t$/b".into(), "%a".into(), "%b".into()]
}

/// Which block occupies each slot on the Home tab (see [`Config::home_panels`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HomePanel {
    RecentAlbums,
    RecentTracks,
    Rediscover,
}

impl HomePanel {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "recent_albums" | "albums" => Some(Self::RecentAlbums),
            "recent_tracks" | "tracks" => Some(Self::RecentTracks),
            "rediscover" => Some(Self::Rediscover),
            _ => None,
        }
    }
}

fn default_home_panels() -> [HomePanel; 3] {
    [
        HomePanel::RecentAlbums,
        HomePanel::RecentTracks,
        HomePanel::Rediscover,
    ]
}

fn parse_home_panels(v: Option<Vec<String>>) -> [HomePanel; 3] {
    let Some(v) = v else {
        return default_home_panels();
    };
    if v.len() != 3 {
        return default_home_panels();
    }
    let mut out = default_home_panels();
    let mut seen = std::collections::HashSet::new();
    for (i, s) in v.iter().enumerate() {
        let Some(p) = HomePanel::parse(s) else {
            return default_home_panels();
        };
        if !seen.insert(p) {
            return default_home_panels();
        }
        out[i] = p;
    }
    out
}

/// Browse tab mode: `artists` (artist/album/track) or `files` (folder navigation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowseMode {
    #[default]
    Artists,
    Genre,
    Files,
}

impl BrowseMode {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "artists" | "artist" => Some(Self::Artists),
            "genre" | "genres" => Some(Self::Genre),
            "files" | "file" => Some(Self::Files),
            _ => None,
        }
    }
}

/// Theme colour strings for `[theme]` (hex, `idx:` / `ansi:` / …, or `reset`); parsed in [`crate::theme`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum ThemePreset {
    /// Use the configured hex palette in `[theme]` (and do **not** extract accent from album art).
    Static,
    /// Use configured hex palette, but allow dynamic accent extracted from album art. (Default)
    #[default]
    Dynamic,
    /// Use terminal/OS palette (ANSI indices / default fg/bg), ignoring the hex palette.
    Terminal,
}

pub(crate) fn theme_preset_from_section(sec: &ThemeSection) -> ThemePreset {
    let Some(p) = sec.preset.as_deref() else {
        return ThemePreset::default();
    };
    match p.trim().to_ascii_lowercase().as_str() {
        "static" => ThemePreset::Static,
        "dynamic" => ThemePreset::Dynamic,
        "os" | "terminal" | "term" => ThemePreset::Terminal,
        _ => ThemePreset::default(),
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct ThemeSection {
    /// Theme mode preset:
    /// - `dynamic` (default): configured palette + album-art accent extraction
    /// - `static`: configured palette only (no dynamic accent)
    /// - `terminal` / `os`: terminal palette defaults; optional fields below still override
    ///
    /// Optional colour fields merge on top of preset defaults. Values may be 6-digit hex (`#rrggbb`
    /// or `rrggbb`), a 256-colour index (`idx:N`, `indexed:N`, `ansi:N`, `color:N`, or `i:N` for N in 0..=255),
    /// or `reset` / `inherit` / `default` / `unset` / `none` / `transparent` to leave a
    /// background unpainted (for terminal transparency).
    #[serde(default)]
    pub preset: Option<String>,
    pub accent: Option<String>,
    /// General chrome (popups, selection inverse fg). Also sets tab/status bars when they are unset.
    pub background: Option<String>,
    /// Tab indicator bar (`Home | Browse | Now Playing`). Falls back to `background`.
    pub tab_bar: Option<String>,
    /// Bottom status bar. Falls back to `background`.
    pub status_bar: Option<String>,
    pub surface: Option<String>,
    pub foreground: Option<String>,
    pub dimmed: Option<String>,
    pub border: Option<String>,
    pub border_active: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct ServerSection {
    #[serde(default)]
    url: String,
    #[serde(default)]
    username: String,
    /// Plain Subsonic password or token (least secure). Prefer empty `password` with OS keyring
    /// or `password_command`. `SUBSONIC_PASS` overrides this field.
    #[serde(default)]
    password: String,
    /// Shell command whose stdout is the Subsonic secret (trimmed). Used when `password` is empty.
    /// Example: `secret-tool lookup --label=ratune service subsonic`.
    #[serde(default)]
    password_command: String,
    /// Linux only: keyring backend when `password` and `password_command` are empty.
    /// `keyutils` (default) or `secret-service` (gnome-keyring / KWallet). Ignored on macOS/Windows.
    #[serde(default = "default_password_keyring")]
    password_keyring: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlayerSection {
    #[serde(default = "default_volume")]
    default_volume: u8,
    #[serde(default)]
    max_bit_rate: u32,
    /// Register on the session D-Bus as an MPRIS player (Linux media keys, etc.).
    #[serde(default = "default_mpris")]
    mpris: bool,
}

impl Default for PlayerSection {
    fn default() -> Self {
        Self {
            default_volume: default_volume(),
            max_bit_rate: 0,
            mpris: default_mpris(),
        }
    }
}

fn default_volume() -> u8 {
    70
}

fn default_mpris() -> bool {
    true
}

fn default_password_keyring() -> String {
    "keyutils".into()
}

// ── Runtime config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Config {
    pub subsonic_url: String,
    pub subsonic_user: String,
    pub subsonic_pass: String,
    pub default_volume: u8,
    pub max_bit_rate: u32,
    /// Linux: register MPRIS on the session bus (media keys, `playerctl`).
    pub mpris_enabled: bool,
    /// Raw keybind strings — parsed into `Keybinds` by `App::new`.
    pub keybinds: KeybindsSection,
    /// Raw theme colour strings — parsed into `Theme` by `App::new`.
    pub theme: ThemeSection,
    /// Whether to show the lyrics overlay on startup.
    pub lyrics_visible: bool,
    /// Whether to show the spectrum visualizer overlay on startup.
    pub visualizer_visible: bool,
    /// Whether the lyrics overlay feature can be toggled at all.
    pub lyrics_enabled: bool,
    /// Whether the visualizer overlay feature can be toggled at all.
    pub visualizer_enabled: bool,
    /// Queue row display template; empty = built-in default columns.
    pub queue_template: String,
    /// Now-playing bar glyphs (three-character progress style).
    pub progress_style: String,
    /// NowPlaying tab: show the album-art column.
    pub nowplaying_show_art: bool,
    pub album_art_backend: AlbumArtBackend,
    /// NowPlaying tab: album art side ("left" or "right").
    pub nowplaying_art_position: String,
    /// NowPlaying tab: queue side ("left" or "right").
    pub nowplaying_queue_position: String,
    /// NowPlaying tab: left column width percent when split (1–99).
    pub nowplaying_left_width_percent: u8,
    /// NowPlaying tab: when exactly two panes stack in one column, top pane percent (1–99).
    pub nowplaying_vertical_fill_top_percent: u8,
    /// Show fzf picker hints in the UI where relevant.
    pub show_fzf_hint: bool,
    /// Now Playing tab: lyrics pane side when visible (`left`, `right`, or `full`). Defaults to
    /// the queue column side when omitted in config.
    pub lyrics_location: String,
    /// Where the visualizer pane appears ("queue" or "art").
    pub visualizer_location: String,
    /// Visualizer type: `spectrum`, `wave`.
    pub visualizer_type: String,
    /// Visualizer update rate (FPS) when visible.
    pub visualizer_fps: u16,
    /// FFT window size for spectrum analysis.
    pub visualizer_fft_size: usize,
    /// Gain in dB applied before spectrum normalisation / waveform scaling.
    pub visualizer_gain_db: f32,
    /// Visualizer colors (strings parsed at render-time).
    pub visualizer_colors: Vec<String>,
    /// Visualizer color mode: `accent`, `fixed`, `gradient_height`.
    pub visualizer_color_mode: String,
    /// Tab bar at top (`top`) or bottom (`bottom`).
    pub tab_bar_position: String,
    /// Show `N%` playback level at the bottom-right of the status bar (in-app gain, not OS volume).
    pub show_volume_indicator: bool,
    /// Now-playing bar height in rows.
    pub now_playing_bar_height: u16,
    /// `row` or `boxed` now-playing layout.
    pub now_playing_layout: String,
    /// Boxed Now Playing pane side (`left` or `right`).
    pub now_playing_box_location: String,
    pub now_playing_show_controls: bool,
    pub now_playing_show_progress: bool,
    pub now_playing_box_include_controls: bool,
    pub now_playing_box_include_progress: bool,
    /// ncmpcpp-style lines for the **row** strip (Home, Browse, NP when using row footer).
    pub now_playing_lines_row: Vec<String>,
    /// ncmpcpp-style lines for the **boxed** Now Playing pane (NP tab only). Falls back to
    /// `now_playing_lines_row` when empty.
    pub now_playing_lines_boxed: Vec<String>,
    /// Whether the offline track cache is enabled.
    pub cache_enabled: bool,
    /// Maximum total cache size in gigabytes.
    pub cache_max_size_gb: f64,
    /// Local metadata index for fzf (see `[library]`).
    pub library_index_enabled: bool,
    pub library_index_path: String,
    pub library_index_max_age_secs: u64,
    pub fzf_binary: String,
    pub fzf_args: Vec<String>,
    pub library_fetch_album_parallelism: usize,
    pub library_fetch_artist_parallelism: usize,
    pub library_navidrome_skip_unchanged_scan: bool,
    /// Desktop notification after a forced library index refresh finishes.
    pub library_notify_on_forced_index_refresh: bool,
    /// Home tab: show Kitty thumbnails in Recently Played when supported.
    pub home_recent_albums_show_art: bool,
    /// Subsonic `getCoverArt` `size` for Home strip (0 = full resolution from server).
    pub home_cover_fetch_max_px: u32,
    /// Home tab: top band height as percent of the content area (25–75).
    pub home_top_height_percent: u8,
    /// Home tab: `[top, bottom_left, bottom_right]` panel assignment.
    pub home_panels: [HomePanel; 3],
    /// Browse tab: `artists` (default), or placeholder `genre` / `files`.
    pub browse_mode: BrowseMode,
    /// When true, folder browsing can be toggled with the keybind and used from the Browse tab.
    pub browse_folder_navigation: bool,
    /// Browse tab: list rows to move per mouse wheel tick.
    pub browse_mouse_wheel_scroll_lines: usize,
    /// Scrobble listens to Last.fm or Libre.fm when enabled and credentials are configured.
    pub scrobble_enabled: bool,
    pub scrobble_service: ratune_scrobble::ScrobbleService,
    pub scrobble_api_key: String,
    pub scrobble_api_secret: String,
    pub scrobble_session_key: String,
    /// Notify the Subsonic server (Navidrome play counts) when a listen is recorded.
    pub scrobble_to_server: bool,
    /// Threshold for local history + Subsonic scrobble.
    pub scrobble_local_threshold: ratune_scrobble::ListenThreshold,
    /// Threshold for Last.fm / Libre.fm scrobble.
    pub scrobble_audioscrobbler_rules: ratune_scrobble::AudioscrobblerRules,
}

impl Config {
    /// Load config from `~/.config/ratune/config.toml`, creating a default
    /// file if it doesn't exist. Env vars override file values.
    /// Returns an error (with message) if no password is configured.
    pub fn load() -> Result<Self> {
        let config_path = config_file_path()?;

        // Create default file if missing.
        if !config_path.exists() {
            create_default(&config_path)?;
        }

        let text = std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;
        let mut file_cfg: FileConfig =
            toml::from_str(&text).with_context(|| format!("parsing {}", config_path.display()))?;

        // Env vars override file values.
        merge_env_overrides(&mut file_cfg);

        let subsonic_pass = resolve_subsonic_secret(&file_cfg.server).with_context(|| {
            format!(
                "Subsonic credentials failed (config {}). Hint: set [server].password_command, leave password empty for the OS keyring (needs url + username + TTY on first run), or set SUBSONIC_PASS.",
                config_path.display()
            )
        })?;

        // Validate password.
        if subsonic_pass.is_empty() {
            bail!(
                "No Subsonic password configured.\n\
                 In {} set [server].password_command, SUBSONIC_PASS / TERMUSIC_SUBSONIC_PASS,\n\
                 [server].password (plaintext), or leave password empty for the keyring (needs url + username).",
                config_path.display()
            );
        }

        let ui = &file_cfg.ui;
        let nptab = ui.nptab.as_ref();
        let row = ui.row_now_playing.as_ref();

        // Strip fields: `[ui.nptab.now_playing]` > `[ui.row_now_playing]` > built-in defaults.
        let progress_style = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|n| n.progress_style.clone())
            .or_else(|| row.and_then(|r| r.progress_style.clone()))
            .unwrap_or_else(default_ui_progress_style);

        let nowplaying_show_art = nptab
            .and_then(|n| n.art.as_ref())
            .and_then(|a| a.show)
            .unwrap_or_else(default_ui_nowplaying_show_art);

        let nowplaying_art_position = nptab
            .and_then(|n| n.art.as_ref())
            .and_then(|a| a.position.clone())
            .unwrap_or_else(default_ui_nowplaying_art_position);

        let nowplaying_queue_position = nptab
            .and_then(|n| n.queue.as_ref())
            .and_then(|q| q.position.clone())
            .unwrap_or_else(|| {
                if nowplaying_art_position.trim().eq_ignore_ascii_case("right") {
                    "left".into()
                } else {
                    "right".into()
                }
            });

        let lyrics_pane = nptab.and_then(|n| n.lyrics_pane.as_ref());
        let lyrics_location = lyrics_pane
            .and_then(|l| l.location.clone())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| nowplaying_queue_position.clone());

        let nowplaying_left_width_percent = nptab
            .and_then(|n| n.layout.as_ref())
            .and_then(|l| l.left_width_percent)
            .unwrap_or_else(default_ui_nowplaying_left_width_percent)
            .clamp(1, 99);

        let nowplaying_vertical_fill_top_percent = nptab
            .and_then(|n| n.layout.as_ref())
            .and_then(|l| l.vertical_fill_top_percent)
            .unwrap_or(50)
            .clamp(1, 99);

        let show_fzf_hint = nptab
            .and_then(|n| n.show_fzf_hint)
            .unwrap_or_else(default_ui_show_fzf_hint);

        let visualizer_location = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.location.clone())
            .unwrap_or_else(default_ui_visualizer_location);

        let mut visualizer_type = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.visualizer_type.clone())
            .unwrap_or_else(default_ui_visualizer_type);
        if visualizer_type.trim().eq_ignore_ascii_case("wave_filled") {
            // Alias: keep configs working, but treat as `wave` for now.
            visualizer_type = "wave".into();
        }

        let visualizer_fps = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.fps)
            .unwrap_or_else(default_ui_visualizer_fps)
            .clamp(1, 240);

        let visualizer_fft_size = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.fft_size)
            .unwrap_or_else(default_ui_visualizer_fft_size)
            .clamp(1024, 4096);

        let visualizer_gain_db = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.gain_db)
            .unwrap_or_else(default_ui_visualizer_gain_db)
            .clamp(-60.0, 60.0);

        let visualizer_colors = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.colors.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(default_ui_visualizer_colors);

        let visualizer_color_mode = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.color_mode.clone())
            .unwrap_or_else(default_ui_visualizer_color_mode);

        let tab_bar_position = ui
            .general
            .as_ref()
            .and_then(|g| g.tab_bar_position.clone())
            .unwrap_or_else(default_ui_tab_bar_position);

        let show_volume_indicator = ui
            .general
            .as_ref()
            .and_then(|g| g.show_volume_indicator)
            .unwrap_or(true);

        let now_playing_bar_height = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.bar_height)
            .or_else(|| row.and_then(|r| r.bar_height))
            .unwrap_or_else(default_ui_now_playing_bar_height);

        let now_playing_layout = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.layout.clone())
            .or_else(|| row.and_then(|r| r.layout.clone()))
            .unwrap_or_else(default_ui_now_playing_layout);

        let now_playing_box_location = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.box_location.clone())
            .or_else(|| row.and_then(|r| r.box_location.clone()))
            .unwrap_or_else(default_ui_now_playing_box_location);

        let now_playing_show_controls = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.show_controls)
            .or_else(|| row.and_then(|r| r.show_controls))
            .unwrap_or_else(default_ui_now_playing_show_controls);

        let now_playing_show_progress = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.show_progress)
            .or_else(|| row.and_then(|r| r.show_progress))
            .unwrap_or_else(default_ui_now_playing_show_progress);

        let now_playing_box_include_controls = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.box_include_controls)
            .or_else(|| row.and_then(|r| r.box_include_controls))
            .unwrap_or(false);

        let now_playing_box_include_progress = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.box_include_progress)
            .or_else(|| row.and_then(|r| r.box_include_progress))
            .unwrap_or(false);

        // Row strip lines: `[ui.row_now_playing].lines` only (`[ui.nptab.now_playing].lines` is boxed pane only).
        let now_playing_lines_row = row
            .and_then(|r| r.lines.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(default_now_playing_lines);

        // Boxed NP pane: `[ui.nptab.now_playing].lines` only; absent or `[]` → same as row strip.
        let now_playing_lines_boxed = {
            let from_nptab = nptab
                .and_then(|n| n.now_playing.as_ref())
                .and_then(|n| n.lines.clone());
            let mut lines = match from_nptab {
                None => now_playing_lines_row.clone(),
                Some(v) if v.is_empty() => now_playing_lines_row.clone(),
                Some(v) => v,
            };
            if lines.is_empty() {
                lines = default_now_playing_lines();
            }
            lines
        };

        let lyrics_visible = lyrics_pane.and_then(|l| l.visible).unwrap_or(false);
        let lyrics_enabled = lyrics_pane
            .and_then(|l| l.enabled)
            .unwrap_or_else(default_ui_lyrics_enabled);
        let visualizer_visible = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.visible)
            .unwrap_or(false);
        let visualizer_enabled = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.enabled)
            .unwrap_or_else(default_ui_visualizer_enabled);
        let queue_template = nptab
            .and_then(|n| {
                n.queue
                    .as_ref()
                    .and_then(|q| q.queue_template.clone())
                    .or_else(|| n.queue_template.clone())
            })
            .unwrap_or_default();

        let ht = ui.hometab.as_ref();
        let home_recent_albums_show_art = ht
            .and_then(|h| h.recent_albums.as_ref())
            .and_then(|r| r.show_art)
            .unwrap_or(true);
        let home_cover_fetch_max_px = match ht
            .and_then(|h| h.recent_albums.as_ref())
            .and_then(|r| r.cover_fetch_max_px)
        {
            None => 320,
            Some(0) => 0,
            Some(n) => n.clamp(64, 2048),
        };
        let home_top_height_percent = ht
            .and_then(|h| h.layout.as_ref())
            .and_then(|l| l.top_height_percent)
            .unwrap_or(50)
            .clamp(25, 75);
        let home_panels = parse_home_panels(
            ht.and_then(|h| h.layout.as_ref())
                .and_then(|l| l.panels.clone()),
        );

        let browse_mode = ui
            .browsetab
            .as_ref()
            .and_then(|b| b.mode.as_deref())
            .and_then(BrowseMode::parse)
            .unwrap_or_default();
        let browse_folder_navigation = ui
            .browsetab
            .as_ref()
            .and_then(|b| b.folder_navigation)
            .unwrap_or(false);
        let browse_mouse_wheel_scroll_lines = ui
            .browsetab
            .as_ref()
            .and_then(|b| b.mouse_wheel_scroll_lines)
            .unwrap_or(1)
            .max(1);

        let scrobble = &file_cfg.scrobble;
        let scrobble_enabled = scrobble.enabled;
        let scrobble_service = ratune_scrobble::ScrobbleService::parse(&scrobble.service)
            .unwrap_or(ratune_scrobble::ScrobbleService::LastFm);
        let (scrobble_api_key, scrobble_api_secret, scrobble_session_key) = if scrobble_enabled {
            resolve_scrobble_credentials(scrobble).unwrap_or_else(|e| {
                eprintln!("warning: scrobbling disabled — {e:#}");
                (String::new(), String::new(), String::new())
            })
        } else {
            (String::new(), String::new(), String::new())
        };
        let scrobble_enabled = scrobble_enabled
            && !scrobble_api_key.is_empty()
            && !scrobble_api_secret.is_empty()
            && !scrobble_session_key.is_empty();

        Ok(Config {
            subsonic_url: file_cfg.server.url,
            subsonic_user: file_cfg.server.username,
            subsonic_pass,
            default_volume: file_cfg.player.default_volume,
            max_bit_rate: file_cfg.player.max_bit_rate,
            mpris_enabled: file_cfg.player.mpris,
            keybinds: file_cfg.keybinds,
            theme: file_cfg.theme,
            lyrics_visible,
            visualizer_visible,
            lyrics_enabled,
            visualizer_enabled,
            queue_template,
            progress_style,
            nowplaying_show_art,
            album_art_backend: ui.album_art_backend,
            nowplaying_art_position,
            nowplaying_queue_position,
            nowplaying_left_width_percent,
            nowplaying_vertical_fill_top_percent,
            show_fzf_hint,
            lyrics_location,
            visualizer_location,
            visualizer_type,
            visualizer_fps,
            visualizer_fft_size,
            visualizer_gain_db,
            visualizer_colors,
            visualizer_color_mode,
            tab_bar_position,
            show_volume_indicator,
            now_playing_bar_height,
            now_playing_layout,
            now_playing_box_location,
            now_playing_show_controls,
            now_playing_show_progress,
            now_playing_box_include_controls,
            now_playing_box_include_progress,
            now_playing_lines_row,
            now_playing_lines_boxed,
            cache_enabled: file_cfg.cache.enabled,
            cache_max_size_gb: file_cfg.cache.max_size_gb,
            library_index_enabled: file_cfg.library.enabled,
            library_index_path: file_cfg.library.index_path,
            library_index_max_age_secs: file_cfg.library.max_age_secs,
            fzf_binary: file_cfg.library.fzf_binary,
            fzf_args: file_cfg.library.fzf_args,
            library_fetch_album_parallelism: file_cfg.library.fetch_album_parallelism.max(1),
            library_fetch_artist_parallelism: file_cfg.library.fetch_artist_parallelism.max(1),
            library_navidrome_skip_unchanged_scan: file_cfg.library.navidrome_skip_unchanged_scan,
            library_notify_on_forced_index_refresh: file_cfg.library.notify_on_forced_index_refresh,
            home_recent_albums_show_art,
            home_cover_fetch_max_px,
            home_top_height_percent,
            home_panels,
            browse_mode,
            browse_folder_navigation,
            browse_mouse_wheel_scroll_lines,
            scrobble_enabled,
            scrobble_service,
            scrobble_api_key,
            scrobble_api_secret,
            scrobble_session_key,
            scrobble_to_server: scrobble.scrobble_to_server,
            scrobble_local_threshold: scrobble.thresholds.local.resolve(),
            scrobble_audioscrobbler_rules: scrobble.thresholds.audioscrobbler.resolve(),
        })
    }

    /// Build an authenticated Audioscrobbler client when scrobbling is enabled.
    pub fn audioscrobbler_client(&self) -> Option<ratune_scrobble::AudioscrobblerClient> {
        if !self.scrobble_enabled {
            return None;
        }
        Some(ratune_scrobble::AudioscrobblerClient::new(
            self.scrobble_service,
            self.scrobble_api_key.clone(),
            self.scrobble_api_secret.clone(),
            self.scrobble_session_key.clone(),
        ))
    }

    /// Tab bar position and now-playing height for [`crate::ui::layout::build_layout`].
    pub fn layout_options(&self) -> crate::ui::layout::LayoutOptions {
        crate::ui::layout::LayoutOptions {
            tab_bar_top: self.tab_bar_position.trim().eq_ignore_ascii_case("top"),
            now_playing_bar_height: self.now_playing_bar_height,
        }
    }

    /// Resolved path for the JSON metadata index.
    pub fn resolved_library_index_path(&self) -> PathBuf {
        if self.library_index_path.trim().is_empty() {
            crate::library_index::default_index_path().unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                Path::new(&home)
                    .join(".cache")
                    .join("ratune")
                    .join("library_index.json")
            })
        } else {
            PathBuf::from(&self.library_index_path)
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn config_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("ratune"));
    }
    let home = std::env::var("HOME").context("HOME env var not set")?;
    Ok(PathBuf::from(home).join(".config").join("ratune"))
}

fn config_file_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

fn create_default(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config dir {}", parent.display()))?;
    }
    // Intentionally a small starter file (credentials + common toggles). Every key lives in
    // `docs/sample-config.toml` in the source tree — copy from there when you want the full menu.
    let default_toml = r##"[server]
url = ""
username = ""
password = ""

[player]
default_volume = 70
max_bit_rate = 0   # 0 = unlimited; set e.g. 320 to cap streaming bitrate
# mpris = true     # Linux: register on session D-Bus for media keys / playerctl (default: true)

[keybinds]
# Shift+letter: use "Shift+n" or "N" (same). Helps Ghostty/kitty vs. classic terminals.
# scroll_up     = "k"
# scroll_down   = "j"
# column_left   = "h"
# column_right  = "l"
# play_pause    = "p"
# next_track    = "n"
# prev_track    = "Shift+n"
# seek_forward  = "Right"
# seek_backward = "Left"
# add_track     = "a"
# add_all       = "Shift+a"
# add_all_replace_album  = "Ctrl+r"
# add_all_replace_artist = "Ctrl+Shift+r"
# add_all_prepend  = "Ctrl+Shift+p"
# shuffle       = "x"
# unshuffle     = "z"
# clear_queue   = "Shift+d"
# remove_from_queue = "d"
# search        = "/"
# volume_up     = "+"
# volume_down   = "-"
# tab_switch    = "Tab"
# tab_switch_reverse = "`"
# go_to_home    = "1"
# go_to_browser = "2"
# go_to_nowplaying = "3"
# quit          = "q"
# library_fzf     = "Ctrl+f"
# library_refresh = "Ctrl+g"
# library_index_append_queue = "Ctrl+a"   # append full index to queue (y/n); "" to disable
# toggle_help = "i"
# toggle_dynamic_theme = "t"
# toggle_lyrics = "Shift+l"
# toggle_visualizer = "Shift+v"
# playlist_overlay = "Shift+p"
# browser_add_to_playlist = ">"
# remove_from_playlist = "<"
# home_section_next = "Shift+j"
# home_section_prev = "Shift+k"
# home_refresh = "r"

[theme]
# accent        = "#ff8c00"   # highlighted items, active borders, progress fill
# background    = "#1a1a1a"   # popups, list fallbacks; legacy default for tab/status bars
# tab_bar       = "#1a1a1a"   # Home | Browse | Now Playing strip (falls back to background)
# status_bar    = "#1a1a1a"   # bottom status line (falls back to background)
# surface       = "#161616"   # panel backgrounds (browser columns, queue)
# surface       = "unset"     # omit painted panel bg (transparent terminals)
# foreground    = "#d4d0c8"   # primary text
# dimmed        = "#5a5858"   # muted / secondary text
# border        = "#252525"   # inactive pane borders
# border_active = "#3a3a3a"   # active pane borders
# preset = "dynamic"          # static | dynamic (default) | terminal | os

[ui]
# album_art_backend = "kitty-apc"   # default: ratatui-image

[ui.general]
tab_bar_position = "bottom"

[ui.row_now_playing]
bar_height = 4
layout = "row"
box_location = "right"
show_controls = true
show_progress = true
box_include_controls = false
box_include_progress = false
progress_style = "██░"
# ncmpcpp-style lines for the bottom strip (Home / Browse / NP row). Not used for the queue list.
# lines = ["$b%t$/b", "%a", "%b"]

[ui.hometab.recent_albums]
show_art = true
# cover_fetch_max_px = 320   # getCoverArt size (0 = full image; lower = faster)

[ui.hometab.layout]
top_height_percent = 50
panels = ["recent_albums", "recent_tracks", "rediscover"]

[ui.browsetab]
mode = "artists"

[ui.nptab]
# show_fzf_hint = false
# You can also set queue_template here; `[ui.nptab.queue]` wins if both are set.

[ui.nptab.art]
show = true
position = "left"

[ui.nptab.layout]
# left_width_percent = 50

[ui.nptab.queue]
position = "right"
# One template per queue ROW — `{title}`, `{n}`, `{artist}`, `{album}`, `{duration}`, `{suffix}`.
# NOT the same syntax as now-playing `lines` above (those use % / $ tags).
# queue_template = "{n}  {title:<40}  {artist:<25}  {duration:>5}"

[ui.nptab.lyrics_pane]
enabled = true
visible = false
location = "right"

[ui.nptab.visualizer_pane]
enabled = true
visible = false
location = "right"

[ui.nptab.now_playing]
# Overrides `row_now_playing` for the Now Playing tab (strip + boxed pane text).
# layout = "boxed"
# lines = ["$b%t$/b", "%a", "%b"]

[library]
# enabled = true
# index_path = ""          # empty = ~/.cache/ratune/library_index.json
# max_age_secs = 86400     # refresh in background when older (0 = always stale)
# fzf_binary = "fzf"       # or "sk" (skim gets --bind=ctrl-r:accept(ctrl-r) for replace-queue)
# fzf_args = ["--delimiter=\\t", "--with-nth=2,3,4,5", "--nth=1,2,3", "--multi", "--expect=ctrl-r", "--border=rounded"]
# aligned --header is added automatically unless you pass your own --header=…
# fetch_album_parallelism = 12    # concurrent getAlbum per artist during index refresh
# fetch_artist_parallelism = 4    # concurrent artists during index refresh
# navidrome_skip_unchanged_scan = false   # Navidrome: skip full walk when lastScan unchanged
# notify_on_forced_index_refresh = true   # desktop notification when forced refresh finishes

[cache]
enabled     = true
max_size_gb = 2   # maximum total cache size in gigabytes

# [scrobble]
# enabled = false
# service = "lastfm"          # or "librefm"
# api_key = ""
# api_secret = ""            # plaintext supported; or api_secret_command / keyring
# session_key = ""           # from `ratune scrobble-auth`; or session_key_command / keyring
# api_secret_command = ""
# session_key_command = ""
# scrobble_to_server = true   # Subsonic /scrobble for Navidrome play counts
#
# CLI helpers (see README § Scrobbling):
#   ratune scrobble-api-secret [--save-keyring]
#   ratune scrobble-auth [--save-keyring]
"##;
    std::fs::write(path, default_toml)
        .with_context(|| format!("writing default config to {}", path.display()))?;
    eprintln!("Created default config: {}", path.display());
    Ok(())
}

/// Keyring "user" field: `url|username` so multiple servers do not collide.
fn subsonic_keyring_user(server_url: &str, username: &str) -> String {
    format!("{}|{}", server_url.trim_end_matches('/'), username.trim())
}

/// Resolution order: plaintext `[server].password` (incl. env `SUBSONIC_PASS`), then
/// `[server].password_command`, then OS keyring (`ratune` / `subsonic_keyring_user`) or
/// interactive prompt via [`inquire`].
fn resolve_subsonic_secret(server: &ServerSection) -> Result<String> {
    let pass = server.password.trim();
    if !pass.is_empty() {
        return Ok(pass.to_string());
    }

    let cmd = server.password_command.trim();
    if !cmd.is_empty() {
        return run_password_command(cmd);
    }

    resolve_subsonic_secret_from_keyring(server)
}

fn run_password_command(shell_cmd: &str) -> Result<String> {
    let output = {
        #[cfg(unix)]
        {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(shell_cmd)
                .output()
        }
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", shell_cmd])
                .output()
        }
        #[cfg(not(any(unix, windows)))]
        {
            anyhow::bail!("password_command is not supported on this platform");
        }
    }
    .with_context(|| format!("running [server].password_command: {shell_cmd}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let hint = stderr.trim();
        bail!(
            "[server].password_command exited with {}: {}",
            output.status,
            if hint.is_empty() { "(no stderr)" } else { hint }
        );
    }

    let secret = String::from_utf8(output.stdout)
        .context("password_command stdout is not valid UTF-8")?
        .trim()
        .to_string();
    if secret.is_empty() {
        bail!("[server].password_command produced empty output");
    }
    Ok(secret)
}

/// OS keyring or one-time [`inquire`] prompt when `password` and `password_command` are empty.
fn resolve_subsonic_secret_from_keyring(server: &ServerSection) -> Result<String> {
    let url = server.url.trim();
    let user = server.username.trim();
    if url.is_empty() && user.is_empty() {
        bail!(
            "With empty [server].password and no password_command, set [server].url and [server].username (or SUBSONIC_URL and SUBSONIC_USER) for keyring auth — both are still empty."
        );
    }
    if url.is_empty() {
        bail!(
            "With empty [server].password and no password_command, [server].url must be set (or SUBSONIC_URL) for keyring auth."
        );
    }
    if user.is_empty() {
        bail!(
            "With empty [server].password and no password_command, [server].username must be set (or SUBSONIC_USER) for keyring auth."
        );
    }

    let label = subsonic_keyring_user(url, user);
    let backend = crate::keyring_init::parse_password_keyring(&server.password_keyring)
        .with_context(|| {
            format!(
                "invalid [server].password_keyring {:?}",
                server.password_keyring
            )
        })?;
    let entry = match crate::keyring_init::keyring_entry("ratune", &label, backend) {
        Ok(e) => e,
        Err(KeyringError::NoDefaultStore) => {
            eprintln!(
                "warning: {} keyring is not available on this platform or startup failed.",
                backend.label()
            );
            return inquire_subsonic_password_session();
        }
        Err(e) => {
            return Err(e).context(format!(
                "keyring entry (service ratune, {})",
                backend.label()
            ))
        }
    };

    match entry.get_password() {
        Ok(s) => {
            let t = s.trim();
            if t.is_empty() {
                prompt_and_store_subsonic_secret(&entry, backend)
            } else {
                Ok(t.to_string())
            }
        }
        Err(KeyringError::NoEntry) => prompt_and_store_subsonic_secret(&entry, backend),
        Err(e) if keyring_storage_unavailable(&e) => {
            eprintln!(
                "warning: {} keyring is not available ({e}).\n\
                 Using a one-time password prompt; the secret is not saved and applies only to this run.\n\
                 To persist without typing each time: set [server].password_command, [server].password, or SUBSONIC_PASS.",
                backend.label()
            );
            inquire_subsonic_password_session()
        }
        Err(e) => Err(e).context(format!(
            "reading Subsonic secret from {} keyring",
            backend.label()
        )),
    }
}

fn keyring_storage_unavailable(err: &KeyringError) -> bool {
    matches!(
        err,
        KeyringError::PlatformFailure(_) | KeyringError::NoStorageAccess(_)
    )
}

/// Prompt for secret; used when the keyring store cannot be used.
fn inquire_subsonic_password_session() -> Result<String> {
    inquire_plain_secret(Some(
        "Enter your Subsonic password or token (this session only — keyring unavailable).\n\
         API overview: https://www.navidrome.org/docs/developers/subsonic-api/",
    ))
}

fn inquire_plain_secret(prefix: Option<&str>) -> Result<String> {
    if let Some(s) = prefix {
        eprintln!("{s}");
    }
    let pw = Password::new("Subsonic password or token:")
        .without_confirmation()
        .prompt()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let pw = pw.trim();
    if pw.is_empty() {
        bail!("empty password");
    }
    Ok(pw.to_string())
}

fn prompt_and_store_subsonic_secret(
    entry: &keyring_core::Entry,
    backend: crate::keyring_init::KeyringBackend,
) -> Result<String> {
    let pw = inquire_plain_secret(Some(&format!(
        "No plaintext password in config — storing your Subsonic secret in the platform keyring \
         (service \"ratune\", user \"url|username\", backend: {}).\n\
         On macOS/Windows use the system credential UI to remove the entry if needed.\n\
         API overview: https://www.navidrome.org/docs/developers/subsonic-api/",
        backend.label()
    )))?;

    match entry.set_password(&pw) {
        Ok(()) => Ok(pw),
        Err(e) if keyring_storage_unavailable(&e) => {
            eprintln!(
                "warning: could not save to keyring ({e}). Using password for this session only."
            );
            Ok(pw)
        }
        Err(e) => Err(e).context("storing Subsonic secret in keyring"),
    }
}

fn merge_env_overrides(cfg: &mut FileConfig) {
    if let Ok(v) = std::env::var("TERMUSIC_SUBSONIC_URL").or_else(|_| std::env::var("SUBSONIC_URL"))
    {
        cfg.server.url = v;
    }
    if let Ok(v) =
        std::env::var("TERMUSIC_SUBSONIC_USER").or_else(|_| std::env::var("SUBSONIC_USER"))
    {
        cfg.server.username = v;
    }
    if let Ok(v) =
        std::env::var("TERMUSIC_SUBSONIC_PASS").or_else(|_| std::env::var("SUBSONIC_PASS"))
    {
        cfg.server.password = v;
    }
    if let Ok(v) = std::env::var("LASTFM_API_KEY").or_else(|_| std::env::var("LIBREFM_API_KEY")) {
        cfg.scrobble.api_key = v;
    }
    if let Ok(v) = std::env::var("LASTFM_API_SECRET")
        .or_else(|_| std::env::var("LASTFM_SHARED_SECRET"))
        .or_else(|_| std::env::var("LIBREFM_API_SECRET"))
    {
        cfg.scrobble.api_secret = v;
    }
    if let Ok(v) =
        std::env::var("LASTFM_SESSION_KEY").or_else(|_| std::env::var("LIBREFM_SESSION_KEY"))
    {
        cfg.scrobble.session_key = v;
    }
}

/// Resolve Audioscrobbler credentials when `[scrobble].enabled` is true.
fn resolve_scrobble_credentials(sec: &ScrobbleSection) -> Result<(String, String, String)> {
    let api_key = sec.api_key.trim();
    if api_key.is_empty() {
        bail!("[scrobble].api_key is empty (or set LASTFM_API_KEY / LIBREFM_API_KEY)");
    }
    let api_secret = resolve_scrobble_api_secret(sec)?;
    let session_key = resolve_scrobble_session_key(sec)?;
    Ok((api_key.to_string(), api_secret, session_key))
}

fn scrobble_service_name(service: ratune_scrobble::ScrobbleService) -> &'static str {
    match service {
        ratune_scrobble::ScrobbleService::LastFm => "lastfm",
        ratune_scrobble::ScrobbleService::LibreFm => "librefm",
    }
}

fn scrobble_keyring_label(service: ratune_scrobble::ScrobbleService, kind: &str) -> String {
    format!("{}|{kind}", scrobble_service_name(service))
}

fn resolve_scrobble_service(sec: &ScrobbleSection) -> ratune_scrobble::ScrobbleService {
    ratune_scrobble::ScrobbleService::parse(&sec.service)
        .unwrap_or(ratune_scrobble::ScrobbleService::LastFm)
}

fn resolve_scrobble_api_secret(sec: &ScrobbleSection) -> Result<String> {
    if !sec.api_secret.trim().is_empty() {
        return Ok(sec.api_secret.trim().to_string());
    }
    if !sec.api_secret_command.trim().is_empty() {
        return run_password_command(sec.api_secret_command.trim());
    }
    resolve_scrobble_api_secret_from_keyring(sec)
}

fn resolve_scrobble_api_secret_from_keyring(sec: &ScrobbleSection) -> Result<String> {
    read_scrobble_keyring(
        sec,
        "api_secret",
        "set api_secret / LASTFM_API_SECRET, api_secret_command, or run `ratune scrobble-api-secret --save-keyring`",
    )
}

fn resolve_scrobble_session_key(sec: &ScrobbleSection) -> Result<String> {
    if !sec.session_key.trim().is_empty() {
        return Ok(sec.session_key.trim().to_string());
    }
    if !sec.session_key_command.trim().is_empty() {
        return run_password_command(sec.session_key_command.trim());
    }
    resolve_scrobble_session_from_keyring(sec)
}

fn resolve_scrobble_session_from_keyring(sec: &ScrobbleSection) -> Result<String> {
    read_scrobble_keyring(
        sec,
        "session",
        "run `ratune scrobble-auth --save-keyring`, or set session_key / session_key_command / LASTFM_SESSION_KEY",
    )
}

fn read_scrobble_keyring(sec: &ScrobbleSection, kind: &str, hint: &str) -> Result<String> {
    use keyring_core::Error as KeyringError;

    let service = resolve_scrobble_service(sec);
    let label = scrobble_keyring_label(service, kind);
    let backend = crate::keyring_init::KeyringBackend::scrobble();
    let entry = match crate::keyring_init::keyring_entry("ratune", &label, backend) {
        Ok(e) => e,
        Err(KeyringError::NoDefaultStore) => {
            bail!("no {kind} configured — {hint}");
        }
        Err(e) => {
            return Err(e).context(format!(
                "keyring entry for scrobble {kind} ({})",
                backend.label()
            ));
        }
    };

    match entry.get_password() {
        Ok(s) => {
            let t = s.trim();
            if t.is_empty() {
                bail!("keyring entry for scrobble {kind} is empty");
            }
            Ok(t.to_string())
        }
        Err(KeyringError::NoEntry) => bail!("no {kind} in keyring — {hint}"),
        Err(e) => Err(e).context(format!("reading scrobble {kind} from keyring")),
    }
}

/// Persist an API shared secret in the OS keyring (service `ratune`).
pub fn store_scrobble_api_secret(
    service: ratune_scrobble::ScrobbleService,
    api_secret: &str,
) -> Result<()> {
    let secret = api_secret.trim();
    if secret.is_empty() {
        bail!("refusing to store empty API secret");
    }
    let label = scrobble_keyring_label(service, "api_secret");
    let backend = crate::keyring_init::KeyringBackend::scrobble();
    let entry = crate::keyring_init::keyring_entry("ratune", &label, backend).context(format!(
        "keyring entry for scrobble api_secret ({})",
        backend.label()
    ))?;
    entry
        .set_password(secret)
        .context("storing scrobble api_secret in keyring")?;
    Ok(())
}

/// Persist a scrobble session key in the OS keyring (service `ratune`).
pub fn store_scrobble_session_key(
    service: ratune_scrobble::ScrobbleService,
    session_key: &str,
) -> Result<()> {
    let key = session_key.trim();
    if key.is_empty() {
        bail!("refusing to store empty session key");
    }
    let label = scrobble_keyring_label(service, "session");
    let backend = crate::keyring_init::KeyringBackend::scrobble();
    let entry = crate::keyring_init::keyring_entry("ratune", &label, backend).context(format!(
        "keyring entry for scrobble session ({})",
        backend.label()
    ))?;
    entry
        .set_password(key)
        .context("storing scrobble session key in keyring")?;
    Ok(())
}

/// Load `[scrobble]` application credentials for the browser auth flow.
///
/// Does not require a session key or Subsonic password.
pub fn load_scrobble_app_credentials() -> Result<(ratune_scrobble::ScrobbleService, String, String)>
{
    let config_path = config_file_path()?;
    if !config_path.exists() {
        bail!(
            "config file not found at {} — create it first (ratune writes a starter on first run)",
            config_path.display()
        );
    }
    let text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let mut file_cfg: FileConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", config_path.display()))?;
    merge_env_overrides(&mut file_cfg);

    let sec = &file_cfg.scrobble;
    let (service, api_key) = {
        let service = resolve_scrobble_service(sec);
        let api_key = sec.api_key.trim();
        if api_key.is_empty() {
            bail!(
                "[scrobble].api_key is empty in {} (or set LASTFM_API_KEY / LIBREFM_API_KEY)",
                config_path.display()
            );
        }
        (service, api_key.to_string())
    };
    let api_secret = resolve_scrobble_api_secret(sec)?;

    Ok((service, api_key, api_secret))
}

/// Load service + application API key (for `scrobble-auth` / store helpers).
pub fn load_scrobble_api_key() -> Result<(ratune_scrobble::ScrobbleService, String)> {
    let config_path = config_file_path()?;
    if !config_path.exists() {
        bail!(
            "config file not found at {} — create it first (ratune writes a starter on first run)",
            config_path.display()
        );
    }
    let text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let mut file_cfg: FileConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", config_path.display()))?;
    merge_env_overrides(&mut file_cfg);

    let sec = &file_cfg.scrobble;
    let service = resolve_scrobble_service(sec);
    let api_key = sec.api_key.trim();
    if api_key.is_empty() {
        bail!(
            "[scrobble].api_key is empty in {} (or set LASTFM_API_KEY / LIBREFM_API_KEY)",
            config_path.display()
        );
    }
    Ok((service, api_key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_toml_as_defaults() {
        let fc: FileConfig = toml::from_str("").expect("empty");
        assert!(fc.server.url.is_empty());
        assert!(fc.cache.enabled);
    }

    #[test]
    fn parses_server_credentials_block() {
        let text = r#"
[server]
url = "http://music.example:4533"
username = "alice"
password = "secret"
"#;
        let fc: FileConfig = toml::from_str(text).expect("toml");
        assert_eq!(fc.server.url, "http://music.example:4533");
        assert_eq!(fc.server.username, "alice");
        assert_eq!(fc.server.password, "secret");
    }

    #[test]
    fn parses_password_command() {
        let text = r#"
[server]
password_command = "secret-tool lookup service ratune"
"#;
        let fc: FileConfig = toml::from_str(text).expect("toml");
        assert_eq!(
            fc.server.password_command,
            "secret-tool lookup service ratune"
        );
    }

    #[test]
    fn parses_password_keyring() {
        let text = r#"
[server]
password_keyring = "secret-service"
"#;
        let fc: FileConfig = toml::from_str(text).expect("toml");
        assert_eq!(fc.server.password_keyring, "secret-service");
        assert_eq!(
            crate::keyring_init::parse_password_keyring(&fc.server.password_keyring)
                .expect("parse"),
            crate::keyring_init::KeyringBackend::SecretService
        );
    }

    #[test]
    fn password_keyring_defaults_to_keyutils() {
        let fc: FileConfig = toml::from_str("[server]\nurl = \"x\"\n").expect("toml");
        assert_eq!(fc.server.password_keyring, "keyutils");
    }

    #[test]
    #[cfg(unix)]
    fn password_command_shell_output() {
        let server = ServerSection {
            url: String::new(),
            username: String::new(),
            password: String::new(),
            password_command: "printf '%s' 'from-cmd'".into(),
            password_keyring: default_password_keyring(),
        };
        assert_eq!(
            resolve_subsonic_secret(&server).expect("command"),
            "from-cmd"
        );
    }

    #[test]
    #[cfg(unix)]
    fn api_secret_command_shell_output() {
        let sec = ScrobbleSection {
            enabled: false,
            service: "lastfm".into(),
            api_key: "key".into(),
            api_secret: String::new(),
            api_secret_command: "printf '%s' 'shh-secret'".into(),
            session_key: String::new(),
            session_key_command: String::new(),
            scrobble_to_server: true,
            thresholds: ScrobbleThresholdsSection::default(),
        };
        assert_eq!(
            resolve_scrobble_api_secret(&sec).expect("command"),
            "shh-secret"
        );
    }

    #[test]
    fn parses_scrobble_thresholds() {
        let text = r#"
[scrobble.thresholds.local]
min_percent = 40
max_listen_seconds = 45

[scrobble.thresholds.audioscrobbler]
min_percent = 60
max_listen_seconds = 180
min_track_seconds = 20
"#;
        let fc: FileConfig = toml::from_str(text).expect("toml");
        assert_eq!(fc.scrobble.thresholds.local.min_percent, 40);
        assert_eq!(fc.scrobble.thresholds.local.max_listen_seconds, 45);
        assert_eq!(fc.scrobble.thresholds.audioscrobbler.min_percent, 60);
        assert_eq!(
            fc.scrobble.thresholds.audioscrobbler.max_listen_seconds,
            180
        );
        assert_eq!(fc.scrobble.thresholds.audioscrobbler.min_track_seconds, 20);
        let local = fc.scrobble.thresholds.local.resolve();
        assert_eq!(local.min_percent, 40);
        assert_eq!(local.max_listen, std::time::Duration::from_secs(45));
        let rules = fc.scrobble.thresholds.audioscrobbler.resolve();
        assert_eq!(rules.listen.min_percent, 60);
        assert_eq!(rules.listen.max_listen, std::time::Duration::from_secs(180));
        assert_eq!(rules.min_track_length, std::time::Duration::from_secs(20));
    }

    #[test]
    fn scrobble_threshold_defaults_match_lastfm() {
        let sec = ScrobbleThresholdsSection::default();
        let local = sec.local.resolve();
        assert_eq!(local, ratune_scrobble::ListenThreshold::local_default());
        let rules = sec.audioscrobbler.resolve();
        assert_eq!(rules, ratune_scrobble::AudioscrobblerRules::default());
    }

    #[test]
    fn parses_scrobble_api_secret_command() {
        let text = r#"
[scrobble]
api_secret_command = "secret-tool lookup service ratune user lastfm|api_secret"
"#;
        let fc: FileConfig = toml::from_str(text).expect("toml");
        assert_eq!(
            fc.scrobble.api_secret_command,
            "secret-tool lookup service ratune user lastfm|api_secret"
        );
    }

    #[test]
    fn browse_mode_parses_artists() {
        assert_eq!(BrowseMode::parse("artists"), Some(BrowseMode::Artists));
        assert_eq!(BrowseMode::parse("bogus"), None);
    }
}
