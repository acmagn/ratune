//! Streaming HTTP → growing-buffer reader for the audio pipeline.
//!
//! `open_stream` starts a background thread that downloads `url` via
//! `reqwest::blocking`, appending chunks to a shared `Vec<u8>`.  The
//! `StreamingReader` returned immediately exposes that buffer as a `Read +
//! Seek` handle that blocks only when the read position overtakes the download.
//!
//! Keeping all bytes (never freeing) means backward seeks always succeed
//! without re-fetching.  Trade-off: memory grows with the file (~14–30 MB for
//! a typical song), which is acceptable for a music player.
//!
//! Playback starts as soon as enough bytes have been buffered (see constants).

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CONTENT_TYPE};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Minimum bytes buffered before [`open_stream`] returns (library-sized payloads).
const PREBUFFER_BYTES: usize = 256 * 1024;

/// Smaller prebuffer for unbounded live radio streams (MP3/OGG).
const LIVE_PREBUFFER_BYTES: usize = 16 * 1024;

/// AAC/Icecast streams often ship kilobytes of headers before the first ADTS frame.
const LIVE_AAC_PREBUFFER_BYTES: usize = 64 * 1024;

/// Hard cap while waiting for the first ADTS sync word.
const LIVE_AAC_PREBUFFER_MAX: usize = 512 * 1024;

/// Minimum bytes to accept when the download ends before reaching prebuffer.
const MIN_LIVE_BYTES: usize = 2 * 1024;

/// Max time to wait for live prebuffer before failing.
const LIVE_PREBUFFER_WAIT: Duration = Duration::from_secs(20);

/// Longer wait for AAC — Icecast can be slow to deliver the first audio frame.
const LIVE_AAC_PREBUFFER_WAIT: Duration = Duration::from_secs(35);

// ── Shared inner state ────────────────────────────────────────────────────────

struct StreamInner {
    /// Append-only byte buffer.  Never shrinks.
    buf: Mutex<Vec<u8>>,
    /// Signalled after every chunk appended (and after download completes).
    cond: Condvar,
    /// Set to `true` once the download thread has finished (success or error).
    done: AtomicBool,
    error: Mutex<Option<String>>,
    /// HTTP `Content-Type` from the stream response, when present.
    content_type: Mutex<Option<String>>,
}

// ── Public reader ─────────────────────────────────────────────────────────────

/// A `Read + Seek` handle to a streaming HTTP response.
pub struct StreamingReader {
    inner: Arc<StreamInner>,
    pos: u64,
}

impl Read for StreamingReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let pos = self.pos as usize;
        // Block until there's data at `pos` or the download is done.
        let buf = {
            let guard = self.inner.buf.lock().unwrap();
            self.inner
                .cond
                .wait_while(guard, |b| {
                    b.len() <= pos && !self.inner.done.load(Ordering::Acquire)
                })
                .unwrap()
        };
        if let Some(err) = self.inner.error.lock().unwrap().clone() {
            return Err(std::io::Error::other(err));
        }
        let available = buf.len().saturating_sub(pos);
        if available == 0 {
            return Ok(0); // EOF
        }
        let n = out.len().min(available);
        out[..n].copy_from_slice(&buf[pos..pos + n]);
        drop(buf);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for StreamingReader {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        let new_pos: i64 = match from {
            SeekFrom::Start(off) => off as i64,
            SeekFrom::Current(off) => self.pos as i64 + off,
            SeekFrom::End(off) => {
                // Use bytes received so far. Waiting for `done` hangs on live radio
                // streams (symphonia probes with SeekFrom::End during format detection).
                let guard = self.inner.buf.lock().unwrap();
                let len = guard.len() as i64;
                drop(guard);
                len + off
            }
        };
        if new_pos < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start of stream",
            ));
        }
        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}

// ── Public constructor ────────────────────────────────────────────────────────

/// Start streaming `url` in a background thread and return a reader that
/// blocks only when the read position overtakes the download.
///
/// Returns after `PREBUFFER_BYTES` have been buffered (or the download
/// completes, whichever comes first).
pub fn open_stream(url: &str) -> Result<StreamingReader> {
    open_stream_with_prebuffer(url, PREBUFFER_BYTES, None, PrebufferMode::MinBytes)
}

