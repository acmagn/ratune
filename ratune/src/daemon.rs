//! Background playback daemon: owns the audio engine so the TUI can close.
//!
//! Unix only (`$XDG_RUNTIME_DIR/ratune/ratune.sock`). The TUI is a client; `q`
//! detaches while a track is loaded, and `ratune stop` shuts the daemon down.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use ratune_player::PlayerEvent;
use ratune_subsonic::Song;

use crate::app::{App, PlayerMode};
use crate::config::Config;
use crate::state::NowPlayingPaneFocus;

const PROTOCOL_VERSION: u32 = 1;
const MAX_FRAME: u32 = 64 * 1024 * 1024;
const CONNECT_WAIT: Duration = Duration::from_secs(8);

// ── Paths ─────────────────────────────────────────────────────────────────────

fn ensure_private_dir(p: &Path) -> Result<()> {
    fs::create_dir_all(p).with_context(|| format!("creating {}", p.display()))?;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o700));
    Ok(())
}

fn runtime_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(dir).join("ratune");
        ensure_private_dir(&p)?;
        return Ok(p);
    }
    let p = std::env::temp_dir().join(format!("ratune-{}", rustix_uid()));
    ensure_private_dir(&p)?;
    Ok(p)
}

fn rustix_uid() -> u32 {
    unsafe { libc::getuid() }
}

pub fn socket_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("ratune.sock"))
}

fn pid_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("ratune.pid"))
}

fn log_path() -> Result<PathBuf> {
    let dir = if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        PathBuf::from(xdg).join("ratune")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("ratune")
    } else {
        runtime_dir()?
    };
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir.join("daemon.log"))
}

// ── Protocol ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub songs: Vec<Song>,
    pub cursor: usize,
    pub loop_enabled: bool,
    pub shuffle_active: bool,
    pub pre_shuffle_order: Option<Vec<Song>>,
    pub volume: u8,
    pub current_song: Option<Song>,
    pub elapsed_ms: u64,
    pub total_ms: Option<u64>,
    pub paused: bool,
    pub player_loaded: bool,
    pub np_pane_focus: NowPlayingPaneFocus,
}

