use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

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
}

// ── [keybinds] ────────────────────────────────────────────────────────────────

/// Raw keybind strings from config.toml. Every field is `Option<String>`;
/// unset fields fall back to built-in defaults inside `Keybinds::from_section`.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct KeybindsSection {
    pub scroll_up:     Option<String>,
    pub scroll_down:   Option<String>,
    pub column_left:   Option<String>,
    pub column_right:  Option<String>,
    pub play_pause:    Option<String>,
    pub next_track:    Option<String>,
    pub prev_track:    Option<String>,
    pub seek_forward:  Option<String>,
    pub seek_backward: Option<String>,
    pub add_track:     Option<String>,
    pub add_all:       Option<String>,
    pub shuffle:       Option<String>,
    pub unshuffle:     Option<String>,
    pub clear_queue:   Option<String>,
    pub search:        Option<String>,
    pub volume_up:     Option<String>,
    pub volume_down:   Option<String>,
    pub tab_switch:         Option<String>,
    /// Reverse tab cycle (Backtick by default)
    pub tab_switch_reverse: Option<String>,
    /// Jump to Home tab (default: '1')
    pub go_to_home:         Option<String>,
    /// Jump to Browser tab (default: '2')
    pub go_to_browser:      Option<String>,
    /// Jump to NowPlaying tab (default: '3')
    pub go_to_nowplaying:   Option<String>,
    pub quit:               Option<String>,
    /// Fuzzy track picker (metadata index). Default: Ctrl+f
    pub library_fzf:        Option<String>,
    /// Force library index refresh. Default: Ctrl+r
    pub library_refresh:    Option<String>,
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

fn default_cache_enabled() -> bool { true }
fn default_cache_max_size_gb() -> f64 { 2.0 }

impl Default for CacheSection {
    fn default() -> Self {
        Self { enabled: default_cache_enabled(), max_size_gb: default_cache_max_size_gb() }
    }
}

// ── [library] — metadata index + fzf picker ───────────────────────────────────

/// Local library metadata index and fuzzy picker (Milestone 2).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LibrarySection {
    /// Build and use the on-disk index for fzf. Default: true.
    #[serde(default = "default_library_enabled")]
    pub enabled: bool,
    /// Path to `library_index.json`. Empty = `~/.cache/playterm/library_index.json`.
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
    /// `getScanStatus.lastScan`, skip the full API walk (still obeys Ctrl+r force refresh).
    #[serde(default)]
    pub navidrome_skip_unchanged_scan: bool,
    /// After a forced index refresh (Ctrl+r), send a desktop notification (FreeDesktop
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

/// App-wide UI (all tabs): tab strip, and other cross-tab options.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiGeneralSection {
    /// Tab strip: `bottom` (default) or `top` (still above the 1-row status bar).
    #[serde(default)]
    pub tab_bar_position: Option<String>,
}

