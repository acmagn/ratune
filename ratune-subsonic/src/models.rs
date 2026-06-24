use std::time::Duration;

use crate::error::SubsonicError;
use serde::{Deserialize, Serialize};

fn deserialize_artists_bucket<'de, D>(deserializer: D) -> Result<Vec<Artist>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Bucket {
        Single(Artist),
        Many(Vec<Artist>),
    }
    Bucket::deserialize(deserializer).map(|b| match b {
        Bucket::Single(a) => vec![a],
        Bucket::Many(v) => v,
    })
}

fn deserialize_indexes_bucket<'de, D>(deserializer: D) -> Result<Vec<ArtistIndex>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Bucket {
        Single(ArtistIndex),
        Many(Vec<ArtistIndex>),
    }
    Bucket::deserialize(deserializer).map(|b| match b {
        Bucket::Single(x) => vec![x],
        Bucket::Many(v) => v,
    })
}

// ── Public domain types ───────────────────────────────────────────────────────

/// A single artist entry as returned by `getArtists`, `getArtist`, or `search3`.
///
/// When returned by `getArtists` the `album` list is empty; when returned by
/// `getArtist` it is populated with album stubs (no songs).
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// One letter-bucket from a `getArtists` / `getIndexes` index response.
#[derive(Debug, Clone, Deserialize)]
pub struct ArtistIndex {
    /// The index letter or prefix (e.g. `"A"`, `"#"`).
    pub name: String,
    /// Some APIs return one `artist` object instead of an array.
    #[serde(default, deserialize_with = "deserialize_artists_bucket")]
    pub artist: Vec<Artist>,
}

/// Top-level `artists` / `indexes` object from `getArtists` / `getIndexes`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artists {
    /// Space-separated articles the server strips when alphabetising names.
    #[serde(default)]
    pub ignored_articles: String,
    /// Alphabetical buckets; some payloads use a single `index` object instead of an array.
    #[serde(default, deserialize_with = "deserialize_indexes_bucket")]
    pub index: Vec<ArtistIndex>,
}

/// Same JSON shape as [`Artists`], nested under `indexes` (`getIndexes`) instead of `artists`.
pub type Indexes = Artists;

/// Cache key prefix for the first Browse level under a **`getMusicFolders` entry**.
///
/// The segment after this prefix must be exactly that entry's **`id` attribute**, which is passed
/// as **`musicFolderId`** to `getIndexes` (matches Subsonic/OpenSubsonic; avoids assuming array indices).
pub const MUSIC_FOLDER_ROOT_ID_PREFIX: &str = "__mf_root_";

#[inline]
#[must_use]
pub fn music_library_root_cache_key(music_folder_id: impl AsRef<str>) -> String {
    format!(
        "{}{}",
        MUSIC_FOLDER_ROOT_ID_PREFIX,
        music_folder_id.as_ref()
    )
}

/// Returns the **`musicFolder` id substring** encoded in `cache_id` (see [`music_library_root_cache_key`]).
#[inline]
pub fn parse_music_library_root_folder_id(cache_id: &str) -> Option<&str> {
    cache_id.strip_prefix(MUSIC_FOLDER_ROOT_ID_PREFIX)
}
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// An internet radio station from `getInternetRadioStations`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternetRadioStation {
    pub id: String,
    pub name: String,
    pub stream_url: String,
    pub home_page_url: Option<String>,
    /// OpenSubsonic / Navidrome uploaded image — pass to `getCoverArt`.
    pub cover_art: Option<String>,
}

impl InternetRadioStation {
    /// Stable cache key for now-playing art (not always a `getCoverArt` id).
    #[must_use]
    pub fn art_cache_key(&self) -> String {
        format!("radio:{}", self.id)
    }

    /// Navidrome `getCoverArt` id when the station has an uploaded logo.
    #[must_use]
    pub fn uploaded_cover_art_id(&self) -> Option<&str> {
        self.cover_art.as_deref().filter(|id| !id.is_empty())
    }

    /// Hostname for the now-playing subtitle (from `homePageUrl`).
    #[must_use]
    pub fn display_subtitle(&self) -> Option<String> {
        self.home_page_url.as_ref().and_then(|u| {
            url::Url::parse(u)
                .ok()
                .and_then(|p| p.host_str().map(str::to_string))
        })
    }

