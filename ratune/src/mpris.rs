//! MPRIS D-Bus integration for Linux (media keys, desktop widgets, `playerctl`).
//!
//! On non-Linux targets this module exposes inert stubs so the rest of the app
//! stays unconditional.

use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

/// Commands from the D-Bus MPRIS interface to the main TUI thread.
#[derive(Debug, Clone)]
pub enum MprisControl {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    /// Delta in microseconds (may be negative).
    SeekDelta(i64),
    /// Absolute position; only applied if `track_path` matches the current track.
    SetPosition {
        track_path: String,
        position_micros: i64,
    },
    /// MPRIS volume 0.0–1.0
    SetVolume(f64),
    Quit,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{MprisControl, MprisNotify, MprisSnapshot};
    use mpris_server::zbus::{self, fdo};
    use mpris_server::{
        LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property,
        RootInterface, Server, Signal, Time, TrackId, Volume,
    };
    use std::sync::mpsc;
    use std::sync::{Arc, RwLock};
    use std::thread::JoinHandle;
    use tokio::sync::mpsc::UnboundedReceiver;

    pub(super) fn track_object_path(song_id: &str) -> String {
        let safe: String = song_id
            .chars()
            .map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' => c,
                '-' => '_',
                _ => '_',
            })
            .collect();
        let tail = if safe.is_empty() {
            "unknown".into()
        } else {
            safe
        };
        format!("/net/ratune/track/{tail}")
    }

    fn track_id_from_song_id(song_id: &str) -> TrackId {
        let path = track_object_path(song_id);
        TrackId::try_from(path.as_str()).unwrap_or(TrackId::NO_TRACK)
    }

    fn build_metadata(s: &MprisSnapshot) -> Metadata {
        if !s.has_track {
            return Metadata::new();
        }
        let tid = track_id_from_song_id(&s.song_id);
        let mut b = Metadata::builder().trackid(tid).title(s.title.clone());
        if !s.artist.is_empty() {
            b = b.artist([s.artist.clone()]);
        }
        if !s.album.is_empty() {
            b = b.album(s.album.clone());
        }
        if let Some(n) = s.track_number {
            b = b.track_number(n as i32);
        }
        if s.length_micros > 0 {
            b = b.length(Time::from_micros(s.length_micros));
        }
        if let Some(ref u) = s.art_url {
            b = b.art_url(u.as_str());
        }
        b.build()
    }

    fn build_properties(s: &MprisSnapshot) -> Vec<Property> {
        vec![
            Property::PlaybackStatus(s.playback_status),
            Property::LoopStatus(LoopStatus::None),
            Property::Rate(PlaybackRate::default()),
            Property::Shuffle(false),
            Property::Metadata(build_metadata(s)),
            Property::Volume(s.volume),
            Property::MinimumRate(PlaybackRate::default()),
            Property::MaximumRate(PlaybackRate::default()),
            Property::CanGoNext(s.can_go_next),
            Property::CanGoPrevious(s.can_go_previous),
            Property::CanPlay(s.can_play),
            Property::CanPause(s.can_pause),
            Property::CanSeek(s.can_seek),
        ]
    }

    struct MprisImp {
        snapshot: Arc<RwLock<MprisSnapshot>>,
        ctrl_tx: mpsc::Sender<MprisControl>,
    }

    impl MprisImp {
        fn send(&self, c: MprisControl) {
            let _ = self.ctrl_tx.send(c);
        }
    }

    impl RootInterface for MprisImp {
        async fn raise(&self) -> fdo::Result<()> {
            Ok(())
        }

        async fn quit(&self) -> fdo::Result<()> {
            self.send(MprisControl::Quit);
            Ok(())
        }

        async fn can_quit(&self) -> fdo::Result<bool> {
            Ok(true)
        }

        async fn fullscreen(&self) -> fdo::Result<bool> {
            Ok(false)
        }

        async fn set_fullscreen(&self, _fullscreen: bool) -> zbus::Result<()> {
            Ok(())
        }

        async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
            Ok(false)
        }

        async fn can_raise(&self) -> fdo::Result<bool> {
            Ok(false)
        }

        async fn has_track_list(&self) -> fdo::Result<bool> {
            Ok(false)
        }

        async fn identity(&self) -> fdo::Result<String> {
            Ok("ratune".to_string())
        }

        async fn desktop_entry(&self) -> fdo::Result<String> {
            Ok("ratune".to_string())
        }

        async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
            Ok(vec!["file".to_string()])
        }

        async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
            Ok(vec![])
        }
    }

    impl PlayerInterface for MprisImp {
        async fn next(&self) -> fdo::Result<()> {
            self.send(MprisControl::Next);
            Ok(())
        }

        async fn previous(&self) -> fdo::Result<()> {
            self.send(MprisControl::Previous);
            Ok(())
        }

        async fn pause(&self) -> fdo::Result<()> {
            self.send(MprisControl::Pause);
            Ok(())
        }

        async fn play_pause(&self) -> fdo::Result<()> {
            self.send(MprisControl::PlayPause);
            Ok(())
        }

        async fn stop(&self) -> fdo::Result<()> {
            self.send(MprisControl::Stop);
            Ok(())
        }

        async fn play(&self) -> fdo::Result<()> {
            self.send(MprisControl::Play);
            Ok(())
        }

        async fn seek(&self, offset: Time) -> fdo::Result<()> {
            self.send(MprisControl::SeekDelta(offset.as_micros()));
            Ok(())
        }

        async fn set_position(&self, track_id: TrackId, position: Time) -> fdo::Result<()> {
            self.send(MprisControl::SetPosition {
                track_path: track_id.as_str().to_string(),
                position_micros: position.as_micros(),
            });
            Ok(())
        }

        async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
            Err(fdo::Error::Failed(
                "ratune does not support open-uri over MPRIS".into(),
            ))
        }

        async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
            Ok(self
                .snapshot
                .read()
                .map(|s| s.playback_status)
                .unwrap_or(PlaybackStatus::Stopped))
        }

        async fn loop_status(&self) -> fdo::Result<LoopStatus> {
            Ok(LoopStatus::None)
        }

        async fn set_loop_status(&self, _loop_status: LoopStatus) -> zbus::Result<()> {
            Ok(())
        }

        async fn rate(&self) -> fdo::Result<PlaybackRate> {
            Ok(PlaybackRate::default())
        }

        async fn set_rate(&self, _rate: PlaybackRate) -> zbus::Result<()> {
            Ok(())
        }

        async fn shuffle(&self) -> fdo::Result<bool> {
            Ok(false)
        }

        async fn set_shuffle(&self, _shuffle: bool) -> zbus::Result<()> {
            Ok(())
        }

        async fn metadata(&self) -> fdo::Result<Metadata> {
            Ok(self
                .snapshot
                .read()
                .map(|s| build_metadata(&s))
                .unwrap_or_else(|_| Metadata::new()))
        }

        async fn volume(&self) -> fdo::Result<Volume> {
            Ok(self.snapshot.read().map(|s| s.volume).unwrap_or(0.7))
        }

        async fn set_volume(&self, volume: Volume) -> zbus::Result<()> {
            self.send(MprisControl::SetVolume(volume));
            Ok(())
        }

        async fn position(&self) -> fdo::Result<Time> {
            Ok(self
                .snapshot
                .read()
                .map(|s| Time::from_micros(s.position_micros))
                .unwrap_or(Time::ZERO))
        }

        async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
            Ok(PlaybackRate::default())
        }

        async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
            Ok(PlaybackRate::default())
        }

        async fn can_go_next(&self) -> fdo::Result<bool> {
            Ok(self.snapshot.read().map(|s| s.can_go_next).unwrap_or(false))
        }

        async fn can_go_previous(&self) -> fdo::Result<bool> {
            Ok(self
                .snapshot
                .read()
                .map(|s| s.can_go_previous)
                .unwrap_or(false))
        }

        async fn can_play(&self) -> fdo::Result<bool> {
            Ok(self.snapshot.read().map(|s| s.can_play).unwrap_or(false))
        }

        async fn can_pause(&self) -> fdo::Result<bool> {
            Ok(self.snapshot.read().map(|s| s.can_pause).unwrap_or(false))
        }

        async fn can_seek(&self) -> fdo::Result<bool> {
            Ok(self.snapshot.read().map(|s| s.can_seek).unwrap_or(false))
        }

        async fn can_control(&self) -> fdo::Result<bool> {
            Ok(true)
        }
    }

    pub(super) fn spawn_server(
        bus_suffix: String,
        snapshot: Arc<RwLock<MprisSnapshot>>,
        ctrl_tx: mpsc::Sender<MprisControl>,
        mut notify_rx: UnboundedReceiver<MprisNotify>,
    ) -> JoinHandle<()> {
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("warn: mpris: could not create async runtime: {e}");
                    return;
                }
            };

            rt.block_on(async move {
                let imp = MprisImp {
                    snapshot: Arc::clone(&snapshot),
                    ctrl_tx,
                };
                let server = match Server::new(&bus_suffix, imp).await {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("warn: mpris: could not register on session bus: {e}");
                        return;
                    }
                };

                loop {
                    match notify_rx.recv().await {
                        None => break,
                        Some(MprisNotify::Shutdown) => break,
                        Some(MprisNotify::Refresh) => {
                            let snap = snapshot.read().map(|s| s.clone()).unwrap_or_default();
                            let _ = server.properties_changed(build_properties(&snap)).await;
                        }
                        Some(MprisNotify::Seeked { position_micros }) => {
                            let _ = server
                                .emit(Signal::Seeked {
                                    position: Time::from_micros(position_micros),
                                })
                                .await;
                        }
                    }
                }
            });
        })
    }
}