/// Optional nested `[ui.now_playing]` table.
///
/// This lets configs group the now-playing settings separately:
///
/// ```toml
/// [ui]
/// lyrics = false
///
/// [ui.now_playing]
/// bar_height = 4
/// layout = "row"
/// # …
/// ```
///
/// All fields here are optional. When present, they override the corresponding
/// flat `[ui]` keys (for backward compatibility).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNowPlayingSection {
    /// Height in terminal rows for the now-playing region (clamped when building layout).
    #[serde(default)]
    pub bar_height: Option<u16>,
    /// `row` = full-width three-column strip (default); `boxed` = bordered panel like Visualizer.
    #[serde(default)]
    pub layout: Option<String>,
    /// When `layout` is `boxed`: dock under `queue` (default) or `art` (requires `show_art`).
    #[serde(default)]
    pub box_location: Option<String>,
    /// Show transport controls (⇄ ⏮ ▶/⏸ ⏭ ↻).
    #[serde(default)]
    pub show_controls: Option<bool>,
    /// Show elapsed / progress / total.
    #[serde(default)]
    pub show_progress: Option<bool>,
    /// When `layout` is `boxed`: draw controls inside the bordered area.
    #[serde(default)]
    pub box_include_controls: Option<bool>,
    /// When `layout` is `boxed`: draw the progress line inside the bordered area.
    #[serde(default)]
    pub box_include_progress: Option<bool>,
    /// Now-playing metadata lines (ncmpcpp-style `%` / `$`; one string per line).
    #[serde(default)]
    pub lines: Option<Vec<String>>,
    /// Now-playing bar glyphs.
    #[serde(default)]
    pub progress_style: Option<String>,
    /// NowPlaying tab: show the album-art column.
    #[serde(default)]
    pub show_art: Option<bool>,
    /// NowPlaying tab: album art column side. Use `left` or `right` (case-insensitive).
    #[serde(default)]
    pub art_position: Option<String>,
    /// NowPlaying tab: album art column width percentage (1–99).
    #[serde(default)]
    pub art_width_percent: Option<u8>,
    /// If true, show a small fzf picker hint when the queue is empty (only when library fzf is enabled).
    #[serde(default)]
    pub show_fzf_hint: Option<bool>,
    /// When the visualizer is open: `queue` = split under the queue column (default);
    /// `art` = split under the album-art column (only if `show_art` is true).
    #[serde(default)]
    pub visualizer_location: Option<String>,
    /// Tab strip: `bottom` (default) or `top` (below the top edge, above main content).
    #[serde(default)]
    pub tab_bar_position: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabArtSection {
    #[serde(default)]
    pub show: Option<bool>,
    #[serde(default)]
    pub position: Option<String>,
    #[serde(default)]
    pub width_percent: Option<u8>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabVisualizerSection {
    /// If false, the visualizer cannot be shown or toggled (`V`). Default: true.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Start with the spectrum visualizer overlay visible (toggle `V`).
    #[serde(default)]
    pub visible: Option<bool>,
    /// When open: `queue` or `art` (under queue column vs album-art column).
    #[serde(default)]
    pub location: Option<String>,
}

/// Now Playing tab: overrides for the bottom strip + boxed pane (see `[ui.row_now_playing]` for shared defaults).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabNowPlayingSection {
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
    /// ncmpcpp-style lines for the **boxed** NP pane only; omit to reuse row strip templates.
    #[serde(default)]
    pub lines: Option<Vec<String>>,
}

/// Shared defaults for the bottom **now-playing strip** (used on Home, Browse, and Now Playing).
///
/// Precedence: `[ui.nptab]` overrides these when set, then `[ui.now_playing]`, then legacy flat `[ui]`.
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
    /// `artists` (default), `genre`, or `files`. Genre/files are placeholders until implemented.
    #[serde(default)]
    pub mode: Option<String>,
}

/// Optional nested `[ui.nptab]` (Now Playing tab) table.
///
/// All fields are optional; when present, they override `[ui.row_now_playing]` / legacy flat `[ui]`.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UiNpTabSection {
    /// Start with the lyrics overlay visible (toggle `L`).
    #[serde(default)]
    pub lyrics: Option<bool>,
    /// Whether lyrics can be toggled at all.
    #[serde(default)]
    pub lyrics_enabled: Option<bool>,
    /// Queue row template (Now Playing tab queue column).
    #[serde(default)]
    pub queue_template: Option<String>,
    /// Empty-queue hint (only when library fzf is enabled).
    #[serde(default)]
    pub show_fzf_hint: Option<bool>,
    /// Album art settings for the Now Playing tab.
    #[serde(default)]
    pub art: Option<UiNpTabArtSection>,
    /// Visualizer: feature toggle, startup visibility, pane docking.
    #[serde(default)]
    pub visualizer_pane: Option<UiNpTabVisualizerSection>,
    /// Bottom strip layout + boxed pane text (overrides `row_now_playing`).
    #[serde(default)]
    pub now_playing: Option<UiNpTabNowPlayingSection>,
}

