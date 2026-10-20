//! Audio playback engine.
//!
//! Runs entirely on a dedicated `std::thread`. No tokio inside this module.
//! The TUI communicates via two `std::sync::mpsc` channels:
//!
//! - `PlayerCommand` (TUI → engine): play a URL, pause, resume, stop, set volume.
//! - `PlayerEvent`  (engine → TUI): progress ticks, track-ended, errors.

use std::collections::VecDeque;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::header::ACCEPT_ENCODING;
use rodio::{Decoder, DeviceSinkBuilder, Player};

use crate::adts_normalize::AdtsNormalizeReader;
use crate::stream::{
    open_live_stream, open_stream, track_http_client, DownloadStatus, StreamFormatHint,
    StreamingReader,
};
use crate::tap::SampleTap;
use crate::track::{is_not_buffered, TrackSource};

type SampleBuffer = Arc<Mutex<VecDeque<f32>>>;

// ── Public channel types ──────────────────────────────────────────────────────

/// Commands sent from the TUI to the player thread.
#[derive(Debug)]
pub enum PlayerCommand {
    /// Start playing the track at `url`. `duration` is the expected total
    /// duration (from Subsonic metadata), used for progress display.
    /// `gen` is a monotonically increasing counter from the TUI; the engine
    /// uses it to discard stale downloads when multiple skips arrive quickly.
    PlayUrl {
        url: String,
        duration: Option<Duration>,
        gen: u64,
    },
    /// Same semantics as [`PlayUrl`](Self::PlayUrl), but reads audio from a local cache file.
    PlayCached {
        path: PathBuf,
        duration: Option<Duration>,
        gen: u64,
    },
    /// Same as [`PlayUrl`](Self::PlayUrl), but streams incrementally for live radio.
    /// `duration` is always `None`; gapless enqueue is not used.
    PlayLiveStream {
        url: String,
        gen: u64,
    },
    /// Append the next track to the player queue for gapless playback.
    ///
    /// Must only be sent in response to `PlayerEvent::AboutToFinish`.
    /// Does NOT stop current playback.
    EnqueueNext {
        url: String,
        duration: Option<Duration>,
    },
    /// Gapless prefetch for an offline-cached next track (see [`PlayCached`](Self::PlayCached)).
    EnqueueNextCached {
        path: PathBuf,
        duration: Option<Duration>,
    },
    Pause,
    Resume,
    Stop,
    SetVolume(f32),
    /// Seek to an absolute position in the current track.
    Seek(Duration),
    /// Stop playback and shut down the player thread cleanly.
    Quit,
}

/// Events sent from the player thread back to the TUI.
#[derive(Debug)]
pub enum PlayerEvent {
    TrackStarted,
    /// Fired every ~500 ms. `total` is `None` when unknown.
    Progress {
        elapsed: Duration,
        total: Option<Duration>,
    },
    /// Fired ~10 s before the current track ends. The TUI should respond with
    /// `PlayerCommand::EnqueueNext` to enable gapless playback.
    AboutToFinish,
    /// Fired when a gaplessly enqueued track begins playing (elapsed resets).
    TrackAdvanced,
    TrackEnded,
    /// A seek was refused because that part of the track hasn't downloaded yet.
    SeekUnavailable,
    /// A streamed track (current or gapless next) finished downloading.
    TrackDownloaded(DownloadedTrack),
    Error(String),
}

/// Completed download of a streamed track, for the offline cache.
pub struct DownloadedTrack {
    /// URL from the `PlayUrl` / `EnqueueNext` command that streamed it.
    pub url: String,
    download: StreamingReader,
}

impl DownloadedTrack {
    /// Copy of the whole body. Only pay for this when actually caching.
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        self.download.snapshot()
    }
}

impl std::fmt::Debug for DownloadedTrack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The URL carries the auth token, and the body is megabytes long.
        f.debug_struct("DownloadedTrack")
            .field("bytes", &self.download.buffered_len())
            .finish_non_exhaustive()
    }
}

// ── Engine spawn ──────────────────────────────────────────────────────────────

