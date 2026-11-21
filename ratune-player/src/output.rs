//! Audio output device with fault detection and reopen-with-backoff.
//!
//! The [`Player`](rodio::Player) is not tied to a device: its queue output is
//! shared, so a failed device can be dropped and a new one opened without
//! losing the current track, its position, or the gapless queue.
//!
//! While no device is open, a null output keeps pulling the (paused) queue so
//! that player controls that wait on the audio thread (`try_seek`, `append`
//! after `stop`) still complete.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::queue::SourcesQueueOutput;
use rodio::{
    ChannelCount, CpalError, CpalErrorKind, DeviceSinkBuilder, DeviceSinkError, MixerDeviceSink,
    SampleRate, Source,
};

/// Recoverable stream errors (xruns etc.) only count as a fault when this many
/// arrive within [`ERROR_WINDOW`]. A stuck device reports thousands per second.
const ERRORS_PER_WINDOW: u32 = 20;
const ERROR_WINDOW: Duration = Duration::from_secs(1);
/// A reopened device must run this long before the outage counts as over.
const STABLE_AFTER: Duration = Duration::from_secs(5);
/// An open device pulls samples continuously (silence when paused or idle).
/// Some failures (e.g. the PipeWire node going away) stop it without an error.
const STALL_AFTER: Duration = Duration::from_secs(3);
/// Reopens after a loss before giving up: with the backoff, about 30 s.
const MAX_REOPEN_ATTEMPTS: u32 = 5;
const NULL_OUTPUT_TICK: Duration = Duration::from_millis(10);

/// Output state change for the engine to report.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OutputChange {
    /// No working device; playback is held while reopening with a backoff.
    Lost(String),
    /// Reopening failed [`MAX_REOPEN_ATTEMPTS`] times. No more attempts until
    /// [`Output::retry`].
    GaveUp(String),
    /// A device has been running without faults for [`STABLE_AFTER`].
    Restored,
}

pub(crate) struct Output {
    source: SharedSource,
    device: Option<OpenDevice>,
    null_output: Option<NullOutput>,
    /// Failed or short-lived opens since the device was last stable.
    attempts: u32,
    /// `None` after giving up.
    retry_at: Option<Instant>,
    /// A loss has been reported and the matching restore has not.
    outage: bool,
}

impl Output {
    /// Starts with no device; the first [`poll`](Self::poll) opens one.
    pub(crate) fn new(queue: SourcesQueueOutput) -> Self {
        let source = SharedSource::new(queue);
        Self {
            null_output: Some(NullOutput::start(source.clone())),
            source,
            device: None,
            attempts: 0,
            retry_at: Some(Instant::now()),
            outage: false,
        }
    }

    pub(crate) fn is_up(&self) -> bool {
        self.device.is_some()
    }

    /// Start a new round of reopen attempts if the last one gave up.
    pub(crate) fn retry(&mut self) {
        if self.device.is_none() && self.retry_at.is_none() {
            self.attempts = 0;
            self.retry_at = Some(Instant::now());
            self.outage = false;
        }
    }

    /// Detect a faulted device, and (re)open one when the backoff has elapsed.
    pub(crate) fn poll(&mut self) -> Option<OutputChange> {
        let now = Instant::now();
        if let Some(device) = &mut self.device {
            let reason = device.health.fault_reason().or_else(|| {
                device
                    .stalled(&self.source, now)
                    .then(|| "audio device stopped playing".to_owned())
            });
            if let Some(reason) = reason {
                let gave_up = self.close(now);
                return self.lost(reason, gave_up);
            }
            if now.duration_since(device.opened_at) >= STABLE_AFTER {
                self.attempts = 0;
                if self.outage {
                    self.outage = false;
                    return Some(OutputChange::Restored);
                }
            }
            return None;
        }
        if self.retry_at.is_none_or(|at| now < at) {
            return None;
        }
        // Only one consumer may pull the queue at a time.
        self.null_output = None;
        match OpenDevice::open(&self.source) {
            Ok(device) => {
                self.device = Some(device);
                None
            }
            Err(reason) => {
                let gave_up = self.close(now);
                self.lost(reason, gave_up)
            }
        }
    }