impl SessionSnapshot {
    pub fn from_app(app: &App) -> Self {
        Self {
            songs: app.queue.songs.clone(),
            cursor: app.queue.cursor,
            loop_enabled: app.queue.loop_enabled,
            shuffle_active: app.queue.shuffle_active,
            pre_shuffle_order: app.queue.pre_shuffle_order.clone(),
            volume: app.config.default_volume,
            current_song: app.playback.current_song.clone(),
            elapsed_ms: app.playback.elapsed.as_millis() as u64,
            total_ms: app.playback.total.map(|d| d.as_millis() as u64),
            paused: app.playback.paused,
            player_loaded: app.playback.player_loaded,
            np_pane_focus: app.np_pane_focus,
        }
    }

    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for s in &self.songs {
            s.id.hash(&mut h);
        }
        self.cursor.hash(&mut h);
        self.loop_enabled.hash(&mut h);
        self.shuffle_active.hash(&mut h);
        self.volume.hash(&mut h);
        self.paused.hash(&mut h);
        self.player_loaded.hash(&mut h);
        self.current_song
            .as_ref()
            .map(|s| s.id.as_str())
            .hash(&mut h);
        self.np_pane_focus.hash(&mut h);
        h.finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello {
        protocol: u32,
    },
    SyncSession(SessionSnapshot),
    /// Start (or restart) the track at the session cursor. Daemon owns `play_gen`.
    PlayNow,
    PlayLive {
        url: String,
        song: Song,
    },
    Pause,
    Resume,
    Stop,
    SeekMs(u64),
    SetVolume(u8),
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    HelloOk {
        protocol: u32,
        snapshot: SessionSnapshot,
        pid: u32,
    },
    Snapshot(SessionSnapshot),
    Event(PlayerEventWire),
    Samples(Vec<f32>),
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlayerEventWire {
    TrackStarted,
    Progress {
        elapsed_ms: u64,
        total_ms: Option<u64>,
    },
    AboutToFinish,
    TrackAdvanced,
    TrackEnded,
    Error(String),
}

impl From<&PlayerEvent> for PlayerEventWire {
    fn from(e: &PlayerEvent) -> Self {
        match e {
            PlayerEvent::TrackStarted => Self::TrackStarted,
            PlayerEvent::Progress { elapsed, total } => Self::Progress {
                elapsed_ms: elapsed.as_millis() as u64,
                total_ms: total.map(|d| d.as_millis() as u64),
            },
            PlayerEvent::AboutToFinish => Self::AboutToFinish,
            PlayerEvent::TrackAdvanced => Self::TrackAdvanced,
            PlayerEvent::TrackEnded => Self::TrackEnded,
            PlayerEvent::Error(s) => Self::Error(s.clone()),
        }
    }
}

impl From<PlayerEventWire> for PlayerEvent {
    fn from(e: PlayerEventWire) -> Self {
        match e {
            PlayerEventWire::TrackStarted => Self::TrackStarted,
            PlayerEventWire::Progress {
                elapsed_ms,
                total_ms,
            } => Self::Progress {
                elapsed: Duration::from_millis(elapsed_ms),
                total: total_ms.map(Duration::from_millis),
            },
            PlayerEventWire::AboutToFinish => Self::AboutToFinish,
            PlayerEventWire::TrackAdvanced => Self::TrackAdvanced,
            PlayerEventWire::TrackEnded => Self::TrackEnded,
            PlayerEventWire::Error(s) => Self::Error(s),
        }
    }
}

// ── Framing ───────────────────────────────────────────────────────────────────

fn write_frame(stream: &mut UnixStream, msg: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(msg).context("serializing ipc frame")?;
    if bytes.len() > MAX_FRAME as usize {
        bail!("ipc frame too large ({} bytes)", bytes.len());
    }
    let len = (bytes.len() as u32).to_le_bytes();
    stream.write_all(&len)?;
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(())
}

fn read_frame<T: for<'de> Deserialize<'de>>(stream: &mut UnixStream) -> Result<T> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf);
    if len == 0 || len > MAX_FRAME {
        bail!("invalid ipc frame length {len}");
    }
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).context("parsing ipc frame")
}

// ── Status / stop ─────────────────────────────────────────────────────────────

fn connect_raw() -> Result<UnixStream> {
    let path = socket_path()?;
    UnixStream::connect(&path).with_context(|| format!("connecting to {}", path.display()))
}

pub fn status_text() -> String {
    match connect_raw() {
        Ok(mut stream) => {
            let _ = write_frame(
                &mut stream,
                &ClientMessage::Hello {
                    protocol: PROTOCOL_VERSION,
                },
            );
            match read_frame::<ServerMessage>(&mut stream) {
                Ok(ServerMessage::HelloOk { pid, .. }) => {
                    format!("ratune daemon running (pid {pid})")
                }
                Ok(_) => "ratune daemon running".into(),
                Err(e) => format!("ratune daemon socket error: {e:#}"),
            }
        }
        Err(_) => "ratune daemon not running".into(),
    }
}

/// Ask a running daemon to exit. No-op if none is listening.
pub fn stop() -> Result<bool> {
    let mut stream = match connect_raw() {
        Ok(s) => s,
        Err(_) => return Ok(false),
    };
    write_frame(&mut stream, &ClientMessage::Shutdown)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if connect_raw().is_err() {
            let _ = remove_stale_files();
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(50));
    }
    bail!("daemon did not exit in time")
}

fn remove_stale_files() -> Result<()> {
    let sock = socket_path()?;
    if sock.exists() {
        let _ = fs::remove_file(&sock);
    }
    let pid = pid_path()?;
    if pid.exists() {
        let _ = fs::remove_file(&pid);
    }
    Ok(())
}

// ── Client connection (TUI) ───────────────────────────────────────────────────

pub struct DaemonCtrl {
    pub msg_tx: mpsc::Sender<ClientMessage>,
    snapshot_rx: mpsc::Receiver<SessionSnapshot>,
    last_fp: Option<u64>,
    _joins: Vec<thread::JoinHandle<()>>,
}

