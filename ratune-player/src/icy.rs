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
        let bytes = &block[..end];
        let text = match std::str::from_utf8(bytes) {
            Ok(text) => std::borrow::Cow::Borrowed(text),
            // Many SHOUTcast servers send Latin-1, where every byte is the same code point.
            Err(_) => std::borrow::Cow::Owned(bytes.iter().copied().map(char::from).collect()),
        };
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

    struct FragmentedReader<R> {
        inner: R,
        max_read: usize,
    }

    impl<R: Read> Read for FragmentedReader<R> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let limit = out.len().min(self.max_read);
            self.inner.read(&mut out[..limit])
        }
    }

    fn frame(audio: &[u8], metadata: &[u8]) -> Vec<u8> {
        let blocks = metadata.len().div_ceil(16);
        let mut bytes = audio.to_vec();
        bytes.push(u8::try_from(blocks).unwrap());
        bytes.extend_from_slice(metadata);
        bytes.resize(audio.len() + 1 + blocks * 16, 0);
        bytes
    }

    #[test]
    fn decodes_latin1_titles() {
        let body = [frame(b"a", b"StreamTitle='Beyonc\xe9';"), b"b".to_vec()].concat();
        let (tx, rx) = mpsc::channel();
        let mut reader = IcyReader::new(Cursor::new(body), 1, Some(tx));
        reader.read_to_end(&mut Vec::new()).unwrap();
        assert_eq!(rx.recv().unwrap(), "Beyonc\u{e9}");
    }

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

    #[test]
    fn handles_fragmented_audio_and_padded_metadata() {
        let body = [
            frame(b"abcd", b"StreamTitle='Artist - Track';"),
            b"efgh".to_vec(),
        ]
        .concat();

        for max_read in [1, 2, 3, 7] {
            let (tx, rx) = mpsc::channel();
            let source = FragmentedReader {
                inner: Cursor::new(&body),
                max_read,
            };
            let mut reader = IcyReader::new(source, 4, Some(tx));
            let mut audio = Vec::new();

            reader.read_to_end(&mut audio).unwrap();

            assert_eq!(audio, b"abcdefgh", "read size: {max_read}");
            assert_eq!(rx.try_iter().collect::<Vec<_>>(), ["Artist - Track"]);
        }
    }

    #[test]
    fn skips_zero_length_metadata_blocks() {
        let body = [
            frame(b"ab", b""),
            frame(b"cd", b"StreamTitle='X';"),
            b"ef".to_vec(),
        ]
        .concat();
        let (tx, rx) = mpsc::channel();
        let mut reader = IcyReader::new(Cursor::new(body), 2, Some(tx));
        let mut audio = Vec::new();

        reader.read_to_end(&mut audio).unwrap();

        assert_eq!(audio, b"abcdef");
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), ["X"]);
    }

    #[test]
    fn ignores_metadata_without_a_nonempty_title() {
        for metadata in [
            b"StreamTitle='';".as_slice(),
            b"StreamTitle='   ';".as_slice(),
            b"StreamUrl='https://radio.test';".as_slice(),
        ] {
            let body = [frame(b"ab", metadata), b"cd".to_vec()].concat();
            let (tx, rx) = mpsc::channel();
            let mut reader = IcyReader::new(Cursor::new(body), 2, Some(tx));
            let mut audio = Vec::new();

            reader.read_to_end(&mut audio).unwrap();

            assert_eq!(audio, b"abcd");
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn rejects_truncated_metadata_without_leaking_it_into_audio() {
        // The length byte announces 32 bytes, but only 16 follow.
        let body = b"ab\x02StreamTitle='X';";
        let (tx, rx) = mpsc::channel();
        let mut reader = IcyReader::new(Cursor::new(body), 2, Some(tx));
        let mut audio = Vec::new();

        let error = reader.read_to_end(&mut audio).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(audio, b"ab");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn emits_each_title_change_in_order() {
        let body = [
            frame(b"a", b"StreamTitle='A';"),
            frame(b"b", b"StreamTitle='B';"),
            frame(b"c", b"StreamTitle='B';"),
            frame(b"d", b"StreamTitle='A';"),
            b"e".to_vec(),
        ]
        .concat();
        let (tx, rx) = mpsc::channel();
        let mut reader = IcyReader::new(Cursor::new(body), 1, Some(tx));
        let mut audio = Vec::new();

        reader.read_to_end(&mut audio).unwrap();

        assert_eq!(audio, b"abcde");
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), ["A", "B", "A"]);
    }
}