/// UI preferences from config.toml.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UiSection {
    /// App-wide settings (all tabs). Prefer this over tab-specific tables for global behavior.
    #[serde(default)]
    pub general: Option<UiGeneralSection>,
    /// Start with the lyrics overlay **visible** on the Now Playing tab (key `L` toggles).
    /// Default: false.
    #[serde(default)]
    pub lyrics: bool,
    /// Start with the spectrum visualizer overlay **visible** on the Now Playing tab (key `V` toggles).
    /// Default: false.
    #[serde(default)]
    pub visualizer: bool,
    /// If false, lyrics cannot be shown or toggled (hides the feature). Default: true.
    #[serde(default = "default_ui_lyrics_enabled")]
    pub lyrics_enabled: bool,
    /// If false, the visualizer cannot be shown or toggled (hides the feature). Default: true.
    #[serde(default = "default_ui_visualizer_enabled")]
    pub visualizer_enabled: bool,
    /// Queue row display template.
    ///
    /// Supported placeholders:
    /// - `{n}` (track #, dotted like `  1.`; blank when unknown)
    /// - `{title}` `{artist}` `{album}` `{duration}` (e.g. `3:04`)
    ///
    /// Optional format mini-spec:
    /// - alignment + width: `{title:<40}` `{artist:<25}` `{duration:>5}`
    /// - width only defaults to left align: `{album:30}`
    ///
    /// Default (empty) matches the current fixed columns.
    #[serde(default)]
    pub queue_template: String,
    /// Now-playing bar: **three** characters. If the first two match (e.g. `██░`), use the
    /// fractional Unicode block bar (original look). If they differ (e.g. `=>-`), ncmpcpp-style
    /// playhead. Empty string = no bar. Invalid length falls back to `██░`.
    #[serde(default = "default_ui_progress_style")]
    pub progress_style: String,
    /// NowPlaying tab: show the album-art column (placeholder today). Default: true.
    #[serde(default = "default_ui_nowplaying_show_art")]
    pub nowplaying_show_art: bool,
    /// NowPlaying tab: album art column side. Use `left` or `right` (case-insensitive; default: left).
    #[serde(default = "default_ui_nowplaying_art_position")]
    pub nowplaying_art_position: String,
    /// NowPlaying tab: album art column width percentage (1–99). Default: 50.
    #[serde(default = "default_ui_nowplaying_art_width_percent")]
    pub nowplaying_art_width_percent: u8,
    /// If true, show a small fzf picker hint when the queue is empty (only when library fzf is enabled).
    /// Default: false.
    #[serde(default = "default_ui_show_fzf_hint")]
    pub show_fzf_hint: bool,
    /// When the visualizer is open: `queue` = split under the queue column (default);
    /// `art` = split under the album-art column (only if `nowplaying_show_art` is true).
    #[serde(default = "default_ui_visualizer_location")]
    pub visualizer_location: String,
    /// Tab strip: `bottom` (default) or `top`. Also settable under `[ui.general]`.
    #[serde(default = "default_ui_tab_bar_position")]
    pub tab_bar_position: String,
    /// Height in terminal rows for the now-playing region (clamped when building layout). Default: 4.
    #[serde(default = "default_ui_now_playing_bar_height")]
    pub now_playing_bar_height: u16,
    /// `row` = full-width three-column strip (default); `boxed` = bordered panel like Visualizer.
    #[serde(default = "default_ui_now_playing_layout")]
    pub now_playing_layout: String,
    /// When `now_playing_layout` is `boxed`: dock under `queue` (default) or `art` (requires `nowplaying_show_art`).
    #[serde(default = "default_ui_now_playing_box_location")]
    pub now_playing_box_location: String,
    /// Show transport controls (⇄ ⏮ ▶/⏸ ⏭ ↻). Default: true.
    #[serde(default = "default_ui_now_playing_show_controls")]
    pub now_playing_show_controls: bool,
    /// Show elapsed / progress / total. Default: true.
    #[serde(default = "default_ui_now_playing_show_progress")]
    pub now_playing_show_progress: bool,
    /// When `now_playing_layout` is `boxed`: draw controls inside the bordered area.
    #[serde(default)]
    pub now_playing_box_include_controls: bool,
    /// When `now_playing_layout` is `boxed`: draw the progress line inside the bordered area.
    #[serde(default)]
    pub now_playing_box_include_progress: bool,
    /// Legacy: now-playing **row** metadata lines (Home/Browse strip and NP row path). Prefer
    /// `[ui.row_now_playing].lines`. Empty = built-in default.
    ///
    /// Extra placeholders: `%P` sum of known queue track durations; `%i` / `%j` current index (1-based)
    /// and queue length; `%v` volume 0–100; `%K` stream bitrate (kbps) when the server reports it.
    #[serde(default = "default_now_playing_lines")]
    pub now_playing_lines: Vec<String>,
    /// Optional nested `[ui.now_playing]` table. When present, its fields override the flat fields
    /// above (e.g. `bar_height` beats `now_playing_bar_height`). This keeps older configs working
    /// while letting newer ones group now-playing settings more cleanly.
    #[serde(default)]
    pub now_playing: Option<UiNowPlayingSection>,
    /// Optional nested `[ui.nptab]` table (Now Playing tab). When present, its fields override both
    /// `[ui.now_playing]` and the legacy flat `[ui]` keys.
    #[serde(default)]
    pub nptab: Option<UiNpTabSection>,
    /// Shared bottom strip defaults (see [`UiRowNowPlayingSection`]).
    #[serde(default)]
    pub row_now_playing: Option<UiRowNowPlayingSection>,
    /// Home tab layout and recently played.
    #[serde(default)]
    pub hometab: Option<UiHomeTabSection>,
    /// Browse tab mode (artists vs future genre/files).
    #[serde(default)]
    pub browsetab: Option<UiBrowseTabSection>,
}