/// Max playlist indirections (M3U/PLS wrappers) before giving up.
const MAX_PLAYLIST_DEPTH: u8 = 3;

/// Same as [`open_stream`] but tuned for unbounded live radio (smaller prebuffer).
/// Follows simple M3U/PLS playlist wrappers to the direct stream URL.
pub fn open_live_stream(url: &str) -> Result<StreamingReader> {
    open_live_stream_resolved(url, 0)
}

fn open_live_stream_resolved(url: &str, depth: u8) -> Result<StreamingReader> {
    if depth >= MAX_PLAYLIST_DEPTH {
        return Err(anyhow!(
            "playlist redirect limit reached — use a direct stream URL"
        ));
    }
    let likely_aac = hint_from_url(url) == StreamFormatHint::Aac;
    let reader = if likely_aac {
        open_stream_with_aac_prebuffer(url)?
    } else {
        open_stream_with_prebuffer(
            url,
            LIVE_PREBUFFER_BYTES,
            Some(LIVE_PREBUFFER_WAIT),
            PrebufferMode::MinBytes,
        )?
    };
    if let Some(next_url) = try_resolve_playlist_url(&reader)? {
        return open_live_stream_resolved(&next_url, depth + 1);
    }
    Ok(reader)
}

#[derive(Clone, Copy)]
enum PrebufferMode {
    MinBytes,
    /// Keep buffering until an ADTS sync word appears (AAC/Icecast).
    AdtsSync,
}

fn open_stream_with_aac_prebuffer(url: &str) -> Result<StreamingReader> {
    open_stream_with_prebuffer(
        url,
        LIVE_AAC_PREBUFFER_BYTES,
        Some(LIVE_AAC_PREBUFFER_WAIT),
        PrebufferMode::AdtsSync,
    )
}

fn open_stream_with_prebuffer(
    url: &str,
    prebuffer: usize,
    max_wait: Option<Duration>,
    mode: PrebufferMode,
) -> Result<StreamingReader> {
    let inner = Arc::new(StreamInner {
        buf: Mutex::new(Vec::new()),
        cond: Condvar::new(),
        done: AtomicBool::new(false),
        error: Mutex::new(None),
        content_type: Mutex::new(None),
    });

    let inner_dl = inner.clone();
    let fetch_url = url.to_owned();
    std::thread::Builder::new()
        .name("ratune-stream".into())
        .spawn(move || download_thread(&fetch_url, inner_dl))
        .context("failed to spawn stream thread")?;

    let deadline = max_wait.map(|d| Instant::now() + d);
    loop {
        if let Some(err) = inner.error.lock().unwrap().clone() {
            return Err(anyhow!(err));
        }
        let (len, done) = {
            let guard = inner.buf.lock().unwrap();
            (guard.len(), inner.done.load(Ordering::Acquire))
        };
        let active_mode = effective_prebuffer_mode(mode, &inner, url);
        let ready = match active_mode {
            PrebufferMode::MinBytes => len >= prebuffer || (done && len >= MIN_LIVE_BYTES),
            PrebufferMode::AdtsSync => {
                find_adts_sync_offset(&inner.buf.lock().unwrap()).is_some()
                    || len >= LIVE_AAC_PREBUFFER_MAX
                    || (done && len >= MIN_LIVE_BYTES)
            }
        };
        if ready {
            break;
        }
        if done && len == 0 {
            return Err(anyhow!("stream returned no data"));
        }
        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "stream prebuffer timeout (got {len} bytes, need {prebuffer})"
                ));
            }
        }
        let guard = inner.buf.lock().unwrap();
        let wait_for = Duration::from_millis(200);
        let (guard, _) = inner.cond.wait_timeout(guard, wait_for).unwrap();
        drop(guard);
    }

    {
        let guard = inner.buf.lock().unwrap();
        let active_mode = effective_prebuffer_mode(mode, &inner, url);
        if matches!(active_mode, PrebufferMode::AdtsSync)
            && find_adts_sync_offset(&guard).is_none()
            && guard.len() >= MIN_LIVE_BYTES
        {
            return Err(anyhow!(
                "no AAC ADTS audio found in stream (got {} bytes) — check the station URL",
                guard.len()
            ));
        }
    }

    Ok(StreamingReader { inner, pos: 0 })
}