    /// Favicon URL used by Navidrome when no uploaded station image exists.
    #[must_use]
    pub fn favicon_url(&self) -> Option<String> {
        Self::site_origin(
            self.home_page_url
                .as_deref()
                .or(Some(self.stream_url.as_str()))?,
        )
        .map(|origin| format!("{origin}/favicon.ico"))
    }

    /// Homepage logo candidates, best/largest sources first (`apple-touch-icon` then favicon).
    #[must_use]
    pub fn station_icon_urls(&self) -> Vec<String> {
        let base = self
            .home_page_url
            .as_deref()
            .or(Some(self.stream_url.as_str()));
        let Some(origin) = base.and_then(Self::site_origin) else {
            return Vec::new();
        };
        [
            format!("{origin}/apple-touch-icon.png"),
            format!("{origin}/apple-touch-icon-precomposed.png"),
            format!("{origin}/favicon.ico"),
        ]
        .into_iter()
        .collect()
    }

    fn site_origin(base: &str) -> Option<String> {
        let mut parsed = url::Url::parse(base).ok()?;
        parsed.set_path("/");
        parsed.set_query(None);
        parsed.set_fragment(None);
        Some(parsed.as_str().trim_end_matches('/').to_string())
    }
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

/// Top-level music library folder from `getMusicFolders`.
#[derive(Debug, Clone)]
pub struct MusicFolder {
    pub id: String,
    pub name: String,
}

/// One row from `getMusicDirectory` (`child` in the API).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryChild {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub id: String,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_id")]
    pub parent: Option<String>,
    #[serde(rename = "isDir", default)]
    pub is_dir: bool,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub duration: Option<u32>,
    pub cover_art: Option<String>,
    pub path: Option<String>,
    pub suffix: Option<String>,
    pub content_type: Option<String>,
}

impl DirectoryChild {
    /// Convert a file entry into a [`Song`] for queue/playback.
    pub fn to_song(&self) -> Song {
        Song {
            id: self.id.clone(),
            title: self.title.clone(),
            album: self.album.clone(),
            artist: self.artist.clone(),
            album_id: self.album_id.clone(),
            artist_id: self.artist_id.clone(),
            track: self.track,
            disc_number: self.disc_number,
            year: None,
            genre: None,
            cover_art: self.cover_art.clone(),
            duration: self.duration,
            bit_rate: None,
            content_type: self.content_type.clone(),
            suffix: self.suffix.clone(),
            size: None,
            path: self.path.clone(),
            starred: None,
        }
    }
}

/// Parsed listing for one directory (`getMusicDirectory`).
#[derive(Debug, Clone)]
pub struct MusicDirectory {
    pub id: String,
    pub name: String,
    pub directories: Vec<DirectoryChild>,
    pub songs: Vec<Song>,
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
#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    /// Playback offset at which to highlight this line; `None` = unsynced.
    pub time: Option<Duration>,
    /// The lyric text.
    pub text: String,
}

/// Convert OpenSubsonic [`StructuredLyricsRaw`] entries into display lines.
///
/// Prefers synced `main` lyrics, then any synced track, then the first entry.
pub(crate) fn structured_lyrics_to_lines(entries: &[StructuredLyricsRaw]) -> Vec<LyricLine> {
    pick_best_structured_lyrics(entries)
        .map(structured_lyrics_entry_to_lines)
        .unwrap_or_default()
}

fn pick_best_structured_lyrics(entries: &[StructuredLyricsRaw]) -> Option<&StructuredLyricsRaw> {
    entries
        .iter()
        .find(|e| e.synced && e.kind.as_deref().unwrap_or("main") == "main")
        .or_else(|| entries.iter().find(|e| e.synced))
        .or_else(|| entries.first())
}

fn structured_lyrics_entry_to_lines(entry: &StructuredLyricsRaw) -> Vec<LyricLine> {
    let offset_ms = entry.offset.unwrap_or(0);
    entry
        .line
        .iter()
        .map(|l| {
            let time = l.start.map(|start| {
                let adjusted = (i64::from(start) + offset_ms).max(0) as u64;
                Duration::from_millis(adjusted)
            });
            LyricLine {
                time,
                text: l.value.clone(),
            }
        })
        .collect()
}

