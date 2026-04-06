//! Subsonic REST API client.
//!
//! Implements the [Subsonic API](http://www.subsonic.org/pages/api.jsp) v1.16.1
//! with MD5 token authentication against a Navidrome server.
//!
//! # Authentication
//!
//! Every request appends five standard query parameters:
//!
//! | Param | Value |
//! |-------|-------|
//! | `u`   | username |
//! | `t`   | MD5(password + salt) as lowercase hex |
//! | `s`   | random alphanumeric salt |
//! | `v`   | Subsonic API version (`1.16.1`) |
//! | `c`   | client name (`playterm`) |

use anyhow::{Result, anyhow};
use reqwest::{Client, ClientBuilder};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::error::check_status;
use crate::models::{
    Album, AlbumEnvelope, Artist, ArtistEnvelope, Artists, ArtistsEnvelope,
    PingEnvelope, Playlist, PlaylistDetail, PlaylistEnvelope, PlaylistsEnvelope,
    ScanStatus, ScanStatusEnvelope, SearchEnvelope, SearchResult3, Song, SongEnvelope,
    SubsonicLibrary,
};

// ── Constants ──────────────────────────────────────────────────────────────────

/// Default Navidrome server used when no URL is supplied.
pub const DEFAULT_SERVER_URL: &str = "http://localhost:4533";

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "playterm";

// ── Auth helpers ───────────────────────────────────────────────────────────────

/// Derive a Subsonic token: MD5(password + salt) rendered as lowercase hex.
fn make_token(password: &str, salt: &str) -> String {
    hex::encode(md5::compute(format!("{password}{salt}")).as_ref())
}

/// Generate `len` random lowercase alphanumeric characters for use as a salt.
///
/// Uses a simple LCG seeded from the current system time — sufficient
/// entropy for a per-request Subsonic salt.
fn random_ascii(len: usize) -> String {
    use std::time::SystemTime;
    let charset = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0xdead_beef_cafe_babe);
    let mut x = seed;
    (0..len)
        .map(|i| {
            // Knuth multiplicative hash step
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407 + i as u64);
            charset[(x >> 33) as usize % charset.len()] as char
        })
        .collect()
}

// ── Client ─────────────────────────────────────────────────────────────────────

/// Async Subsonic API client.
///
/// Create one instance and reuse it — the underlying `reqwest::Client` maintains
/// a connection pool.
///
/// ```no_run
/// # use playterm_subsonic::client::{SubsonicClient, DEFAULT_SERVER_URL};
/// # async fn example() -> anyhow::Result<()> {
/// let client = SubsonicClient::new(DEFAULT_SERVER_URL, "admin", "s3cr3t")?;
/// client.ping().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct SubsonicClient {
    base_url: String,
    username: String,
    password: String,
    http: Client,
}

