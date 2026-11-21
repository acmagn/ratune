//! Daemon log file: stdout and stderr (including output from C libraries such
//! as ALSA) go through a pipe to a thread that timestamps each line, collapses
//! repeats, and rotates the file so it stays bounded.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

/// The live log rotates to `<name>.1` past this size, so at most twice this is kept.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
/// Longer lines (or output without newlines) are split.
const MAX_LINE_BYTES: usize = 8 * 1024;
/// Repeats are summarized when the output goes quiet this long...
const IDLE_FLUSH: Duration = Duration::from_secs(1);
/// ...or, while they keep coming, this often.
const REPEAT_FLUSH: Duration = Duration::from_secs(60);

/// Written through the pipe by [`sync`]; never logged.
const SYNC_MARKER: &str = "\u{1}ratune-log-sync";
const SYNC_TIMEOUT: Duration = Duration::from_secs(2);
const THREAD_NAME: &str = "ratune-log";

/// Markers the log thread has reached.
#[derive(Default)]
struct Synced {
    count: Mutex<u64>,
    reached: Condvar,
}

static SYNCED: OnceLock<Arc<Synced>> = OnceLock::new();

/// Redirect stdout and stderr into the log at `path`.
pub fn install(path: &Path) -> Result<()> {
    let log =
        LogFile::open(path.to_path_buf()).with_context(|| format!("opening {}", path.display()))?;
    let (read_end, write_end) = pipe().context("creating log pipe")?;
    let synced = SYNCED.get_or_init(Default::default).clone();
    thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || pump(File::from(read_end), log, &synced))
        .context("log thread")?;
    unsafe {
        libc::dup2(write_end.as_raw_fd(), libc::STDOUT_FILENO);
        libc::dup2(write_end.as_raw_fd(), libc::STDERR_FILENO);
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        sync();
    }));
    Ok(())
}

/// Wait until everything written to stdout/stderr so far is in the log file.
/// Call before the process exits: unread pipe contents are lost.
pub fn sync() {
    let Some(synced) = SYNCED.get() else {
        return;
    };
    if thread::current().name() == Some(THREAD_NAME) {
        return;
    }
    // Not held across the write: the log thread needs it to drain earlier markers.
    let target = *synced.count.lock().unwrap_or_else(PoisonError::into_inner) + 1;
    let marker = format!("{SYNC_MARKER}\n");
    let written = unsafe { libc::write(libc::STDERR_FILENO, marker.as_ptr().cast(), marker.len()) };
    if written != marker.len() as isize {
        return;
    }
    let count = synced.count.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = synced
        .reached
        .wait_timeout_while(count, SYNC_TIMEOUT, |n| *n < target);
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds: [RawFd; 2] = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    for fd in fds {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Must never exit while the process runs: writes to a pipe with no reader
/// make `eprintln!` panic.
fn pump(mut pipe: File, mut log: LogFile, synced: &Synced) {
    let mut collapser = Collapser::default();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        if !wait_readable(&pipe, IDLE_FLUSH) {
            collapser.flush(&mut |line| log.write_line(line));
            continue;
        }
        let n = match pipe.read(&mut buf) {
            Ok(0) => {
                let mut emit = |line: &str| log.write_line(line);
                if !pending.is_empty() {
                    let rest = String::from_utf8_lossy(&pending).into_owned();
                    collapser.feed(&rest, Instant::now(), &mut emit);
                }
                collapser.flush(&mut emit);
                return;
            }
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                thread::sleep(IDLE_FLUSH);
                continue;
            }
        };
        pending.extend_from_slice(&buf[..n]);
        let now = Instant::now();
        let mut emit = |line: &str| log.write_line(line);
        while let Some(line) = take_line(&mut pending) {
            if line == SYNC_MARKER {
                collapser.flush(&mut emit);
                *synced.count.lock().unwrap_or_else(PoisonError::into_inner) += 1;
                synced.reached.notify_all();
            } else {
                collapser.feed(&line, now, &mut emit);
            }
        }
    }
}