/// Guess audio container from the first buffered bytes (after skipping whitespace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFormatHint {
    Mp3,
    Ogg,
    Aac,
    Unknown,
}

impl StreamingReader {
    /// Inspect bytes downloaded so far without consuming them.
    pub fn format_hint(&self) -> StreamFormatHint {
        let guard = self.inner.buf.lock().unwrap();
        sniff_format(&guard)
    }

    /// Detect format from URL/buffer, align AAC streams to the first ADTS frame, and
    /// return hints for symphonia. Without explicit hints symphonia probes every format
    /// against the live stream and can hang indefinitely.
    pub fn prepare_for_decode(&mut self, url: &str) -> LiveDecodeHint {
        let guard = self.inner.buf.lock().unwrap();
        let mut hint = sniff_format(&guard);
        drop(guard);
        if hint == StreamFormatHint::Unknown {
            hint = hint_from_url(url);
        }
        if hint == StreamFormatHint::Unknown {
            if let Some(ct) = self.inner.content_type.lock().unwrap().as_deref() {
                hint = hint_from_content_type(ct);
            }
        }
        if hint == StreamFormatHint::Aac {
            let guard = self.inner.buf.lock().unwrap();
            if let Some(offset) = find_preferred_adts_sync_offset(&guard) {
                self.pos = offset as u64;
            }
        }
        LiveDecodeHint { format: hint }
    }
}

/// Format detected for a live stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveDecodeHint {
    pub format: StreamFormatHint,
}

impl LiveDecodeHint {
    #[must_use]
    pub fn extension_hint(&self) -> Option<&'static str> {
        match self.format {
            StreamFormatHint::Mp3 => Some("mp3"),
            StreamFormatHint::Aac => Some("aac"),
            StreamFormatHint::Ogg => Some("ogg"),
            StreamFormatHint::Unknown => None,
        }
    }

    #[must_use]
    pub fn mime_hint(&self) -> Option<&'static str> {
        match self.format {
            StreamFormatHint::Mp3 => Some("audio/mpeg"),
            StreamFormatHint::Aac => Some("audio/aac"),
            StreamFormatHint::Ogg => Some("audio/ogg"),
            StreamFormatHint::Unknown => None,
        }
    }
}

/// ADTS sync is 12 bits `0xFFF` with layer field `00` (byte1 bits 2–1).
fn is_adts_sync(b0: u8, b1: u8) -> bool {
    b0 == 0xFF && (b1 & 0xF0) == 0xF0 && (b1 & 0x06) == 0x00
}

/// MP3 frame sync is 11 bits `0xFFE` with a non-zero layer field.
fn is_mp3_sync(b0: u8, b1: u8) -> bool {
    b0 == 0xFF && (b1 & 0xE0) == 0xE0 && (b1 & 0x06) != 0x00
}

fn sniff_format(buf: &[u8]) -> StreamFormatHint {
    let trimmed = trim_leading_ws(buf);
    if trimmed.starts_with(b"ID3") {
        return StreamFormatHint::Mp3;
    }
    if trimmed.starts_with(b"OggS") {
        return StreamFormatHint::Ogg;
    }
    // Only inspect stream start — MP3 frames (`0xFF 0xFB`) share the ADTS prefix.
    if trimmed.len() >= 2 && is_adts_sync(trimmed[0], trimmed[1]) {
        return StreamFormatHint::Aac;
    }
    if trimmed.len() >= 2 && is_mp3_sync(trimmed[0], trimmed[1]) {
        return StreamFormatHint::Mp3;
    }
    StreamFormatHint::Unknown
}

