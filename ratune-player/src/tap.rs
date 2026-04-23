use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, SampleRate, Source};

pub struct SampleTap<S: Source<Item = f32>> {
    inner: S,
    buffer: Arc<Mutex<VecDeque<f32>>>,
}

impl<S: Source<Item = f32>> SampleTap<S> {
    pub fn new(inner: S, buffer: Arc<Mutex<VecDeque<f32>>>) -> Self {
        Self { inner, buffer }
    }
}

impl<S: Source<Item = f32>> Iterator for SampleTap<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;
        if let Ok(mut buf) = self.buffer.try_lock() {
            buf.push_back(sample);
            if buf.len() > 4096 {
                buf.pop_front();
            }
        }
        Some(sample)
    }
}

impl<S: Source<Item = f32>> Source for SampleTap<S> {
    fn current_span_len(&self) -> Option<usize> { self.inner.current_span_len() }
    fn channels(&self) -> ChannelCount { self.inner.channels() }
    fn sample_rate(&self) -> SampleRate { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        // Flush stale samples so the visualizer doesn't show pre-seek audio.
        if let Ok(mut buf) = self.buffer.try_lock() {
            buf.clear();
        }
        self.inner.try_seek(pos)
    }
}
