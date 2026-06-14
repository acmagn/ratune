//! Lyrics fetcher — LRCLib or Subsonic, selected in `[lyrics].source`.
//!
//! All errors are soft-failed — callers always receive a `Vec`, possibly empty.

use std::time::Duration;

use ratune_subsonic::{LyricLine, SubsonicClient};
use serde::Deserialize;

use crate::config::LyricsSource;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibResponse {
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

/// Fetch lyrics using the configured source.
pub async fn fetch_lyrics(
    source: LyricsSource,
    lrclib_url: &str,
    client: &SubsonicClient,
    song_id: &str,
    artist: &str,
    title: &str,
    album: &str,
) -> Vec<LyricLine> {
    match source {
        LyricsSource::LrcLib => fetch_lrclib(lrclib_url, artist, title, album)
            .await
            .unwrap_or_default(),
        LyricsSource::Subsonic => fetch_subsonic(client, song_id, artist, title)
            .await
            .unwrap_or_default(),
    }
}

async fn fetch_lrclib(
    base_url: &str,
    artist: &str,
    title: &str,
    album: &str,
) -> Result<Vec<LyricLine>, Box<dyn std::error::Error + Send + Sync>> {
    let base = base_url.trim_end_matches('/');
    let endpoint = format!("{base}/api/get");

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

    if let Some(lrc) = body.synced_lyrics.filter(|s| !s.is_empty()) {
        return Ok(parse_lrc(&lrc));
    }

    if let Some(plain) = body.plain_lyrics.filter(|s| !s.is_empty()) {
        return Ok(parse_lyrics_text(&plain));
    }

    Ok(vec![])
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
    if text.lines().any(|l| l.trim_start().starts_with('[') && l.contains(']')) {
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
    let cs: u64 = tag[dot + 1..].parse().ok()?;

    let ms = (mins * 60 + secs) * 1000 + cs * 10;
    Some(LyricLine {
        time: Some(Duration::from_millis(ms)),
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lrc_timestamps() {
        let lrc = "[00:01.50] Hello\n[00:03.00] World";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Hello");
        assert_eq!(lines[0].time, Some(Duration::from_millis(1500)));
    }

    #[test]
    fn parse_plain_text_lines() {
        let text = "Line one\nLine two";
        let lines = parse_lyrics_text(text);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.time.is_none()));
    }
}