/// Snapshot of playback state read by the D-Bus thread (must stay cheap to clone for refresh).
#[derive(Debug, Clone)]
pub struct MprisSnapshot {
    pub song_id: String,
    pub track_path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_number: Option<u32>,
    pub length_micros: i64,
    pub position_micros: i64,
    pub volume: f64,
    /// `mpris:artUrl` — typically `file:///…` for the current cover image.
    pub art_url: Option<String>,
    #[cfg(target_os = "linux")]
    pub playback_status: mpris_server::PlaybackStatus,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_seek: bool,
    pub has_track: bool,
}

impl Default for MprisSnapshot {
    fn default() -> Self {
        Self {
            song_id: String::new(),
            track_path: String::new(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            track_number: None,
            length_micros: 0,
            position_micros: 0,
            volume: 0.7,
            art_url: None,
            #[cfg(target_os = "linux")]
            playback_status: mpris_server::PlaybackStatus::Stopped,
            can_go_next: false,
            can_go_previous: false,
            can_play: false,
            can_pause: false,
            can_seek: false,
            has_track: false,
        }
    }
}

pub enum MprisNotify {
    Refresh,
    Seeked { position_micros: i64 },
    Shutdown,
}

pub struct MprisLink {
    pub snapshot: Arc<RwLock<MprisSnapshot>>,
    notify_tx: tokio::sync::mpsc::UnboundedSender<MprisNotify>,
    thread: Option<JoinHandle<()>>,
}

impl MprisLink {
    pub fn notify_refresh(&self) {
        let _ = self.notify_tx.send(MprisNotify::Refresh);
    }

