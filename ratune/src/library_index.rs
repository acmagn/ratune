//! On-disk library metadata index for fuzzy picking (fzf) without live Subsonic
//! calls per keystroke.
//!
//! Stored as JSON under `~/.cache/ratune/library_index.json` by default (see
//! config). Text only. No art or audio.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ratune_subsonic::{DiscTitle, Song};
use serde::{Deserialize, Serialize};

use crate::text_width::{self, Align};

/// Display width per TSV column fed to fzf (terminal columns). `0` = no truncation or padding.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct FzfColumns {
    #[serde(default = "default_fzf_col_artist")]
    pub artist: usize,
    #[serde(default = "default_fzf_col_album")]
    pub album: usize,
    #[serde(default = "default_fzf_col_title")]
    pub title: usize,
    #[serde(default = "default_fzf_col_duration")]
    pub duration: usize,
}

impl Default for FzfColumns {
    fn default() -> Self {
        Self {
            artist: default_fzf_col_artist(),
            album: default_fzf_col_album(),
            title: default_fzf_col_title(),
            duration: default_fzf_col_duration(),
        }
    }
}

fn default_fzf_col_artist() -> usize {
    26
}
fn default_fzf_col_album() -> usize {
    28
}
fn default_fzf_col_title() -> usize {
    36
}
fn default_fzf_col_duration() -> usize {
    6
}

const FORMAT_VERSION: u32 = 1;

/// Serialized snapshot written to disk.
#[derive(Serialize, Deserialize)]
pub struct LibraryIndexFile {
    pub version: u32,
    /// Unix seconds when this index was last fully refreshed from the server.
    pub refreshed_at_unix: u64,
    pub tracks: Vec<Song>,
    /// Navidrome: `getScanStatus.lastScan` after the last full walk (RFC3339). Used to skip
    /// redundant refreshes when `[library] navidrome_skip_unchanged_scan` is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navidrome_last_scan: Option<String>,
    /// OpenSubsonic disc subtitles keyed by album id. Missing in indexes written before
    /// this field existed; a forced refresh fills it in.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub disc_titles: HashMap<String, Vec<DiscTitle>>,
}

/// Default path: `~/.cache/ratune/library_index.json`.
pub fn default_index_path() -> Option<PathBuf> {
    let base = dirs_cache_base()?;
    Some(base.join("library_index.json"))
}

/// Default path for library index refresh warnings: `~/.cache/ratune/library_index_refresh.log`.
pub fn default_refresh_log_path() -> Option<PathBuf> {
    let base = dirs_cache_base()?;
    Some(base.join("library_index_refresh.log"))
}

/// Write refresh warnings to `path` (overwrites). Returns the number of lines written.
pub fn write_refresh_log(path: &Path, warnings: &[String]) -> Result<usize> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut body = String::new();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    body.push_str(&format!(
        "# ratune library index refresh log (unix {ts})\n# {} warning(s)\n",
        warnings.len()
    ));
    for w in warnings {
        body.push_str(w);
        body.push('\n');
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_extension(format!("log.{nanos}.part"));
    let mut f = fs::File::create(&temp).with_context(|| format!("writing {}", temp.display()))?;
    f.write_all(body.as_bytes())?;
    f.sync_all().ok();
    drop(f);
    fs::rename(&temp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(warnings.len())
}

fn dirs_cache_base() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return Some(PathBuf::from(xdg).join("ratune"));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".cache").join("ratune"))
}

/// Load an index from disk. Returns `None` if missing or unreadable.
pub fn load(path: &Path) -> Option<LibraryIndexFile> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Borrowing mirror of [`LibraryIndexFile`] so saving does not clone the whole library.
/// Field order and skip rules must match [`LibraryIndexFile`].
#[derive(Serialize)]
struct LibraryIndexFileRef<'a> {
    version: u32,
    refreshed_at_unix: u64,
    tracks: &'a [Song],
    #[serde(skip_serializing_if = "Option::is_none")]
    navidrome_last_scan: Option<&'a str>,
    #[serde(skip_serializing_if = "map_is_empty")]
    disc_titles: &'a HashMap<String, Vec<DiscTitle>>,
}

fn map_is_empty(m: &&HashMap<String, Vec<DiscTitle>>) -> bool {
    m.is_empty()
}

/// Same layout as [`LibraryIndexFile`], but leaves the large fields as raw JSON text.
#[derive(Serialize, Deserialize)]
struct LibraryIndexFileRaw<'a> {
    version: u32,
    refreshed_at_unix: u64,
    #[serde(borrow)]
    tracks: &'a serde_json::value::RawValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    navidrome_last_scan: Option<String>,
    #[serde(default, borrow, skip_serializing_if = "Option::is_none")]
    disc_titles: Option<&'a serde_json::value::RawValue>,
}

/// Index metadata without the track list.
#[derive(Deserialize)]
pub struct LibraryIndexHeader {
    pub refreshed_at_unix: u64,
    #[serde(default)]
    pub navidrome_last_scan: Option<String>,
}

/// Read only the index header. Track entries are skipped without being allocated.
pub fn load_header(path: &Path) -> Option<LibraryIndexHeader> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Rewrite `refreshed_at_unix` (and the scan token) while keeping the stored tracks verbatim.
/// Fails when the file is not the current format so the caller can fall back to a full load.
pub fn save_refreshed_at(
    path: &Path,
    refreshed_at_unix: u64,
    navidrome_last_scan: Option<&str>,
) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut file: LibraryIndexFileRaw<'_> =
        serde_json::from_str(&text).context("parse library index")?;
    anyhow::ensure!(
        file.version == FORMAT_VERSION,
        "library index version {} != {FORMAT_VERSION}",
        file.version
    );
    file.refreshed_at_unix = refreshed_at_unix;
    file.navidrome_last_scan = navidrome_last_scan.map(String::from);
    if file.disc_titles.is_some_and(|d| d.get().trim() == "{}") {
        file.disc_titles = None;
    }
    write_atomic(path, |w| {
        serde_json::to_writer_pretty(w, &file).context("serialize library index")
    })
}