fn default_ui_lyrics_enabled() -> bool { true }
fn default_ui_visualizer_enabled() -> bool { true }
fn default_ui_progress_style() -> String { "██░".into() }
fn default_ui_nowplaying_show_art() -> bool { true }
fn default_ui_nowplaying_art_position() -> String { "left".into() }
fn default_ui_nowplaying_art_width_percent() -> u8 { 50 }
fn default_ui_show_fzf_hint() -> bool { false }
fn default_ui_visualizer_location() -> String { "queue".into() }
fn default_ui_tab_bar_position() -> String { "bottom".into() }
fn default_ui_now_playing_bar_height() -> u16 { 4 }
fn default_ui_now_playing_layout() -> String { "row".into() }
fn default_ui_now_playing_box_location() -> String { "queue".into() }
fn default_ui_now_playing_show_controls() -> bool { true }
fn default_ui_now_playing_show_progress() -> bool { true }

fn default_now_playing_lines() -> Vec<String> {
    vec!["$b%t$/b".into(), "%a".into(), "%b".into()]
}

impl Default for UiSection {
    fn default() -> Self {
        Self {
            lyrics: false,
            visualizer: false,
            general: None,
            lyrics_enabled: default_ui_lyrics_enabled(),
            visualizer_enabled: default_ui_visualizer_enabled(),
            queue_template: String::new(),
            progress_style: default_ui_progress_style(),
            nowplaying_show_art: default_ui_nowplaying_show_art(),
            nowplaying_art_position: default_ui_nowplaying_art_position(),
            nowplaying_art_width_percent: default_ui_nowplaying_art_width_percent(),
            show_fzf_hint: default_ui_show_fzf_hint(),
            visualizer_location: default_ui_visualizer_location(),
            tab_bar_position: default_ui_tab_bar_position(),
            now_playing_bar_height: default_ui_now_playing_bar_height(),
            now_playing_layout: default_ui_now_playing_layout(),
            now_playing_box_location: default_ui_now_playing_box_location(),
            now_playing_show_controls: default_ui_now_playing_show_controls(),
            now_playing_show_progress: default_ui_now_playing_show_progress(),
            now_playing_box_include_controls: false,
            now_playing_box_include_progress: false,
            now_playing_lines: default_now_playing_lines(),
            now_playing: None,
            nptab: None,
            row_now_playing: None,
            hometab: None,
            browsetab: None,
        }
    }
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
    [HomePanel::RecentAlbums, HomePanel::RecentTracks, HomePanel::Rediscover]
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

/// Browse tab mode (`artists` is the only fully implemented path today).
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

/// Raw hex colour strings from config.toml. Defaults inside `Theme::from_section`.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct ThemeSection {
    pub accent:        Option<String>,
    pub background:    Option<String>,
    pub surface:       Option<String>,
    pub foreground:    Option<String>,
    pub dimmed:        Option<String>,
    pub border:        Option<String>,
    pub border_active: Option<String>,
    /// Whether to extract and apply a dynamic accent colour from album art.
    /// Default: true.
    pub dynamic:       Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct ServerSection {
    #[serde(default)]
    url: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
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

fn default_volume() -> u8 { 70 }

fn default_mpris() -> bool {
    true
}

// ── Runtime config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Config {
    pub subsonic_url:   String,
    pub subsonic_user:  String,
    pub subsonic_pass:  String,
    pub default_volume: u8,
    pub max_bit_rate:   u32,
    /// Linux: register MPRIS on the session bus (media keys, `playerctl`).
    pub mpris_enabled:  bool,
    /// Raw keybind strings — parsed into `Keybinds` by `App::new`.
    pub keybinds: KeybindsSection,
    /// Raw theme colour strings — parsed into `Theme` by `App::new`.
    pub theme:    ThemeSection,
    /// Whether to show the lyrics overlay on startup.
    pub lyrics_visible: bool,
    /// Whether to show the spectrum visualizer overlay on startup.
    pub visualizer_visible: bool,
    /// Whether the lyrics overlay feature can be toggled at all.
    pub lyrics_enabled: bool,
    /// Whether the visualizer overlay feature can be toggled at all.
    pub visualizer_enabled: bool,
    /// Queue row display template; empty = default (current behavior).
    pub queue_template: String,
    /// Now-playing bar glyphs (see `[ui].progress_style`).
    pub progress_style: String,
    /// NowPlaying tab: show the album-art column.
    pub nowplaying_show_art: bool,
    /// NowPlaying tab: album art side ("left" or "right").
    pub nowplaying_art_position: String,
    /// NowPlaying tab: album art width percentage.
    pub nowplaying_art_width_percent: u8,
    /// Show fzf picker hints in the UI where relevant.
    pub show_fzf_hint: bool,
    /// Where the visualizer pane appears ("queue" or "art").
    pub visualizer_location: String,
    /// Tab bar at top (`top`) or bottom (`bottom`).
    pub tab_bar_position: String,
    /// Now-playing bar height in rows.
    pub now_playing_bar_height: u16,
    /// `row` or `boxed` now-playing layout.
    pub now_playing_layout: String,
    /// `queue` or `art` — boxed pane dock (when layout is boxed).
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
    pub cache_enabled:     bool,
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
    /// Home tab: top band height as percent of the content area (25–75).
    pub home_top_height_percent: u8,
    /// Home tab: `[top, bottom_left, bottom_right]` panel assignment.
    pub home_panels: [HomePanel; 3],
    /// Browse tab: `artists` (default), or placeholder `genre` / `files`.
    pub browse_mode: BrowseMode,
}

impl Config {
    /// Load config from `~/.config/playterm/config.toml`, creating a default
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
        let mut file_cfg: FileConfig = toml::from_str(&text)
            .with_context(|| format!("parsing {}", config_path.display()))?;