impl DaemonCtrl {
    pub fn try_recv_snapshot(&mut self) -> Option<SessionSnapshot> {
        self.snapshot_rx.try_recv().ok()
    }

    pub fn send(&self, msg: ClientMessage) {
        let _ = self.msg_tx.send(msg);
    }

    pub fn flush_if_changed(&mut self, snap: SessionSnapshot) {
        let fp = snap.fingerprint();
        if self.last_fp == Some(fp) {
            return;
        }
        self.last_fp = Some(fp);
        let _ = self.msg_tx.send(ClientMessage::SyncSession(snap));
    }

    pub fn note_fingerprint(&mut self, fp: u64) {
        self.last_fp = Some(fp);
    }
}

pub struct ClientIo {
    pub event_rx: mpsc::Receiver<PlayerEvent>,
    pub sample_buffer: ratune_player::SampleBuffer,
    pub ctrl: DaemonCtrl,
}

pub fn connect() -> Result<ClientIo> {
    let mut stream = connect_raw()?;
    write_frame(
        &mut stream,
        &ClientMessage::Hello {
            protocol: PROTOCOL_VERSION,
        },
    )?;
    // Progress snapshots may arrive on this socket before HelloOk.
    let initial = loop {
        match read_frame::<ServerMessage>(&mut stream)? {
            ServerMessage::HelloOk {
                protocol, snapshot, ..
            } => {
                if protocol != PROTOCOL_VERSION {
                    bail!("daemon protocol {protocol} != client {PROTOCOL_VERSION}");
                }
                break snapshot;
            }
            ServerMessage::Snapshot(s) => break s,
            ServerMessage::Event(_) | ServerMessage::Samples(_) | ServerMessage::Error(_) => {
                continue
            }
        }
    };

    let mut write_stream = stream.try_clone().context("cloning daemon socket")?;
    let mut read_stream = stream;

    let (msg_tx, msg_rx) = mpsc::channel::<ClientMessage>();
    let (event_tx, event_rx) = mpsc::channel::<PlayerEvent>();
    let (snap_tx, snap_rx) = mpsc::channel::<SessionSnapshot>();
    let sample_buffer: ratune_player::SampleBuffer =
        Arc::new(Mutex::new(std::collections::VecDeque::with_capacity(4096)));
    let samples = sample_buffer.clone();

    let _ = snap_tx.send(initial);

    let writer = thread::Builder::new()
        .name("ratune-ipc-w".into())
        .spawn(move || {
            while let Ok(msg) = msg_rx.recv() {
                if write_frame(&mut write_stream, &msg).is_err() {
                    break;
                }
                if matches!(msg, ClientMessage::Shutdown) {
                    break;
                }
            }
        })
        .context("ipc writer thread")?;

    let reader = thread::Builder::new()
        .name("ratune-ipc-r".into())
        .spawn(move || loop {
            match read_frame::<ServerMessage>(&mut read_stream) {
                Ok(ServerMessage::Snapshot(s) | ServerMessage::HelloOk { snapshot: s, .. }) => {
                    if snap_tx.send(s).is_err() {
                        break;
                    }
                }
                Ok(ServerMessage::Event(e)) => {
                    if event_tx.send(e.into()).is_err() {
                        break;
                    }
                }
                Ok(ServerMessage::Samples(v)) => {
                    if let Ok(mut buf) = samples.lock() {
                        buf.clear();
                        buf.extend(v);
                    }
                }
                Ok(ServerMessage::Error(e)) => {
                    let _ = event_tx.send(PlayerEvent::Error(e));
                }
                Err(_) => break,
            }
        })
        .context("ipc reader thread")?;

    Ok(ClientIo {
        event_rx,
        sample_buffer,
        ctrl: DaemonCtrl {
            msg_tx,
            snapshot_rx: snap_rx,
            last_fp: None,
            _joins: vec![writer, reader],
        },
    })
}

pub fn spawn_daemon_process() -> Result<()> {
    let exe = std::env::current_exe().context("current_exe")?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path()?)
        .context("opening daemon log")?;
    let err = log.try_clone()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err));
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut cmd, || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn().context("spawning ratune daemon")?;
    Ok(())
}