/// Atomically write the index (temp + rename).
pub fn save(
    path: &Path,
    tracks: &[Song],
    refreshed_at_unix: u64,
    navidrome_last_scan: Option<&str>,
    disc_titles: &HashMap<String, Vec<DiscTitle>>,
) -> Result<()> {
    let file = LibraryIndexFileRef {
        version: FORMAT_VERSION,
        refreshed_at_unix,
        tracks,
        navidrome_last_scan,
        disc_titles,
    };
    write_atomic(path, |w| {
        serde_json::to_writer_pretty(w, &file).context("serialize library index")
    })
}

fn write_atomic(
    path: &Path,
    write: impl FnOnce(&mut std::io::BufWriter<&mut fs::File>) -> Result<()>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_extension(format!("json.{nanos}.part"));
    let mut f = fs::File::create(&temp).with_context(|| format!("writing {}", temp.display()))?;
    let written = (|| {
        let mut w = std::io::BufWriter::new(&mut f);
        write(&mut w)?;
        w.flush()?;
        Ok(())
    })();
    if let Err(e) = written {
        drop(f);
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    f.sync_all().ok();
    drop(f);
    fs::rename(&temp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(())
}

fn sanitize_field(s: &str) -> String {
    s.replace(['\t', '\n'], " ")
}

/// Truncate then pad with spaces so columns line up in a monospace terminal.
/// When `width` is 0, pass through unchanged (full text for fzf search and display).
fn format_fzf_column(s: &str, width: usize) -> String {
    if width == 0 {
        return s.to_string();
    }
    text_width::fit_to_width(s, width, Align::Left)
}

/// Default `--with-nth` when omitted from fzf argv (hide song id, show artist–duration).
pub const FZF_DEFAULT_WITH_NTH: &[usize] = &[2, 3, 4, 5];

/// Parse `--with-nth=2,3,4,5` (or `--with-nth 2,3,4,5`) from fzf argv; `default` when absent.
pub fn parse_fzf_with_nth_or(args: &[String], default: &[usize]) -> Vec<usize> {
    parse_fzf_flag_value(args, "--with-nth")
        .map(|s| parse_fzf_field_indices(&s))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_vec())
}

fn parse_fzf_flag_value(args: &[String], flag: &str) -> Option<String> {
    let eq = format!("{flag}=");
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(v) = a.strip_prefix(&eq) {
            return Some(v.to_string());
        }
        if a == flag {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

fn parse_fzf_field_indices(s: &str) -> Vec<usize> {
    s.split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok().filter(|&n| n > 0))
        .collect()
}

fn fzf_field_header(field: usize, cols: FzfColumns) -> Option<String> {
    let (label, width) = match field {
        2 => ("Artist", cols.artist),
        3 => ("Album", cols.album),
        4 => ("Title", cols.title),
        5 => ("Time", cols.duration),
        _ => return None,
    };
    Some(format_fzf_column(label, width))
}

/// Tab-separated header labels padded to match [`write_fzf_input_lines`] columns in the order
/// given by fzf's `--with-nth` (defaults to artist, album, title, time). Pass as
/// `fzf --header=…` so labels line up with data rows.
pub fn fzf_header_line(cols: FzfColumns, with_nth: &[usize]) -> String {
    with_nth
        .iter()
        .filter_map(|&field| fzf_field_header(field, cols))
        .collect::<Vec<_>>()
        .join("\t")
}

/// One TSV line per track for fzf: id, artist, album, title, duration.
/// Field 1 is the song id (hidden in the fzf *list* via `--with-nth=2,3,4,5`).
/// Default `[library.fzf].args` uses `--nth=1,2,3` (artist, album, title; duration is
/// shown but excluded from fuzzy search). Column widths come from [`FzfColumns`]; set a
/// width to `0` to send the full field (needed for searching long titles).
pub fn write_fzf_input_lines<'a>(
    out: &mut (impl Write + ?Sized),
    tracks: impl IntoIterator<Item = &'a Song>,
    cols: FzfColumns,
) -> std::io::Result<()> {
    for s in tracks {
        let artist = s.artist.as_deref().unwrap_or("—");
        let album = s.album.as_deref().unwrap_or("—");
        let title = s.title.as_str();
        let dur = s
            .duration
            .map(fmt_duration_ms)
            .unwrap_or_else(|| "—".to_string());
        let id = sanitize_field(&s.id);
        let artist = format_fzf_column(&sanitize_field(artist), cols.artist);
        let album = format_fzf_column(&sanitize_field(album), cols.album);
        let title = format_fzf_column(&sanitize_field(title), cols.title);
        let dur = format_fzf_column(&sanitize_field(&dur), cols.duration);
        writeln!(out, "{id}\t{artist}\t{album}\t{title}\t{dur}")?;
    }
    Ok(())
}

/// Display width per TSV column in the album picker. `0` = no truncation or padding.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct FzfAlbumColumns {
    #[serde(default = "default_fzf_col_artist")]
    pub artist: usize,
    #[serde(default = "default_fzf_album_col_album")]
    pub album: usize,
    #[serde(default = "default_fzf_album_col_year")]
    pub year: usize,
    #[serde(default = "default_fzf_album_col_tracks")]
    pub tracks: usize,
    #[serde(default = "default_fzf_album_col_duration")]
    pub duration: usize,
}

impl Default for FzfAlbumColumns {
    fn default() -> Self {
        Self {
            artist: default_fzf_col_artist(),
            album: default_fzf_album_col_album(),
            year: default_fzf_album_col_year(),
            tracks: default_fzf_album_col_tracks(),
            duration: default_fzf_album_col_duration(),
        }
    }
}

fn default_fzf_album_col_album() -> usize {
    40
}
fn default_fzf_album_col_year() -> usize {
    4
}
fn default_fzf_album_col_tracks() -> usize {
    6
}
fn default_fzf_album_col_duration() -> usize {
    8
}

/// Default `--with-nth` for the album picker (hide album id, show the rest).
pub const FZF_ALBUM_DEFAULT_WITH_NTH: &[usize] = &[2, 3, 4, 5, 6];

fn fzf_album_field_header(field: usize, cols: FzfAlbumColumns) -> Option<String> {
    let (label, width) = match field {
        2 => ("Artist", cols.artist),
        3 => ("Album", cols.album),
        4 => ("Year", cols.year),
        5 => ("Tracks", cols.tracks),
        6 => ("Time", cols.duration),
        _ => return None,
    };
    Some(format_fzf_column(label, width))
}

