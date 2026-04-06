//! On-disk library metadata index for fuzzy picking (fzf) without live Subsonic
//! calls per keystroke.
//!
//! Stored as JSON under `~/.cache/playterm/library_index.json` by default (see
//! config). Text only — no art or audio.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use playterm_subsonic::Song;
use serde::{Deserialize, Serialize};

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
}

impl LibraryIndexFile {
    pub fn new(
        tracks: Vec<Song>,
        refreshed_at_unix: u64,
        navidrome_last_scan: Option<String>,
    ) -> Self {
        Self {
            version: FORMAT_VERSION,
            refreshed_at_unix,
            tracks,
            navidrome_last_scan,
        }
    }
}

/// Default path: `~/.cache/playterm/library_index.json`.
pub fn default_index_path() -> Option<PathBuf> {
    let base = dirs_cache_base()?;
    Some(base.join("library_index.json"))
}

fn dirs_cache_base() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return Some(PathBuf::from(xdg).join("playterm"));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".cache").join("playterm"))
}

/// Load an index from disk. Returns `None` if missing or unreadable.
pub fn load(path: &Path) -> Option<LibraryIndexFile> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Atomically write the index (temp + rename).
pub fn save(
    path: &Path,
    tracks: &[Song],
    refreshed_at_unix: u64,
    navidrome_last_scan: Option<&str>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let file = LibraryIndexFile::new(
        tracks.to_vec(),
        refreshed_at_unix,
        navidrome_last_scan.map(String::from),
    );
    let json = serde_json::to_string_pretty(&file).context("serialize library index")?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_extension(format!("json.{nanos}.part"));
    let mut f = fs::File::create(&temp).with_context(|| format!("writing {}", temp.display()))?;
    f.write_all(json.as_bytes())?;
    f.sync_all().ok();
    drop(f);
    fs::rename(&temp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(())
}

/// Max display width (Unicode scalars) per column after truncation. Keeps fzf rows
/// readable; full metadata stays in the index for enqueue.
const FZF_COL_ARTIST: usize = 26;
const FZF_COL_ALBUM: usize = 28;
const FZF_COL_TITLE: usize = 36;
const FZF_COL_DUR: usize = 6;

fn sanitize_field(s: &str) -> String {
    s.replace('\t', " ").replace('\n', " ")
}

/// Truncate with ellipsis; never wider than `max_chars` scalars.
fn truncate_display(s: &str, max_chars: usize) -> String {
    let t = s.trim();
    let count = t.chars().count();
    if count <= max_chars {
        return t.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    t.chars().take(max_chars.saturating_sub(1)).chain(Some('…')).collect()
}

/// Truncate then pad with spaces so columns line up in a monospace terminal.
fn format_fzf_column(s: &str, width: usize) -> String {
    let t = truncate_display(s, width);
    let n = t.chars().count();
    if n < width {
        format!("{t}{}", " ".repeat(width - n))
    } else {
        t
    }
}

/// Tab-separated header labels padded to match [`fzf_input_lines`] columns (artist–time;
/// song id is not shown). Pass as `fzf --header=…` so labels line up with data.
pub fn fzf_header_line() -> String {
    let artist = format_fzf_column("Artist", FZF_COL_ARTIST);
    let album = format_fzf_column("Album", FZF_COL_ALBUM);
    let title = format_fzf_column("Title", FZF_COL_TITLE);
    let time = format_fzf_column("Time", FZF_COL_DUR);
    format!("{artist}\t{album}\t{title}\t{time}")
}

/// One TSV line per track for fzf: id, artist, album, title, duration.
/// Field 1 is the song id (hidden in the fzf *list* via `--with-nth=2,3,4,5`).
/// Default `[library] fzf_args` uses `--nth=1,2,3` (artist, album, title; duration is
/// shown but excluded from fuzzy search). Long strings are truncated for display only.
pub fn fzf_input_lines(tracks: &[Song]) -> String {
    let mut out = String::new();
    for s in tracks {
        let artist = s.artist.as_deref().unwrap_or("—");
        let album = s.album.as_deref().unwrap_or("—");
        let title = s.title.as_str();
        let dur = s
            .duration
            .map(fmt_duration_ms)
            .unwrap_or_else(|| "—".to_string());
        let id = sanitize_field(&s.id);
        let artist = format_fzf_column(&sanitize_field(artist), FZF_COL_ARTIST);
        let album = format_fzf_column(&sanitize_field(album), FZF_COL_ALBUM);
        let title = format_fzf_column(&sanitize_field(title), FZF_COL_TITLE);
        let dur = format_fzf_column(&sanitize_field(&dur), FZF_COL_DUR);
        out.push_str(&format!("{id}\t{artist}\t{album}\t{title}\t{dur}\n"));
    }
    out
}

/// Parse the first field (song id) from a line emitted by [`fzf_input_lines`].
pub fn parse_pick_line(line: &str) -> Option<String> {
    let line = line.trim_end_matches('\n');
    line.split('\t').next().map(String::from)
}

fn fmt_duration_ms(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}

/// Build an id → song map for enqueue after fzf.
pub fn index_by_id(tracks: &[Song]) -> std::collections::HashMap<String, Song> {
    tracks.iter().cloned().map(|s| (s.id.clone(), s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pick_line_basic() {
        let line = "song1\tArtist\tAlbum\tTitle\t3:00\n";
        assert_eq!(parse_pick_line(line).as_deref(), Some("song1"));
    }

    #[test]
    fn fzf_header_matches_column_widths() {
        let h = fzf_header_line();
        assert_eq!(h.matches('\t').count(), 3, "four columns: Artist | Album | Title | Time");
        let cols: Vec<&str> = h.split('\t').collect();
        assert_eq!(cols.len(), 4);
        assert_eq!(cols[0].chars().count(), 26);
        assert_eq!(cols[1].chars().count(), 28);
        assert_eq!(cols[2].chars().count(), 36);
        assert_eq!(cols[3].chars().count(), 6);
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
        };
        let line = fzf_input_lines(std::slice::from_ref(&s));
        assert_eq!(line.matches('\t').count(), 4, "exactly 4 tabs as delimiters");
    }
}
