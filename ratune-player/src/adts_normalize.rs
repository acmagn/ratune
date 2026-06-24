//! Rewrite MPEG-2 ADTS sync (`0xFF 0xF9`) to MPEG-4 (`0xFF 0xF1`) on the fly.
//!
//! Symphonia's ADTS demuxer only scans for `0xFFF1`. Browser decoders and ffmpeg
//! accept both; this adapter bridges the gap for live radio.

use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};

const ADTS_HEADER_LEN: usize = 7;

/// Pass-through reader that normalizes MPEG-2 ADTS headers for symphonia.
pub struct AdtsNormalizeReader<R> {
    inner: R,
    /// AAC payload bytes remaining for the current ADTS frame (after the 7-byte header).
    payload_remaining: usize,
    pending: VecDeque<u8>,
}

impl<R> AdtsNormalizeReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            payload_remaining: 0,
            pending: VecDeque::new(),
        }
    }

    fn reset_frame_state(&mut self) {
        self.payload_remaining = 0;
    }
}

/// ADTS frame length from a 7-byte header (protection-absent frames only).
fn adts_frame_length(header: &[u8; ADTS_HEADER_LEN]) -> Option<usize> {
    if header[0] != 0xFF || (header[1] & 0xF0) != 0xF0 {
        return None;
    }
    let len = ((header[3] as usize & 0x03) << 11)
        | ((header[4] as usize) << 3)
        | (header[5] as usize >> 5);
    (len >= ADTS_HEADER_LEN).then_some(len)
}

fn normalize_sync_byte(b: u8) -> u8 {
    if b == 0xF9 {
        0xF1
    } else {
        b
    }
}

impl<R: Read> AdtsNormalizeReader<R> {
    fn read_next_header(&mut self) -> std::io::Result<()> {
        let mut header = [0u8; ADTS_HEADER_LEN];
        self.inner.read_exact(&mut header[..2])?;
        header[1] = normalize_sync_byte(header[1]);
        self.inner.read_exact(&mut header[2..])?;
        let frame_len = adts_frame_length(&header).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid ADTS header")
        })?;
        self.payload_remaining = frame_len - ADTS_HEADER_LEN;
        self.pending.extend(header);
        Ok(())
    }

    fn fill_pending(&mut self) -> std::io::Result<()> {
        while self.payload_remaining == 0 && self.pending.is_empty() {
            self.read_next_header()?;
        }
        Ok(())
    }
}

impl<R: Read> Read for AdtsNormalizeReader<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut written = 0usize;
        while written < out.len() {
            while !self.pending.is_empty() && written < out.len() {
                out[written] = self.pending.pop_front().expect("non-empty");
                written += 1;
            }
            if written >= out.len() {
                break;
            }
            if self.payload_remaining > 0 {
                let want = out.len() - written;
                let take = want.min(self.payload_remaining);
                let n = self.inner.read(&mut out[written..written + take])?;
                if n == 0 {
                    return if written == 0 { Ok(0) } else { Ok(written) };
                }
                written += n;
                self.payload_remaining -= n;
                continue;
            }
            match self.fill_pending() {
                Ok(()) => {}
                Err(_) if written > 0 => return Ok(written),
                Err(e) => return Err(e),
            }
        }
        Ok(written)
    }
}

impl<R: Seek + Read> Seek for AdtsNormalizeReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.reset_frame_state();
        self.pending.clear();
        self.inner.seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rewrites_mpeg2_sync_to_mpeg4() {
        // Minimal valid-ish ADTS frame: 7-byte header + 4 payload bytes.
        let mut raw = vec![0xFF, 0xF9, 0x50, 0x80, 0x03, 0xE0, 0x00, 1, 2, 3, 4];
        // Fix frame length field to 11 (7 header + 4 payload): bits in bytes 3-5
        raw[3] = 0x00;
        raw[4] = 0x01;
        raw[5] = 0x58;

        let mut norm = AdtsNormalizeReader::new(Cursor::new(raw));
        let mut out = vec![0u8; 11];
        let n = norm.read(&mut out).unwrap();
        assert_eq!(out[0], 0xFF);
        assert_eq!(out[1], 0xF1, "MPEG-2 sync should be rewritten to MPEG-4");
        assert!(n >= 7, "expected at least ADTS header, got {n} bytes");
    }
}