    pub fn notify_seeked(&self, position: Duration) {
        let _ = self.notify_tx.send(MprisNotify::Seeked {
            position_micros: position.as_micros() as i64,
        });
    }

    pub fn shutdown(mut self) {
        let _ = self.notify_tx.send(MprisNotify::Shutdown);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

/// Start MPRIS when `enabled` and the session bus is available.
#[cfg(target_os = "linux")]
pub fn setup(enabled: bool) -> Option<(MprisLink, mpsc::Receiver<MprisControl>)> {
    if !enabled {
        return None;
    }
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<MprisControl>();
    let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel::<MprisNotify>();
    let snapshot = Arc::new(RwLock::new(MprisSnapshot::default()));
    let pid = std::process::id();
    let bus_suffix = format!("ratune.instance{}", pid);
    let thread = linux::spawn_server(bus_suffix, Arc::clone(&snapshot), ctrl_tx, notify_rx);
    let link = MprisLink {
        snapshot,
        notify_tx,
        thread: Some(thread),
    };
    Some((link, ctrl_rx))
}

#[cfg(not(target_os = "linux"))]
pub fn setup(_enabled: bool) -> Option<(MprisLink, mpsc::Receiver<MprisControl>)> {
    None
}

#[cfg(target_os = "linux")]
mod mpris_art {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static LAST_EXPORT: Mutex<Option<(String, String)>> = Mutex::new(None);

    const EXT_CANDIDATES: [&str; 4] = ["jpg", "png", "gif", "webp"];

    fn sniff_ext(bytes: &[u8]) -> &'static str {
        if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
            return "jpg";
        }
        if bytes.len() >= 8 && bytes[0..8] == [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'] {
            return "png";
        }
        if bytes.len() >= 6 && (bytes[0..6] == *b"GIF87a" || bytes[0..6] == *b"GIF89a") {
            return "gif";
        }
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return "webp";
        }
        "jpg"
    }