/// [`fzf_header_line`] for [`write_fzf_album_lines`] columns.
pub fn fzf_album_header_line(cols: FzfAlbumColumns, with_nth: &[usize]) -> String {
    with_nth
        .iter()
        .filter_map(|&field| fzf_album_field_header(field, cols))
        .collect::<Vec<_>>()
        .join("\t")
}

/// One TSV line per album for fzf: id, album artist, album, year, track count, total time.
/// Field 1 is the album id (hidden via `--with-nth=2,3,4,5,6`).
pub fn write_fzf_album_lines<'a>(
    out: &mut (impl Write + ?Sized),
    albums: impl IntoIterator<Item = &'a ratune_subsonic::Album>,
    cols: FzfAlbumColumns,
) -> std::io::Result<()> {
    for a in albums {
        let id = sanitize_field(&a.id);
        let artist = a.artist.as_deref().unwrap_or("—");
        let year = a
            .year
            .filter(|&y| y > 0)
            .map(|y| y.to_string())
            .unwrap_or_else(|| "—".to_string());
        let tracks = a
            .song_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".to_string());
        let dur = a
            .duration
            .map(fmt_long_duration)
            .unwrap_or_else(|| "—".to_string());
        let artist = format_fzf_column(&sanitize_field(artist), cols.artist);
        let album = format_fzf_column(&sanitize_field(&a.name), cols.album);
        let year = format_fzf_column(&year, cols.year);
        let tracks = format_fzf_column(&tracks, cols.tracks);
        let dur = format_fzf_column(&dur, cols.duration);
        writeln!(out, "{id}\t{artist}\t{album}\t{year}\t{tracks}\t{dur}")?;
    }
    Ok(())
}

/// `m:ss` under an hour, `h:mm:ss` otherwise.
fn fmt_long_duration(secs: u32) -> String {
    if secs < 3600 {
        return fmt_duration_ms(secs);
    }
    format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

/// Parse the first field (song or album id) from a line emitted by
/// [`write_fzf_input_lines`] / [`write_fzf_album_lines`].
pub fn parse_pick_line(line: &str) -> Option<String> {
    let line = line.trim_end_matches('\n');
    line.split('\t').next().map(String::from)
}

fn fmt_duration_ms(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}

/// Build an id → position map into `tracks` for enqueue after fzf.
/// Duplicate ids resolve to the last occurrence.
pub fn index_by_id(tracks: &[Song]) -> std::collections::HashMap<String, usize> {
    tracks
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i))
        .collect()
}

/// Artist/album/track hierarchy derived from a flat library index. Used to drive the
/// Browse tab from local data (online when the index is available, and offline).
#[derive(Debug, Clone)]
pub struct BrowseSnapshot {
    pub artists: Vec<ratune_subsonic::Artist>,
    pub albums_by_artist: std::collections::HashMap<String, Vec<ratune_subsonic::Album>>,
    /// Deduped flat album list (one entry per album id) for Albums browse mode.
    pub flat_albums: Vec<ratune_subsonic::Album>,
    /// Positions into the track slice the snapshot was built from, in disc/track order.
    tracks_by_album: std::collections::HashMap<String, Vec<usize>>,
}