/// Spawn the player thread.
///
/// Returns `(cmd_tx, evt_rx, join_handle, sample_buffer)`.  The caller should send
/// `PlayerCommand::Quit` and then join the handle (with a timeout) on
/// shutdown to ensure the audio device is released cleanly.
///
/// `sample_buffer` is a ring buffer of the most recent decoded f32 samples;
/// the TUI reads it each frame to drive the visualizer FFT.
pub fn spawn_player() -> (
    mpsc::Sender<PlayerCommand>,
    mpsc::Receiver<PlayerEvent>,
    std::thread::JoinHandle<()>,
    SampleBuffer,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<PlayerCommand>();
    let (evt_tx, evt_rx) = mpsc::channel::<PlayerEvent>();

    let sample_buffer: SampleBuffer = Arc::new(Mutex::new(VecDeque::with_capacity(4096)));
    let thread_buffer = sample_buffer.clone();

    let handle = std::thread::Builder::new()
        .name("ratune-player".into())
        .spawn(move || player_thread(cmd_rx, evt_tx, thread_buffer))
        .expect("failed to spawn player thread");

    (cmd_tx, evt_rx, handle, sample_buffer)
}

// ── Player thread ─────────────────────────────────────────────────────────────

fn player_thread(
    cmd_rx: mpsc::Receiver<PlayerCommand>,
    evt_tx: mpsc::Sender<PlayerEvent>,
    sample_buffer: SampleBuffer,
) {
    // MixerDeviceSink must live for the duration of playback.
    let mut device = match DeviceSinkBuilder::open_default_sink() {
        Ok(d) => d,
        Err(e) => {
            let _ = evt_tx.send(PlayerEvent::Error(format!("audio device error: {e}")));
            return;
        }
    };
    // Suppress the default stderr message on drop. We control shutdown explicitly.
    device.log_on_drop(false);

    let player = Player::connect_new(device.mixer());

    Engine {
        cmd_rx: &cmd_rx,
        evt_tx: &evt_tx,
        player: &player,
        sample_buffer: &sample_buffer,
        skip_gen: 0,
        track: TrackState::default(),
        downloads: Vec::new(),
    }
    .run();

    // Stop playback before releasing the audio device.
    player.stop();
    drop(player);
    drop(device);
}

/// Next gapless track, still loading on a background thread.
struct PendingNext {
    rx: mpsc::Receiver<Result<TrackSource>>,
    total: Option<Duration>,
    /// Stream URL, when the next track isn't from the cache.
    url: Option<String>,
}

/// Per-track playback state, reset on every new play or stop.
#[derive(Default)]
struct TrackState {
    current_total: Option<Duration>,
    /// Whether the previous tick saw a non-empty player (to detect natural end).
    was_playing: bool,
    next_total: Option<Duration>,
    next_queued: bool,
    /// Dropping it abandons the load (and its download).
    pending_next: Option<PendingNext>,
    about_to_finish_sent: bool,
    prev_elapsed: Duration,
}

struct Engine<'a> {
    cmd_rx: &'a mpsc::Receiver<PlayerCommand>,
    evt_tx: &'a mpsc::Sender<PlayerEvent>,
    player: &'a Player,
    sample_buffer: &'a SampleBuffer,
    /// Skip-generation counter: updated every time a PlayUrl / PlayCached is received.
    /// Used to discard stale loads when the user skips rapidly.
    skip_gen: u64,
    track: TrackState,
    /// In-flight downloads of loaded tracks, reported once complete. Cleared on
    /// every new play or stop, which also abandons unfinished ones.
    downloads: Vec<(String, StreamingReader)>,
}

#[derive(Debug, Clone)]
enum PlayPayload {
    Url(String),
    Cached(PathBuf),
}