        // Env vars override file values.
        merge_env_overrides(&mut file_cfg);

        // Validate password.
        if file_cfg.server.password.is_empty() {
            bail!(
                "No Subsonic password configured.\n\
                 Edit {} or set TERMUSIC_SUBSONIC_PASS.",
                config_path.display()
            );
        }

        let ui = &file_cfg.ui;
        let legacy_np = ui.now_playing.as_ref();
        let nptab = ui.nptab.as_ref();
        let row = ui.row_now_playing.as_ref();

        // Strip fields — `[ui.nptab]` > `[ui.row_now_playing]` (shared Home/Browse/NP defaults) >
        // `[ui.now_playing]` > flat `[ui]`.
        let progress_style = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|n| n.progress_style.clone())
            .or_else(|| row.and_then(|r| r.progress_style.clone()))
            .or_else(|| legacy_np.and_then(|n| n.progress_style.clone()))
            .unwrap_or_else(|| ui.progress_style.clone());

        let nowplaying_show_art = nptab
            .and_then(|n| n.art.as_ref())
            .and_then(|a| a.show)
            .or_else(|| legacy_np.and_then(|n| n.show_art))
            .unwrap_or(ui.nowplaying_show_art);

        let nowplaying_art_position = nptab
            .and_then(|n| n.art.as_ref())
            .and_then(|a| a.position.clone())
            .or_else(|| legacy_np.and_then(|n| n.art_position.clone()))
            .unwrap_or_else(|| ui.nowplaying_art_position.clone());