fn wait_readable(file: &File, timeout: Duration) -> bool {
    let mut pfd = libc::pollfd {
        fd: file.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = timeout.as_millis() as libc::c_int;
    unsafe { libc::poll(&mut pfd, 1, ms) > 0 }
}

/// Next complete line from `pending`, without its newline.
fn take_line(pending: &mut Vec<u8>) -> Option<String> {
    let end = match pending.iter().position(|&b| b == b'\n') {
        Some(i) if i <= MAX_LINE_BYTES => i,
        _ if pending.len() > MAX_LINE_BYTES => MAX_LINE_BYTES,
        _ => return None,
    };
    let line: Vec<u8> = pending.drain(..end).collect();
    if pending.first() == Some(&b'\n') {
        pending.remove(0);
    }
    Some(
        String::from_utf8_lossy(&line)
            .trim_end_matches('\r')
            .to_owned(),
    )
}

/// Collapses runs of identical lines into "last message repeated N times".
#[derive(Default)]
struct Collapser {
    last: Option<String>,
    repeats: u64,
    repeats_since: Option<Instant>,
}

impl Collapser {
    fn feed(&mut self, line: &str, now: Instant, emit: &mut impl FnMut(&str)) {
        if self.last.as_deref() == Some(line) {
            self.repeats += 1;
            let since = *self.repeats_since.get_or_insert(now);
            if now.duration_since(since) >= REPEAT_FLUSH {
                self.flush(emit);
            }
            return;
        }
        self.flush(emit);
        emit(line);
        self.last = Some(line.to_owned());
    }

    fn flush(&mut self, emit: &mut impl FnMut(&str)) {
        if self.repeats > 0 {
            emit(&format!("last message repeated {} times", self.repeats));
        }
        self.repeats = 0;
        self.repeats_since = None;
    }
}

struct LogFile {
    path: PathBuf,
    file: Option<File>,
    len: u64,
}

impl LogFile {
    fn open(path: PathBuf) -> io::Result<Self> {
        let rotated = rotated_path(&path);
        let mut trimmed = Vec::new();
        // Logs from before rotation existed could grow without bound (#110).
        for p in [&rotated, &path] {
            if let Some(len) = fs::metadata(p).ok().map(|m| m.len()) {
                if len > 2 * MAX_LOG_BYTES {
                    keep_tail(p, &rotated);
                    trimmed.push(len);
                }
            }
        }
        let mut log = Self {
            path,
            file: None,
            len: 0,
        };
        if fs::metadata(&log.path).is_ok_and(|m| m.len() >= MAX_LOG_BYTES) {
            log.rotate();
        } else {
            log.reopen()?;
        }
        for len in trimmed {
            log.write_line(&format!(
                "trimmed an oversized old log ({}) to its last {}",
                human_size(len),
                human_size(MAX_LOG_BYTES)
            ));
        }
        Ok(log)
    }

    fn reopen(&mut self) -> io::Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.len = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }

    /// On failure the file stays closed and lines are dropped, keeping the cap.
    fn rotate(&mut self) {
        self.file = None;
        if fs::rename(&self.path, rotated_path(&self.path)).is_ok() {
            let _ = self.reopen();
        }
    }

    fn write_line(&mut self, line: &str) {
        if self.len >= MAX_LOG_BYTES {
            self.rotate();
        }
        let Some(file) = &mut self.file else {
            return;
        };
        let entry = format!("{} {line}\n", timestamp());
        if file.write_all(entry.as_bytes()).is_ok() {
            self.len += entry.len() as u64;
        }
    }
}

fn rotated_path(path: &Path) -> PathBuf {
    let mut rotated = path.to_path_buf().into_os_string();
    rotated.push(".1");
    rotated.into()
}

/// Replace `to` with the last [`MAX_LOG_BYTES`] of `from` (from a line
/// start), then delete `from` if it's a different file. `from` is deleted
/// even if the copy fails: a full disk is likely when the log is huge.
fn keep_tail(from: &Path, to: &Path) {
    let mut tmp = to.to_path_buf().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let copied = (|| -> io::Result<()> {
        let mut src = File::open(from)?;
        let len = src.metadata()?.len();
        src.seek(SeekFrom::Start(len.saturating_sub(MAX_LOG_BYTES)))?;
        let mut tail = Vec::new();
        src.take(MAX_LOG_BYTES).read_to_end(&mut tail)?;
        let start = tail.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1);
        fs::write(&tmp, &tail[start..])?;
        fs::rename(&tmp, to)
    })();
    if copied.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    if from != to {
        let _ = fs::remove_file(from);
    }
}

fn human_size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= 1024 * MB {
        format!("{:.1} GB", bytes as f64 / (1024 * MB) as f64)
    } else {
        format!("{} MB", bytes / MB)
    }
}