impl Engine<'_> {
    fn run(&mut self) {
        loop {
            // ── Drain all pending commands (non-blocking) ─────────────────────
            loop {
                use mpsc::TryRecvError;
                match self.cmd_rx.try_recv() {
                    Ok(PlayerCommand::Quit) | Err(TryRecvError::Disconnected) => return,
                    Ok(PlayerCommand::PlayUrl { url, duration, gen }) => {
                        self.play_payload(PlayPayload::Url(url), duration, gen);
                    }
                    Ok(PlayerCommand::PlayCached {
                        path,
                        duration,
                        gen,
                    }) => self.play_payload(PlayPayload::Cached(path), duration, gen),
                    Ok(PlayerCommand::PlayLiveStream { url, gen }) => {
                        self.play_live_stream(url, gen);
                    }
                    Ok(cmd) => self.handle_command(cmd),
                    Err(TryRecvError::Empty) => break,
                }
            }

            self.poll_pending_next();
            self.poll_downloads();
            self.tick();
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn send(&self, event: PlayerEvent) {
        let _ = self.evt_tx.send(event);
    }

    fn watch_download(&mut self, url: Option<String>, source: &TrackSource) {
        if let (Some(url), Some(download)) = (url, source.download()) {
            self.downloads.push((url, download));
        }
    }

    fn poll_downloads(&mut self) {
        let mut finished = Vec::new();
        self.downloads
            .retain(|(url, download)| match download.status() {
                DownloadStatus::InProgress => true,
                DownloadStatus::Complete => {
                    finished.push(DownloadedTrack {
                        url: url.clone(),
                        download: download.reopen(),
                    });
                    false
                }
                DownloadStatus::Failed => false,
            });
        for track in finished {
            self.send(PlayerEvent::TrackDownloaded(track));
        }
    }

    /// Forget the current and next track (new play or stop).
    fn reset_tracks(&mut self) {
        self.player.stop();
        self.track = TrackState::default();
        self.downloads.clear();
    }

    fn start(&mut self, source: TrackSource) {
        let tapped = SampleTap::new(source, self.sample_buffer.clone());
        self.player.append(tapped);
        self.player.play();
        self.send(PlayerEvent::TrackStarted);
    }

    /// Progress tick, gapless hand-off detection, and natural end detection.
    fn tick(&mut self) {
        let player = self.player;
        if !player.is_paused() && !player.empty() {
            let elapsed = player.get_pos();
            let t = &mut self.track;

            // Detect gapless track transition: elapsed resets to near zero
            // while we know a next track was appended.  Use a 2 s window rather
            // than 500 ms to tolerate rodio's first-tick imprecision.
            if t.next_queued
                && t.prev_elapsed > Duration::from_secs(2)
                && elapsed < Duration::from_secs(2)
            {
                t.current_total = t.next_total.take();
                t.next_queued = false;
                t.about_to_finish_sent = false;
                let _ = self.evt_tx.send(PlayerEvent::TrackAdvanced);
            }
            t.prev_elapsed = elapsed;

            let _ = self.evt_tx.send(PlayerEvent::Progress {
                elapsed,
                total: t.current_total,
            });

            // Send AboutToFinish ~10 s before the end so the TUI can enqueue next.
            // The next track only needs its prebuffer (not the whole file) before
            // it can be appended, and loads off this thread.
            if !t.about_to_finish_sent && !t.next_queued && t.pending_next.is_none() {
                if let Some(total) = t.current_total {
                    let remaining = total.saturating_sub(elapsed);
                    if remaining <= Duration::from_secs(10) && remaining > Duration::ZERO {
                        t.about_to_finish_sent = true;
                        let _ = self.evt_tx.send(PlayerEvent::AboutToFinish);
                    }
                }
            }

            t.was_playing = true;
        }

        // ── Natural track end detection (no next track was enqueued) ──────────
        // While the next track is still loading, wait for it instead: it is
        // appended late and reported as TrackAdvanced, not TrackEnded.
        if self.track.was_playing && player.empty() && self.track.pending_next.is_none() {
            self.track = TrackState::default();
            self.send(PlayerEvent::TrackEnded);
        }
    }

    /// Append the next gapless track once its background load finishes.
    fn poll_pending_next(&mut self) {
        let Some(pending) = &self.track.pending_next else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow!("track loader exited")),
        };
        let total = pending.total;
        let url = pending.url.clone();
        self.track.pending_next = None;
        match result {
            Ok(source) => {
                self.track.next_total = total;
                self.track.next_queued = true;
                self.watch_download(url, &source);
                let tapped = SampleTap::new(source, self.sample_buffer.clone());
                self.player.append(tapped);
            }
            Err(e) => self.send(PlayerEvent::Error(format!("enqueue error: {e}"))),
        }
    }

    fn spawn_enqueue(
        &mut self,
        total: Option<Duration>,
        url: Option<String>,
        load: impl FnOnce() -> Result<TrackSource> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("ratune-enqueue".into())
            .spawn(move || {
                let _ = tx.send(load());
            });
        match spawned {
            Ok(_) => self.track.pending_next = Some(PendingNext { rx, total, url }),
            Err(e) => self.send(PlayerEvent::Error(format!("enqueue error: {e}"))),
        }
    }

    /// Handle [`PlayerCommand::PlayUrl`] / [`PlayerCommand::PlayCached`] with skip-generation cancellation.
    ///
    /// Before loading, drains any further play commands already queued, turning N rapid
    /// skips into one fetch/read. After the blocking load, drains again; if a newer play
    /// command arrived mid-load, discards the decoder and recurses.
    fn play_payload(&mut self, payload: PlayPayload, duration: Option<Duration>, gen: u64) {
        self.skip_gen = gen;

        // ── Pre-load drain ────────────────────────────────────────────────────
        let mut final_payload = payload;
        let mut final_duration = duration;
        let mut final_gen = gen;
        loop {
            match self.cmd_rx.try_recv() {
                Ok(PlayerCommand::PlayUrl {
                    url: u,
                    duration: d,
                    gen: g,
                }) => {
                    final_payload = PlayPayload::Url(u);
                    final_duration = d;
                    final_gen = g;
                    self.skip_gen = g;
                }
                Ok(PlayerCommand::PlayCached {
                    path: p,
                    duration: d,
                    gen: g,
                }) => {
                    final_payload = PlayPayload::Cached(p);
                    final_duration = d;
                    final_gen = g;
                    self.skip_gen = g;
                }
                Ok(_other) => break,
                Err(_) => break,
            }
        }

        self.reset_tracks();

        // ── Load (network or disk) ──────────────────────────────────────────
        let loaded = match &final_payload {
            PlayPayload::Url(url) => load_url_track(url),
            PlayPayload::Cached(path) => read_cached_and_decode(path).map(TrackSource::from_memory),
        };
        let source = match loaded {
            Ok(s) => s,
            Err(e) => {
                self.send(PlayerEvent::Error(format!("playback error: {e}")));
                return;
            }
        };

        // ── Post-load drain ───────────────────────────────────────────────────
        let mut newer: Option<(PlayPayload, Option<Duration>, u64)> = None;
        loop {
            match self.cmd_rx.try_recv() {
                Ok(PlayerCommand::PlayLiveStream { url: u, gen: g }) => {
                    self.skip_gen = g;
                    drop(source);
                    self.play_live_stream(u, g);
                    return;
                }
                Ok(PlayerCommand::PlayUrl {
                    url: u,
                    duration: d,
                    gen: g,
                }) => {
                    self.skip_gen = g;
                    newer = Some((PlayPayload::Url(u), d, g));
                }
                Ok(PlayerCommand::PlayCached {
                    path: p,
                    duration: d,
                    gen: g,
                }) => {
                    self.skip_gen = g;
                    newer = Some((PlayPayload::Cached(p), d, g));
                }
                Ok(_other) => break,
                Err(_) => break,
            }
        }

        if self.skip_gen != final_gen {
            drop(source);
            if let Some((p, d, g)) = newer {
                self.play_payload(p, d, g);
            }
            return;
        }

        self.track.current_total = final_duration;
        if let PlayPayload::Url(url) = final_payload {
            self.watch_download(Some(url), &source);
        }
        self.start(source);
    }

    /// Incrementally stream a live radio URL (no known duration, no gapless enqueue).
    fn play_live_stream(&mut self, url: String, gen: u64) {
        self.skip_gen = gen;

        let mut final_url = url;
        let mut final_gen = gen;
        loop {
            match self.cmd_rx.try_recv() {
                Ok(PlayerCommand::PlayLiveStream { url: u, gen: g }) => {
                    final_url = u;
                    final_gen = g;
                    self.skip_gen = g;
                }
                Ok(_other) => break,
                Err(_) => break,
            }
        }

        self.reset_tracks();

        let source = match live_stream_and_decode(&final_url) {
            Ok(s) => s,
            Err(e) => {
                self.send(PlayerEvent::Error(format!("playback error: {e}")));
                return;
            }
        };

        match self.cmd_rx.try_recv() {
            Ok(PlayerCommand::PlayLiveStream { url: u, gen: g }) if g != final_gen => {
                drop(source);
                self.play_live_stream(u, g);
                return;
            }
            Ok(PlayerCommand::PlayUrl {
                url: u,
                duration: d,
                gen: g,
            }) => {
                drop(source);
                self.play_payload(PlayPayload::Url(u), d, g);
                return;
            }
            Ok(PlayerCommand::PlayCached {
                path: p,
                duration: d,
                gen: g,
            }) => {
                drop(source);
                self.play_payload(PlayPayload::Cached(p), d, g);
                return;
            }
            Ok(_) | Err(_) => {}
        }

        if self.skip_gen != final_gen {
            drop(source);
            return;
        }

        self.track.current_total = None;
        let tapped = SampleTap::new(source, self.sample_buffer.clone());
        self.player.append(tapped);
        self.player.play();
        self.send(PlayerEvent::TrackStarted);
    }

    fn handle_command(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::PlayUrl { .. }
            | PlayerCommand::PlayCached { .. }
            | PlayerCommand::PlayLiveStream { .. } => {
                unreachable!(
                    "PlayUrl / PlayCached / PlayLiveStream must be dispatched via play_payload()"
                );
            }
            PlayerCommand::EnqueueNext { url, duration } => {
                let watch_url = url.clone();
                self.spawn_enqueue(duration, Some(watch_url), move || load_url_track(&url));
            }
            PlayerCommand::EnqueueNextCached { path, duration } => {
                self.spawn_enqueue(duration, None, move || {
                    read_cached_and_decode(&path).map(TrackSource::from_memory)
                });
            }
            PlayerCommand::Pause => self.player.pause(),
            PlayerCommand::Resume => self.player.play(),
            PlayerCommand::Stop => self.reset_tracks(),
            PlayerCommand::SetVolume(v) => self.player.set_volume(v),
            PlayerCommand::Seek(pos) => match self.player.try_seek(pos) {
                // Update prev_elapsed so the gapless-transition heuristic isn't
                // confused by the sudden position jump.
                Ok(()) => self.track.prev_elapsed = pos,
                Err(e) if is_not_buffered(&e) => self.send(PlayerEvent::SeekUnavailable),
                Err(_) => {}
            },
            PlayerCommand::Quit => {
                // Handled by the command-drain loop in `run`.
                unreachable!("Quit must be handled in the command-drain loop");
            }
        }
    }
}

