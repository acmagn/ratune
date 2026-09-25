//! Lyrics fetcher. LRCLib, NetEase, or Subsonic, selected in `[lyrics].source`.
//!
//! All errors are soft-failed. Callers always receive a `Vec`, possibly empty.

use std::future::Future;
use std::time::Duration;

use ratune_subsonic::{LyricLine, SubsonicClient};
use reqwest::header::{REFERER, USER_AGENT};
use serde::Deserialize;

const NETEASE_SEARCH_URL: &str = "https://music.163.com/api/search/get";
const NETEASE_LYRIC_URL: &str = "https://music.163.com/api/song/lyric";
const NETEASE_REFERER: &str = "https://music.163.com/";
const NETEASE_USER_AGENT: &str = concat!("ratune/", env!("CARGO_PKG_VERSION"));
type LyricsError = Box<dyn std::error::Error + Send + Sync>;

use crate::config::LyricsSource;
use crate::lyrics_cache::LyricsDiskCache;

const PROVIDER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibResponse {
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

#[derive(Deserialize)]
struct NeteaseSearchResponse {
    code: u16,
    result: Option<NeteaseSearchResult>,
}

#[derive(Deserialize)]
struct NeteaseSearchResult {
    #[serde(default)]
    songs: Vec<NeteaseSong>,
}

#[derive(Deserialize)]
struct NeteaseSong {
    id: u64,
    name: String,
    duration: Option<u64>,
}

#[derive(Deserialize)]
struct NeteaseLyricsResponse {
    code: u16,
    lrc: Option<NeteaseLyrics>,
}

#[derive(Deserialize)]
struct NeteaseLyrics {
    lyric: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct LyricsTrack<'a> {
    pub song_id: &'a str,
    pub artist: &'a str,
    pub title: &'a str,
    pub album: &'a str,
    /// NetEase uses duration to disambiguate different versions of the same track.
    pub duration_secs: Option<u32>,
}

/// Fetch lyrics from the configured providers in priority order.
pub async fn fetch_lyrics(
    sources: &[LyricsSource],
    lrclib_url: &str,
    client: &SubsonicClient,
    disk_cache: &LyricsDiskCache,
    cache_enabled: bool,
    network_available: bool,
    track: LyricsTrack<'_>,
) -> Vec<LyricLine> {
    // Check every provider's cache first so a later hit isn't blocked by an
    // earlier provider that would need a network round-trip.
    if cache_enabled {
        for &source in sources {
            let source_key = source.cache_dir_name();
            if let Some(lines) = disk_cache.get(source_key, track.song_id) {
                if !lines.is_empty() {
                    crate::debug::log(format!(
                        "lyrics[{}]: {source_key} cache hit ({} lines)",
                        track.song_id,
                        lines.len()
                    ));
                    return lines;
                }
            }
        }
    }

    first_available(
        sources,
        PROVIDER_TIMEOUT,
        track.song_id,
        |source| async move {
            let source_key = source.cache_dir_name();

            if !network_available {
                crate::debug::log(format!(
                    "lyrics[{}]: {source_key} network fetch skipped (offline)",
                    track.song_id
                ));
                return Ok::<_, LyricsError>(Vec::new());
            }

            crate::debug::log(format!(
                "lyrics[{}]: fetching from {source_key}",
                track.song_id
            ));
            let lines = fetch_lyrics_from_source(source, lrclib_url, client, track).await?;
            if cache_enabled && !lines.is_empty() {
                disk_cache.put(source_key, track.song_id, &lines);
                crate::debug::log(format!(
                    "lyrics[{}]: cached {} {source_key} lines",
                    track.song_id,
                    lines.len()
                ));
            }
            Ok(lines)
        },
    )
    .await
}

async fn first_available<F, Fut, E>(
    sources: &[LyricsSource],
    timeout: Duration,
    song_id: &str,
    mut attempt: F,
) -> Vec<LyricLine>
where
    F: FnMut(LyricsSource) -> Fut,
    Fut: Future<Output = Result<Vec<LyricLine>, E>>,
{
    for &source in sources {
        let source_key = source.cache_dir_name();
        crate::debug::log(format!("lyrics[{song_id}]: trying {source_key}"));
        match tokio::time::timeout(timeout, attempt(source)).await {
            Ok(Ok(lines)) if !lines.is_empty() => {
                crate::debug::log(format!(
                    "lyrics[{song_id}]: selected {source_key} ({} lines)",
                    lines.len()
                ));
                return lines;
            }
            Ok(Ok(_)) => crate::debug::log(format!(
                "lyrics[{song_id}]: {source_key} returned no lyrics; falling back"
            )),
            Ok(Err(_)) => crate::debug::log(format!(
                "lyrics[{song_id}]: {source_key} request failed; falling back"
            )),
            Err(_) => crate::debug::log(format!(
                "lyrics[{song_id}]: {source_key} timed out after {:.1}s; falling back",
                timeout.as_secs_f64()
            )),
        }
    }
    crate::debug::log(format!(
        "lyrics[{song_id}]: provider chain exhausted; no lyrics available"
    ));
    Vec::new()
}

async fn fetch_lyrics_from_source(
    source: LyricsSource,
    lrclib_url: &str,
    client: &SubsonicClient,
    track: LyricsTrack<'_>,
) -> Result<Vec<LyricLine>, LyricsError> {
    match source {
        LyricsSource::LrcLib => {
            fetch_lrclib(lrclib_url, track.artist, track.title, track.album).await
        }
        LyricsSource::Netease => {
            fetch_netease(track.artist, track.title, track.duration_secs).await
        }
        LyricsSource::Subsonic => {
            fetch_subsonic(client, track.song_id, track.artist, track.title).await
        }
    }
}

/// Build the LRCLib `get` endpoint from a configured base URL.
fn lrclib_api_url(base_url: &str) -> String {
    format!("{}/api/get", base_url.trim_end_matches('/'))
}

/// Convert an LRCLib API JSON body into display lines.
fn lines_from_lrclib_body(body: &LrcLibResponse) -> Vec<LyricLine> {
    if let Some(lrc) = body.synced_lyrics.as_deref().filter(|s| !s.is_empty()) {
        return parse_lrc(lrc);
    }
    if let Some(plain) = body.plain_lyrics.as_deref().filter(|s| !s.is_empty()) {
        return parse_lyrics_text(plain);
    }
    vec![]
}

async fn fetch_lrclib(
    base_url: &str,
    artist: &str,
    title: &str,
    album: &str,
) -> Result<Vec<LyricLine>, Box<dyn std::error::Error + Send + Sync>> {
    let endpoint = lrclib_api_url(base_url);

    let resp = reqwest::Client::new()
        .get(&endpoint)
        .query(&[
            ("artist_name", artist),
            ("track_name", title),
            ("album_name", album),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        return Ok(vec![]);
    }

    let body: LrcLibResponse = resp.json().await?;
    Ok(lines_from_lrclib_body(&body))
}

async fn fetch_netease(
    artist: &str,
    title: &str,
    duration_secs: Option<u32>,
) -> Result<Vec<LyricLine>, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let query = if artist.trim().is_empty() {
        title.to_string()
    } else {
        format!("{title} {artist}")
    };
    let response = client
        .get(NETEASE_SEARCH_URL)
        .header(USER_AGENT, NETEASE_USER_AGENT)
        .header(REFERER, NETEASE_REFERER)
        .query(&[
            ("s", query.as_str()),
            ("type", "1"),
            ("offset", "0"),
            ("total", "true"),
            ("limit", "10"),
        ])
        .send()
        .await?;

    if !response.status().is_success() {
        return Ok(vec![]);
    }

    let body: NeteaseSearchResponse = response.json().await?;
    if body.code != 200 {
        return Ok(vec![]);
    }
    let songs = body.result.map(|result| result.songs).unwrap_or_default();
    let Some(song) = select_netease_song(&songs, title, duration_secs) else {
        return Ok(vec![]);
    };

    let response = client
        .get(NETEASE_LYRIC_URL)
        .header(USER_AGENT, NETEASE_USER_AGENT)
        .header(REFERER, NETEASE_REFERER)
        .query(&[
            ("id", song.id.to_string()),
            ("lv", "-1".to_string()),
            ("kv", "-1".to_string()),
            ("tv", "-1".to_string()),
        ])
        .send()
        .await?;

    if !response.status().is_success() {
        return Ok(vec![]);
    }

    let body: NeteaseLyricsResponse = response.json().await?;
    if body.code != 200 {
        return Ok(vec![]);
    }
    Ok(lines_from_netease_body(&body))
}

fn select_netease_song<'a>(
    songs: &'a [NeteaseSong],
    title: &str,
    duration_secs: Option<u32>,
) -> Option<&'a NeteaseSong> {
    let duration_ms = duration_secs.map(|seconds| u64::from(seconds) * 1000);
    songs
        .iter()
        .find(|song| {
            netease_titles_match(&song.name, title)
                && duration_ms
                    .zip(song.duration)
                    .is_some_and(|(expected, actual)| expected.abs_diff(actual) <= 2_000)
        })
        .or_else(|| {
            songs
                .iter()
                .find(|song| netease_titles_match(&song.name, title))
        })
}

