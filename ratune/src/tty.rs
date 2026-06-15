//! Detect when the controlling terminal has closed (PTY hangup).
//!
//! `isatty` and stdout write errors are enough on some emulators (e.g. Alacritty).
//! Kitty and Ghostty often keep the slave PTY open with `isatty` still true while
//! crossterm spins on `read(0) → EIO`; `poll(POLLHUP)` catches that case.

use std::io;
use std::io::IsTerminal;

/// I/O errors that mean the terminal session is gone.
pub fn io_disconnect(err: &io::Error) -> bool {
    if matches!(
        err.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::NotConnected
            | io::ErrorKind::WriteZero
    ) {
        return true;
    }
    #[cfg(unix)]
    if err.raw_os_error() == Some(libc::EIO) {
        return true;
    }
    false
}

/// Signal interrupted a syscall (e.g. SIGWINCH during `poll` on resize). Retry, don't quit.
pub fn io_interrupted(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted
        || (cfg!(unix) && err.raw_os_error() == Some(libc::EINTR))
}

/// True when stdin/stdout no longer represent a live terminal session.
pub fn terminal_disconnected() -> bool {
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return true;
    }
    #[cfg(unix)]
    {
        pty_hung_up()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(unix)]
fn poll_fd(pfd: &mut libc::pollfd, timeout: i32) -> Result<i32, io::Error> {
    loop {
        let ret = unsafe { libc::poll(pfd, 1, timeout) };
        if ret < 0 {
            let e = io::Error::last_os_error();
            if io_interrupted(&e) {
                continue;
            }
            return Err(e);
        }
        return Ok(ret);
    }
}

#[cfg(unix)]
fn fionread(fd: std::os::unix::io::RawFd) -> Result<i32, io::Error> {
    loop {
        let mut queued = 0i32;
        let rc = unsafe { libc::ioctl(fd, libc::FIONREAD, &mut queued as *mut _) };
        if rc < 0 {
            let e = io::Error::last_os_error();
            if io_interrupted(&e) {
                continue;
            }
            return Err(e);
        }
        return Ok(queued);
    }
}

#[cfg(unix)]
fn pty_hung_up() -> bool {
    use std::os::unix::io::AsRawFd;

    pty_fd_hung_up(io::stdin().as_raw_fd()) || pty_fd_hung_up(io::stdout().as_raw_fd())
}

#[cfg(unix)]
fn pty_fd_hung_up(fd: std::os::unix::io::RawFd) -> bool {
    if fd < 0 {
        return true;
    }
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN | libc::POLLOUT | libc::POLLHUP | libc::POLLERR,
        revents: 0,
    };
    match poll_fd(&mut pfd, 0) {
        Ok(_) => {}
        Err(e) => return io_disconnect(&e),
    }
    if pfd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
        return true;
    }
    // Linux PTY master close often sets POLLIN with a pending zero-byte read (EOF).
    if pfd.revents & libc::POLLIN != 0 && pty_fd_eof_pending(fd) {
        return true;
    }
    false
}

#[cfg(unix)]
fn pty_fd_eof_pending(fd: std::os::unix::io::RawFd) -> bool {
    match fionread(fd) {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => io_disconnect(&e),
    }
}

/// Block until `poll_ms` elapses or stdin has input. Uses `poll(2)` on Unix so we
/// detect PTY hangup before crossterm's reader (which can spin on `EIO` in Kitty/Ghostty).
pub fn wait_for_input(poll_ms: u64, should_quit: &mut bool) -> anyhow::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        if terminal_disconnected() {
            *should_quit = true;
            return Ok(false);
        }

        let fd = io::stdin().as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        };
        let timeout = poll_ms.min(i32::MAX as u64) as i32;
        match poll_fd(&mut pfd, timeout) {
            Ok(_) => {}
            Err(e) if io_disconnect(&e) => {
                *should_quit = true;
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        }
        if pfd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            *should_quit = true;
            return Ok(false);
        }
        if pfd.revents & libc::POLLIN != 0 {
            if pty_fd_eof_pending(fd) {
                *should_quit = true;
                return Ok(false);
            }
            return Ok(true);
        }
        Ok(false)
    }
    #[cfg(not(unix))]
    {
        use crossterm::event;
        use std::time::Duration;

        if terminal_disconnected() {
            *should_quit = true;
            return Ok(false);
        }
        match event::poll(Duration::from_millis(poll_ms)) {
            Err(e) if io_disconnect(&e) => {
                *should_quit = true;
                Ok(false)
            }
            Err(e) if io_interrupted(&e) => Ok(false),
            Err(e) => Err(e.into()),
            Ok(ready) => Ok(ready),
        }
    }
}

/// Non-blocking check for more stdin data (burst keypresses).
pub fn stdin_has_input() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        if terminal_disconnected() {
            return false;
        }
        let fd = io::stdin().as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ret = match poll_fd(&mut pfd, 0) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if ret <= 0 {
            return false;
        }
        pfd.revents & libc::POLLIN != 0 && !pty_fd_eof_pending(fd)
    }
    #[cfg(not(unix))]
    {
        use crossterm::event;
        use std::time::Duration;

        event::poll(Duration::ZERO).unwrap_or(false)
    }
}