impl BrowseSnapshot {
    /// Songs of `album_id`. `tracks` must be the slice this snapshot was built from.
    pub fn album_tracks(&self, tracks: &[Song], album_id: &str) -> Vec<Song> {
        self.tracks_by_album
            .get(album_id)
            .map(|idxs| {
                idxs.iter()
                    .filter_map(|&i| tracks.get(i).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks_by_album.is_empty()
    }
}

/// Sort order for the flat Albums browse list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumListSort {
    /// Newest library additions first (`created` desc).
    #[default]
    Newest,
    AlphabeticalByName,
    AlphabeticalByArtist,
    /// Highest play count first.
    Frequent,
    /// Starred albums only (name order). Requires `starred` set on albums.
    Starred,
}

impl AlbumListSort {
    pub fn next(self) -> Self {
        match self {
            Self::Newest => Self::AlphabeticalByName,
            Self::AlphabeticalByName => Self::AlphabeticalByArtist,
            Self::AlphabeticalByArtist => Self::Frequent,
            Self::Frequent => Self::Starred,
            Self::Starred => Self::Newest,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::AlphabeticalByName => "name",
            Self::AlphabeticalByArtist => "artist",
            Self::Frequent => "frequent",
            Self::Starred => "starred",
        }
    }

    /// Subsonic `getAlbumList2` `type` parameter for the no-index fallback.
    pub fn api_type(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::AlphabeticalByName => "alphabeticalByName",
            Self::AlphabeticalByArtist => "alphabeticalByArtist",
            Self::Frequent => "frequent",
            Self::Starred => "starred",
        }
    }
}

/// Sort a flat album list in place according to [`AlbumListSort`].
pub fn sort_flat_albums(albums: &mut Vec<ratune_subsonic::Album>, sort: AlbumListSort) {
    match sort {
        AlbumListSort::Newest => {
            albums.sort_by(|a, b| match (a.created.as_deref(), b.created.as_deref()) {
                (Some(ca), Some(cb)) => cb
                    .cmp(ca)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });
        }
        AlbumListSort::AlphabeticalByName => {
            albums.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        }
        AlbumListSort::AlphabeticalByArtist => {
            albums.sort_by(|a, b| {
                let aa = a.artist.as_deref().unwrap_or("").to_lowercase();
                let ba = b.artist.as_deref().unwrap_or("").to_lowercase();
                aa.cmp(&ba)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
        }
        AlbumListSort::Frequent => {
            albums.sort_by(|a, b| {
                let pa = a.play_count.unwrap_or(0);
                let pb = b.play_count.unwrap_or(0);
                pb.cmp(&pa)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
        }
        AlbumListSort::Starred => {
            albums.retain(|a| a.starred.is_some());
            albums.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        }
    }
}

/// Whether the indexed album list has enough metadata for a local sort.
///
/// Stale indexes (from before `album_created` / `album_play_count` were stamped) return
/// false for newest/frequent so the caller can fall back to `getAlbumList2`.
pub fn index_supports_album_sort(albums: &[ratune_subsonic::Album], sort: AlbumListSort) -> bool {
    if albums.is_empty() {
        return true;
    }
    match sort {
        AlbumListSort::Newest => {
            let with_created = albums.iter().filter(|a| a.created.is_some()).count();
            // Require at least half the albums to have dates; otherwise newest collapses to A–Z.
            with_created * 2 >= albums.len()
        }
        AlbumListSort::Frequent => albums.iter().any(|a| a.play_count.unwrap_or(0) > 0),
        AlbumListSort::AlphabeticalByName
        | AlbumListSort::AlphabeticalByArtist
        | AlbumListSort::Starred => true,
    }
}

fn offline_artist_id(song: &Song) -> String {
    song.artist_id
        .clone()
        .or_else(|| {
            song.artist
                .as_ref()
                .map(|name| format!("__offline_artist__{name}"))
        })
        .unwrap_or_else(|| "__offline_unknown_artist__".to_string())
}

fn non_empty(opt: &Option<String>) -> Option<&str> {
    opt.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn offline_album_id(song: &Song) -> String {
    song.album_id.clone().unwrap_or_else(|| {
        let album = song.album.as_deref().unwrap_or("Unknown Album");
        let artist = non_empty(&song.album_artist)
            .or_else(|| non_empty(&song.artist))
            .unwrap_or("unknown");
        format!("__offline_album__{artist}__{album}")
    })
}

/// Artist id/name used for Browse columns. Prefers album artist so compilations match
/// server `getArtists` (not per-track guest/feature artists).
///
/// Returns **all** album artists when OpenSubsonic `albumArtists` lists several (classical
/// composer + performer). Falls back to a single album-artist / legacy grouping.
fn browse_artists_for_album(songs: &[&Song]) -> Vec<(String, String)> {
    let mut by_id: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for s in songs {
        for a in &s.album_artists {
            let id = a.id.trim();
            let name = a.name.trim();
            if id.is_empty() || name.is_empty() {
                continue;
            }
            by_id
                .entry(id.to_string())
                .or_insert_with(|| name.to_string());
        }
    }
    if !by_id.is_empty() {
        return by_id.into_iter().collect();
    }

    // Single album-artist stamp / legacy path.
    vec![browse_artist_for_album_fallback(songs)]
}

fn browse_artist_for_album_fallback(songs: &[&Song]) -> (String, String) {
    for s in songs {
        if let Some(id) = non_empty(&s.album_artist_id) {
            let name = non_empty(&s.album_artist)
                .or_else(|| non_empty(&s.artist))
                .unwrap_or("Unknown Artist")
                .to_string();
            return (id.to_string(), name);
        }
        if let Some(name) = non_empty(&s.album_artist) {
            return (format!("__offline_artist__{name}"), name.to_string());
        }
    }

    // Legacy indexes without album_artist fields: if an album has multiple track artists
    // (typical of compilations), bucket under Various Artists instead of exploding the list.
    let mut distinct: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for s in songs {
        let id = offline_artist_id(s);
        let name = non_empty(&s.artist).unwrap_or("Unknown Artist").to_string();
        distinct.entry(id).or_insert(name);
    }
    if distinct.len() > 1 {
        return (
            "__offline_various_artists__".to_string(),
            "Various Artists".to_string(),
        );
    }
    distinct.into_iter().next().unwrap_or_else(|| {
        (
            "__offline_unknown_artist__".to_string(),
            "Unknown Artist".to_string(),
        )
    })
}

fn album_sort_key(a: &ratune_subsonic::Album) -> (u32, String) {
    (a.year.unwrap_or(0), a.name.to_lowercase())
}

fn sort_songs(songs: &mut [usize], tracks: &[Song]) {
    songs.sort_by_key(|&i| {
        let s = &tracks[i];
        (s.disc_number.unwrap_or(1), s.track.unwrap_or(0))
    });
}

/// Derive browse columns from the on-disk library index (same data as the fzf picker).
///
/// Albums are grouped under **album artist** (not track artist) so Browse matches the
/// live Subsonic `getArtists` / `getArtist` hierarchy.
pub fn build_browse_snapshot(tracks: &[Song]) -> BrowseSnapshot {
    build_browse_snapshot_filtered(tracks, |_| true)
}

/// [`build_browse_snapshot`] over only the songs where `keep` returns true.
/// Track positions still refer to the full `tracks` slice.
pub fn build_browse_snapshot_filtered(
    tracks: &[Song],
    keep: impl Fn(&Song) -> bool,
) -> BrowseSnapshot {
    use ratune_subsonic::{Album, Artist};
    use std::collections::HashMap;

    struct AlbumAcc {
        name: String,
        /// Browse-column artist this album is listed under (may be one of several album artists).
        column_artist_id: String,
        /// Display album-artist string (e.g. "Muzio Clementi; Andreas Staier").
        display_artist_name: String,
        song_count: u32,
        duration: u32,
        year: Option<u32>,
        genre: Option<String>,
        cover_art: Option<String>,
        user_rating: Option<u8>,
        created: Option<String>,
        play_count: Option<u64>,
    }

    // Phase 1: gather songs by album id (shared album_id stays one album even on compilations).
    let mut by_album: HashMap<String, Vec<usize>> = HashMap::new();
    let mut album_order_names: HashMap<String, String> = HashMap::new();
    for (i, song) in tracks.iter().enumerate() {
        if !keep(song) {
            continue;
        }
        let album_id = offline_album_id(song);
        album_order_names
            .entry(album_id.clone())
            .or_insert_with(|| {
                song.album
                    .clone()
                    .unwrap_or_else(|| "Unknown Album".to_string())
            });
        by_album.entry(album_id).or_default().push(i);
    }

    // Phase 2: list each album under every album artist (classical composer + performer).
    let mut by_artist: HashMap<String, (String, HashMap<String, AlbumAcc>)> = HashMap::new();
    let mut tracks_by_album: HashMap<String, Vec<usize>> = HashMap::new();
    let mut flat_by_id: HashMap<String, AlbumAcc> = HashMap::new();

    for (album_id, mut song_idxs) in by_album {
        sort_songs(&mut song_idxs, tracks);
        let songs: Vec<&Song> = song_idxs.iter().map(|&i| &tracks[i]).collect();
        let owners = browse_artists_for_album(&songs);
        let album_name = album_order_names
            .remove(&album_id)
            .unwrap_or_else(|| "Unknown Album".to_string());
        let display_artist_name = songs
            .iter()
            .find_map(|s| non_empty(&s.album_artist).map(|s| s.to_string()))
            .or_else(|| owners.first().map(|(_, n)| n.clone()))
            .unwrap_or_else(|| "Unknown Artist".to_string());

        let song_count = songs.len() as u32;
        let duration: u32 = songs.iter().filter_map(|s| s.duration).sum();
        let year = songs.iter().find_map(|s| s.year);
        let genre = songs.iter().find_map(|s| s.genre.clone());
        let cover_art = songs
            .iter()
            .find_map(|s| s.cover_art.clone())
            .or_else(|| Some(album_id.clone()));
        let user_rating = songs.iter().find_map(|s| s.album_user_rating);
        let created = songs.iter().find_map(|s| s.album_created.clone());
        let play_count = songs.iter().find_map(|s| s.album_play_count);
        tracks_by_album.insert(album_id.clone(), song_idxs);

        let primary_artist_id = owners
            .first()
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| "__offline_unknown_artist__".to_string());

        flat_by_id.insert(
            album_id.clone(),
            AlbumAcc {
                name: album_name.clone(),
                column_artist_id: primary_artist_id,
                display_artist_name: display_artist_name.clone(),
                song_count,
                duration,
                year,
                genre: genre.clone(),
                cover_art: cover_art.clone(),
                user_rating,
                created: created.clone(),
                play_count,
            },
        );

        for (artist_id, artist_name) in owners {
            let entry = by_artist
                .entry(artist_id.clone())
                .or_insert_with(|| (artist_name.clone(), HashMap::new()));
            if entry.0 == "Unknown Artist" && artist_name != "Unknown Artist" {
                entry.0 = artist_name.clone();
            }
            entry.1.insert(
                album_id.clone(),
                AlbumAcc {
                    name: album_name.clone(),
                    column_artist_id: artist_id,
                    display_artist_name: display_artist_name.clone(),
                    song_count,
                    duration,
                    year,
                    genre: genre.clone(),
                    cover_art: cover_art.clone(),
                    user_rating,
                    created: created.clone(),
                    play_count,
                },
            );
        }
    }

    let mut artists: Vec<Artist> = by_artist
        .iter()
        .map(|(id, (name, albums))| Artist {
            id: id.clone(),
            name: name.clone(),
            album_count: Some(albums.len() as u32),
            cover_art: albums.values().find_map(|a| a.cover_art.clone()),
            // Favorites are merged later from getStarred2; ratings come from overlay / stamp.
            starred: None,
            user_rating: None,
            album: Vec::new(),
        })
        .collect();
    artists.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let mut albums_by_artist = HashMap::new();

    for (artist_id, (_artist_name, albums_map)) in by_artist {
        let mut albums: Vec<Album> = albums_map
            .into_iter()
            .map(|(album_id, album_acc)| Album {
                id: album_id,
                name: album_acc.name,
                artist: Some(album_acc.display_artist_name),
                artist_id: Some(album_acc.column_artist_id),
                artists: vec![],
                cover_art: album_acc.cover_art,
                song_count: Some(album_acc.song_count),
                duration: if album_acc.duration > 0 {
                    Some(album_acc.duration)
                } else {
                    None
                },
                year: album_acc.year,
                genre: album_acc.genre,
                starred: None,
                user_rating: album_acc.user_rating,
                release_date: None,
                created: album_acc.created,
                play_count: album_acc.play_count,
                disc_titles: Vec::new(),
                song: Vec::new(),
            })
            .collect();

        albums.sort_by(|a, b| album_sort_key(a).cmp(&album_sort_key(b)));
        albums_by_artist.insert(artist_id, albums);
    }

    let mut flat_albums: Vec<Album> = flat_by_id
        .into_iter()
        .map(|(album_id, album_acc)| Album {
            id: album_id,
            name: album_acc.name,
            artist: Some(album_acc.display_artist_name),
            artist_id: Some(album_acc.column_artist_id),
            artists: vec![],
            cover_art: album_acc.cover_art,
            song_count: Some(album_acc.song_count),
            duration: if album_acc.duration > 0 {
                Some(album_acc.duration)
            } else {
                None
            },
            year: album_acc.year,
            genre: album_acc.genre,
            starred: None,
            user_rating: album_acc.user_rating,
            release_date: None,
            created: album_acc.created,
            play_count: album_acc.play_count,
            disc_titles: Vec::new(),
            song: Vec::new(),
        })
        .collect();
    sort_flat_albums(&mut flat_albums, AlbumListSort::Newest);

    BrowseSnapshot {
        artists,
        albums_by_artist,
        flat_albums,
        tracks_by_album,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fzf_input_lines(tracks: &[Song], cols: FzfColumns) -> String {
        let mut out = Vec::new();
        write_fzf_input_lines(&mut out, tracks, cols).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn index_file_without_disc_titles_still_loads() {
        let j = r#"{"version":1,"refreshed_at_unix":5,"tracks":[]}"#;
        let f: LibraryIndexFile = serde_json::from_str(j).unwrap();
        assert!(f.disc_titles.is_empty());
    }

    #[test]
    fn index_save_load_round_trips_disc_titles() {
        let dir = std::env::temp_dir().join(format!("ratune-index-test-{}", std::process::id()));
        let path = dir.join("library_index.json");
        let titles = HashMap::from([(
            "al1".to_string(),
            vec![DiscTitle {
                disc: 2,
                title: "Live".into(),
            }],
        )]);
        save(&path, &[], 7, None, &titles).unwrap();
        let loaded = load(&path).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(loaded.disc_titles, titles);
    }

    fn plain_song(id: &str, album: &str, track: u32) -> Song {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "title": format!("T{track} \"q\" é"),
            "album": album,
            "albumId": format!("al-{album}"),
            "albumArtist": "A",
            "albumArtistId": "ar-a",
            "track": track,
        }))
        .unwrap()
    }

    fn temp_index_path(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("ratune-index-{tag}-{}-{nanos}", std::process::id()))
            .join("library_index.json")
    }

    #[test]
    fn save_matches_owned_pretty_serialization() {
        let tracks = vec![plain_song("1", "X", 1), plain_song("2", "Y", 2)];
        let titles = HashMap::from([(
            "al-X".to_string(),
            vec![DiscTitle {
                disc: 1,
                title: "One".into(),
            }],
        )]);
        for (tok, titles) in [(Some("scan"), titles), (None, HashMap::new())] {
            let path = temp_index_path("bytes");
            save(&path, &tracks, 9, tok, &titles).unwrap();
            let expected = serde_json::to_string_pretty(&LibraryIndexFile {
                version: FORMAT_VERSION,
                refreshed_at_unix: 9,
                tracks: tracks.clone(),
                navidrome_last_scan: tok.map(String::from),
                disc_titles: titles.clone(),
            })
            .unwrap();
            let written = fs::read_to_string(&path).unwrap();
            let _ = fs::remove_dir_all(path.parent().unwrap());
            assert_eq!(written, expected);
        }
    }

    #[test]
    fn save_refreshed_at_matches_full_resave() {
        let tracks = vec![plain_song("1", "X", 1), plain_song("2", "Y", 2)];
        let titles = HashMap::from([(
            "al-X".to_string(),
            vec![DiscTitle {
                disc: 2,
                title: "Live".into(),
            }],
        )]);
        for titles in [titles, HashMap::new()] {
            let path = temp_index_path("touch");
            save(&path, &tracks, 5, Some("old"), &titles).unwrap();
            save_refreshed_at(&path, 42, Some("new")).unwrap();
            let touched = fs::read_to_string(&path).unwrap();
            save(&path, &tracks, 42, Some("new"), &titles).unwrap();
            let full = fs::read_to_string(&path).unwrap();
            let header = load_header(&path).unwrap();
            let _ = fs::remove_dir_all(path.parent().unwrap());
            assert_eq!(touched, full);
            assert_eq!(header.refreshed_at_unix, 42);
            assert_eq!(header.navidrome_last_scan.as_deref(), Some("new"));
        }
    }

    #[test]
    fn filtered_snapshot_positions_refer_to_full_slice() {
        let tracks = vec![
            plain_song("1", "X", 1),
            plain_song("2", "Y", 2),
            plain_song("3", "Y", 1),
        ];
        let snap = build_browse_snapshot_filtered(&tracks, |s| s.id != "1");
        assert!(snap.album_tracks(&tracks, "al-X").is_empty());
        let ids: Vec<String> = snap
            .album_tracks(&tracks, "al-Y")
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["3", "2"]);
        assert!(build_browse_snapshot_filtered(&tracks, |_| false).is_empty());
        let by_id = index_by_id(&tracks);
        assert_eq!(tracks[by_id["2"]].id, "2");
    }

    #[test]
    fn browse_snapshot_groups_artists_albums_tracks() {
        fn song(id: &str, artist: &str, album: &str, track: u32) -> Song {
            Song {
                id: id.into(),
                title: format!("Track {track}"),
                album: Some(album.into()),
                artist: Some(artist.into()),
                album_id: Some(format!("al-{album}")),
                artist_id: Some(format!("ar-{artist}")),
                album_artist: Some(artist.into()),
                album_artist_id: Some(format!("ar-{artist}")),
                album_artists: Vec::new(),
                album_user_rating: None,
                artist_user_rating: None,
                album_created: None,
                album_play_count: None,
                track: Some(track),
                disc_number: Some(1),
                year: Some(2000),
                genre: None,
                cover_art: None,
                duration: Some(180),
                bit_rate: None,
                content_type: None,
                suffix: None,
                size: None,
                path: None,
                starred: None,
                user_rating: None,
            }
        }

        let tracks = vec![
            song("1", "Alice", "Alpha", 2),
            song("2", "Alice", "Alpha", 1),
            song("3", "Bob", "Beta", 1),
        ];
        let snap = build_browse_snapshot(&tracks);
        assert_eq!(snap.artists.len(), 2);
        assert_eq!(snap.artists[0].name, "Alice");
        assert_eq!(snap.artists[1].name, "Bob");
        let alice = snap.artists[0].id.clone();
        let albums = snap.albums_by_artist.get(&alice).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].name, "Alpha");
        let album_tracks = snap.album_tracks(&tracks, &albums[0].id);
        assert_eq!(album_tracks.len(), 2);
        assert_eq!(album_tracks[0].track, Some(1));
        assert_eq!(album_tracks[1].track, Some(2));
    }