    fn remove_cover_variants(dir: &Path) {
        for ext in EXT_CANDIDATES {
            let _ = std::fs::remove_file(dir.join(format!("mpris_cover.{ext}")));
            let _ = std::fs::remove_file(dir.join(format!("mpris_cover.{ext}.part")));
        }
    }

    fn clear_export_state() {
        if let Ok(mut g) = LAST_EXPORT.lock() {
            *g = None;
        }
        if let Some(dir) = crate::cache::ratune_cache_dir() {
            remove_cover_variants(&dir);
        }
    }

    fn write_and_url(dir: &Path, bytes: &[u8]) -> Option<String> {
        let ext = sniff_ext(bytes);
        remove_cover_variants(dir);
        let path: PathBuf = dir.join(format!("mpris_cover.{ext}"));
        let tmp = dir.join(format!("mpris_cover.{ext}.part"));
        let mut f = std::fs::File::create(&tmp).ok()?;
        f.write_all(bytes).ok()?;
        f.sync_all().ok()?;
        drop(f);
        std::fs::rename(&tmp, &path).ok()?;
        url::Url::from_file_path(&path).ok().map(|u| u.to_string())
    }

    pub fn cover_art_url(app: &crate::app::App) -> Option<String> {
        let Some(song) = app.playback.current_song.as_ref() else {
            clear_export_state();
            return None;
        };
        let Some(want_id) = song.cover_art.as_ref() else {
            clear_export_state();
            return None;
        };
        let Some((have_id, bytes)) = app.art_cache.as_ref() else {
            clear_export_state();
            return None;
        };
        if have_id != want_id || bytes.is_empty() {
            clear_export_state();
            return None;
        }

        if let Ok(guard) = LAST_EXPORT.lock() {
            if let Some((id, url)) = guard.as_ref() {
                if id == want_id {
                    return Some(url.clone());
                }
            }
        }

        let dir = crate::cache::ratune_cache_dir()?;
        std::fs::create_dir_all(&dir).ok()?;
        let url = write_and_url(&dir, bytes)?;
        if let Ok(mut g) = LAST_EXPORT.lock() {
            *g = Some((want_id.clone(), url.clone()));
        }
        Some(url)
    }
}

/// D-Bus object path used as `mpris:trackid` for a Subsonic song id.
pub fn dbus_track_path_for_song_id(song_id: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        linux::track_object_path(song_id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        String::new()
    }
}

/// Update the shared snapshot from app state (call before `notify_refresh`).
pub fn write_snapshot(app: &crate::app::App, snap: &RwLock<MprisSnapshot>) {
    let mut s = MprisSnapshot::default();
    let song = app.playback.current_song.as_ref();
    if let Some(track) = song {
        s.has_track = true;
        s.song_id = track.id.clone();
        s.track_path = dbus_track_path_for_song_id(&track.id);
        s.title = track.title.clone();
        s.artist = track.artist.clone().unwrap_or_default();
        s.album = track.album.clone().unwrap_or_default();
        s.track_number = track.track;
        s.length_micros = track
            .duration
            .map(|d| i64::from(d) * 1_000_000)
            .unwrap_or(0);
    }
    s.position_micros = app.playback.elapsed.as_micros() as i64;
    s.volume = app.config.default_volume as f64 / 100.0;
    s.can_go_next = app.queue.cursor + 1 < app.queue.songs.len();
    s.can_go_previous = app.queue.cursor > 0;
    s.can_play = app.queue.current().is_some();
    s.can_pause = app.playback.player_loaded;
    s.can_seek = app.playback.player_loaded && app.playback.total.is_some();

    s.art_url = {
        #[cfg(target_os = "linux")]
        {
            mpris_art::cover_art_url(app)
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    };

    #[cfg(target_os = "linux")]
    {
        use mpris_server::PlaybackStatus;
        s.playback_status = if song.is_none() || !app.playback.player_loaded {
            PlaybackStatus::Stopped
        } else if app.playback.paused {
            PlaybackStatus::Paused
        } else {
            PlaybackStatus::Playing
        };
    }

    if let Ok(mut w) = snap.write() {
        *w = s;
    }
}
