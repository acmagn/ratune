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

/// Parse `--with-nth=2,3,4,5` (or `--with-nth 2,3,4,5`) from fzf argv.
pub fn parse_fzf_with_nth(args: &[String]) -> Vec<usize> {
    parse_fzf_flag_value(args, "--with-nth")
        .map(|s| parse_fzf_field_indices(&s))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| FZF_DEFAULT_WITH_NTH.to_vec())
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

/// Tab-separated header labels padded to match [`fzf_input_lines`] columns in the order
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
pub fn fzf_input_lines(tracks: &[Song], cols: FzfColumns) -> String {
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
        let artist = format_fzf_column(&sanitize_field(artist), cols.artist);
        let album = format_fzf_column(&sanitize_field(album), cols.album);
        let title = format_fzf_column(&sanitize_field(title), cols.title);
        let dur = format_fzf_column(&sanitize_field(&dur), cols.duration);
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
/// Browse tab from local data (online when the index is available, and offline).
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

/// Artist id/name used for Browse columns — prefers album artist so compilations match
/// server `getArtists` (not per-track guest/feature artists).
///
/// Returns **all** album artists when OpenSubsonic `albumArtists` lists several (classical
/// composer + performer). Falls back to a single album-artist / legacy grouping.
fn browse_artists_for_album(songs: &[Song]) -> Vec<(String, String)> {
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

fn browse_artist_for_album_fallback(songs: &[Song]) -> (String, String) {
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

fn sort_songs(songs: &mut [Song]) {
    songs.sort_by_key(|s| (s.disc_number.unwrap_or(1), s.track.unwrap_or(0)));
}

/// Derive browse columns from the on-disk library index (same data as the fzf picker).
///
/// Albums are grouped under **album artist** (not track artist) so Browse matches the
/// live Subsonic `getArtists` / `getArtist` hierarchy.
pub fn build_browse_snapshot(tracks: &[Song]) -> BrowseSnapshot {
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
    }

    // Phase 1: gather songs by album id (shared album_id stays one album even on compilations).
    let mut by_album: HashMap<String, Vec<Song>> = HashMap::new();
    let mut album_order_names: HashMap<String, String> = HashMap::new();
    for song in tracks {
        let album_id = offline_album_id(song);
        album_order_names
            .entry(album_id.clone())
            .or_insert_with(|| {
                song.album
                    .clone()
                    .unwrap_or_else(|| "Unknown Album".to_string())
            });
        by_album.entry(album_id).or_default().push(song.clone());
    }

    // Phase 2: list each album under every album artist (classical composer + performer).
    let mut by_artist: HashMap<String, (String, HashMap<String, AlbumAcc>)> = HashMap::new();
    let mut tracks_by_album: HashMap<String, Vec<Song>> = HashMap::new();

    for (album_id, mut songs) in by_album {
        sort_songs(&mut songs);
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
        tracks_by_album.insert(album_id.clone(), songs);

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
                song: Vec::new(),
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
                album_artist: Some(artist.into()),
                album_artist_id: Some(format!("ar-{artist}")),
                album_artists: Vec::new(),
                album_user_rating: None,
                artist_user_rating: None,
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
        let album_tracks = snap.tracks_by_album.get(&albums[0].id).unwrap();
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
        assert_eq!(snap.tracks_by_album.get(&albums[0].id).unwrap().len(), 3);
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

        let snap = build_browse_snapshot(&[track("1", "Alice", "Alpha", 1)]);
        // Artist 1–5 ratings are overlaid live from getArtists; album ratings come from the index.
        let albums = snap.albums_by_artist.get(&snap.artists[0].id).unwrap();
        assert_eq!(albums[0].user_rating, Some(4));
        let songs = snap.tracks_by_album.get(&albums[0].id).unwrap();
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
        assert_eq!(parse_fzf_with_nth(&args), vec![2, 3, 5, 4]);
        assert_eq!(parse_fzf_with_nth(&[]), vec![2, 3, 4, 5]);
        let spaced = vec!["--with-nth".into(), "2,3,5,4".into()];
        assert_eq!(parse_fzf_with_nth(&spaced), vec![2, 3, 5, 4]);
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
    fn fzf_column_cjk_uses_display_width() {
        let cols = FzfColumns {
            title: 6,
            ..FzfColumns::default()
        };
        let field = format_fzf_column("日本語", cols.title);
        assert_eq!(text_width::str_width(&field), cols.title);
    }
}