    #[test]
    fn browse_snapshot_compilations_use_album_artist_not_track_artists() {
        fn track(
            id: &str,
            track_artist: &str,
            album: &str,
            album_artist: &str,
            album_artist_id: &str,
            n: u32,
        ) -> Song {
            Song {
                id: id.into(),
                title: format!("Track {n}"),
                album: Some(album.into()),
                artist: Some(track_artist.into()),
                album_id: Some(format!("al-{album}")),
                artist_id: Some(format!("ar-{track_artist}")),
                album_artist: Some(album_artist.into()),
                album_artist_id: Some(album_artist_id.into()),
                album_artists: Vec::new(),
                album_user_rating: None,
                artist_user_rating: None,
                album_created: None,
                album_play_count: None,
                track: Some(n),
                disc_number: Some(1),
                year: Some(2000),
                genre: None,
                cover_art: None,
                duration: Some(180),
                bit_rate: None,
                content_type: None,
                suffix: None,
                size: None,
                path: None,
                starred: None,
                user_rating: None,
            }
        }

        let tracks = vec![
            track("1", "Guest A", "Hits", "Various Artists", "ar-va", 1),
            track("2", "Guest B", "Hits", "Various Artists", "ar-va", 2),
            track("3", "Guest C", "Hits", "Various Artists", "ar-va", 3),
        ];
        let snap = build_browse_snapshot(&tracks);
        assert_eq!(
            snap.artists.len(),
            1,
            "compilations must not expand track artists"
        );
        assert_eq!(snap.artists[0].name, "Various Artists");
        assert_eq!(snap.artists[0].id, "ar-va");
        let albums = snap.albums_by_artist.get("ar-va").unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(snap.album_tracks(&tracks, &albums[0].id).len(), 3);
    }

