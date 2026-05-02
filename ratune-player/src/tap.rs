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
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        // Flush stale samples so the visualizer doesn't show pre-seek audio.
        if let Ok(mut buf) = self.buffer.try_lock() {
            buf.clear();
        }
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::num::NonZero;
    use rodio::buffer::SamplesBuffer;
    use rodio::Source;

    #[test]
    fn sample_tap_retains_at_most_4096_samples() {
        let buffer: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let ch = NonZero::new(1u16).expect("channels");
        let rate = NonZero::new(48_000u32).expect("rate");
        let data: Vec<f32> = (0..6_000).map(|i| (i as f32) * 0.0001).collect();
        let inner = SamplesBuffer::new(ch, rate, data);
        let mut tap = SampleTap::new(inner, buffer.clone());
        for _ in 0..6_000 {
            assert!(tap.next().is_some());
        }
        assert!(tap.next().is_none());

        let buf = buffer.lock().expect("lock");
        assert_eq!(buf.len(), 4_096, "ring buffer should cap at 4096");
    }

    #[test]
    fn try_seek_clears_buffer() {
        let buffer: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let ch = NonZero::new(1u16).expect("channels");
        let rate = NonZero::new(48_000u32).expect("rate");
        let inner = SamplesBuffer::new(ch, rate, vec![0.3f32; 100]);
        let mut tap = SampleTap::new(inner, buffer.clone());
        for _ in 0..50 {
            let _ = tap.next();
        }
        assert!(!buffer.lock().expect("lock").is_empty());
        let _ = tap.try_seek(Duration::from_millis(0));
        assert_eq!(buffer.lock().expect("lock after seek").len(), 0);
    }
}