        let nowplaying_art_width_percent = nptab
            .and_then(|n| n.art.as_ref())
            .and_then(|a| a.width_percent)
            .or_else(|| legacy_np.and_then(|n| n.art_width_percent))
            .unwrap_or(ui.nowplaying_art_width_percent);

        let show_fzf_hint = nptab
            .and_then(|n| n.show_fzf_hint)
            .or_else(|| legacy_np.and_then(|n| n.show_fzf_hint))
            .unwrap_or(ui.show_fzf_hint);

        let visualizer_location = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.location.clone())
            .or_else(|| legacy_np.and_then(|n| n.visualizer_location.clone()))
            .unwrap_or_else(|| ui.visualizer_location.clone());

        let tab_bar_position = ui
            .general
            .as_ref()
            .and_then(|g| g.tab_bar_position.clone())
            .or_else(|| legacy_np.and_then(|n| n.tab_bar_position.clone()))
            .unwrap_or_else(|| ui.tab_bar_position.clone());

        let now_playing_bar_height = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.bar_height)
            .or_else(|| row.and_then(|r| r.bar_height))
            .or_else(|| legacy_np.and_then(|n| n.bar_height))
            .unwrap_or(ui.now_playing_bar_height);

        let now_playing_layout = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.layout.clone())
            .or_else(|| row.and_then(|r| r.layout.clone()))
            .or_else(|| legacy_np.and_then(|n| n.layout.clone()))
            .unwrap_or_else(|| ui.now_playing_layout.clone());

        let now_playing_box_location = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.box_location.clone())
            .or_else(|| row.and_then(|r| r.box_location.clone()))
            .or_else(|| legacy_np.and_then(|n| n.box_location.clone()))
            .unwrap_or_else(|| ui.now_playing_box_location.clone());

        let now_playing_show_controls = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.show_controls)
            .or_else(|| row.and_then(|r| r.show_controls))
            .or_else(|| legacy_np.and_then(|n| n.show_controls))
            .unwrap_or(ui.now_playing_show_controls);

        let now_playing_show_progress = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.show_progress)
            .or_else(|| row.and_then(|r| r.show_progress))
            .or_else(|| legacy_np.and_then(|n| n.show_progress))
            .unwrap_or(ui.now_playing_show_progress);

        let now_playing_box_include_controls = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.box_include_controls)
            .or_else(|| row.and_then(|r| r.box_include_controls))
            .or_else(|| legacy_np.and_then(|n| n.box_include_controls))
            .unwrap_or(ui.now_playing_box_include_controls);

        let now_playing_box_include_progress = nptab
            .and_then(|n| n.now_playing.as_ref())
            .and_then(|l| l.box_include_progress)
            .or_else(|| row.and_then(|r| r.box_include_progress))
            .or_else(|| legacy_np.and_then(|n| n.box_include_progress))
            .unwrap_or(ui.now_playing_box_include_progress);

        // Row strip (never uses `[ui.nptab.now_playing].lines` — that is boxed-only).
        let now_playing_lines_row = {
            let from_row = row.and_then(|r| r.lines.clone());
            let from_legacy_np = legacy_np.and_then(|n| n.lines.clone());
            let mut lines = from_row
                .or(from_legacy_np)
                .unwrap_or_else(|| ui.now_playing_lines.clone());
            if lines.is_empty() {
                lines = default_now_playing_lines();
            }
            lines
        };

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

        let lyrics_visible = nptab
            .and_then(|n| n.lyrics)
            .unwrap_or(ui.lyrics);
        let lyrics_enabled = nptab
            .and_then(|n| n.lyrics_enabled)
            .unwrap_or(ui.lyrics_enabled);
        let visualizer_visible = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.visible)
            .unwrap_or(ui.visualizer);
        let visualizer_enabled = nptab
            .and_then(|n| n.visualizer_pane.as_ref())
            .and_then(|v| v.enabled)
            .unwrap_or(ui.visualizer_enabled);
        let queue_template = nptab
            .and_then(|n| n.queue_template.clone())
            .unwrap_or_else(|| ui.queue_template.clone());

        let ht = ui.hometab.as_ref();
        let home_recent_albums_show_art = ht
            .and_then(|h| h.recent_albums.as_ref())
            .and_then(|r| r.show_art)
            .unwrap_or(true);
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

        Ok(Config {
            subsonic_url:      file_cfg.server.url,
            subsonic_user:     file_cfg.server.username,
            subsonic_pass:     file_cfg.server.password,
            default_volume:    file_cfg.player.default_volume,
            max_bit_rate:      file_cfg.player.max_bit_rate,
            mpris_enabled:     file_cfg.player.mpris,
            keybinds:          file_cfg.keybinds,
            theme:             file_cfg.theme,
            lyrics_visible,
            visualizer_visible,
            lyrics_enabled,
            visualizer_enabled,
            queue_template,
            progress_style,
            nowplaying_show_art,
            nowplaying_art_position,
            nowplaying_art_width_percent,
            show_fzf_hint,
            visualizer_location,
            tab_bar_position,
            now_playing_bar_height,
            now_playing_layout,
            now_playing_box_location,
            now_playing_show_controls,
            now_playing_show_progress,
            now_playing_box_include_controls,
            now_playing_box_include_progress,
            now_playing_lines_row,
            now_playing_lines_boxed,
            cache_enabled:     file_cfg.cache.enabled,
            cache_max_size_gb: file_cfg.cache.max_size_gb,
            library_index_enabled: file_cfg.library.enabled,
            library_index_path:    file_cfg.library.index_path,
            library_index_max_age_secs: file_cfg.library.max_age_secs,
            fzf_binary:            file_cfg.library.fzf_binary,
            fzf_args:              file_cfg.library.fzf_args,
            library_fetch_album_parallelism: file_cfg.library.fetch_album_parallelism.max(1),
            library_fetch_artist_parallelism: file_cfg.library.fetch_artist_parallelism.max(1),
            library_navidrome_skip_unchanged_scan: file_cfg.library.navidrome_skip_unchanged_scan,
            library_notify_on_forced_index_refresh: file_cfg.library.notify_on_forced_index_refresh,
            home_recent_albums_show_art,
            home_top_height_percent,
            home_panels,
            browse_mode,
        })
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
                    .join("playterm")
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
        return Ok(PathBuf::from(xdg).join("playterm"));
    }
    let home = std::env::var("HOME").context("HOME env var not set")?;
    Ok(PathBuf::from(home).join(".config").join("playterm"))
}