fn hint_from_url(url: &str) -> StreamFormatHint {
    let lower = url.to_ascii_lowercase();
    if lower.contains("aac")
        || lower.ends_with(".aacp")
        || lower.contains("_aac")
        || lower.contains("aac_")
        || lower.contains("_sc")
        || lower.contains("fmaac")
    {
        StreamFormatHint::Aac
    } else if lower.ends_with(".mp3") || lower.ends_with("-mp3") {
        StreamFormatHint::Mp3
    } else if lower.ends_with(".ogg") || lower.ends_with(".oga") {
        StreamFormatHint::Ogg
    } else {
        StreamFormatHint::Unknown
    }
}

fn hint_from_content_type(content_type: &str) -> StreamFormatHint {
    let lower = content_type.to_ascii_lowercase();
    if lower.contains("aac") {
        StreamFormatHint::Aac
    } else if lower.contains("mpeg") || lower.contains("mp3") {
        StreamFormatHint::Mp3
    } else if lower.contains("ogg") {
        StreamFormatHint::Ogg
    } else {
        StreamFormatHint::Unknown
    }
}

fn effective_prebuffer_mode(mode: PrebufferMode, inner: &StreamInner, url: &str) -> PrebufferMode {
    if matches!(mode, PrebufferMode::AdtsSync) {
        return PrebufferMode::AdtsSync;
    }
    if hint_from_url(url) == StreamFormatHint::Aac {
        return PrebufferMode::AdtsSync;
    }
    if inner
        .content_type
        .lock()
        .unwrap()
        .as_deref()
        .is_some_and(|ct| hint_from_content_type(ct) == StreamFormatHint::Aac)
    {
        return PrebufferMode::AdtsSync;
    }
    PrebufferMode::MinBytes
}

/// Prefer MPEG-4 ADTS (`0xFFF1`); fall back to any ADTS sync (incl. MPEG-2 `0xFFF9`).
fn find_preferred_adts_sync_offset(buf: &[u8]) -> Option<usize> {
    find_mpeg4_adts_sync_offset(buf).or_else(|| find_adts_sync_offset(buf))
}

fn find_mpeg4_adts_sync_offset(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w[0] == 0xFF && w[1] == 0xF1)
}

/// Byte offset of the first ADTS sync word (`0xFFF` + layer `00`).
fn find_adts_sync_offset(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| is_adts_sync(w[0], w[1]))
}

fn trim_leading_ws(buf: &[u8]) -> &[u8] {
    buf.iter()
        .position(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        .map_or(&[][..], |i| &buf[i..])
}

/// If the buffered response is a playlist wrapper, return the stream URL inside it.
fn try_resolve_playlist_url(reader: &StreamingReader) -> Result<Option<String>> {
    let guard = reader.inner.buf.lock().unwrap();
    try_resolve_playlist_bytes(&guard)
}

fn try_resolve_playlist_bytes(buf: &[u8]) -> Result<Option<String>> {
    let trimmed = trim_leading_ws(buf);
    let head = &trimmed[..trimmed.len().min(4096)];
    if head.starts_with(b"#EXTM3U") {
        let text = std::str::from_utf8(head).context("playlist is not valid UTF-8")?;
        if text.contains("#EXT-X-") || text.to_ascii_lowercase().contains(".m3u8") {
            return Err(anyhow!(
                "HLS stream (.m3u8) is not supported — find a direct MP3/Icecast URL"
            ));
        }
        return Ok(parse_m3u_stream_url(text));
    }
    if head.starts_with(b"[playlist]") {
        let text = std::str::from_utf8(head).context("playlist is not valid UTF-8")?;
        return Ok(parse_pls_stream_url(text));
    }
    if let Some(url) = parse_bare_stream_url(head) {
        return Ok(Some(url));
    }
    let lower: Vec<u8> = head
        .iter()
        .copied()
        .take(64)
        .map(|b| b.to_ascii_lowercase())
        .collect();
    if lower.starts_with(b"<!doctype") || lower.starts_with(b"<html") {
        return Err(anyhow!(
            "stream URL returned a web page, not audio — check the station URL"
        ));
    }
    Ok(None)
}

fn parse_m3u_stream_url(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("http://") || line.starts_with("https://") {
            return Some(line.to_string());
        }
    }
    None
}

fn parse_pls_stream_url(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(url) = line.strip_prefix("File1=") {
            let url = url.trim();
            if url.starts_with("http://") || url.starts_with("https://") {
                return Some(url.to_string());
            }
        }
    }
    None
}