impl SubsonicClient {
    /// Create a new client.
    ///
    /// `base_url` should be the server root, e.g. `"http://localhost:4533"`.
    /// Trailing slashes are stripped automatically.
    pub fn new(base_url: &str, username: &str, password: &str) -> Result<Self> {
        let http = ClientBuilder::new()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            username: username.to_string(),
            password: password.to_string(),
            http,
        })
    }

    // ── Private helpers ────────────────────────────────────────────────────────

    /// Build the standard authentication parameters.
    ///
    /// A fresh random salt — and therefore a fresh token — is generated on
    /// every call so that repeated requests are not replayable.
    fn auth_params(&self) -> Vec<(&'static str, String)> {
        let salt = random_ascii(12);
        let token = make_token(&self.password, &salt);
        vec![
            ("u", self.username.clone()),
            ("t", token),
            ("s", salt),
            ("v", API_VERSION.to_string()),
            ("c", CLIENT_NAME.to_string()),
            ("f", "json".to_string()),
        ]
    }

    fn endpoint_url(&self, name: &str) -> String {
        format!("{}/rest/{name}", self.base_url)
    }

    // ── Public API ─────────────────────────────────────────────────────────────

    /// Library scan progress (`getScanStatus`, Subsonic 1.15+). Navidrome sets
    /// [`ScanStatus::last_scan`] when a scan has finished.
    pub async fn get_scan_status(&self) -> Result<ScanStatus> {
        let env: ScanStatusEnvelope = self
            .http
            .get(self.endpoint_url("getScanStatus"))
            .query(&self.auth_params())
            .send()
            .await?
            .json()
            .await?;
        let r = &env.response;
        check_status(&r.status, r.error.as_ref())?;
        r.scan_status
            .clone()
            .ok_or_else(|| anyhow!("missing 'scanStatus' in getScanStatus response"))
    }

    /// Ping the server to verify connectivity and authentication.
    pub async fn ping(&self) -> Result<()> {
        let env: PingEnvelope = self
            .http
            .get(self.endpoint_url("ping"))
            .query(&self.auth_params())
            .send()
            .await?
            .json()
            .await?;
        check_status(&env.response.status, env.response.error.as_ref())
    }

    /// Fetch all artists, grouped by index letter (`getArtists`).
    pub async fn get_artists(&self) -> Result<Artists> {
        let env: ArtistsEnvelope = self
            .http
            .get(self.endpoint_url("getArtists"))
            .query(&self.auth_params())
            .send()
            .await?
            .json()
            .await?;
        let r = &env.response;
        check_status(&r.status, r.error.as_ref())?;
        r.artists
            .clone()
            .ok_or_else(|| anyhow!("missing 'artists' field in getArtists response"))
    }

    /// Fetch a single artist by ID, including album stubs (`getArtist`).
    pub async fn get_artist(&self, id: &str) -> Result<Artist> {
        let mut params = self.auth_params();
        params.push(("id", id.to_string()));
        let env: ArtistEnvelope = self
            .http
            .get(self.endpoint_url("getArtist"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        let r = &env.response;
        check_status(&r.status, r.error.as_ref())?;
        r.artist
            .clone()
            .ok_or_else(|| anyhow!("missing 'artist' field in getArtist response"))
    }

    /// Fetch a full album including its track list by album ID (`getAlbum`).
    pub async fn get_album(&self, id: &str) -> Result<Album> {
        let mut params = self.auth_params();
        params.push(("id", id.to_string()));
        let env: AlbumEnvelope = self
            .http
            .get(self.endpoint_url("getAlbum"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        let r = &env.response;
        check_status(&r.status, r.error.as_ref())?;
        r.album
            .clone()
            .ok_or_else(|| anyhow!("missing 'album' field in getAlbum response"))
    }

    /// Fetch a single song by its ID (`getSong`).
    pub async fn get_song(&self, id: &str) -> Result<Song> {
        let mut params = self.auth_params();
        params.push(("id", id.to_string()));
        let env: SongEnvelope = self
            .http
            .get(self.endpoint_url("getSong"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        let r = &env.response;
        check_status(&r.status, r.error.as_ref())?;
        r.song
            .clone()
            .ok_or_else(|| anyhow!("missing 'song' field in getSong response"))
    }

    /// Construct a signed streaming URL for a song (`stream`).
    ///
    /// The returned URL is self-contained and can be handed directly to a media
    /// player without any further signing.
    ///
    /// Set `max_bit_rate` to `0` to request the original file without
    /// transcoding.
    #[must_use]
    pub fn stream_url(&self, id: &str, max_bit_rate: u32) -> String {
        let Ok(mut url) = url::Url::parse(&format!("{}/rest/stream", self.base_url)) else {
            return self.stream_url_fallback(id, max_bit_rate);
        };
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in self.auth_params() {
                q.append_pair(k, &v);
            }
            q.append_pair("id", id);
            q.append_pair("maxBitRate", &max_bit_rate.to_string());
        }
        url.into()
    }

    /// Pre-encoding fallback if `base_url` is not a valid URL prefix (should be rare).
    fn stream_url_fallback(&self, id: &str, max_bit_rate: u32) -> String {
        let params = self.auth_params();
        let mut parts: Vec<String> = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        parts.push(format!("id={id}"));
        parts.push(format!("maxBitRate={max_bit_rate}"));
        format!("{}/rest/stream?{}", self.base_url, parts.join("&"))
    }

    /// Search for artists, albums, and songs matching `query` (`search3`).
    pub async fn search3(
        &self,
        query: &str,
        artist_count: u32,
        album_count: u32,
        song_count: u32,
    ) -> Result<SearchResult3> {
        let mut params = self.auth_params();
        params.push(("query", query.to_string()));
        params.push(("artistCount", artist_count.to_string()));
        params.push(("albumCount", album_count.to_string()));
        params.push(("songCount", song_count.to_string()));
        let env: SearchEnvelope = self
            .http
            .get(self.endpoint_url("search3"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        let r = &env.response;
        check_status(&r.status, r.error.as_ref())?;
        r.search_result3
            .clone()
            .ok_or_else(|| anyhow!("missing 'searchResult3' field in search3 response"))
    }

    /// Fetch raw cover art bytes for the given cover art ID (`getCoverArt`).
    ///
    /// Returns the raw image bytes (JPEG, PNG, etc.) as returned by Navidrome.
    /// The `id` is the `cover_art` field on a `Song` or `Album`.
    pub async fn get_cover_art(&self, id: &str) -> Result<Vec<u8>> {
        let mut params = self.auth_params();
        params.push(("id", id.to_string()));
        let response = self
            .http
            .get(self.endpoint_url("getCoverArt"))
            .query(&params)
            .send()
            .await?
            .error_for_status()?;
        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }

    /// Fetch all playlists visible to the authenticated user (`getPlaylists`).
    pub async fn get_playlists(&self) -> Result<Vec<Playlist>> {
        let env: PlaylistsEnvelope = self
            .http
            .get(self.endpoint_url("getPlaylists"))
            .query(&self.auth_params())
            .send()
            .await?
            .json()
            .await?;
        let r = &env.response;
        check_status(&r.status, r.error.as_ref())?;
        Ok(r.playlists
            .as_ref()
            .map(|p| p.playlist.clone())
            .unwrap_or_default())
    }

    /// Fetch a single playlist including its full track list by ID (`getPlaylist`).
    pub async fn get_playlist(&self, id: &str) -> Result<PlaylistDetail> {
        let mut params = self.auth_params();
        params.push(("id", id.to_string()));
        let env: PlaylistEnvelope = self
            .http
            .get(self.endpoint_url("getPlaylist"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        let r = &env.response;
        check_status(&r.status, r.error.as_ref())?;
        r.playlist
            .clone()
            .ok_or_else(|| anyhow!("missing 'playlist' field in getPlaylist response"))
    }

    /// Create a new empty playlist with the given name (`createPlaylist`).
    ///
    /// Returns the created playlist object.  Navidrome nests it under
    /// `subsonic-response > playlist` (same shape as `getPlaylist`).
    pub async fn create_playlist(&self, name: &str) -> Result<Playlist> {
        let mut params = self.auth_params();
        params.push(("name", name.to_string()));
        let env: PlaylistEnvelope = self
            .http
            .post(self.endpoint_url("createPlaylist"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        let r = &env.response;
        check_status(&r.status, r.error.as_ref())?;
        let detail = r
            .playlist
            .clone()
            .ok_or_else(|| anyhow!("missing 'playlist' field in createPlaylist response"))?;
        Ok(Playlist {
            id: detail.id,
            name: detail.name,
            song_count: detail.song_count,
            duration: detail.duration,
            owner: None,
            public: None,
        })
    }

    /// Append a single track to a playlist (`updatePlaylist` + `songIdToAdd`).
    pub async fn add_track_to_playlist(
        &self,
        playlist_id: &str,
        song_id: &str,
    ) -> Result<()> {
        let mut params = self.auth_params();
        params.push(("playlistId", playlist_id.to_string()));
        params.push(("songIdToAdd", song_id.to_string()));
        let env: PingEnvelope = self
            .http
            .get(self.endpoint_url("updatePlaylist"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        check_status(&env.response.status, env.response.error.as_ref())
    }

    /// Remove the track at `index` from a playlist (`updatePlaylist` + `songIndexToRemove`).
    pub async fn remove_track_from_playlist(
        &self,
        playlist_id: &str,
        index: usize,
    ) -> Result<()> {
        let mut params = self.auth_params();
        params.push(("playlistId", playlist_id.to_string()));
        params.push(("songIndexToRemove", index.to_string()));
        let env: PingEnvelope = self
            .http
            .get(self.endpoint_url("updatePlaylist"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        check_status(&env.response.status, env.response.error.as_ref())
    }

    /// Rename a playlist (`updatePlaylist` + `name`).
    pub async fn rename_playlist(&self, playlist_id: &str, new_name: &str) -> Result<()> {
        let mut params = self.auth_params();
        params.push(("playlistId", playlist_id.to_string()));
        params.push(("name", new_name.to_string()));
        let env: PingEnvelope = self
            .http
            .get(self.endpoint_url("updatePlaylist"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        check_status(&env.response.status, env.response.error.as_ref())
    }

    /// Delete a playlist by ID (`deletePlaylist`).
    pub async fn delete_playlist(&self, id: &str) -> Result<()> {
        let mut params = self.auth_params();
        params.push(("id", id.to_string()));
        let env: PingEnvelope = self
            .http
            .get(self.endpoint_url("deletePlaylist"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        check_status(&env.response.status, env.response.error.as_ref())
    }

    /// Mark a song as played (scrobble).
    pub async fn scrobble(&self, id: &str) -> Result<()> {
        let mut params = self.auth_params();
        params.push(("id", id.to_string()));
        params.push(("submission", "true".to_string()));
        let env: PingEnvelope = self
            .http
            .get(self.endpoint_url("scrobble"))
            .query(&params)
            .send()
            .await?
            .json()
            .await?;
        check_status(&env.response.status, env.response.error.as_ref())
    }
}

// ── Library helpers ────────────────────────────────────────────────────────────

/// Fetch the top-level artist list. One network request.
pub async fn fetch_library(client: &SubsonicClient) -> Result<SubsonicLibrary> {
    let artists_response = client.get_artists().await?;
    let mut artists: Vec<Artist> = artists_response
        .index
        .into_iter()
        .flat_map(|bucket| bucket.artist)
        .collect();
    artists.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(SubsonicLibrary { artists })
}

/// Many servers (including Navidrome) omit `artist` on each `Song` in `getAlbum`
/// even when the album has `artist` set. Fill from album, then the library artist
/// name, so UIs and indexes (e.g. fzf) can search by performer name.
fn apply_album_artist_fallback(song: &mut Song, album_artist: Option<&str>, library_artist_name: &str) {
    let track_has_artist = song
        .artist
        .as_deref()
        .map(|a| !a.trim().is_empty())
        .unwrap_or(false);
    if track_has_artist {
        return;
    }
    if let Some(a) = album_artist {
        if !a.trim().is_empty() {
            song.artist = Some(a.to_string());
            return;
        }
    }
    if !library_artist_name.trim().is_empty() {
        song.artist = Some(library_artist_name.to_string());
    }
}

/// Concurrency for full-library metadata walks ([`fetch_all_library_songs_with_options`]).
#[derive(Debug, Clone, Copy)]
pub struct FetchLibraryOptions {
    /// Concurrent `getAlbum` requests per artist (minimum 1).
    pub album_parallelism: usize,
    /// How many artists to walk concurrently (minimum 1).
    pub artist_parallelism: usize,
}

impl Default for FetchLibraryOptions {
    fn default() -> Self {
        Self {
            album_parallelism: 12,
            artist_parallelism: 4,
        }
    }
}

async fn fetch_songs_for_artist_inner(
    client: &SubsonicClient,
    artist: &Artist,
    album_parallelism: usize,
) -> Vec<Song> {
    let artist_detail = match client.get_artist(&artist.id).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("playterm-subsonic: get_artist({}) failed — {e}", artist.id);
            return Vec::new();
        }
    };

    let library_name = artist_detail.name.clone();
    let library_name_ref = library_name.as_str();
    let limit = album_parallelism.max(1);
    let sem = Arc::new(Semaphore::new(limit));
    let mut set = JoinSet::new();

    for album_stub in artist_detail.album {
        let client = client.clone();
        let sem = sem.clone();
        let aid = album_stub.id.clone();
        set.spawn(async move {
            let _permit = match sem.acquire().await {
                Ok(p) => p,
                Err(_) => return (aid, Err(anyhow!("semaphore closed"))),
            };
            (aid, client.get_album(&album_stub.id).await)
        });
    }

    let mut songs: Vec<Song> = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((_album_id, Ok(album))) => {
                let album_artist_owned = album.artist.clone();
                let album_artist = album_artist_owned.as_deref();
                for mut s in album.song {
                    apply_album_artist_fallback(&mut s, album_artist, library_name_ref);
                    songs.push(s);
                }
            }
            Ok((album_id, Err(e))) => {
                eprintln!("playterm-subsonic: get_album({}) failed — {e}", album_id);
            }
            Err(e) => eprintln!("playterm-subsonic: album task join — {e}"),
        }
    }

    songs.sort_by_key(|s| (s.disc_number.unwrap_or(1), s.track.unwrap_or(0)));
    songs
}

/// Fetch all songs for a single artist: `getArtist`, then `getAlbum` for each album.
///
/// Uses the same default album concurrency as [`FetchLibraryOptions::default`].
/// Returns a flat, disc+track-number-sorted `Vec<Song>` across all albums.
pub async fn fetch_songs_for_artist(client: &SubsonicClient, artist: &Artist) -> Vec<Song> {
    fetch_songs_for_artist_inner(client, artist, FetchLibraryOptions::default().album_parallelism).await
}

/// Fetch metadata for every track in the library: `getArtists`, then for each
/// artist `getArtist` + parallel `getAlbum` (see [`FetchLibraryOptions`]).
///
/// Deduplicates by song ID and sorts by artist name, album id, disc, track.
pub async fn fetch_all_library_songs_with_options(
    client: &SubsonicClient,
    opts: FetchLibraryOptions,
) -> Result<Vec<Song>> {
    let lib = fetch_library(client).await?;
    let artist_limit = opts.artist_parallelism.max(1);
    let album_p = opts.album_parallelism.max(1);
    let sem = Arc::new(Semaphore::new(artist_limit));
    let mut set = JoinSet::new();

    for artist in lib.artists {
        let client = client.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem
                .acquire()
                .await
                .expect("library index artist semaphore");
            fetch_songs_for_artist_inner(&client, &artist, album_p).await
        });
    }

    let mut by_id: HashMap<String, Song> = HashMap::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(song_vecs) => {
                for s in song_vecs {
                    by_id.insert(s.id.clone(), s);
                }
            }
            Err(e) => eprintln!("playterm-subsonic: artist task join — {e}"),
        }
    }

    let mut tracks: Vec<Song> = by_id.into_values().collect();
    tracks.sort_by(|a, b| {
        let an = a.artist.as_deref().unwrap_or("");
        let bn = b.artist.as_deref().unwrap_or("");
        an.cmp(bn)
            .then_with(|| a.album_id.cmp(&b.album_id))
            .then_with(|| a.disc_number.unwrap_or(1).cmp(&b.disc_number.unwrap_or(1)))
            .then_with(|| a.track.unwrap_or(0).cmp(&b.track.unwrap_or(0)))
    });
    Ok(tracks)
}

/// Like [`fetch_all_library_songs_with_options`] with [`FetchLibraryOptions::default`].
pub async fn fetch_all_library_songs(client: &SubsonicClient) -> Result<Vec<Song>> {
    fetch_all_library_songs_with_options(client, FetchLibraryOptions::default()).await
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn song_with_artist(artist: Option<String>) -> Song {
        Song {
            id: "1".into(),
            title: "T".into(),
            album: Some("Alb".into()),
            artist,
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
        }
    }

    #[test]
    fn album_artist_fallback_fills_empty_track_artist() {
        let mut s = song_with_artist(None);
        apply_album_artist_fallback(&mut s, Some("Album Artist"), "Library Artist");
        assert_eq!(s.artist.as_deref(), Some("Album Artist"));
    }

    #[test]
    fn album_artist_fallback_uses_library_when_album_empty() {
        let mut s = song_with_artist(None);
        apply_album_artist_fallback(&mut s, None, "Library Artist");
        assert_eq!(s.artist.as_deref(), Some("Library Artist"));
    }

    #[test]
    fn album_artist_fallback_keeps_track_artist() {
        let mut s = song_with_artist(Some("Feat".into()));
        apply_album_artist_fallback(&mut s, Some("Album Artist"), "Library Artist");
        assert_eq!(s.artist.as_deref(), Some("Feat"));
    }

    /// Build a test client from environment variables, falling back to the
    /// hard-coded Navidrome instance.
    ///
    /// Override at runtime:
    /// ```sh
    /// SUBSONIC_URL=http://... SUBSONIC_USER=alice SUBSONIC_PASS=s3cr3t \
    ///   cargo test -p playterm-subsonic -- --nocapture
    /// ```
    fn test_client() -> SubsonicClient {
        let url  = std::env::var("SUBSONIC_URL").expect("set SUBSONIC_URL to run integration tests");
        let user = std::env::var("SUBSONIC_USER").expect("set SUBSONIC_USER to run integration tests");
        let pass = std::env::var("SUBSONIC_PASS").expect("set SUBSONIC_PASS to run integration tests");
        SubsonicClient::new(&url, &user, &pass).expect("client construction must not fail")
    }

    /// Live integration test — pings the Navidrome instance to verify that
    /// MD5 token auth is wired up correctly.
    #[tokio::test]
    async fn ping_live_navidrome() {
        let client = test_client();
        client
            .ping()
            .await
            .expect("ping must succeed against live Navidrome — check credentials and connectivity");
        println!("ping OK");
    }
}