fn config_file_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

fn create_default(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config dir {}", parent.display()))?;
    }
    let default_toml = r##"[server]
url = ""
username = ""
password = ""

[player]
default_volume = 70
max_bit_rate = 0   # 0 = unlimited; set e.g. 320 to cap streaming bitrate
# mpris = true     # Linux: register on session D-Bus for media keys / playerctl (default: true)

[keybinds]
# scroll_up     = "k"
# scroll_down   = "j"
# column_left   = "h"
# column_right  = "l"
# play_pause    = "p"
# next_track    = "n"
# prev_track    = "N"
# seek_forward  = "Right"
# seek_backward = "Left"
# add_track     = "a"
# add_all       = "Shift+a"
# shuffle       = "x"
# unshuffle     = "z"
# clear_queue   = "D"
# search        = "/"
# volume_up     = "+"
# volume_down   = "-"
# tab_switch    = "Tab"
# quit          = "q"
# library_fzf     = "Ctrl+f"   # fuzzy track picker; "" disables
# library_refresh = "Ctrl+r"   # force full index refresh; "" disables

[theme]
# accent        = "#ff8c00"   # highlighted items, active borders, progress fill
# background    = "#1a1a1a"   # outer background (status bar, now-playing bar)
# surface       = "#161616"   # panel backgrounds (browser columns, queue)
# foreground    = "#d4d0c8"   # primary text
# dimmed        = "#5a5858"   # muted / secondary text
# border        = "#252525"   # inactive pane borders
# border_active = "#3a3a3a"   # active pane borders
# dynamic       = true         # extract accent colour from album art