// ── Stream + decode ───────────────────────────────────────────────────────────

/// Retrying an unreachable host keeps the single player thread blocked and
/// prevents a subsequent cached track from starting, so callers fail fast on these.
fn is_connectivity_failure(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|request| request.is_connect() || request.is_timeout())
    })
}

/// Start a library track as soon as its prebuffer arrives.
///
/// Falls back to decoding after the whole body has arrived (formats symphonia
/// can't probe unseekable), then to a fresh whole-body download.
fn load_url_track(url: &str) -> Result<TrackSource> {
    let reader = open_track_stream(url)?;
    let fallback = reader.reopen();
    match TrackSource::from_stream(reader) {
        Ok(source) => Ok(source),
        Err(_) => TrackSource::from_completed_stream(fallback)
            .or_else(|_| download_and_decode(url).map(TrackSource::from_memory)),
    }
}

fn open_track_stream(url: &str) -> Result<StreamingReader> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(200 * u64::from(attempt)));
        }
        match open_stream(url) {
            Ok(reader) => return Ok(reader),
            Err(e) => {
                let connectivity_failure = is_connectivity_failure(&e);
                last_err = Some(e);
                if connectivity_failure {
                    break;
                }
            }
        }
    }
    Err(last_err.expect("loop runs at least once"))
}

