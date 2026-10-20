//! Library-track source that can start playing before its download finishes.
//!
//! rodio fixes a decoder's seekability and byte length when it is built, and
//! symphonia needs the byte length to seek. So:
//!
//! - With `Content-Length`, the decoder is seekable from the start, but seeks
//!   are refused while the target is estimated to lie beyond the downloaded
//!   bytes (reads there would stall the audio thread until the download
//!   catches up).
//! - Without it, the decoder starts unseekable. On the first seek after the
//!   download completes, a seekable decoder is built over the full buffer and
//!   swapped in.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rodio::source::SeekError;
use rodio::{ChannelCount, Decoder, SampleRate, Source};

use crate::stream::StreamingReader;

/// Slack added to the estimated byte offset of a seek target before checking it
/// against the downloaded length (bitrate varies within a track).
const SEEK_BUFFER_MARGIN: u64 = 256 * 1024;

/// Seek refused because the target has not been downloaded yet.
#[derive(Debug)]
pub struct NotBuffered;

impl std::fmt::Display for NotBuffered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("seek target not downloaded yet")
    }
}

impl std::error::Error for NotBuffered {}

/// `true` when `err` came from a [`TrackSource`] refusing an unbuffered seek.
#[must_use]
pub fn is_not_buffered(err: &SeekError) -> bool {
    matches!(err, SeekError::Other(inner) if inner.downcast_ref::<NotBuffered>().is_some())
}

enum TrackDecoder {
    Memory(Decoder<Cursor<Vec<u8>>>),
    Stream(Decoder<StreamingReader>),
}

pub struct TrackSource {
    decoder: TrackDecoder,
    /// Download behind a streamed decoder, until it has completed and the decoder is seekable.
    stream: Option<StreamingReader>,
    /// `decoder` was built with a byte length, so symphonia can seek it.
    seekable: bool,
    /// HTTP download this track decodes from (`None` for in-memory sources).
    download: Option<StreamingReader>,
}

impl TrackSource {
    /// Fully in-memory track (cache file or whole-body download).
    #[must_use]
    pub fn from_memory(decoder: Decoder<Cursor<Vec<u8>>>) -> Self {
        Self {
            decoder: TrackDecoder::Memory(decoder),
            stream: None,
            seekable: true,
            download: None,
        }
    }

    /// Decode while `reader` is still downloading.
    pub fn from_stream(reader: StreamingReader) -> Result<Self> {
        let len = reader.content_length();
        let stream = reader.reopen();
        let download = reader.reopen();
        let decoder = build_stream_decoder(reader, len)?;
        Ok(Self {
            decoder: TrackDecoder::Stream(decoder),
            stream: Some(stream),
            seekable: len.is_some(),
            download: Some(download),
        })
    }

    /// Wait for `reader` to finish downloading, then decode it with a known length.
    ///
    /// For formats symphonia can't probe without seeking (e.g. MP4 with the index
    /// at the end) when the server sent no `Content-Length`.
    pub fn from_completed_stream(reader: StreamingReader) -> Result<Self> {
        let len = reader.wait_complete()?;
        let download = reader.reopen();
        let decoder = build_stream_decoder(reader, Some(len))?;
        Ok(Self {
            decoder: TrackDecoder::Stream(decoder),
            stream: None,
            seekable: true,
            download: Some(download),
        })
    }

    /// Another handle to the HTTP download behind this track, if it has one.
    #[must_use]
    pub fn download(&self) -> Option<StreamingReader> {
        self.download.as_ref().map(StreamingReader::reopen)
    }

    fn target_is_buffered(&self, stream: &StreamingReader, pos: Duration) -> bool {
        let (Some(total_len), Some(total_time)) = (stream.content_length(), self.total_duration())
        else {
            return false;
        };
        if total_time.is_zero() {
            return false;
        }
        let fraction = (pos.as_secs_f64() / total_time.as_secs_f64()).clamp(0.0, 1.0);
        let estimate = (fraction * total_len as f64) as u64;
        estimate.saturating_add(SEEK_BUFFER_MARGIN).min(total_len) <= stream.buffered_len()
    }
}

fn build_stream_decoder(
    reader: StreamingReader,
    len: Option<u64>,
) -> Result<Decoder<StreamingReader>> {
    let mut builder = Decoder::builder().with_data(reader).with_coarse_seek(true);
    if let Some(len) = len {
        builder = builder.with_byte_len(len);
    }
    builder
        .build()
        .context("decoding audio (unsupported or corrupt file?)")
}

macro_rules! with_decoder {
    ($self:expr, $d:ident => $body:expr) => {
        match $self {
            TrackDecoder::Memory($d) => $body,
            TrackDecoder::Stream($d) => $body,
        }
    };
}

impl Iterator for TrackSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        with_decoder!(&mut self.decoder, d => d.next())
    }
}

impl Source for TrackSource {
    fn current_span_len(&self) -> Option<usize> {
        with_decoder!(&self.decoder, d => d.current_span_len())
    }
    fn channels(&self) -> ChannelCount {
        with_decoder!(&self.decoder, d => d.channels())
    }
    fn sample_rate(&self) -> SampleRate {
        with_decoder!(&self.decoder, d => d.sample_rate())
    }
    fn total_duration(&self) -> Option<Duration> {
        with_decoder!(&self.decoder, d => d.total_duration())
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let Some(stream) = &self.stream else {
            return with_decoder!(&mut self.decoder, d => d.try_seek(pos));
        };
        if stream.is_complete() {
            if !self.seekable {
                let len = stream.buffered_len();
                let mut decoder =
                    build_stream_decoder(stream.reopen(), Some(len)).map_err(|e| {
                        SeekError::Other(Arc::new(std::io::Error::other(e.to_string())))
                    })?;
                decoder.try_seek(pos)?;
                self.decoder = TrackDecoder::Stream(decoder);
                self.seekable = true;
                self.stream = None;
                return Ok(());
            }
            self.stream = None;
            return with_decoder!(&mut self.decoder, d => d.try_seek(pos));
        }
        if self.seekable && self.target_is_buffered(stream, pos) {
            return with_decoder!(&mut self.decoder, d => d.try_seek(pos));
        }
        Err(SeekError::Other(Arc::new(NotBuffered)))
    }
}
