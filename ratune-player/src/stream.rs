//! Streaming HTTP → growing-buffer reader for the audio pipeline.
//!
//! `open_stream` sends the request, then a background thread appends the body
//! to a shared `Vec<u8>`.  The `StreamingReader` returned exposes that buffer
//! as a `Read + Seek` handle that blocks only when the read position overtakes
//! the download.
//!
//! Keeping all bytes (never freeing) means backward seeks always succeed
//! without re-fetching.  Trade-off: memory grows with the file (~14–30 MB for
//! a typical song), which is acceptable for a music player.
//!
//! Playback starts as soon as enough bytes have been buffered (see constants).
//! The download stops early once every reader has been dropped.

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CONTENT_TYPE, RANGE};
use reqwest::StatusCode;

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

/// Longer wait for AAC. Icecast can be slow to deliver the first audio frame.
const LIVE_AAC_PREBUFFER_WAIT: Duration = Duration::from_secs(35);

/// Max wait for the library-track prebuffer before giving up.
const TRACK_PREBUFFER_WAIT: Duration = Duration::from_secs(30);

/// Range-request retries after a library track's body is cut off mid-download.
const TRACK_MAX_RESUMES: u32 = 3;

// ── Shared inner state ────────────────────────────────────────────────────────

/// Library tracks are finite and may be resumed; live radio never ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Live,
    Track,
}

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
    /// `Content-Length` of a library track (never set for live radio).
    total_len: Option<u64>,
}

impl StreamInner {
    fn new(content_type: Option<String>, total_len: Option<u64>) -> Self {
        Self {
            buf: Mutex::new(Vec::new()),
            cond: Condvar::new(),
            done: AtomicBool::new(false),
            error: Mutex::new(None),
            content_type: Mutex::new(content_type),
            total_len,
        }
    }
}

// ── Public reader ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    InProgress,
    Complete,
    Failed,
}

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
        let available = buf.len().saturating_sub(pos);
        if available == 0 {
            if let Some(err) = self.inner.error.lock().unwrap().clone() {
                return Err(std::io::Error::other(err));
            }
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
                // Without a Content-Length, use bytes received so far. Waiting for
                // `done` hangs on live radio streams (symphonia probes with
                // SeekFrom::End during format detection).
                let len = match self.inner.total_len {
                    Some(total) => total as i64,
                    None => self.inner.buf.lock().unwrap().len() as i64,
                };
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

/// Start streaming a library track and return a reader that blocks only when
/// the read position overtakes the download.
///
/// Returns after `PREBUFFER_BYTES` have been buffered (or the download
/// completes, whichever comes first). [`StreamingReader::content_length`]
/// reports the server's `Content-Length`, when it sent one.
pub fn open_stream(url: &str) -> Result<StreamingReader> {
    open_stream_with_prebuffer(
        url,
        StreamKind::Track,
        PREBUFFER_BYTES,
        Some(TRACK_PREBUFFER_WAIT),
        PrebufferMode::MinBytes,
    )
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
            StreamKind::Live,
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
        StreamKind::Live,
        LIVE_AAC_PREBUFFER_BYTES,
        Some(LIVE_AAC_PREBUFFER_WAIT),
        PrebufferMode::AdtsSync,
    )
}

fn open_stream_with_prebuffer(
    url: &str,
    kind: StreamKind,
    prebuffer: usize,
    max_wait: Option<Duration>,
    mode: PrebufferMode,
) -> Result<StreamingReader> {
    let response = send_request(url, kind, None)?;
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let total_len = match kind {
        StreamKind::Track => response.content_length(),
        StreamKind::Live => None,
    };
    let inner = Arc::new(StreamInner::new(content_type, total_len));
    let min_bytes = match kind {
        StreamKind::Track => 1,
        StreamKind::Live => MIN_LIVE_BYTES,
    };

    let inner_dl = inner.clone();
    let fetch_url = url.to_owned();
    std::thread::Builder::new()
        .name("ratune-stream".into())
        .spawn(move || download_thread(&fetch_url, kind, response, inner_dl))
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
            PrebufferMode::MinBytes => len >= prebuffer || (done && len >= min_bytes),
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
    /// `Content-Length` of a library track, when the server sent one.
    #[must_use]
    pub fn content_length(&self) -> Option<u64> {
        self.inner.total_len
    }

    /// Bytes downloaded so far.
    #[must_use]
    pub fn buffered_len(&self) -> u64 {
        self.inner.buf.lock().unwrap().len() as u64
    }

    /// `true` once the whole body has been downloaded without error.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.status() == DownloadStatus::Complete
    }

    #[must_use]
    pub fn status(&self) -> DownloadStatus {
        if !self.inner.done.load(Ordering::Acquire) {
            DownloadStatus::InProgress
        } else if self.inner.error.lock().unwrap().is_some() {
            DownloadStatus::Failed
        } else {
            DownloadStatus::Complete
        }
    }

    /// Copy of every byte downloaded so far.
    #[must_use]
    pub fn snapshot(&self) -> Vec<u8> {
        self.inner.buf.lock().unwrap().clone()
    }

    /// Block until the download finishes; returns the total byte length.
    pub fn wait_complete(&self) -> Result<u64> {
        let guard = self.inner.buf.lock().unwrap();
        let guard = self
            .inner
            .cond
            .wait_while(guard, |_| !self.inner.done.load(Ordering::Acquire))
            .unwrap();
        let len = guard.len() as u64;
        drop(guard);
        match self.inner.error.lock().unwrap().clone() {
            Some(err) => Err(anyhow!(err)),
            None => Ok(len),
        }
    }

    /// Another reader over the same download, positioned at the start.
    #[must_use]
    pub fn reopen(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            pos: 0,
        }
    }

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
    // Only inspect stream start. MP3 frames (`0xFF 0xFB`) share the ADTS prefix.
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