fn fetch_track_bytes(url: &str, accept_identity: bool) -> Result<Vec<u8>> {
    let mut req = track_http_client().get(url);
    if accept_identity {
        // Some proxies / servers serve odd combinations of Content-Encoding and
        // body bytes; asking for identity avoids reqwest's decompress errors on
        // those tracks while still returning raw audio.
        req = req.header(ACCEPT_ENCODING, "identity");
    }
    let response = req.send().context("connecting to stream URL")?;
    let status = response.status();
    let response = response
        .error_for_status()
        .with_context(|| format!("stream HTTP {status}"))?;
    let bytes = response
        .bytes()
        .context("reading stream body (connection dropped or truncated?)")?;
    Ok(bytes.to_vec())
}

fn read_cached_and_decode(path: &std::path::Path) -> Result<Decoder<Cursor<Vec<u8>>>> {
    let bytes =
        std::fs::read(path).with_context(|| format!("reading cached track {}", path.display()))?;
    build_symphonia_decoder(bytes)
}

fn build_symphonia_decoder(bytes: Vec<u8>) -> Result<Decoder<Cursor<Vec<u8>>>> {
    let byte_len = bytes.len() as u64;
    let cursor = Cursor::new(bytes);
    Decoder::builder()
        .with_data(cursor)
        .with_byte_len(byte_len)
        .with_coarse_seek(true)
        .build()
        .context("decoding audio (unsupported or corrupt file?)")
}

