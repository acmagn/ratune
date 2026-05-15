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
#[derive(Debug, Clone)]
pub struct LyricLine {
    /// Playback offset at which to highlight this line; `None` = unsynced.
    pub time: Option<Duration>,
    /// The lyric text.
    pub text: String,
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
}