fn deserialize_structured_lyrics_list<'de, D>(
    deserializer: D,
) -> Result<Vec<StructuredLyricsRaw>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Single(StructuredLyricsRaw),
        List(Vec<StructuredLyricsRaw>),
    }
    match OneOrMany::deserialize(deserializer)? {
        OneOrMany::Single(o) => Ok(vec![o]),
        OneOrMany::List(v) => Ok(v),
    }
}

/// One structured lyrics block from `getLyricsBySongId`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StructuredLyricsRaw {
    #[serde(default)]
    pub(crate) kind: Option<String>,
    #[serde(default)]
    pub(crate) synced: bool,
    #[serde(default)]
    pub(crate) offset: Option<i64>,
    #[serde(default)]
    pub(crate) line: Vec<StructuredLyricLineRaw>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct StructuredLyricLineRaw {
    #[serde(default)]
    pub(crate) start: Option<u32>,
    pub(crate) value: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LyricsListRaw {
    #[serde(
        default,
        rename = "structuredLyrics",
        deserialize_with = "deserialize_structured_lyrics_list"
    )]
    pub(crate) structured_lyrics: Vec<StructuredLyricsRaw>,
}

#[derive(Deserialize)]
pub(crate) struct LyricsBySongIdEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: LyricsBySongIdBody,
}

#[derive(Deserialize)]
pub(crate) struct LyricsBySongIdBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    #[serde(rename = "lyricsList")]
    pub lyrics_list: Option<LyricsListRaw>,
}

#[derive(Deserialize)]
pub(crate) struct LegacyLyricsEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: LegacyLyricsBody,
}

#[derive(Deserialize)]
pub(crate) struct LegacyLyricsBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    pub lyrics: Option<LegacyLyricsRaw>,
}

#[derive(Deserialize)]
pub(crate) struct LegacyLyricsRaw {
    pub value: Option<String>,
}

// ── Private serde envelope types ──────────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct IndexesEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: IndexesBody,
}

#[derive(Deserialize)]
pub(crate) struct IndexesBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    pub indexes: Option<Indexes>,
}

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

/// Starred library items from `getStarred2`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Starred2 {
    #[serde(default)]
    pub artist: Vec<Artist>,
    #[serde(default)]
    pub album: Vec<Album>,
    #[serde(default)]
    pub song: Vec<Song>,
    /// Legacy `getStarred` responses use `entry` for songs.
    #[serde(default, rename = "entry", skip_serializing)]
    song_entry: Vec<Song>,
}

impl Starred2 {
    /// Songs from either `song` or legacy `entry` arrays.
    pub fn songs(&self) -> &[Song] {
        if !self.song.is_empty() {
            &self.song
        } else {
            &self.song_entry
        }
    }

    pub fn normalize(mut self) -> Self {
        if self.song.is_empty() && !self.song_entry.is_empty() {
            self.song = std::mem::take(&mut self.song_entry);
        }
        self
    }
}

fn deserialize_internet_radio_station_list<'de, D>(
    deserializer: D,
) -> Result<Vec<InternetRadioStation>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Single(InternetRadioStation),
        List(Vec<InternetRadioStation>),
    }
    match OneOrMany::deserialize(deserializer)? {
        OneOrMany::Single(o) => Ok(vec![o]),
        OneOrMany::List(v) => Ok(v),
    }
}

#[derive(Deserialize)]
pub(crate) struct InternetRadioStationsContainer {
    #[serde(
        default,
        rename = "internetRadioStation",
        deserialize_with = "deserialize_internet_radio_station_list"
    )]
    pub internet_radio_station: Vec<InternetRadioStation>,
}

#[derive(Deserialize)]
pub(crate) struct InternetRadioStationsEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: InternetRadioStationsBody,
}

#[derive(Deserialize)]
pub(crate) struct InternetRadioStationsBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    #[serde(rename = "internetRadioStations")]
    pub internet_radio_stations: Option<InternetRadioStationsContainer>,
}

#[derive(Deserialize)]
pub(crate) struct Starred2Envelope {
    #[serde(rename = "subsonic-response")]
    pub response: Starred2Body,
}

#[derive(Deserialize)]
pub(crate) struct Starred2Body {
    pub status: String,
    pub error: Option<SubsonicError>,
    pub starred2: Option<Starred2>,
}