pub fn ensure_and_connect() -> Result<ClientIo> {
    if let Ok(c) = connect() {
        return Ok(c);
    }
    spawn_daemon_process()?;
    let deadline = Instant::now() + CONNECT_WAIT;
    let mut last_err = None;
    while Instant::now() < deadline {
        match connect() {
            Ok(c) => return Ok(c),
            Err(e) => last_err = Some(e),
        }
        thread::sleep(Duration::from_millis(50));
    }
    bail!(
        "timed out waiting for ratune daemon ({})",
        last_err
            .map(|e| format!("{e:#}"))
            .unwrap_or_else(|| "no socket".into())
    )
}

// ── Daemon server ─────────────────────────────────────────────────────────────

struct Broadcaster {
    clients: Mutex<Vec<mpsc::Sender<ServerMessage>>>,
}

impl Broadcaster {
    fn new() -> Self {
        Self {
            clients: Mutex::new(Vec::new()),
        }
    }

    fn add(&self, tx: mpsc::Sender<ServerMessage>) {
        self.clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tx);
    }

    fn has_clients(&self) -> bool {
        !self
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    fn send_all(&self, msg: ServerMessage) {
        let mut g = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|tx| tx.send(msg.clone()).is_ok());
    }
}

fn bind_listener() -> Result<UnixListener> {
    let path = socket_path()?;
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            bail!("ratune daemon already running");
        }
        let _ = fs::remove_file(&path);
    }
    let listener =
        UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    let pid = std::process::id();
    let pid_file = pid_path()?;
    fs::write(&pid_file, format!("{pid}\n")).with_context(|| "writing pid file")?;
    let _ = fs::set_permissions(&pid_file, fs::Permissions::from_mode(0o600));
    Ok(listener)
}

fn redirect_stdio(log: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("opening {}", log.display()))?;
    let fd = file.as_raw_fd();
    unsafe {
        libc::dup2(fd, libc::STDOUT_FILENO);
        libc::dup2(fd, libc::STDERR_FILENO);
    }
    Ok(())
}

