//! Library-track streaming against a throttled local HTTP server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ratune_player::stream::open_stream;
use ratune_player::track::{is_not_buffered, TrackSource};
use rodio::Source;

const RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
const SECONDS: u32 = 10;

fn wav_bytes() -> Vec<u8> {
    let data_len = RATE * u32::from(CHANNELS) * 2 * SECONDS;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&CHANNELS.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * u32::from(CHANNELS) * 2).to_le_bytes());
    out.extend_from_slice(&(CHANNELS * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..(data_len / 2) {
        out.extend_from_slice(&((i % 2000) as i16 - 1000).to_le_bytes());
    }
    out
}

#[derive(Clone, Copy)]
struct ServerOpts {
    content_length: bool,
    /// Close the first response after this many body bytes.
    truncate_first_at: Option<usize>,
}

struct Server {
    url: String,
    requests: Arc<AtomicUsize>,
    /// Body bytes written across all connections.
    sent: Arc<AtomicUsize>,
}

/// Serves `body` in 64 KiB chunks every 20 ms (~3 MB/s). Honours `Range: bytes=N-`.
fn serve(body: Vec<u8>, opts: ServerOpts) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/rest/stream", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let sent = Arc::new(AtomicUsize::new(0));
    let (counter, sent_total) = (requests.clone(), sent.clone());
    let body = Arc::new(body);
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let (body, sent_total) = (body.clone(), sent_total.clone());
            thread::spawn(move || respond(conn, &body, opts, n == 0, &sent_total));
        }
    });
    Server {
        url,
        requests,
        sent,
    }
}

fn respond(mut conn: TcpStream, body: &[u8], opts: ServerOpts, first: bool, sent: &AtomicUsize) {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut offset = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
            offset = v.trim().trim_end_matches('-').parse().unwrap();
        }
    }
    let status = if offset > 0 {
        "206 Partial Content"
    } else {
        "200 OK"
    };
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Type: audio/wav\r\nConnection: close\r\n");
    if opts.content_length {
        head.push_str(&format!("Content-Length: {}\r\n", body.len() - offset));
    }
    head.push_str("\r\n");
    if conn.write_all(head.as_bytes()).is_err() {
        return;
    }
    let end = match opts.truncate_first_at {
        Some(at) if first => at,
        _ => body.len(),
    };
    for chunk in body[offset..end].chunks(64 * 1024) {
        if conn.write_all(chunk).is_err() {
            return;
        }
        sent.fetch_add(chunk.len(), Ordering::SeqCst);
        thread::sleep(Duration::from_millis(20));
    }
}

fn remaining_secs(source: &mut TrackSource) -> f64 {
    let samples = source.by_ref().count();
    samples as f64 / f64::from(RATE * u32::from(CHANNELS))
}

#[test]
fn starts_before_download_and_seeks_once_buffered() {
    let body = wav_bytes();
    let server = serve(
        body.clone(),
        ServerOpts {
            content_length: true,
            truncate_first_at: None,
        },
    );
    let start = Instant::now();
    let reader = open_stream(&server.url).unwrap();
    assert_eq!(reader.content_length(), Some(body.len() as u64));
    assert!(
        reader.buffered_len() < body.len() as u64,
        "should not wait for the full body"
    );
    let progress = reader.reopen();
    let mut source = TrackSource::from_stream(reader).unwrap();
    assert!(start.elapsed() < Duration::from_millis(400));

    let err = source.try_seek(Duration::from_secs(9)).unwrap_err();
    assert!(is_not_buffered(&err), "unexpected error: {err}");
    while progress.buffered_len() < 600_000 {
        thread::sleep(Duration::from_millis(10));
    }
    // Near the start is buffered by now (estimate plus margin is well under 600 KB).
    source.try_seek(Duration::from_millis(100)).unwrap();

    progress.wait_complete().unwrap();
    let download = source
        .download()
        .expect("streamed source exposes its download");
    assert!(
        download.snapshot() == body,
        "cached bytes differ from the served file"
    );
    source.try_seek(Duration::from_secs(9)).unwrap();
    let left = remaining_secs(&mut source);
    assert!((left - 1.0).abs() < 0.1, "remaining {left}s after seek");
}

#[test]
fn without_content_length_seeks_after_download_completes() {
    let server = serve(
        wav_bytes(),
        ServerOpts {
            content_length: false,
            truncate_first_at: None,
        },
    );
    let reader = open_stream(&server.url).unwrap();
    assert_eq!(reader.content_length(), None);
    let progress = reader.reopen();
    let mut source = TrackSource::from_stream(reader).unwrap();

    let err = source.try_seek(Duration::from_secs(1)).unwrap_err();
    assert!(is_not_buffered(&err), "unexpected error: {err}");

    progress.wait_complete().unwrap();
    for _ in 0..1000 {
        source.next();
    }
    source.try_seek(Duration::from_secs(8)).unwrap();
    let left = remaining_secs(&mut source);
    assert!((left - 2.0).abs() < 0.1, "remaining {left}s after seek");
}

#[test]
fn truncated_download_resumes_with_range_request() {
    let body = wav_bytes();
    let server = serve(
        body.clone(),
        ServerOpts {
            content_length: true,
            truncate_first_at: Some(700_000),
        },
    );
    let mut reader = open_stream(&server.url).unwrap();
    assert_eq!(reader.reopen().wait_complete().unwrap(), body.len() as u64);
    assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    let mut got = Vec::new();
    reader.read_to_end(&mut got).unwrap();
    assert!(got == body, "resumed body differs from the original");
}

#[test]
fn dropping_every_reader_stops_the_download() {
    let body = wav_bytes();
    let server = serve(
        body.clone(),
        ServerOpts {
            content_length: true,
            truncate_first_at: None,
        },
    );
    let reader = open_stream(&server.url).unwrap();
    let second = reader.reopen();
    drop(reader);
    drop(second);
    // The full body takes ~550 ms at the server's pace.
    thread::sleep(Duration::from_millis(800));
    let sent = server.sent.load(Ordering::SeqCst);
    assert!(
        sent < body.len() / 2,
        "server kept sending ({sent} of {} bytes)",
        body.len()
    );
}