fn deserialize_music_folder_list<'de, D>(deserializer: D) -> Result<Vec<MusicFolderRaw>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Single(MusicFolderRaw),
        List(Vec<MusicFolderRaw>),
    }
    match OneOrMany::deserialize(deserializer)? {
        OneOrMany::Single(o) => Ok(vec![o]),
        OneOrMany::List(v) => Ok(v),
    }
}

#[derive(Deserialize)]
pub(crate) struct MusicFolderRaw {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Deserialize)]
pub(crate) struct MusicFoldersContainer {
    #[serde(
        default,
        rename = "musicFolder",
        deserialize_with = "deserialize_music_folder_list"
    )]
    pub(crate) music_folder: Vec<MusicFolderRaw>,
}

#[derive(Deserialize)]
pub(crate) struct MusicFoldersEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: MusicFoldersBody,
}

#[derive(Deserialize)]
pub(crate) struct MusicFoldersBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    #[serde(rename = "musicFolders")]
    pub music_folders: Option<MusicFoldersContainer>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct DirectoryRaw {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) child: Vec<DirectoryChild>,
}

#[derive(Deserialize)]
pub(crate) struct MusicDirectoryEnvelope {
    #[serde(rename = "subsonic-response")]
    pub response: MusicDirectoryBody,
}

#[derive(Deserialize)]
pub(crate) struct MusicDirectoryBody {
    pub status: String,
    pub error: Option<SubsonicError>,
    pub directory: Option<DirectoryRaw>,
}

fn deserialize_flexible_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    struct IdVisitor;
    impl Visitor<'_> for IdVisitor {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or integer id")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<String, E> {
            Ok(v)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<String, E> {
            Ok(v.to_string())
        }
    }
    deserializer.deserialize_any(IdVisitor)
}