/// Entry point for `ratune daemon`.
pub async fn run_daemon() -> Result<()> {
    crate::keyring_init::install_default_keyring_store();
    let _ = redirect_stdio(&log_path()?);
    eprintln!(
        "ratune daemon starting pid={} sock={}",
        std::process::id(),
        socket_path()?.display()
    );

    let listener = match bind_listener() {
        Ok(l) => l,
        Err(e) if e.to_string().contains("already running") => {
            eprintln!("already running");
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    });
    let mut app = App::new_with_mode(config, PlayerMode::Daemon, None)?;

    if let Err(e) = crate::persist::restore_state(&mut app) {
        eprintln!("warn: could not restore state: {e}");
    }
    let history_path = crate::history::history_path();
    match crate::history::PlayHistory::load(&history_path) {
        Ok(h) => app.history = h,
        Err(e) => eprintln!("warn: could not load history: {e}"),
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let mpris_ctrl_rx = if let Some((link, rx)) = crate::mpris::setup(app.config.mpris_enabled) {
        app.mpris = Some(link);
        app.mpris_sync_now();
        Some(rx)
    } else {
        None
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let mpris_ctrl_rx: Option<std::sync::mpsc::Receiver<crate::mpris::MprisControl>> = None;

    app.spawn_startup_ping();

    let (incoming_tx, incoming_rx) = mpsc::channel::<ClientMessage>();
    let (accepted_tx, accepted_rx) = mpsc::channel::<UnixStream>();
    let broadcast = Arc::new(Broadcaster::new());
    let broadcast_accept = broadcast.clone();
    let incoming_accept = incoming_tx.clone();

    thread::Builder::new()
        .name("ratune-ipc-accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => {
                        if accepted_tx.send(s).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("accept: {e}");
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        })
        .context("accept thread")?;

    let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let flag = shutdown.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM");
            let mut sigint = signal(SignalKind::interrupt()).expect("SIGINT");
            tokio::select! {
                _ = sigterm.recv() => {}
                _ = sigint.recv() => {}
            }
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }

    let mut last_persist = Instant::now();
    let mut last_samples = Instant::now();
    let connectivity_interval = if app.config.connection_check_interval_secs > 0 {
        Duration::from_secs(app.config.connection_check_interval_secs)
    } else {
        Duration::MAX
    };
    let mut last_connectivity_check = Instant::now();

    loop {
        if shutdown.load(std::sync::atomic::Ordering::Relaxed) || app.should_quit {
            break;
        }

        while let Ok(stream) = accepted_rx.try_recv() {
            spawn_client_threads(stream, incoming_accept.clone(), broadcast_accept.clone());
        }

        while let Ok(msg) = incoming_rx.try_recv() {
            handle_client_message(&mut app, msg, &broadcast);
        }

        while let Ok(event) = app.player_rx.try_recv() {
            let wire = PlayerEventWire::from(&event);
            let persist = matches!(
                event,
                PlayerEvent::TrackEnded | PlayerEvent::TrackAdvanced | PlayerEvent::TrackStarted
            );
            app.handle_player_event(event);
            broadcast.send_all(ServerMessage::Event(wire));
            broadcast.send_all(ServerMessage::Snapshot(SessionSnapshot::from_app(&app)));
            if persist {
                let _ = crate::persist::save_state(&app);
                let _ = app.history.save(&history_path);
            }
        }

        if let Some(rx) = &mpris_ctrl_rx {
            while let Ok(c) = rx.try_recv() {
                app.handle_mpris_control(c);
                broadcast.send_all(ServerMessage::Snapshot(SessionSnapshot::from_app(&app)));
            }
        }

        while let Ok(update) = app.library_rx.try_recv() {
            app.apply_library_update(update);
        }

        if connectivity_interval != Duration::MAX
            && last_connectivity_check.elapsed() >= connectivity_interval
        {
            last_connectivity_check = Instant::now();
            app.spawn_connectivity_check(false);
        }

        if last_samples.elapsed() >= Duration::from_millis(50) {
            last_samples = Instant::now();
            if broadcast.has_clients() {
                if let Ok(buf) = app.sample_buffer.lock() {
                    if !buf.is_empty() {
                        let v: Vec<f32> = buf.iter().copied().collect();
                        drop(buf);
                        broadcast.send_all(ServerMessage::Samples(v));
                    }
                }
            }
        }

        if last_persist.elapsed() >= Duration::from_secs(30) {
            last_persist = Instant::now();
            let _ = crate::persist::save_state(&app);
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    broadcast.send_all(ServerMessage::Error("daemon stopping".into()));
    let _ = crate::persist::save_state(&app);
    let _ = app.history.save(&history_path);
    app.persist_scrobble_queue();

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(m) = app.mpris.take() {
        m.shutdown();
    }

    let _ = app.player_tx.send(ratune_player::PlayerCommand::Quit);
    if let Some(handle) = app.player_join.take() {
        let (done_tx, done_rx) = mpsc::channel::<()>();
        thread::spawn(move || {
            let _ = handle.join();
            let _ = done_tx.send(());
        });
        let _ = done_rx.recv_timeout(Duration::from_secs(1));
    }

    let _ = remove_stale_files();
    eprintln!("ratune daemon stopped");
    Ok(())
}

fn spawn_client_threads(
    stream: UnixStream,
    incoming_tx: mpsc::Sender<ClientMessage>,
    broadcast: Arc<Broadcaster>,
) {
    let read_stream = stream;
    let write_stream = match read_stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("clone client socket: {e}");
            return;
        }
    };
    let (out_tx, out_rx) = mpsc::channel::<ServerMessage>();
    broadcast.add(out_tx);

    thread::spawn(move || {
        let mut w = write_stream;
        while let Ok(msg) = out_rx.recv() {
            if write_frame(&mut w, &msg).is_err() {
                break;
            }
        }
    });

    thread::spawn(move || {
        let mut r = read_stream;
        while let Ok(msg) = read_frame::<ClientMessage>(&mut r) {
            let shutdown = matches!(msg, ClientMessage::Shutdown);
            if incoming_tx.send(msg).is_err() {
                break;
            }
            if shutdown {
                break;
            }
        }
    });
}