    /// Schedules the next attempt; returns `true` when giving up instead.
    fn close(&mut self, now: Instant) -> bool {
        // Dropping the sink joins the stream thread, so the queue is free afterwards.
        self.device = None;
        self.null_output = Some(NullOutput::start(self.source.clone()));
        if self.attempts >= MAX_REOPEN_ATTEMPTS {
            self.retry_at = None;
            return true;
        }
        self.retry_at = Some(now + backoff(self.attempts));
        self.attempts += 1;
        false
    }

    fn lost(&mut self, reason: String, gave_up: bool) -> Option<OutputChange> {
        let first = !self.outage;
        self.outage = true;
        if gave_up {
            Some(OutputChange::GaveUp(reason))
        } else {
            first.then_some(OutputChange::Lost(reason))
        }
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(1 << attempt)
}

// ── Device ────────────────────────────────────────────────────────────────────

struct OpenDevice {
    _sink: MixerDeviceSink,
    health: Arc<Health>,
    opened_at: Instant,
    stall: StallWatch,
}

impl OpenDevice {
    fn open(source: &SharedSource) -> Result<Self, String> {
        let health = Arc::new(Health::default());
        let mut sink = open_sink(error_callback(health.clone())).map_err(|e| describe(&e))?;
        sink.log_on_drop(false);
        sink.mixer().add(source.clone());
        let now = Instant::now();
        Ok(Self {
            _sink: sink,
            health,
            opened_at: now,
            stall: StallWatch {
                last_pulls: source.pulls(),
                last_pull_at: now,
            },
        })
    }

    fn stalled(&mut self, source: &SharedSource, now: Instant) -> bool {
        self.stall.check(source.pulls(), source.pulling(), now)
    }
}

struct StallWatch {
    last_pulls: u64,
    last_pull_at: Instant,
}

impl StallWatch {
    /// No samples pulled for [`STALL_AFTER`]. A pull blocked on a slow
    /// source (e.g. a stream waiting for the network) doesn't count.
    fn check(&mut self, pulls: u64, pulling: bool, now: Instant) -> bool {
        if pulls != self.last_pulls || pulling {
            self.last_pulls = pulls;
            self.last_pull_at = now;
            return false;
        }
        now.duration_since(self.last_pull_at) >= STALL_AFTER
    }
}

/// Same device selection as [`DeviceSinkBuilder::open_default_sink`], which
/// can't take a custom error callback: the default device, then any non-null one.
fn open_sink<E>(callback: E) -> Result<MixerDeviceSink, DeviceSinkError>
where
    E: FnMut(CpalError) + Send + Clone + 'static,
{
    DeviceSinkBuilder::from_default_device()
        .and_then(|b| {
            b.with_error_callback(callback.clone())
                .open_sink_or_fallback()
        })
        .or_else(|original| {
            let Ok(devices) = rodio::cpal::default_host().output_devices() else {
                return Err(original);
            };
            devices
                .filter(|dev| {
                    dev.description()
                        .map(|desc| desc.driver().is_some_and(|driver| driver != "null"))
                        .unwrap_or(false)
                })
                .find_map(|dev| {
                    DeviceSinkBuilder::from_device(dev)
                        .and_then(|b| {
                            b.with_error_callback(callback.clone())
                                .open_sink_or_fallback()
                        })
                        .ok()
                })
                .ok_or(original)
        })
}

fn describe(err: &dyn std::error::Error) -> String {
    let mut msg = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        msg.push_str(": ");
        msg.push_str(&cause.to_string());
        source = cause.source();
    }
    msg
}

/// Written by the stream's error callback, read by the player thread.
#[derive(Default)]
struct Health {
    faulted: AtomicBool,
    reason: Mutex<Option<String>>,
}

impl Health {
    fn fault(&self, reason: String) {
        *self.reason.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason);
        self.faulted.store(true, Ordering::Release);
    }

    fn is_faulted(&self) -> bool {
        self.faulted.load(Ordering::Acquire)
    }

    fn fault_reason(&self) -> Option<String> {
        if !self.is_faulted() {
            return None;
        }
        let reason = self.reason.lock().unwrap_or_else(PoisonError::into_inner);
        Some(
            reason
                .clone()
                .unwrap_or_else(|| "audio stream error".into()),
        )
    }
}