/// Some servers return a one-line M3U (URL only, no `#EXTM3U` header).
fn parse_bare_stream_url(buf: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(buf).ok()?.trim();
    if text.len() > 512 || text.lines().count() > 3 {
        return None;
    }
    let line = text.lines().find(|l| !l.trim().is_empty())?.trim();
    if (line.starts_with("http://") || line.starts_with("https://"))
        && line
            .chars()
            .all(|c| !c.is_control() || c == '\r' || c == '\n')
    {
        Some(line.to_string())
    } else {
        None
    }
}

// ── Download thread ───────────────────────────────────────────────────────────

fn download_thread(url: &str, inner: Arc<StreamInner>) {
    if let Err(e) = download_into(url, &inner) {
        *inner.error.lock().unwrap() = Some(e.to_string());
    }
    inner.done.store(true, Ordering::Release);
    inner.cond.notify_all();
}

fn live_http_client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            // Live streams never finish — only bound connect time, not total body read.
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!(
                "Mozilla/5.0 (compatible; ratune/",
                env!("CARGO_PKG_VERSION"),
                "; +https://github.com/acmagn/ratune)"
            ))
            .build()
            .expect("live stream HTTP client")
    })
}

fn download_into(url: &str, inner: &StreamInner) -> Result<()> {
    use std::io::Read as _;
    let response = live_http_client()
        .get(url)
        .header(ACCEPT, "*/*")
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .context("HTTP request failed")?
        .error_for_status()
        .with_context(|| format!("stream HTTP error for {url}"))?;
    if let Some(ct) = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        *inner.content_type.lock().unwrap() = Some(ct.to_string());
    }
    let mut response = response;
    let mut chunk = vec![0u8; 32 * 1024];
    loop {
        let n = response.read(&mut chunk).context("stream read error")?;
        if n == 0 {
            break;
        }
        {
            let mut buf = inner.buf.lock().unwrap();
            buf.extend_from_slice(&chunk[..n]);
        }
        inner.cond.notify_all();
    }
    Ok(())
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use std::time::Instant;

    /// Regression: symphonia probes with SeekFrom::End; waiting for EOF hung live radio.
    #[test]
    fn live_stream_end_seek_does_not_wait_for_eof() {
        let inner = Arc::new(StreamInner {
            buf: Mutex::new(vec![0u8; 4096]),
            cond: Condvar::new(),
            done: AtomicBool::new(false),
            error: Mutex::new(None),
            content_type: Mutex::new(None),
        });
        let mut reader = StreamingReader { inner, pos: 0 };
        let start = Instant::now();
        reader.seek(SeekFrom::End(0)).unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn resolves_m3u_playlist_bytes() {
        let url = parse_m3u_stream_url("#EXTM3U\n#EXTINF:-1,Test\nhttp://x/stream.mp3\n");
        assert_eq!(url.as_deref(), Some("http://x/stream.mp3"));
    }

    #[test]
    fn detects_hls_in_m3u() {
        let body = b"#EXTM3U\n#EXT-X-VERSION:3\nhttp://x/playlist.m3u8\n";
        assert!(try_resolve_playlist_bytes(body).is_err());
    }

    #[test]
    fn sniff_aac_adts_at_stream_start() {
        let buf = [0xFFu8, 0xF1, 0x50, 0x80, 0x03, 0xE0, 0x00];
        assert_eq!(sniff_format(&buf), StreamFormatHint::Aac);
    }

    #[test]
    fn sniff_mp3_frame_not_aac() {
        // Common Icecast MP3 frame header — old sniff misclassified this as ADTS.
        let buf = [0xFFu8, 0xFB, 0x90, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(sniff_format(&buf), StreamFormatHint::Mp3);
        assert!(find_adts_sync_offset(&buf).is_none());
    }

    #[test]
    fn hint_from_url_somafm_mp3_suffix() {
        assert_eq!(
            hint_from_url("http://ice1.somafm.com/groovesalad-128-mp3"),
            StreamFormatHint::Mp3
        );
    }
}