fn netease_titles_match(candidate: &str, title: &str) -> bool {
    let candidate = candidate.trim().to_lowercase();
    let title = title.trim().to_lowercase();
    !candidate.is_empty()
        && !title.is_empty()
        && (candidate.contains(&title) || title.contains(&candidate))
}

fn lines_from_netease_body(body: &NeteaseLyricsResponse) -> Vec<LyricLine> {
    body.lrc
        .as_ref()
        .and_then(|lrc| lrc.lyric.as_deref())
        .filter(|lyric| !lyric.trim().is_empty() && lyric.trim() != "暂无歌词")
        .map(parse_lyrics_text)
        .unwrap_or_default()
}

async fn fetch_subsonic(
    client: &SubsonicClient,
    song_id: &str,
    artist: &str,
    title: &str,
) -> Result<Vec<LyricLine>, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(lines) = client.get_lyrics_by_song_id(song_id).await {
        if !lines.is_empty() {
            return Ok(lines);
        }
    }

    if let Some(text) = client.get_lyrics(artist, title).await? {
        return Ok(parse_lyrics_text(&text));
    }

    Ok(vec![])
}

/// Parse plain or LRC-formatted lyrics text into display lines.
fn parse_lyrics_text(text: &str) -> Vec<LyricLine> {
    if text
        .lines()
        .any(|l| l.trim_start().starts_with('[') && l.contains(']'))
    {
        let synced = parse_lrc(text);
        if !synced.is_empty() {
            return synced;
        }
    }
    text.lines()
        .map(|l| LyricLine {
            time: None,
            text: l.to_string(),
        })
        .collect()
}

