use std::time::Duration;

use serde::{Deserialize, Serialize};
use crate::error::SubsonicError;

// ── Public domain types ───────────────────────────────────────────────────────

/// A single artist entry as returned by `getArtists`, `getArtist`, or `search3`.
///
/// When returned by `getArtists` the `album` list is empty; when returned by
/// `getArtist` it is populated with album stubs (no songs).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub album_count: Option<u32>,
    pub cover_art: Option<String>,
    pub starred: Option<String>,
    /// Album stubs — populated only by `getArtist`, empty from `getArtists`.
    #[serde(default)]
    pub album: Vec<Album>,
}

/// One letter-bucket from a `getArtists` index response.
#[derive(Debug, Clone, Deserialize)]
pub struct ArtistIndex {
    /// The index letter or prefix (e.g. `"A"`, `"#"`).
    pub name: String,
    #[serde(default)]
    pub artist: Vec<Artist>,
}

/// Top-level `artists` object from a `getArtists` response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artists {
    /// Space-separated articles the server strips when alphabetising names.
    #[serde(default)]
    pub ignored_articles: String,
    /// Alphabetical buckets, each containing one or more artists.
    #[serde(default)]
    pub index: Vec<ArtistIndex>,
}

/// A single track (song) as returned by `getSong`, `getAlbum`, or `search3`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub id: String,
    pub title: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub cover_art: Option<String>,
    /// Duration in seconds.
    pub duration: Option<u32>,
    /// Bitrate in kbps.
    pub bit_rate: Option<u32>,
    pub content_type: Option<String>,
    pub suffix: Option<String>,
    pub size: Option<u64>,
    pub path: Option<String>,
    pub starred: Option<String>,
}

/// An album as returned by `getAlbum` or `search3`.
///
/// When returned by `getAlbum` the `song` list is populated; in search results
/// it is empty.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub cover_art: Option<String>,
    pub song_count: Option<u32>,
    /// Total duration in seconds.
    pub duration: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub starred: Option<String>,
    /// Tracks — populated only by `getAlbum`, empty for search results.
    #[serde(default)]
    pub song: Vec<Song>,
}

/// A playlist entry as returned by `getPlaylists`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub song_count: Option<u32>,
    pub duration: Option<u64>,
    pub owner: Option<String>,
    pub public: Option<bool>,
}

/// A playlist with its full track list as returned by `getPlaylist`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistDetail {
    pub id: String,
    pub name: String,
    pub song_count: Option<u32>,
    pub duration: Option<u64>,
    /// Track entries — the Subsonic API uses the key `entry` for these.
    #[serde(default, rename = "entry")]
    pub songs: Vec<Song>,
}

/// Combined search result from `search3`.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult3 {
    #[serde(default)]
    pub artist: Vec<Artist>,
    #[serde(default)]
    pub album: Vec<Album>,
    #[serde(default)]
    pub song: Vec<Song>,
}

/// A snapshot of the Navidrome library sufficient for browsing.
///
/// Built cheaply at startup with a single `getArtists` call; album tracks are
/// fetched lazily via [`crate::client::fetch_songs_for_artist`] only when the
/// user selects an artist.
#[derive(Debug, Clone)]
pub struct SubsonicLibrary {
    /// All artists, sorted by name.
    pub artists: Vec<Artist>,
}

/// Library scan state from `getScanStatus` (Subsonic 1.15+). Navidrome includes
/// `last_scan` as an RFC3339 timestamp after a completed scan.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    #[serde(default)]
    pub scanning: bool,
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub folder_count: i64,
    pub last_scan: Option<String>,
}

// ── Lyrics types ──────────────────────────────────────────────────────────────

/// One line of lyrics returned by `getLyricsBySongId`.
///
/// `time` is `Some(offset)` for synced (LRC-style) lyrics where the line
/// should be highlighted at the given playback position, or `None` for plain
/// unsynced text.
#[derive(Debug, Clone)]
pub struct LyricLine {
    /// Playback offset at which to highlight this line; `None` = unsynced.
    pub time: Option<Duration>,
    /// The lyric text.
    pub text: String,
}

// ── Private serde envelope types ──────────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct PingEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: PingBody,
}

#[derive(Deserialize)]
pub(crate) struct PingBody {
    pub status: String,
    pub error: Option<SubsonicError>,
}

#[derive(Deserialize)]
pub(crate) struct ArtistsEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: ArtistsBody,
}

#[derive(Deserialize)]
pub(crate) struct ArtistsBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    pub artists: Option<Artists>,
}

#[derive(Deserialize)]
pub(crate) struct ArtistEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: ArtistBody,
}

#[derive(Deserialize)]
pub(crate) struct ArtistBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    pub artist: Option<Artist>,
}

#[derive(Deserialize)]
pub(crate) struct AlbumEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: AlbumBody,
}

#[derive(Deserialize)]
pub(crate) struct AlbumBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    pub album: Option<Album>,
}

#[derive(Deserialize)]
pub(crate) struct SongEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: SongBody,
}

#[derive(Deserialize)]
pub(crate) struct SongBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    pub song: Option<Song>,
}

#[derive(Deserialize)]
pub(crate) struct SearchEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: SearchBody,
}

#[derive(Deserialize)]
pub(crate) struct SearchBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    #[serde(rename = "searchResult3")]
    pub search_result3: Option<SearchResult3>,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistsContainer {
    #[serde(default)]
    pub playlist: Vec<Playlist>,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistsEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: PlaylistsBody,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistsBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    pub playlists: Option<PlaylistsContainer>,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: PlaylistBody,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    pub playlist: Option<PlaylistDetail>,
}

#[derive(Deserialize)]
pub(crate) struct ScanStatusEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: ScanStatusBody,
}

#[derive(Deserialize)]
pub(crate) struct ScanStatusBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    #[serde(rename = "scanStatus")]
    pub scan_status: Option<ScanStatus>,
}
