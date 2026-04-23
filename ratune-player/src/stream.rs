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
//! Playback starts as soon as 256 KB have been buffered (PREBUFFER_BYTES).

use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Minimum bytes buffered before `open_stream` returns.
const PREBUFFER_BYTES: usize = 256 * 1024;

// ── Shared inner state ────────────────────────────────────────────────────────

struct StreamInner {
    /// Append-only byte buffer.  Never shrinks.
    buf: Mutex<Vec<u8>>,
    /// Signalled after every chunk appended (and after download completes).
    cond: Condvar,
    /// Set to `true` once the download thread has finished (success or error).
    done: AtomicBool,
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
                .wait_while(guard, |b| b.len() <= pos && !self.inner.done.load(Ordering::Acquire))
                .unwrap()
        };
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
                // Must wait until download is complete to know total length.
                let buf = {
                    let guard = self.inner.buf.lock().unwrap();
                    self.inner
                        .cond
                        .wait_while(guard, |_| !self.inner.done.load(Ordering::Acquire))
                        .unwrap()
                };
                let len = buf.len() as i64;
                drop(buf);
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
    let inner = Arc::new(StreamInner {
        buf: Mutex::new(Vec::new()),
        cond: Condvar::new(),
        done: AtomicBool::new(false),
    });

    // Spawn download thread.
    let inner_dl = inner.clone();
    let url = url.to_owned();
    std::thread::Builder::new()
        .name("ratune-stream".into())
        .spawn(move || download_thread(&url, inner_dl))
        .context("failed to spawn stream thread")?;

    // Wait until PREBUFFER_BYTES are ready (or download finishes early).
    {
        let guard = inner.buf.lock().unwrap();
        let _guard = inner
            .cond
            .wait_while(guard, |b| {
                b.len() < PREBUFFER_BYTES && !inner.done.load(Ordering::Acquire)
            })
            .unwrap();
    }

    Ok(StreamingReader { inner, pos: 0 })
}

// ── Download thread ───────────────────────────────────────────────────────────

fn download_thread(url: &str, inner: Arc<StreamInner>) {
    let _ = download_into(url, &inner);
    inner.done.store(true, Ordering::Release);
    inner.cond.notify_all();
}

fn download_into(url: &str, inner: &StreamInner) -> Result<()> {
    use std::io::Read as _;
    let mut response = reqwest::blocking::get(url).context("HTTP request failed")?;
    let mut chunk = vec![0u8; 32 * 1024]; // 32 KB read buffer
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