fn handle_client_message(app: &mut App, msg: ClientMessage, broadcast: &Broadcaster) {
    match msg {
        ClientMessage::Hello { .. } => {
            broadcast.send_all(ServerMessage::HelloOk {
                protocol: PROTOCOL_VERSION,
                snapshot: SessionSnapshot::from_app(app),
                pid: std::process::id(),
            });
        }
        ClientMessage::SyncSession(snap) => {
            app.apply_daemon_snapshot(snap, false);
        }
        ClientMessage::PlayNow => {
            app.play_current_from_daemon();
            broadcast.send_all(ServerMessage::Snapshot(SessionSnapshot::from_app(app)));
        }
        ClientMessage::PlayLive { url, song } => {
            app.play_live_from_daemon(url, song);
            broadcast.send_all(ServerMessage::Snapshot(SessionSnapshot::from_app(app)));
        }
        ClientMessage::Pause => {
            app.playback.paused = true;
            let _ = app.player_tx.send(ratune_player::PlayerCommand::Pause);
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            app.mpris_emit_props();
            broadcast.send_all(ServerMessage::Snapshot(SessionSnapshot::from_app(app)));
        }
        ClientMessage::Resume => {
            app.playback.paused = false;
            let _ = app.player_tx.send(ratune_player::PlayerCommand::Resume);
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            app.mpris_emit_props();
            broadcast.send_all(ServerMessage::Snapshot(SessionSnapshot::from_app(app)));
        }
        ClientMessage::Stop => {
            let _ = app.player_tx.send(ratune_player::PlayerCommand::Stop);
            app.playback.player_loaded = false;
            app.playback.elapsed = Duration::ZERO;
            app.playback.paused = false;
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            app.mpris_emit_props();
            broadcast.send_all(ServerMessage::Snapshot(SessionSnapshot::from_app(app)));
        }
        ClientMessage::SeekMs(ms) => {
            let pos = Duration::from_millis(ms);
            let _ = app.player_tx.send(ratune_player::PlayerCommand::Seek(pos));
            app.playback.elapsed = pos;
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            app.mpris_emit_seek(pos);
            broadcast.send_all(ServerMessage::Snapshot(SessionSnapshot::from_app(app)));
        }
        ClientMessage::SetVolume(v) => {
            app.config.default_volume = v.min(100);
            let _ = app.player_tx.send(ratune_player::PlayerCommand::SetVolume(
                app.config.default_volume as f32 / 100.0,
            ));
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            app.mpris_emit_props();
            broadcast.send_all(ServerMessage::Snapshot(SessionSnapshot::from_app(app)));
        }
        ClientMessage::Shutdown => {
            app.should_quit = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_roundtrip() {
        let snap = SessionSnapshot {
            songs: Vec::new(),
            cursor: 3,
            loop_enabled: true,
            shuffle_active: false,
            pre_shuffle_order: None,
            volume: 70,
            current_song: None,
            elapsed_ms: 1500,
            total_ms: Some(240_000),
            paused: true,
            player_loaded: true,
            np_pane_focus: NowPlayingPaneFocus::Queue,
        };
        let bytes = serde_json::to_vec(&ClientMessage::SyncSession(snap.clone())).unwrap();
        let msg: ClientMessage = serde_json::from_slice(&bytes).unwrap();
        match msg {
            ClientMessage::SyncSession(s) => {
                assert_eq!(s.cursor, 3);
                assert_eq!(s.elapsed_ms, 1500);
                assert!(s.paused);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_wire_roundtrip() {
        let e = PlayerEvent::Progress {
            elapsed: Duration::from_millis(500),
            total: Some(Duration::from_secs(3)),
        };
        let w = PlayerEventWire::from(&e);
        let back = PlayerEvent::from(w);
        match back {
            PlayerEvent::Progress { elapsed, total } => {
                assert_eq!(elapsed, Duration::from_millis(500));
                assert_eq!(total, Some(Duration::from_secs(3)));
            }
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn socket_path_is_under_runtime() {
        let p = socket_path().unwrap();
        assert!(p.ends_with("ratune.sock"));
    }
}