    #[test]
    fn browse_snapshot_lists_album_under_each_album_artist() {
        use ratune_subsonic::ArtistRef;
        let track = Song {
            id: "1".into(),
            title: "I. Allegro".into(),
            album: Some("Sonatas".into()),
            artist: Some("Muzio Clementi".into()),
            album_id: Some("al-sonatas".into()),
            artist_id: Some("ar-clementi".into()),
            album_artist: Some("Muzio Clementi; Andreas Staier".into()),
            album_artist_id: Some("ar-clementi".into()),
            album_artists: vec![
                ArtistRef {
                    id: "ar-clementi".into(),
                    name: "Muzio Clementi".into(),
                },
                ArtistRef {
                    id: "ar-staier".into(),
                    name: "Andreas Staier".into(),
                },
            ],
            album_user_rating: None,
            artist_user_rating: None,
            album_created: None,
            album_play_count: None,
            track: Some(1),
            disc_number: Some(1),
            year: Some(1990),
            genre: None,
            cover_art: None,
            duration: Some(180),
            bit_rate: None,
            content_type: None,
            suffix: None,
            size: None,
            path: None,
            starred: None,
            user_rating: None,
        };
        let snap = build_browse_snapshot(&[track]);
        let names: Vec<_> = snap.artists.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"Andreas Staier"), "{names:?}");
        assert!(names.contains(&"Muzio Clementi"), "{names:?}");
        assert_eq!(snap.artists.len(), 2);
        assert!(snap.albums_by_artist.get("ar-staier").is_some());
        assert!(snap.albums_by_artist.get("ar-clementi").is_some());
        assert_eq!(
            snap.albums_by_artist.get("ar-staier").unwrap()[0].id,
            "al-sonatas"
        );
    }

    #[test]
    fn browse_snapshot_legacy_multi_artist_album_collapses_to_various() {
        // No album_artist fields (old index): distinct track artists on one album.
        fn track(id: &str, track_artist: &str, album: &str, n: u32) -> Song {
            Song {
                id: id.into(),
                title: format!("Track {n}"),
                album: Some(album.into()),
                artist: Some(track_artist.into()),
                album_id: Some(format!("al-{album}")),
                artist_id: Some(format!("ar-{track_artist}")),
                album_artist: None,
                album_artist_id: None,
                album_artists: Vec::new(),
                album_user_rating: None,
                artist_user_rating: None,
                album_created: None,
                album_play_count: None,
                track: Some(n),
                disc_number: Some(1),
                year: Some(2000),
                genre: None,
                cover_art: None,
                duration: Some(180),
                bit_rate: None,
                content_type: None,
                suffix: None,
                size: None,
                path: None,
                starred: None,
                user_rating: None,
            }
        }

        let tracks = vec![
            track("1", "Guest A", "Hits", 1),
            track("2", "Guest B", "Hits", 2),
        ];
        let snap = build_browse_snapshot(&tracks);
        assert_eq!(snap.artists.len(), 1);
        assert_eq!(snap.artists[0].name, "Various Artists");
        assert_eq!(snap.artists[0].id, "__offline_various_artists__");
    }

    #[test]
    fn browse_snapshot_preserves_stamped_artist_and_album_ratings() {
        fn track(id: &str, artist: &str, album: &str, n: u32) -> Song {
            Song {
                id: id.into(),
                title: format!("Track {n}"),
                album: Some(album.into()),
                artist: Some(artist.into()),
                album_id: Some(format!("al-{album}")),
                artist_id: Some(format!("ar-{artist}")),
                album_artist: Some(artist.into()),
                album_artist_id: Some(format!("ar-{artist}")),
                album_artists: Vec::new(),
                album_user_rating: Some(4),
                artist_user_rating: Some(5),
                album_created: None,
                album_play_count: None,
                track: Some(n),
                disc_number: Some(1),
                year: Some(2000),
                genre: None,
                cover_art: None,
                duration: Some(180),
                bit_rate: None,
                content_type: None,
                suffix: None,
                size: None,
                path: None,
                starred: None,
                user_rating: Some(3),
            }
        }

        let tracks = [track("1", "Alice", "Alpha", 1)];
        let snap = build_browse_snapshot(&tracks);
        // Artist 1–5 ratings are overlaid live from getArtists; album ratings come from the index.
        let albums = snap.albums_by_artist.get(&snap.artists[0].id).unwrap();
        assert_eq!(albums[0].user_rating, Some(4));
        let songs = snap.album_tracks(&tracks, &albums[0].id);
        assert_eq!(songs[0].user_rating, Some(3));
    }

    #[test]
    fn parse_pick_line_basic() {
        let line = "song1\tArtist\tAlbum\tTitle\t3:00\n";
        assert_eq!(parse_pick_line(line).as_deref(), Some("song1"));
    }

    #[test]
    fn fzf_header_matches_column_widths() {
        let cols = FzfColumns::default();
        let h = fzf_header_line(cols, FZF_DEFAULT_WITH_NTH);
        assert_eq!(
            h.matches('\t').count(),
            3,
            "four columns: Artist | Album | Title | Time"
        );
        let parts: Vec<&str> = h.split('\t').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(text_width::str_width(parts[0]), cols.artist);
        assert_eq!(text_width::str_width(parts[1]), cols.album);
        assert_eq!(text_width::str_width(parts[2]), cols.title);
        assert_eq!(text_width::str_width(parts[3]), cols.duration);
    }

    #[test]
    fn fzf_header_follows_with_nth_order() {
        let cols = FzfColumns::default();
        let h = fzf_header_line(cols, &[2, 3, 5, 4]);
        let parts: Vec<&str> = h.split('\t').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].trim(), "Artist");
        assert_eq!(parts[1].trim(), "Album");
        assert_eq!(parts[2].trim(), "Time");
        assert_eq!(parts[3].trim(), "Title");
    }

    #[test]
    fn parse_fzf_with_nth_from_args() {
        let args = vec![
            "--delimiter=\t".into(),
            "--with-nth=2,3,5,4".into(),
            "--nth=1,2,3".into(),
        ];
        let d = FZF_DEFAULT_WITH_NTH;
        assert_eq!(parse_fzf_with_nth_or(&args, d), vec![2, 3, 5, 4]);
        assert_eq!(parse_fzf_with_nth_or(&[], d), vec![2, 3, 4, 5]);
        let spaced = vec!["--with-nth".into(), "2,3,5,4".into()];
        assert_eq!(parse_fzf_with_nth_or(&spaced, d), vec![2, 3, 5, 4]);
        assert_eq!(
            parse_fzf_with_nth_or(&[], FZF_ALBUM_DEFAULT_WITH_NTH),
            vec![2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn fzf_album_lines_one_row_per_album_with_header() {
        let album: ratune_subsonic::Album = serde_json::from_value(serde_json::json!({
            "id": "al1",
            "name": "Long\tName",
            "artist": "Artist",
            "year": 1999,
            "songCount": 12,
            "duration": 3753,
        }))
        .unwrap();
        let cols = FzfAlbumColumns {
            album: 0,
            ..FzfAlbumColumns::default()
        };
        let mut out = Vec::new();
        write_fzf_album_lines(&mut out, [&album, &album], cols).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let fields: Vec<&str> = lines[0].split('\t').collect();
        assert_eq!(fields.len(), 6);
        assert_eq!(fields[0], "al1");
        assert_eq!(fields[2], "Long Name");
        assert_eq!(fields[3], "1999");
        assert_eq!(fields[4].trim_end(), "12");
        assert_eq!(fields[5].trim_end(), "1:02:33");
        assert_eq!(parse_pick_line(lines[0]).as_deref(), Some("al1"));

        let header = fzf_album_header_line(cols, FZF_ALBUM_DEFAULT_WITH_NTH);
        let labels: Vec<&str> = header.split('\t').map(str::trim_end).collect();
        assert_eq!(labels, ["Artist", "Album", "Year", "Tracks", "Time"]);
    }

    #[test]
    fn fzf_column_width_zero_passes_full_title() {
        let s = Song {
            id: "id".into(),
            title: "a".repeat(50),
            album: None,
            artist: None,
            album_id: None,
            artist_id: None,
            album_artist: None,
            album_artist_id: None,
            album_artists: Vec::new(),
            album_user_rating: None,
            artist_user_rating: None,
            album_created: None,
            album_play_count: None,
            track: None,
            disc_number: None,
            year: None,
            genre: None,
            cover_art: None,
            duration: None,
            bit_rate: None,
            content_type: None,
            suffix: None,
            size: None,
            path: None,
            starred: None,
            user_rating: None,
        };
        let cols = FzfColumns {
            title: 0,
            ..FzfColumns::default()
        };
        let line = fzf_input_lines(std::slice::from_ref(&s), cols);
        let title_field = line.split('\t').nth(3).unwrap();
        assert_eq!(title_field, "a".repeat(50));
    }

    #[test]
    fn fzf_lines_escape_tabs_in_title() {
        let s = Song {
            id: "id".into(),
            title: "a\tb".into(),
            album: None,
            artist: None,
            album_id: None,
            artist_id: None,
            album_artist: None,
            album_artist_id: None,
            album_artists: Vec::new(),
            album_user_rating: None,
            artist_user_rating: None,
            album_created: None,
            album_play_count: None,
            track: None,
            disc_number: None,
            year: None,
            genre: None,
            cover_art: None,
            duration: None,
            bit_rate: None,
            content_type: None,
            suffix: None,
            size: None,
            path: None,
            starred: None,
            user_rating: None,
        };
        let line = fzf_input_lines(std::slice::from_ref(&s), FzfColumns::default());
        assert_eq!(
            line.matches('\t').count(),
            4,
            "exactly 4 tabs as delimiters"
        );
    }

    #[test]
    fn flat_albums_newest_sort_uses_created() {
        fn track(id: &str, album: &str, created: &str) -> Song {
            Song {
                id: id.into(),
                title: "T".into(),
                album: Some(album.into()),
                artist: Some("A".into()),
                album_id: Some(format!("al-{album}")),
                artist_id: Some("ar-a".into()),
                album_artist: Some("A".into()),
                album_artist_id: Some("ar-a".into()),
                album_artists: Vec::new(),
                album_user_rating: None,
                artist_user_rating: None,
                album_created: Some(created.into()),
                album_play_count: None,
                track: Some(1),
                disc_number: Some(1),
                year: None,
                genre: None,
                cover_art: None,
                duration: Some(60),
                bit_rate: None,
                content_type: None,
                suffix: None,
                size: None,
                path: None,
                starred: None,
                user_rating: None,
            }
        }
        let snap = build_browse_snapshot(&[
            track("1", "Old", "2020-01-01T00:00:00Z"),
            track("2", "New", "2024-06-01T00:00:00Z"),
        ]);
        assert_eq!(snap.flat_albums.len(), 2);
        assert_eq!(snap.flat_albums[0].name, "New");
        assert_eq!(snap.flat_albums[1].name, "Old");
        assert!(index_supports_album_sort(
            &snap.flat_albums,
            AlbumListSort::Newest
        ));

        let mut by_name = snap.flat_albums.clone();
        sort_flat_albums(&mut by_name, AlbumListSort::AlphabeticalByName);
        assert_eq!(by_name[0].name, "New");
        assert_eq!(by_name[1].name, "Old");
    }

    #[test]
    fn index_supports_newest_false_without_created() {
        let snap = build_browse_snapshot(&[Song {
            id: "1".into(),
            title: "T".into(),
            album: Some("A".into()),
            artist: Some("Art".into()),
            album_id: Some("al-a".into()),
            artist_id: Some("ar".into()),
            album_artist: Some("Art".into()),
            album_artist_id: Some("ar".into()),
            album_artists: Vec::new(),
            album_user_rating: None,
            artist_user_rating: None,
            album_created: None,
            album_play_count: None,
            track: Some(1),
            disc_number: Some(1),
            year: None,
            genre: None,
            cover_art: None,
            duration: Some(60),
            bit_rate: None,
            content_type: None,
            suffix: None,
            size: None,
            path: None,
            starred: None,
            user_rating: None,
        }]);
        assert!(!index_supports_album_sort(
            &snap.flat_albums,
            AlbumListSort::Newest
        ));
        assert!(index_supports_album_sort(
            &snap.flat_albums,
            AlbumListSort::AlphabeticalByName
        ));
    }

    #[test]
    fn fzf_column_cjk_uses_display_width() {
        let cols = FzfColumns {
            title: 6,
            ..FzfColumns::default()
        };
        let field = format_fzf_column("日本語", cols.title);
        assert_eq!(text_width::str_width(&field), cols.title);
    }
}