[ui]

[ui.general]
tab_bar_position = "bottom"

[ui.row_now_playing]
bar_height = 4
layout = "row"
box_location = "queue"
show_controls = true
show_progress = true
box_include_controls = false
box_include_progress = false
progress_style = "██░"
# lines = ["$b%t$/b", "%a", "%b"]

[ui.hometab.recent_albums]
show_art = true

[ui.hometab.layout]
top_height_percent = 50
panels = ["recent_albums", "recent_tracks", "rediscover"]

[ui.browsetab]
mode = "artists"

[ui.nptab]
lyrics = false
# lyrics_enabled = true
# queue_template = ""
# show_fzf_hint = false

[ui.nptab.art]
show = true
position = "left"
width_percent = 50

[ui.nptab.visualizer_pane]
enabled = true
visible = false
location = "queue"

[ui.nptab.now_playing]
# Overrides `row_now_playing` for the Now Playing tab (strip + boxed pane text).
# layout = "boxed"
# lines = ["$b%t$/b", "%a", "%b"]

[library]
# enabled = true
# index_path = ""          # empty = ~/.cache/playterm/library_index.json
# max_age_secs = 86400     # refresh in background when older (0 = always stale)
# fzf_binary = "fzf"       # or "sk"
# fzf_args = ["--delimiter=\\t", "--with-nth=2,3,4,5", "--nth=1,2,3", "--multi", "--expect=ctrl-r", "--border=rounded"]
# aligned --header is added automatically unless you pass your own --header=…
# fetch_album_parallelism = 12    # concurrent getAlbum per artist during index refresh
# fetch_artist_parallelism = 4    # concurrent artists during index refresh
# navidrome_skip_unchanged_scan = false   # Navidrome: skip full walk when lastScan unchanged
# notify_on_forced_index_refresh = true   # desktop notification when Ctrl+r refresh finishes

[cache]
enabled     = true
max_size_gb = 2   # maximum total cache size in gigabytes
"##;
    std::fs::write(path, default_toml)
        .with_context(|| format!("writing default config to {}", path.display()))?;
    eprintln!("Created default config: {}", path.display());
    Ok(())
}

fn merge_env_overrides(cfg: &mut FileConfig) {
    if let Ok(v) = std::env::var("TERMUSIC_SUBSONIC_URL").or_else(|_| std::env::var("SUBSONIC_URL")) {
        cfg.server.url = v;
    }
    if let Ok(v) = std::env::var("TERMUSIC_SUBSONIC_USER").or_else(|_| std::env::var("SUBSONIC_USER")) {
        cfg.server.username = v;
    }
    if let Ok(v) = std::env::var("TERMUSIC_SUBSONIC_PASS").or_else(|_| std::env::var("SUBSONIC_PASS")) {
        cfg.server.password = v;
    }
}