fn deserialize_optional_flexible_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    struct OptIdVisitor;
    impl<'de> Visitor<'de> for OptIdVisitor {
        type Value = Option<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an optional string or integer id")
        }
        fn visit_none<E: de::Error>(self) -> Result<Option<String>, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Option<String>, E> {
            Ok(None)
        }
        fn visit_some<D2>(self, deserializer: D2) -> Result<Option<String>, D2::Error>
        where
            D2: serde::Deserializer<'de>,
        {
            deserialize_flexible_id(deserializer).map(Some)
        }
    }
    deserializer.deserialize_option(OptIdVisitor)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_ping_ok_envelope() {
        let j = r#"{"subsonic-response":{"status":"ok"}}"#;
        let env: PingEnvelope = serde_json::from_str(j).unwrap();
        assert_eq!(env.response.status, "ok");
        assert!(env.response.error.is_none());
    }

    #[test]
    fn starred2_songs_from_legacy_entry() {
        let j = r#"{"artist":[],"album":[],"entry":[{"id":"1","title":"Loved"}]}"#;
        let starred: Starred2 = serde_json::from_str(j).unwrap();
        let starred = starred.normalize();
        assert_eq!(starred.songs().len(), 1);
        assert_eq!(starred.songs()[0].id, "1");
    }

    #[test]
    fn deserialize_internet_radio_stations_envelope() {
        let j = r#"{
            "subsonic-response": {
                "status": "ok",
                "internetRadioStations": {
                    "internetRadioStation": [
                        {
                            "id": "1",
                            "name": "Dream Factory",
                            "streamUrl": "http://example.com/stream.aac",
                            "homePageUrl": "http://example.com/"
                        },
                        {
                            "id": "2",
                            "name": "Solo Station",
                            "streamUrl": "http://example.com/solo.ogg"
                        }
                    ]
                }
            }
        }"#;
        let env: InternetRadioStationsEnvelope = serde_json::from_str(j).unwrap();
        assert_eq!(env.response.status, "ok");
        let stations = env
            .response
            .internet_radio_stations
            .unwrap()
            .internet_radio_station;
        assert_eq!(stations.len(), 2);
        assert_eq!(stations[0].name, "Dream Factory");
        assert_eq!(stations[0].stream_url, "http://example.com/stream.aac");
        assert_eq!(stations[1].home_page_url, None);
    }

    #[test]
    fn internet_radio_station_art_cache_key() {
        let s = InternetRadioStation {
            id: "rd-1".into(),
            name: "Test".into(),
            stream_url: "http://x/stream".into(),
            home_page_url: None,
            cover_art: Some("ra-rd-1_abc".into()),
        };
        assert_eq!(s.art_cache_key(), "radio:rd-1");
        assert_eq!(s.uploaded_cover_art_id(), Some("ra-rd-1_abc"));
        let s2 = InternetRadioStation {
            cover_art: None,
            ..s
        };
        assert_eq!(s2.uploaded_cover_art_id(), None);
        let s3 = InternetRadioStation {
            cover_art: Some(String::new()),
            ..s2
        };
        assert_eq!(s3.uploaded_cover_art_id(), None);
    }

    #[test]
    fn deserialize_radio_station_with_cover_art() {
        let j = r#"{"id":"1","name":"FM","streamUrl":"http://x/aac","coverArt":"ra-1_0"}"#;
        let s: InternetRadioStation = serde_json::from_str(j).unwrap();
        assert_eq!(s.cover_art.as_deref(), Some("ra-1_0"));
    }

    #[test]
    fn internet_radio_station_favicon_url_from_homepage() {
        let s = InternetRadioStation {
            id: "1".into(),
            name: "YourClassical".into(),
            stream_url: "http://stream.example/aac".into(),
            home_page_url: Some("https://www.yourclassical.org".into()),
            cover_art: None,
        };
        assert_eq!(
            s.favicon_url().as_deref(),
            Some("https://www.yourclassical.org/favicon.ico")
        );
        assert_eq!(
            s.station_icon_urls(),
            vec![
                "https://www.yourclassical.org/apple-touch-icon.png".to_string(),
                "https://www.yourclassical.org/apple-touch-icon-precomposed.png".to_string(),
                "https://www.yourclassical.org/favicon.ico".to_string(),
            ]
        );
    }

    #[test]
    fn internet_radio_station_icon_urls_from_homepage_with_path() {
        let s = InternetRadioStation {
            id: "1".into(),
            name: "Site".into(),
            stream_url: "http://stream.example/aac".into(),
            home_page_url: Some("https://www.example.com/listen/live".into()),
            cover_art: None,
        };
        assert_eq!(
            s.station_icon_urls().first().map(String::as_str),
            Some("https://www.example.com/apple-touch-icon.png")
        );
    }

    #[test]
    fn deserialize_starred2_envelope() {
        let j = r#"{
            "subsonic-response": {
                "status": "ok",
                "starred2": {
                    "song": [{"id":"1","title":"Loved","albumId":"al1"}]
                }
            }
        }"#;
        let env: Starred2Envelope = serde_json::from_str(j).unwrap();
        assert_eq!(env.response.status, "ok");
        let starred = env.response.starred2.unwrap();
        assert_eq!(starred.song.len(), 1);
        assert_eq!(starred.song[0].id, "1");
        assert_eq!(starred.song[0].album_id.as_deref(), Some("al1"));
    }

    #[test]
    fn deserialize_song_camel_case() {
        let j = r#"{"id":"42","title":"Track","albumId":"al1","discNumber":1,"track":3}"#;
        let s: Song = serde_json::from_str(j).unwrap();
        assert_eq!(s.id, "42");
        assert_eq!(s.title, "Track");
        assert_eq!(s.album_id.as_deref(), Some("al1"));
        assert_eq!(s.disc_number, Some(1));
        assert_eq!(s.track, Some(3));
    }

    #[test]
    fn deserialize_playlist_detail_entry_songs() {
        let j = r#"{"id":"p1","name":"Mix","entry":[{"id":"1","title":"A"}]}"#;
        let d: PlaylistDetail = serde_json::from_str(j).unwrap();
        assert_eq!(d.songs.len(), 1);
        assert_eq!(d.songs[0].title, "A");
    }

    #[test]
    fn deserialize_music_directory_splits_dirs_and_songs() {
        let j = r#"{
            "id": "1",
            "name": "music",
            "child": [
                {"id": "2", "parent": "1", "isDir": true, "title": "VA"},
                {"id": "9", "parent": "1", "isDir": false, "title": "Track One", "duration": 200}
            ]
        }"#;
        let d: DirectoryRaw = serde_json::from_str(j).unwrap();
        assert_eq!(d.child.len(), 2);
        assert!(d.child[0].is_dir);
        assert!(!d.child[1].is_dir);
    }

    #[test]
    fn deserialize_music_folders_single_object() {
        let j = r#"{"musicFolder":{"id":0,"name":"music"}}"#;
        let c: MusicFoldersContainer = serde_json::from_str(j).unwrap();
        assert_eq!(c.music_folder.len(), 1);
        assert_eq!(c.music_folder[0].id.as_str(), "0");
        assert_eq!(c.music_folder[0].name, "music");
    }

    #[test]
    fn deserialize_indexes_single_bucket_and_single_artist() {
        let j = r#"{"ignoredArticles":"","index":{"name":"A","artist":{"id":"1","name":"Solo"}}}"#;
        let a: Artists = serde_json::from_str(j).unwrap();
        assert_eq!(a.index.len(), 1);
        assert_eq!(a.index[0].artist.len(), 1);
        assert_eq!(a.index[0].artist[0].id, "1");
    }

    #[test]
    fn music_library_root_cache_key_roundtrip() {
        let k = music_library_root_cache_key("4");
        assert_eq!(parse_music_library_root_folder_id(&k), Some("4"));
    }

    #[test]
    fn deserialize_indexes_envelope() {
        let j = r#"{"subsonic-response":{"status":"ok","indexes":{"ignoredArticles":"","index":[{"name":"V","artist":[{"id":"abc","name":"VA"}]}]}}}"#;
        let env: IndexesEnvelope = serde_json::from_str(j).unwrap();
        let ix = env.response.indexes.expect("indexes");
        assert_eq!(ix.index.len(), 1);
        assert_eq!(ix.index[0].artist[0].id, "abc");
        assert_eq!(ix.index[0].artist[0].name, "VA");
    }

    #[test]
    fn subsonic_library_roundtrip_debug() {
        let lib = SubsonicLibrary {
            artists: vec![Artist {
                id: "a".into(),
                name: "Artist".into(),
                album_count: None,
                cover_art: None,
                starred: None,
                album: vec![],
            }],
        };
        let _ = format!("{lib:?}");
    }

    #[test]
    fn structured_lyrics_to_lines_prefers_synced_main() {
        let j = r#"[
            {"lang":"eng","synced":false,"line":[{"value":"plain"}]},
            {"kind":"main","lang":"eng","synced":true,"offset":-100,"line":[
                {"start":0,"value":"It's bugging me"},
                {"start":2000,"value":"Grating me"}
            ]}
        ]"#;
        let entries: Vec<StructuredLyricsRaw> = serde_json::from_str(j).unwrap();
        let lines = structured_lyrics_to_lines(&entries);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "It's bugging me");
        assert_eq!(lines[0].time, Some(Duration::from_millis(0)));
        assert_eq!(lines[1].time, Some(Duration::from_millis(1900)));
    }

    #[test]
    fn deserialize_lyrics_by_song_id_envelope() {
        let j = r#"{"subsonic-response":{"status":"ok","lyricsList":{"structuredLyrics":{
            "kind":"main","synced":true,"line":[{"start":0,"value":"Hello"}]
        }}}}"#;
        let env: LyricsBySongIdEnvelope = serde_json::from_str(j).unwrap();
        let list = env.response.lyrics_list.expect("lyricsList");
        let lines = structured_lyrics_to_lines(&list.structured_lyrics);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Hello");
    }

    #[test]
    fn deserialize_legacy_get_lyrics_envelope() {
        let j = r#"{"subsonic-response":{"status":"ok","lyrics":{
            "artist":"Muse","title":"Hysteria","value":"Line one\nLine two"
        }}}"#
            .replace('\n', "");
        let env: LegacyLyricsEnvelope = serde_json::from_str(&j).unwrap();
        let value = env.response.lyrics.expect("lyrics").value.expect("value");
        assert!(value.contains("Line one"));
    }

    #[test]
    fn structured_lyrics_unsynced_lines_have_no_timestamps() {
        let j = r#"[{"synced":false,"line":[{"value":"A"},{"value":"B"}]}]"#;
        let entries: Vec<StructuredLyricsRaw> = serde_json::from_str(j).unwrap();
        let lines = structured_lyrics_to_lines(&entries);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.time.is_none()));
    }
}