/// Parse LRC-format text into timestamped `LyricLine`s.
fn parse_lrc(lrc: &str) -> Vec<LyricLine> {
    lrc.lines().filter_map(parse_lrc_line).collect()
}

fn parse_lrc_line(line: &str) -> Option<LyricLine> {
    let line = line.trim();
    if !line.starts_with('[') {
        return None;
    }
    let close = line.find(']')?;
    let tag = &line[1..close];
    let text = line[close + 1..].trim().to_string();

    let colon = tag.find(':')?;
    let dot = tag.find('.')?;
    if dot <= colon {
        return None;
    }

    let mins: u64 = tag[..colon].parse().ok()?;
    let secs: u64 = tag[colon + 1..dot].parse().ok()?;
    let fraction = &tag[dot + 1..];
    if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let fraction_ms = match fraction.len() {
        1 => fraction.parse::<u64>().ok()? * 100,
        2 => fraction.parse::<u64>().ok()? * 10,
        _ => fraction[..3].parse().ok()?,
    };

    let ms = (mins * 60 + secs) * 1000 + fraction_ms;
    Some(LyricLine {
        time: Some(Duration::from_millis(ms)),
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrclib_api_url_trims_trailing_slash() {
        assert_eq!(
            lrclib_api_url("https://lrclib.net/"),
            "https://lrclib.net/api/get"
        );
        assert_eq!(
            lrclib_api_url("https://example.com"),
            "https://example.com/api/get"
        );
    }

    #[test]
    fn lines_from_lrclib_body_prefers_synced_over_plain() {
        let body = LrcLibResponse {
            synced_lyrics: Some("[00:01.00] Synced line".into()),
            plain_lyrics: Some("Plain only\nSecond line".into()),
        };
        let lines = lines_from_lrclib_body(&body);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Synced line");
        assert_eq!(lines[0].time, Some(Duration::from_millis(1000)));
    }

    #[test]
    fn lines_from_lrclib_body_falls_back_to_plain() {
        let body = LrcLibResponse {
            synced_lyrics: None,
            plain_lyrics: Some("Line one\nLine two".into()),
        };
        let lines = lines_from_lrclib_body(&body);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.time.is_none()));
    }

    #[test]
    fn lines_from_lrclib_body_empty_when_missing() {
        let body = LrcLibResponse {
            synced_lyrics: None,
            plain_lyrics: None,
        };
        assert!(lines_from_lrclib_body(&body).is_empty());
    }

    #[test]
    fn netease_search_response_deserializes() {
        let body: NeteaseSearchResponse = serde_json::from_str(
            r#"{"code":200,"result":{"songs":[{"id":42,"name":"Test Song","duration":183000}]}}"#,
        )
        .expect("netease search response");
        let songs = body.result.expect("result").songs;
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].id, 42);
        assert_eq!(songs[0].duration, Some(183_000));
    }

    #[test]
    fn netease_match_prefers_title_and_duration() {
        let songs = vec![
            NeteaseSong {
                id: 1,
                name: "Test Song".into(),
                duration: Some(240_000),
            },
            NeteaseSong {
                id: 2,
                name: "Test Song (Album Version)".into(),
                duration: Some(183_500),
            },
        ];
        let matched = select_netease_song(&songs, "test song", Some(183)).expect("match");
        assert_eq!(matched.id, 2);
    }

    #[test]
    fn netease_match_falls_back_to_title() {
        let songs = vec![
            NeteaseSong {
                id: 1,
                name: "Different Song".into(),
                duration: Some(180_000),
            },
            NeteaseSong {
                id: 2,
                name: "Test Song".into(),
                duration: Some(300_000),
            },
        ];
        assert_eq!(
            select_netease_song(&songs, "Test Song", Some(180))
                .expect("title match")
                .id,
            2
        );
        assert!(select_netease_song(&songs, "Unknown", Some(180)).is_none());
    }

    #[test]
    fn lines_from_netease_body_parses_lrc() {
        let body = NeteaseLyricsResponse {
            code: 200,
            lrc: Some(NeteaseLyrics {
                lyric: Some("[00:01.50] First\n[00:03.00] Second".into()),
            }),
        };
        let lines = lines_from_netease_body(&body);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "First");
        assert_eq!(lines[0].time, Some(Duration::from_millis(1500)));
    }

    #[test]
    fn lines_from_netease_body_ignores_placeholder() {
        let body = NeteaseLyricsResponse {
            code: 200,
            lrc: Some(NeteaseLyrics {
                lyric: Some("暂无歌词".into()),
            }),
        };
        assert!(lines_from_netease_body(&body).is_empty());
    }

    #[test]
    fn parse_lrc_timestamps() {
        let lrc = "[00:01.50] Hello\n[00:03.684] World";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Hello");
        assert_eq!(lines[0].time, Some(Duration::from_millis(1500)));
        assert_eq!(lines[1].time, Some(Duration::from_millis(3684)));
    }

    #[test]
    fn parse_plain_text_lines() {
        let text = "Line one\nLine two";
        let lines = parse_lyrics_text(text);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.time.is_none()));
    }

    #[test]
    fn parse_lyrics_text_detects_lrc_in_plain_field() {
        let text = "[00:02.00] First\n[00:04.00] Second";
        let lines = parse_lyrics_text(text);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time, Some(Duration::from_millis(2000)));
    }

    #[tokio::test]
    async fn ordered_sources_stop_at_first_non_empty_result() {
        use std::sync::{Arc, Mutex};

        let attempts = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&attempts);
        let lines = first_available(
            &[
                LyricsSource::LrcLib,
                LyricsSource::Subsonic,
                LyricsSource::Netease,
            ],
            Duration::from_secs(1),
            "test-song",
            move |source| {
                let seen = Arc::clone(&seen);
                async move {
                    seen.lock().expect("attempts").push(source);
                    let lines = match source {
                        LyricsSource::LrcLib => Vec::new(),
                        LyricsSource::Subsonic => vec![LyricLine {
                            time: None,
                            text: "server lyrics".into(),
                        }],
                        LyricsSource::Netease => panic!("fallback should have stopped"),
                    };
                    Ok::<_, &'static str>(lines)
                }
            },
        )
        .await;

        assert_eq!(lines[0].text, "server lyrics");
        assert_eq!(
            *attempts.lock().expect("attempts"),
            vec![LyricsSource::LrcLib, LyricsSource::Subsonic]
        );
    }

    #[tokio::test]
    async fn ordered_sources_fall_back_after_errors_and_timeouts() {
        use std::sync::{Arc, Mutex};

        let attempts = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&attempts);
        let lines = first_available(
            &[
                LyricsSource::LrcLib,
                LyricsSource::Subsonic,
                LyricsSource::Netease,
            ],
            Duration::from_millis(10),
            "test-song",
            move |source| {
                let seen = Arc::clone(&seen);
                async move {
                    seen.lock().expect("attempts").push(source);
                    match source {
                        LyricsSource::LrcLib => Err("test provider error"),
                        LyricsSource::Subsonic => {
                            std::future::pending::<()>().await;
                            Ok(Vec::new())
                        }
                        LyricsSource::Netease => Ok(vec![LyricLine {
                            time: None,
                            text: "fallback lyrics".into(),
                        }]),
                    }
                }
            },
        )
        .await;

        assert_eq!(lines[0].text, "fallback lyrics");
        assert_eq!(
            *attempts.lock().expect("attempts"),
            vec![
                LyricsSource::LrcLib,
                LyricsSource::Subsonic,
                LyricsSource::Netease
            ]
        );
    }

    #[tokio::test]
    async fn offline_sources_use_first_positive_cache_entry() {
        let dir = std::env::temp_dir().join(format!(
            "ratune-lyrics-ordered-cache-{}",
            std::process::id()
        ));
        let cache = LyricsDiskCache::from_dir(dir.clone());
        let empty_dir = cache.cache_dir_for("lrclib");
        LyricsDiskCache::put_at(&empty_dir, "song-1", &[]);
        cache.put(
            "subsonic",
            "song-1",
            &[LyricLine {
                time: None,
                text: "cached server lyrics".into(),
            }],
        );
        cache.put(
            "netease",
            "song-1",
            &[LyricLine {
                time: None,
                text: "later cached lyrics".into(),
            }],
        );
        let client =
            SubsonicClient::new("http://localhost:4533", "user", "pass").expect("subsonic client");

        let lines = fetch_lyrics(
            &[
                LyricsSource::LrcLib,
                LyricsSource::Subsonic,
                LyricsSource::Netease,
            ],
            "https://lrclib.net",
            &client,
            &cache,
            true,
            false,
            LyricsTrack {
                song_id: "song-1",
                artist: "Artist",
                title: "Title",
                album: "Album",
                duration_secs: Some(180),
            },
        )
        .await;

        assert_eq!(lines[0].text, "cached server lyrics");
        let _ = std::fs::remove_dir_all(dir);
    }
}
