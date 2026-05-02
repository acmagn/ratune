//! LRCLib lyrics fetcher.
//!
//! Fetches synced or plain lyrics from the public LRCLib API (no auth required).
//! All errors are soft-failed — callers always receive a `Vec`, possibly empty.

use std::time::Duration;

use ratune_subsonic::LyricLine;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibResponse {
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

/// Fetch lyrics for a track from LRCLib.
///
/// Returns synced `LyricLine`s (with `time: Some`) when LRC data is available,
/// plain lines (with `time: None`) when only plain text exists, or an empty
/// `Vec` when the track is not found or any error occurs.
pub async fn fetch_lyrics(artist: &str, title: &str, album: &str) -> Vec<LyricLine> {
    fetch_inner(artist, title, album).await.unwrap_or_default()
}

async fn fetch_inner(
    artist: &str,
    title: &str,
    album: &str,
) -> Result<Vec<LyricLine>, Box<dyn std::error::Error + Send + Sync>> {
    let resp = reqwest::Client::new()
        .get("https://lrclib.net/api/get")
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

    // Prefer synced LRC lyrics.
    if let Some(lrc) = body.synced_lyrics.filter(|s| !s.is_empty()) {
        return Ok(parse_lrc(&lrc));
    }

    // Fall back to plain text.
    if let Some(plain) = body.plain_lyrics.filter(|s| !s.is_empty()) {
        return Ok(plain
            .lines()
            .map(|l| LyricLine {
                time: None,
                text: l.to_string(),
            })
            .collect());
    }

    Ok(vec![])
}

/// Parse LRC-format text into timestamped `LyricLine`s.
///
/// Expected line format: `[MM:SS.xx] lyric text`
/// Lines that don't match are skipped.
fn parse_lrc(lrc: &str) -> Vec<LyricLine> {
    lrc.lines().filter_map(parse_lrc_line).collect()
}

fn parse_lrc_line(line: &str) -> Option<LyricLine> {
    // Expect `[MM:SS.xx] text` — find the enclosing brackets first.
    let line = line.trim();
    if !line.starts_with('[') {
        return None;
    }
    let close = line.find(']')?;
    let tag = &line[1..close];
    let text = line[close + 1..].trim().to_string();

    // Parse `MM:SS.xx`
    let colon = tag.find(':')?;
    let dot = tag.find('.')?;
    if dot <= colon {
        return None;
    }

    let mins: u64 = tag[..colon].parse().ok()?;
    let secs: u64 = tag[colon + 1..dot].parse().ok()?;
    let cs: u64 = tag[dot + 1..].parse().ok()?; // centiseconds

    let ms = (mins * 60 + secs) * 1000 + cs * 10;
    Some(LyricLine {
        time: Some(Duration::from_millis(ms)),
        text,
    })
}