/// Local time, e.g. `2026-11-13 20:55:26.123`.
fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return format!("@{}.{:03}", now.as_secs(), now.subsec_millis());
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        now.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(collapser: &mut Collapser, lines: &[(&str, u64)]) -> Vec<String> {
        let start = Instant::now();
        let mut out = Vec::new();
        for (line, at) in lines {
            let now = start + Duration::from_secs(*at);
            collapser.feed(line, now, &mut |l| out.push(l.to_owned()));
        }
        out
    }

    #[test]
    fn collapses_repeats_until_a_different_line() {
        let mut c = Collapser::default();
        let out = feed_all(&mut c, &[("a", 0), ("b", 0), ("b", 0), ("b", 0), ("c", 0)]);
        assert_eq!(out, ["a", "b", "last message repeated 2 times", "c"]);
    }

    #[test]
    fn flush_reports_pending_repeats_once() {
        let mut c = Collapser::default();
        let mut out = feed_all(&mut c, &[("x", 0), ("x", 0)]);
        c.flush(&mut |l| out.push(l.to_owned()));
        c.flush(&mut |l| out.push(l.to_owned()));
        assert_eq!(out, ["x", "last message repeated 1 times"]);
    }

    #[test]
    fn long_runs_of_repeats_are_summarized_periodically() {
        let mut c = Collapser::default();
        let lines: Vec<(&str, u64)> = (0..=130).map(|s| ("spin", s)).collect();
        let out = feed_all(&mut c, &lines);
        assert_eq!(
            out,
            [
                "spin",
                "last message repeated 61 times",
                "last message repeated 61 times"
            ]
        );
    }

    #[test]
    fn splits_lines_and_caps_their_length() {
        let mut pending = b"one\r\ntwo\npart".to_vec();
        assert_eq!(take_line(&mut pending).as_deref(), Some("one"));
        assert_eq!(take_line(&mut pending).as_deref(), Some("two"));
        assert_eq!(take_line(&mut pending), None);
        assert_eq!(pending, b"part");

        let mut long = vec![b'z'; MAX_LINE_BYTES + 10];
        assert_eq!(take_line(&mut long).map(|l| l.len()), Some(MAX_LINE_BYTES));
        assert_eq!(long.len(), 10);
    }

    #[test]
    fn rotates_past_the_size_cap() {
        let dir = std::env::temp_dir().join(format!("ratune-log-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.log");
        let rotated = dir.join("daemon.log.1");
        fs::write(&path, vec![b'.'; MAX_LOG_BYTES as usize]).unwrap();

        let mut log = LogFile::open(path.clone()).unwrap();
        assert_eq!(fs::metadata(&rotated).unwrap().len(), MAX_LOG_BYTES);
        log.write_line("hello");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with(" hello\n"), "{text:?}");

        log.len = MAX_LOG_BYTES;
        log.write_line("next");
        assert!(fs::read_to_string(&rotated).unwrap().ends_with(" hello\n"));
        assert!(fs::read_to_string(&path).unwrap().ends_with(" next\n"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trims_runaway_logs_to_their_tail() {
        let dir = std::env::temp_dir().join(format!("ratune-trim-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.log");
        let rotated = dir.join("daemon.log.1");
        let huge = |p: &Path, last: &str| {
            let mut f = File::create(p).unwrap();
            f.set_len(3 * MAX_LOG_BYTES).unwrap();
            f.seek(SeekFrom::End(0)).unwrap();
            write!(f, "\nold line\n{last}\n").unwrap();
        };
        huge(&rotated, "end of rotated");
        huge(&path, "end of live");

        let mut log = LogFile::open(path.clone()).unwrap();
        log.write_line("fresh");

        let kept = fs::read(&rotated).unwrap();
        assert!(kept.len() as u64 <= MAX_LOG_BYTES);
        assert!(kept.ends_with(b"old line\nend of live\n"));
        assert!(!dir.join("daemon.log.1.tmp").exists());
        let live = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = live
            .lines()
            .map(|l| l.splitn(3, ' ').nth(2).unwrap())
            .collect();
        assert_eq!(
            lines,
            [
                "trimmed an oversized old log (15 MB) to its last 5 MB",
                "trimmed an oversized old log (15 MB) to its last 5 MB",
                "fresh"
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pump_logs_pipe_output_and_acknowledges_sync_markers() {
        let dir = std::env::temp_dir().join(format!("ratune-pump-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.log");
        let _ = fs::remove_file(&path);
        let (read_end, write_end) = pipe().unwrap();
        let log = LogFile::open(path.clone()).unwrap();
        let synced = Arc::new(Synced::default());
        let pump_synced = synced.clone();
        let pump = thread::spawn(move || pump(File::from(read_end), log, &pump_synced));

        let mut writer = File::from(write_end);
        let spam = "audio stream error: POLLERR\n".repeat(1000);
        write!(writer, "start\n{spam}{SYNC_MARKER}\nlast").unwrap();
        drop(writer);
        pump.join().unwrap();

        assert_eq!(*synced.count.lock().unwrap(), 1);
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text
            .lines()
            .map(|l| l.splitn(3, ' ').nth(2).unwrap())
            .collect();
        assert_eq!(
            lines,
            [
                "start",
                "audio stream error: POLLERR",
                "last message repeated 999 times",
                "last"
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn timestamp_has_date_and_millis() {
        let ts = timestamp();
        assert_eq!(ts.len(), "2026-11-13 20:55:26.123".len(), "{ts}");
    }
}