/// Live HTTP reader, optionally normalizing MPEG-2 ADTS for symphonia.
enum LiveDecodeReader {
    Raw(StreamingReader),
    Aac(AdtsNormalizeReader<StreamingReader>),
}

impl std::io::Read for LiveDecodeReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Raw(r) => r.read(out),
            Self::Aac(r) => r.read(out),
        }
    }
}

impl std::io::Seek for LiveDecodeReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Raw(r) => r.seek(pos),
            Self::Aac(r) => r.seek(pos),
        }
    }
}

/// Max time to probe/decode after the stream has connected and prebuffered.
const LIVE_DECODE_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

fn live_stream_and_decode(url: &str) -> Result<Decoder<LiveDecodeReader>> {
    let mut reader = open_live_stream(url)?;
    let decode_hint = reader.prepare_for_decode(url);
    let reader = if decode_hint.format == StreamFormatHint::Aac {
        LiveDecodeReader::Aac(AdtsNormalizeReader::new(reader))
    } else {
        LiveDecodeReader::Raw(reader)
    };

    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("ratune-live-decode".into())
        .spawn(move || {
            let mut builder = Decoder::builder().with_data(reader).with_coarse_seek(true);
            if let Some(ext) = decode_hint.extension_hint() {
                builder = builder.with_hint(ext);
            }
            if let Some(mime) = decode_hint.mime_hint() {
                builder = builder.with_mime_type(mime);
            }
            let result = builder
                .build()
                .context("decoding live stream (unsupported format?)");
            let _ = tx.send(result);
        })
        .context("failed to spawn live decode thread")?;

    match rx.recv_timeout(LIVE_DECODE_PROBE_TIMEOUT) {
        Ok(Ok(decoder)) => Ok(decoder),
        Ok(Err(e)) => Err(e),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(match decode_hint.format {
            StreamFormatHint::Aac => anyhow!(
                "AAC radio stream timed out during decode — server may be blocking or using an unsupported AAC variant"
            ),
            StreamFormatHint::Ogg => anyhow!(
                "OGG radio stream timed out during decode"
            ),
            StreamFormatHint::Unknown => anyhow!(
                "live stream timed out during decode (unsupported format or blocked URL)"
            ),
            StreamFormatHint::Mp3 => anyhow!(
                "MP3 stream timed out during decode — server may be slow or blocking this client"
            ),
        }),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(anyhow!("live stream decode thread exited unexpectedly"))
        }
    }
}

/// Whole-body download into RAM: the last-resort path when streaming decode fails.
///
/// with_byte_len: tells symphonia the exact byte length, which also sets
///   is_seekable = true internally.
/// with_coarse_seek: bypasses the time_base requirement for accurate seeking
///   (unavailable on transcoded MP3 streams); seeks to the nearest keyframe.
fn download_and_decode(url: &str) -> Result<Decoder<Cursor<Vec<u8>>>> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..4u32 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(200 * u64::from(attempt)));
        }
        // After a few normal tries (compressed transfer), ask for identity once.
        let identity = attempt == 3;
        match fetch_track_bytes(url, identity).and_then(build_symphonia_decoder) {
            Ok(decoder) => return Ok(decoder),
            Err(e) => {
                // Preserve retries for truncated bodies, HTTP failures, and decode
                // quirks, but fail fast for connect/timeout errors.
                let connectivity_failure = is_connectivity_failure(&e);
                last_err = Some(e);
                if connectivity_failure {
                    break;
                }
            }
        }
    }
    Err(last_err.expect("loop runs at least once"))
}
