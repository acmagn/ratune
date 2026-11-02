use std::io::{self, Read};
use std::sync::mpsc::Sender;

pub(crate) struct IcyReader<R> {
    inner: R,
    interval: usize,
    audio_remaining: usize,
    metadata_tx: Option<Sender<String>>,
    last_title: Option<String>,
}

impl<R> IcyReader<R> {
    pub(crate) fn new(inner: R, interval: usize, metadata_tx: Option<Sender<String>>) -> Self {
        Self {
            inner,
            interval,
            audio_remaining: interval,
            metadata_tx,
            last_title: None,
        }
    }
}

impl<R: Read> IcyReader<R> {
    fn read_metadata(&mut self) -> io::Result<bool> {
        let mut length = [0u8; 1];
        if self.inner.read(&mut length)? == 0 {
            return Ok(false);
        }

        let length = usize::from(length[0]) * 16;
        if length == 0 {
            return Ok(true);
        }

        let mut block = vec![0u8; length];
        self.inner.read_exact(&mut block)?;
        let end = block.iter().position(|&byte| byte == 0).unwrap_or(length);
        let text = String::from_utf8_lossy(&block[..end]);
        let Some(title) = stream_title(&text).filter(|title| !title.is_empty()) else {
            return Ok(true);
        };
        if self.last_title.as_deref() == Some(title) {
            return Ok(true);
        }

        self.last_title = Some(title.to_owned());
        if let Some(tx) = &self.metadata_tx {
            let _ = tx.send(title.to_owned());
        }
        Ok(true)
    }
}

impl<R: Read> Read for IcyReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.audio_remaining == 0 {
            if !self.read_metadata()? {
                return Ok(0);
            }
            self.audio_remaining = self.interval;
        }

        let limit = out.len().min(self.audio_remaining);
        let read = self.inner.read(&mut out[..limit])?;
        self.audio_remaining -= read;
        Ok(read)
    }
}

fn stream_title(metadata: &str) -> Option<&str> {
    let value = metadata.split_once("StreamTitle=")?.1.trim_start();
    let Some(quote) = value.chars().next().filter(|c| matches!(c, '\'' | '"')) else {
        return Some(value.split(';').next()?.trim());
    };
    let value = &value[quote.len_utf8()..];
    let end = value
        .find(&format!("{quote};"))
        .or_else(|| value.rfind(quote))?;
    Some(value[..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::mpsc;

    #[test]
    fn strips_metadata_and_ignores_repeated_titles() {
        let body = b"a\x01StreamTitle='X';b\x01StreamTitle='X';c";
        let (tx, rx) = mpsc::channel();
        let mut reader = IcyReader::new(Cursor::new(body), 1, Some(tx));
        let mut audio = Vec::new();

        reader.read_to_end(&mut audio).unwrap();
        drop(reader);

        assert_eq!(audio, b"abc");
        assert_eq!(rx.recv().unwrap(), "X");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn keeps_apostrophes_in_titles() {
        assert_eq!(
            stream_title("StreamTitle='Guns N' Roses - Don't Cry';"),
            Some("Guns N' Roses - Don't Cry")
        );
    }
}
