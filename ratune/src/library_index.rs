//! On-disk library metadata index for fuzzy picking (fzf) without live Subsonic
//! calls per keystroke.
//!
//! Stored as JSON under `~/.cache/ratune/library_index.json` by default (see
//! config). Text only — no art or audio.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ratune_subsonic::Song;
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

/// Default path: `~/.cache/ratune/library_index.json`.
pub fn default_index_path() -> Option<PathBuf> {
    let base = dirs_cache_base()?;
    Some(base.join("library_index.json"))
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

/// Atomically write the index (temp + rename).
pub fn save(
    path: &Path,
    tracks: &[Song],
    refreshed_at_unix: u64,
    navidrome_last_scan: Option<&str>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
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
    s.replace(['\t', '\n'], " ")
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
    t.chars()
        .take(max_chars.saturating_sub(1))
        .chain(Some('…'))
        .collect()
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

/// Artist/album/track hierarchy derived from a flat library index — used to drive the
/// Browse tab while offline (no live Subsonic calls).
#[derive(Debug, Clone)]
pub struct BrowseSnapshot {
    pub artists: Vec<ratune_subsonic::Artist>,
    pub albums_by_artist: std::collections::HashMap<String, Vec<ratune_subsonic::Album>>,
    pub tracks_by_album: std::collections::HashMap<String, Vec<Song>>,
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

fn offline_album_id(song: &Song, artist_id: &str) -> String {
    song.album_id
        .clone()
        .or_else(|| {
            song.album
                .as_ref()
                .map(|name| format!("__offline_album__{artist_id}__{name}"))
        })
        .unwrap_or_else(|| format!("__offline_unknown_album__{artist_id}"))
}

fn album_sort_key(a: &ratune_subsonic::Album) -> (u32, String) {
    (a.year.unwrap_or(0), a.name.to_lowercase())
}

fn sort_songs(songs: &mut [Song]) {
    songs.sort_by_key(|s| (s.disc_number.unwrap_or(1), s.track.unwrap_or(0)));
}

/// Derive browse columns from the on-disk library index (same data as the fzf picker).
pub fn build_browse_snapshot(tracks: &[Song]) -> BrowseSnapshot {
    use ratune_subsonic::{Album, Artist};
    use std::collections::HashMap;

    struct AlbumAcc {
        name: String,
        artist_name: String,
        songs: Vec<Song>,
    }

    struct ArtistAcc {
        name: String,
        albums: HashMap<String, AlbumAcc>,
    }

    let mut by_artist: HashMap<String, ArtistAcc> = HashMap::new();

    for song in tracks {
        let artist_id = offline_artist_id(song);
        let artist_name = song
            .artist
            .clone()
            .unwrap_or_else(|| "Unknown Artist".to_string());
        let album_id = offline_album_id(song, &artist_id);
        let album_name = song
            .album
            .clone()
            .unwrap_or_else(|| "Unknown Album".to_string());

        let artist = by_artist
            .entry(artist_id.clone())
            .or_insert_with(|| ArtistAcc {
                name: artist_name.clone(),
                albums: HashMap::new(),
            });
        if artist.name == "Unknown Artist" && artist_name != "Unknown Artist" {
            artist.name = artist_name.clone();
        }

        let album = artist.albums.entry(album_id.clone()).or_insert_with(|| AlbumAcc {
            name: album_name.clone(),
            artist_name: artist_name.clone(),
            songs: Vec::new(),
        });
        if album.name == "Unknown Album" && album_name != "Unknown Album" {
            album.name = album_name;
        }
        album.songs.push(song.clone());
    }

    let mut artists: Vec<Artist> = by_artist
        .iter()
        .map(|(id, acc)| Artist {
            id: id.clone(),
            name: acc.name.clone(),
            album_count: Some(acc.albums.len() as u32),
            cover_art: acc
                .albums
                .values()
                .flat_map(|a| a.songs.first())
                .find_map(|s| s.cover_art.clone()),
            starred: None,
            album: Vec::new(),
        })
        .collect();
    artists.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let mut albums_by_artist = HashMap::new();
    let mut tracks_by_album = HashMap::new();

    for (artist_id, acc) in by_artist {
        let mut albums: Vec<Album> = acc
            .albums
            .into_iter()
            .map(|(album_id, album_acc)| {
                let mut songs = album_acc.songs;
                sort_songs(&mut songs);
                let song_count = songs.len() as u32;
                let duration: u32 = songs.iter().filter_map(|s| s.duration).sum();
                let year = songs.iter().find_map(|s| s.year);
                let genre = songs.iter().find_map(|s| s.genre.clone());
                let cover_art = songs
                    .iter()
                    .find_map(|s| s.cover_art.clone())
                    .or_else(|| Some(album_id.clone()));
                tracks_by_album.insert(album_id.clone(), songs);
                Album {
                    id: album_id,
                    name: album_acc.name,
                    artist: Some(album_acc.artist_name),
                    artist_id: Some(artist_id.clone()),
                    cover_art,
                    song_count: Some(song_count),
                    duration: if duration > 0 { Some(duration) } else { None },
                    year,
                    genre,
                    starred: None,
                    song: Vec::new(),
                }
            })
            .collect();

        albums.sort_by(|a, b| album_sort_key(a).cmp(&album_sort_key(b)));
        albums_by_artist.insert(artist_id, albums);
    }

    BrowseSnapshot {
        artists,
        albums_by_artist,
        tracks_by_album,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let album_tracks = snap.tracks_by_album.get(&albums[0].id).unwrap();
        assert_eq!(album_tracks.len(), 2);
        assert_eq!(album_tracks[0].track, Some(1));
        assert_eq!(album_tracks[1].track, Some(2));
    }

    #[test]
    fn parse_pick_line_basic() {
        let line = "song1\tArtist\tAlbum\tTitle\t3:00\n";
        assert_eq!(parse_pick_line(line).as_deref(), Some("song1"));
    }

    #[test]
    fn fzf_header_matches_column_widths() {
        let h = fzf_header_line();
        assert_eq!(
            h.matches('\t').count(),
            3,
            "four columns: Artist | Album | Title | Time"
        );
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
        assert_eq!(
            line.matches('\t').count(),
            4,
            "exactly 4 tabs as delimiters"
        );
    }
}