/// Runs on the audio thread, possibly thousands of times per second: no I/O.
fn error_callback(health: Arc<Health>) -> impl FnMut(CpalError) + Send + Clone + 'static {
    let mut window = ErrorWindow::default();
    move |err| {
        if health.is_faulted() {
            return;
        }
        let fatal = match classify(err.kind()) {
            ErrorClass::Fatal => true,
            ErrorClass::Transient => window.record(Instant::now()),
            ErrorClass::Ignored => false,
        };
        if fatal {
            health.fault(err.to_string());
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ErrorClass {
    /// The stream is dead and must be reopened.
    Fatal,
    /// Fatal only when repeated (see [`ErrorWindow`]).
    Transient,
    Ignored,
}

fn classify(kind: CpalErrorKind) -> ErrorClass {
    match kind {
        CpalErrorKind::DeviceNotAvailable | CpalErrorKind::StreamInvalidated => ErrorClass::Fatal,
        CpalErrorKind::RealtimeDenied => ErrorClass::Ignored,
        _ => ErrorClass::Transient,
    }
}

#[derive(Clone, Default)]
struct ErrorWindow {
    start: Option<Instant>,
    count: u32,
}

impl ErrorWindow {
    /// Returns `true` once [`ERRORS_PER_WINDOW`] errors fall within one window.
    fn record(&mut self, now: Instant) -> bool {
        match self.start {
            Some(start) if now.duration_since(start) < ERROR_WINDOW => self.count += 1,
            _ => {
                self.start = Some(now);
                self.count = 1;
            }
        }
        self.count >= ERRORS_PER_WINDOW
    }
}

// ── Shared queue ──────────────────────────────────────────────────────────────

/// The player's queue output, pulled by whichever device (or null output) is current.
#[derive(Clone)]
struct SharedSource {
    queue: Arc<Mutex<SourcesQueueOutput>>,
    pulls: Arc<AtomicU64>,
    pulling: Arc<AtomicBool>,
}

impl SharedSource {
    fn new(queue: SourcesQueueOutput) -> Self {
        Self {
            queue: Arc::new(Mutex::new(queue)),
            pulls: Arc::default(),
            pulling: Arc::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, SourcesQueueOutput> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn pulls(&self) -> u64 {
        self.pulls.load(Ordering::Relaxed)
    }

    fn pulling(&self) -> bool {
        self.pulling.load(Ordering::Relaxed)
    }
}

impl Iterator for SharedSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.pulling.store(true, Ordering::Relaxed);
        let sample = self.lock().next();
        self.pulling.store(false, Ordering::Relaxed);
        self.pulls.fetch_add(1, Ordering::Relaxed);
        sample
    }
}

impl Source for SharedSource {
    fn current_span_len(&self) -> Option<usize> {
        self.lock().current_span_len()
    }
    fn channels(&self) -> ChannelCount {
        self.lock().channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.lock().sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Pulls the queue in real time and discards the samples.
struct NullOutput {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl NullOutput {
    fn start(source: SharedSource) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = thread::Builder::new()
            .name("ratune-null-output".into())
            .spawn(move || {
                // Sleep first: the engine pauses the player right after a device is lost.
                while !sleep_unless_stopped(&flag) {
                    let mut queue = source.lock();
                    let per_tick = usize::from(queue.channels().get())
                        * queue.sample_rate().get() as usize
                        / (1000 / NULL_OUTPUT_TICK.as_millis() as usize);
                    for _ in 0..per_tick {
                        queue.next();
                    }
                }
            })
            .ok();
        Self { stop, handle }
    }
}

fn sleep_unless_stopped(stop: &AtomicBool) -> bool {
    thread::sleep(NULL_OUTPUT_TICK);
    stop.load(Ordering::Acquire)
}

impl Drop for NullOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::num::NonZero;
    use rodio::buffer::SamplesBuffer;
    use rodio::Player;

    #[test]
    fn device_loss_is_fatal_and_xruns_are_not() {
        assert_eq!(
            classify(CpalErrorKind::DeviceNotAvailable),
            ErrorClass::Fatal
        );
        assert_eq!(
            classify(CpalErrorKind::StreamInvalidated),
            ErrorClass::Fatal
        );
        assert_eq!(classify(CpalErrorKind::Xrun), ErrorClass::Transient);
        assert_eq!(classify(CpalErrorKind::BackendError), ErrorClass::Transient);
        assert_eq!(classify(CpalErrorKind::RealtimeDenied), ErrorClass::Ignored);
    }

    #[test]
    fn error_window_trips_only_on_a_burst() {
        let start = Instant::now();
        let mut window = ErrorWindow::default();
        // One xrun every 100 ms is a glitchy but working device.
        for i in 0..100 {
            assert!(!window.record(start + Duration::from_millis(100 * i)));
        }
        let burst = start + Duration::from_secs(60);
        let tripped = (0..ERRORS_PER_WINDOW)
            .map(|i| window.record(burst + Duration::from_micros(u64::from(i))));
        assert_eq!(tripped.filter(|&t| t).count(), 1);
    }

    #[test]
    fn gives_up_after_the_last_reopen_and_retries_on_request() {
        let (_player, queue) = Player::new();
        let mut output = Output::new(queue);
        let start = Instant::now();
        let mut delays = Vec::new();
        while !output.close(start) {
            delays.push(output.retry_at.expect("scheduled") - start);
        }
        let secs: Vec<u64> = delays.iter().map(Duration::as_secs).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16]);
        assert_eq!(
            output.lost("gone".into(), true),
            Some(OutputChange::GaveUp("gone".into()))
        );
        // Given up: polling doesn't try to open a device.
        assert_eq!(output.poll(), None);
        assert!(!output.is_up());

        output.retry();
        assert_eq!(output.attempts, 0);
        assert!(output.retry_at.is_some());
        assert_eq!(
            output.lost("gone".into(), false),
            Some(OutputChange::Lost("gone".into()))
        );
    }

    #[test]
    fn stall_needs_no_pulls_and_no_pull_in_progress() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut watch = StallWatch {
            last_pulls: 0,
            last_pull_at: start,
        };
        assert!(!watch.check(100, false, at(2_000)));
        assert!(!watch.check(100, false, at(4_000)));
        assert!(watch.check(100, false, at(5_000)));
        // Blocked inside a pull: the source is slow, not the device.
        assert!(!watch.check(100, true, at(9_000)));
        assert!(!watch.check(100, false, at(11_000)));
        assert!(watch.check(100, false, at(12_000)));
    }

    #[test]
    fn health_reports_first_fault() {
        let health = Arc::new(Health::default());
        let mut callback = error_callback(health.clone());
        assert_eq!(health.fault_reason(), None);
        callback(CpalError::from(CpalErrorKind::DeviceNotAvailable));
        let reason = health.fault_reason().expect("faulted");
        callback(CpalError::from(CpalErrorKind::Xrun));
        assert_eq!(health.fault_reason(), Some(reason));
    }

    #[test]
    fn null_output_runs_controls_without_advancing_a_paused_player() {
        let (player, queue) = Player::new();
        let source = SharedSource::new(queue);
        let _null = NullOutput::start(source);
        let ch = NonZero::new(2u16).expect("channels");
        let rate = NonZero::new(48_000u32).expect("rate");
        let ten_seconds = || SamplesBuffer::new(ch, rate, vec![0.1f32; 2 * 48_000 * 10]);

        player.pause();
        player.append(ten_seconds());
        player.try_seek(Duration::from_secs(3)).expect("seek");
        thread::sleep(Duration::from_millis(200));
        let pos = player.get_pos();
        assert!(
            pos >= Duration::from_secs(3) && pos < Duration::from_millis(3_050),
            "paused player moved to {pos:?}"
        );

        // `append` after `stop` waits for the stopped track to be drained.
        player.stop();
        player.append(ten_seconds());
        assert_eq!(player.len(), 1);
    }
}