fn download_thread(url: &str, kind: StreamKind, response: Response, inner: Arc<StreamInner>) {
    if let Err(e) = download_into(url, kind, response, &inner) {
        *inner.error.lock().unwrap() = Some(format!("{e:#}"));
    }
    {
        // Set under the buffer lock so a waiter can't miss the wakeup.
        let _guard = inner.buf.lock().unwrap();
        inner.done.store(true, Ordering::Release);
    }
    inner.cond.notify_all();
}

/// Shared client for library tracks: default `get()` uses short timeouts and can
/// truncate large or slow streams; long tracks need a generous read deadline.
pub(crate) fn track_http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            // Whole request including body (large lossless files over slow links).
            .timeout(Duration::from_secs(900))
            // Keep a dead server from monopolizing the player thread. This is no
            // longer than the app's connectivity probe, so cached commands are not
            // left queued behind a stale online request after entering offline mode.
            .connect_timeout(Duration::from_secs(4))
            .pool_idle_timeout(Some(Duration::from_secs(90)))
            .user_agent(concat!("ratune/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("stream HTTP client")
    })
}

fn live_http_client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            // Live streams never finish. Only bound connect time, not total body read.
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

/// Send the GET for `url`, optionally resuming at byte `offset`.
///
/// Identity encoding keeps `Content-Length` equal to the audio byte length.
fn send_request(url: &str, kind: StreamKind, offset: Option<u64>) -> Result<Response> {
    let client = match kind {
        StreamKind::Track => track_http_client(),
        StreamKind::Live => live_http_client(),
    };
    let mut req = client
        .get(url)
        .header(ACCEPT, "*/*")
        .header(ACCEPT_ENCODING, "identity");
    if let Some(offset) = offset {
        req = req.header(RANGE, format!("bytes={offset}-"));
    }
    let response = req.send().context("connecting to stream URL")?;
    let status = response.status();
    // Never put `url` in errors: library stream URLs carry the auth token.
    response
        .error_for_status()
        .with_context(|| format!("stream HTTP {status}"))
}

enum PumpEnd {
    Eof,
    /// Every reader was dropped; nobody needs the rest of the body.
    Abandoned,
}

fn pump_body(response: &mut Response, inner: &Arc<StreamInner>) -> Result<PumpEnd> {
    let mut chunk = vec![0u8; 32 * 1024];
    loop {
        if Arc::strong_count(inner) == 1 {
            return Ok(PumpEnd::Abandoned);
        }
        let n = response.read(&mut chunk).context("stream read error")?;
        if n == 0 {
            return Ok(PumpEnd::Eof);
        }
        {
            let mut buf = inner.buf.lock().unwrap();
            buf.extend_from_slice(&chunk[..n]);
        }
        inner.cond.notify_all();
    }
}

fn download_into(
    url: &str,
    kind: StreamKind,
    mut response: Response,
    inner: &Arc<StreamInner>,
) -> Result<()> {
    let mut resumes = 0u32;
    loop {
        let err = match pump_body(&mut response, inner) {
            Ok(PumpEnd::Abandoned) => return Ok(()),
            Ok(PumpEnd::Eof) => {
                let got = inner.buf.lock().unwrap().len() as u64;
                match inner.total_len {
                    Some(total) if got < total => {
                        anyhow!("stream ended early ({got} of {total} bytes)")
                    }
                    _ => return Ok(()),
                }
            }
            Err(e) => e,
        };
        // Only a library track with a known length can be resumed by byte offset.
        if kind != StreamKind::Track || inner.total_len.is_none() || resumes >= TRACK_MAX_RESUMES {
            return Err(err);
        }
        resumes += 1;
        std::thread::sleep(Duration::from_millis(250 * u64::from(resumes)));
        let offset = inner.buf.lock().unwrap().len() as u64;
        response = match send_request(url, kind, Some(offset)) {
            Ok(r) if r.status() == StatusCode::PARTIAL_CONTENT => r,
            // A 200 would restart from byte 0; we can't splice that onto the buffer.
            Ok(_) => return Err(err.context("server does not support resuming downloads")),
            Err(_) => return Err(err),
        };
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use std::time::Instant;

    /// Regression: symphonia probes with SeekFrom::End; waiting for EOF hung live radio.
    #[test]
    fn live_stream_end_seek_does_not_wait_for_eof() {
        let inner = StreamInner::new(None, None);
        *inner.buf.lock().unwrap() = vec![0u8; 4096];
        let mut reader = StreamingReader {
            inner: Arc::new(inner),
            pos: 0,
        };
        let start = Instant::now();
        reader.seek(SeekFrom::End(0)).unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn track_end_seek_uses_content_length() {
        let inner = StreamInner::new(None, Some(10_000));
        *inner.buf.lock().unwrap() = vec![0u8; 4096];
        let mut reader = StreamingReader {
            inner: Arc::new(inner),
            pos: 0,
        };
        assert_eq!(reader.seek(SeekFrom::End(-100)).unwrap(), 9_900);
    }

    #[test]
    fn read_returns_buffered_bytes_before_download_error() {
        let inner = StreamInner::new(None, None);
        *inner.buf.lock().unwrap() = vec![7u8; 8];
        *inner.error.lock().unwrap() = Some("dropped".into());
        inner.done.store(true, Ordering::Release);
        let mut reader = StreamingReader {
            inner: Arc::new(inner),
            pos: 0,
        };
        let mut out = [0u8; 16];
        assert_eq!(reader.read(&mut out).unwrap(), 8);
        assert!(reader.read(&mut out).is_err());
        assert!(!reader.is_complete());
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
        // Common Icecast MP3 frame header. Old sniff misclassified this as ADTS.
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
