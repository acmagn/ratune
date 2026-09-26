//! OS media-key / now-playing integration.
//!
//! - **Linux:** MPRIS D-Bus (media keys, desktop widgets, `playerctl`).
//! - **macOS:** `MPNowPlayingInfoCenter` + `MPRemoteCommandCenter` (Control Center,
//!   keyboard media keys).
//!
//! On other targets this module exposes inert stubs so the rest of the app stays
//! unconditional.

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

/// Shared playback status for OS media integrations (MPRIS / Now Playing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaPlaybackStatus {
    Playing,
    Paused,
    #[default]
    Stopped,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{MediaPlaybackStatus, MprisControl, MprisNotify, MprisSnapshot};
    use mpris_server::zbus::{self, fdo};
    use mpris_server::{
        LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property,
        RootInterface, Server, Signal, Time, TrackId, Volume,
    };

    fn to_mpris_status(s: MediaPlaybackStatus) -> PlaybackStatus {
        match s {
            MediaPlaybackStatus::Playing => PlaybackStatus::Playing,
            MediaPlaybackStatus::Paused => PlaybackStatus::Paused,
            MediaPlaybackStatus::Stopped => PlaybackStatus::Stopped,
        }
    }
    use std::sync::mpsc;
    use std::sync::{Arc, RwLock};
    use std::thread::JoinHandle;
    use tokio::sync::mpsc::UnboundedReceiver;

    pub(super) fn track_object_path(song_id: &str) -> String {
        super::dbus_track_path_for_song_id(song_id)
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
        if let Some(r) = s.user_rating_mpris {
            b = b.user_rating(r);
        }
        b.build()
    }

    fn build_properties(s: &MprisSnapshot) -> Vec<Property> {
        vec![
            Property::PlaybackStatus(to_mpris_status(s.playback_status)),
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
                .map(|s| to_mpris_status(s.playback_status))
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

#[cfg(target_os = "macos")]
mod macos {
    use super::{MediaPlaybackStatus, MprisControl, MprisNotify, MprisSnapshot};
    use block2::RcBlock;
    use dispatch::Queue;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AnyThread, ClassType, MainThreadMarker, Message};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSImage};
    use objc2_core_foundation::CGSize;
    use objc2_core_foundation::{CFRunLoopGetMain, CFRunLoopStop};
    use objc2_foundation::{
        NSBundle, NSDictionary, NSMutableDictionary, NSNumber, NSRunLoop, NSString, NSURL,
    };
    use objc2_media_player::{
        MPChangePlaybackPositionCommandEvent, MPMediaItemArtwork, MPMediaItemPropertyAlbumTitle,
        MPMediaItemPropertyAlbumTrackNumber, MPMediaItemPropertyArtist, MPMediaItemPropertyArtwork,
        MPMediaItemPropertyPlaybackDuration, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter,
        MPNowPlayingInfoMediaType, MPNowPlayingInfoPropertyElapsedPlaybackTime,
        MPNowPlayingInfoPropertyMediaType, MPNowPlayingInfoPropertyPlaybackRate,
        MPNowPlayingPlaybackState, MPRemoteCommand, MPRemoteCommandCenter, MPRemoteCommandEvent,
        MPRemoteCommandHandlerStatus,
    };
    use std::cell::RefCell;
    use std::ptr::NonNull;
    use std::sync::mpsc;
    use std::sync::{Arc, RwLock};
    use std::thread::JoinHandle;
    use tokio::sync::mpsc::UnboundedReceiver;

    thread_local! {
        static ART_CACHE: RefCell<Option<(String, Retained<MPMediaItemArtwork>)>> =
            RefCell::new(None);
    }

    fn to_now_playing_state(s: MediaPlaybackStatus) -> MPNowPlayingPlaybackState {
        match s {
            MediaPlaybackStatus::Playing => MPNowPlayingPlaybackState::Playing,
            MediaPlaybackStatus::Paused => MPNowPlayingPlaybackState::Paused,
            MediaPlaybackStatus::Stopped => MPNowPlayingPlaybackState::Stopped,
        }
    }

    fn playback_rate(s: MediaPlaybackStatus) -> f64 {
        match s {
            MediaPlaybackStatus::Playing => 1.0,
            MediaPlaybackStatus::Paused | MediaPlaybackStatus::Stopped => 0.0,
        }
    }

    unsafe fn set_enabled(cmd: &MPRemoteCommand, enabled: bool) {
        unsafe { cmd.setEnabled(enabled) };
    }

    fn register_simple(
        cmd: &MPRemoteCommand,
        ctrl_tx: mpsc::Sender<MprisControl>,
        control: MprisControl,
        targets: &mut Vec<Retained<AnyObject>>,
    ) {
        let label = format!("{control:?}");
        let handler = RcBlock::new(move |_event: NonNull<MPRemoteCommandEvent>| {
            if crate::debug::enabled() {
                crate::debug::log(format!("now-playing: remote {label}"));
            }
            let _ = ctrl_tx.send(control.clone());
            MPRemoteCommandHandlerStatus::Success
        });
        let target = unsafe { cmd.addTargetWithHandler(&handler) };
        targets.push(target);
        unsafe { set_enabled(cmd, true) };
    }

    fn artwork_from_url(
        url_str: &str,
        cache: &mut Option<(String, Retained<MPMediaItemArtwork>)>,
    ) -> Option<Retained<MPMediaItemArtwork>> {
        if let Some((cached_url, art)) = cache.as_ref() {
            if cached_url == url_str {
                return Some(art.clone());
            }
        }
        let ns_url = NSURL::URLWithString(&NSString::from_str(url_str))?;
        let image = NSImage::initWithContentsOfURL(NSImage::alloc(), &ns_url)?;
        let size = image.size();
        if size.width <= 0.0 || size.height <= 0.0 {
            return None;
        }
        let bounds = CGSize::new(size.width, size.height);
        let image_for_block = image.clone();
        let handler = RcBlock::new(move |_request_size: CGSize| -> NonNull<NSImage> {
            NonNull::from(&*image_for_block)
        });
        let art = unsafe {
            MPMediaItemArtwork::initWithBoundsSize_requestHandler(
                MPMediaItemArtwork::alloc(),
                bounds,
                &handler,
            )
        };
        *cache = Some((url_str.to_string(), art.clone()));
        Some(art)
    }

    fn as_any_object<T: Message>(obj: &T) -> &AnyObject {
        // SAFETY: every Objective-C object is an AnyObject at runtime.
        unsafe { &*(std::ptr::from_ref(obj) as *const AnyObject) }
    }

    fn push_now_playing(snap: &MprisSnapshot, log_refresh: bool) {
        ART_CACHE.with(|cell| {
            let mut art_cache = cell.borrow_mut();
            let center = unsafe { MPNowPlayingInfoCenter::defaultCenter() };
            if !snap.has_track {
                *art_cache = None;
                unsafe {
                    center.setNowPlayingInfo(None);
                    center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
                }
                if log_refresh {
                    crate::debug::log("now-playing: cleared (no track)");
                }
                return;
            }

            let dict = NSMutableDictionary::<NSString, AnyObject>::new();
            // MediaPlayer property keys are `extern static` — reading them is unsafe (E0133).
            unsafe {
                dict.insert(
                    MPMediaItemPropertyTitle,
                    as_any_object(&*NSString::from_str(&snap.title)),
                );
                if !snap.artist.is_empty() {
                    dict.insert(
                        MPMediaItemPropertyArtist,
                        as_any_object(&*NSString::from_str(&snap.artist)),
                    );
                }
                if !snap.album.is_empty() {
                    dict.insert(
                        MPMediaItemPropertyAlbumTitle,
                        as_any_object(&*NSString::from_str(&snap.album)),
                    );
                }
                if let Some(n) = snap.track_number {
                    dict.insert(
                        MPMediaItemPropertyAlbumTrackNumber,
                        as_any_object(&*NSNumber::new_u32(n)),
                    );
                }
                if snap.length_micros > 0 {
                    dict.insert(
                        MPMediaItemPropertyPlaybackDuration,
                        as_any_object(&*NSNumber::new_f64(snap.length_micros as f64 / 1_000_000.0)),
                    );
                }
                dict.insert(
                    MPNowPlayingInfoPropertyElapsedPlaybackTime,
                    as_any_object(&*NSNumber::new_f64(
                        snap.position_micros as f64 / 1_000_000.0,
                    )),
                );
                dict.insert(
                    MPNowPlayingInfoPropertyPlaybackRate,
                    as_any_object(&*NSNumber::new_f64(playback_rate(snap.playback_status))),
                );
                dict.insert(
                    MPNowPlayingInfoPropertyMediaType,
                    as_any_object(&*NSNumber::new_u64(
                        MPNowPlayingInfoMediaType::Audio.0 as u64,
                    )),
                );
                if let Some(ref url) = snap.art_url {
                    if let Some(art) = artwork_from_url(url, &mut art_cache) {
                        dict.insert(MPMediaItemPropertyArtwork, as_any_object(&*art));
                    }
                } else {
                    *art_cache = None;
                }
            }

            let info: &NSDictionary<NSString, AnyObject> = dict.as_ref();
            unsafe {
                center.setNowPlayingInfo(Some(info));
                center.setPlaybackState(to_now_playing_state(snap.playback_status));
            }
            if log_refresh && crate::debug::enabled() {
                crate::debug::log(format!(
                    "now-playing: refresh title={:?} status={:?} pos_s={:.1}",
                    snap.title,
                    snap.playback_status,
                    snap.position_micros as f64 / 1_000_000.0
                ));
            }
        });
    }

    fn sync_command_availability(snap: &MprisSnapshot) {
        let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };
        unsafe {
            set_enabled(center.playCommand().as_ref(), snap.can_play);
            set_enabled(center.pauseCommand().as_ref(), snap.can_pause);
            set_enabled(
                center.togglePlayPauseCommand().as_ref(),
                snap.can_play || snap.can_pause,
            );
            set_enabled(
                center.stopCommand().as_ref(),
                snap.has_track || snap.can_pause,
            );
            set_enabled(center.nextTrackCommand().as_ref(), snap.can_go_next);
            set_enabled(center.previousTrackCommand().as_ref(), snap.can_go_previous);
            set_enabled(
                center.changePlaybackPositionCommand().as_super(),
                snap.can_seek,
            );
        }
    }

    fn register_all_commands(ctrl_tx: mpsc::Sender<MprisControl>) {
        let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };
        let mut targets: Vec<Retained<AnyObject>> = Vec::new();

        register_simple(
            unsafe { center.togglePlayPauseCommand().as_ref() },
            ctrl_tx.clone(),
            MprisControl::PlayPause,
            &mut targets,
        );
        register_simple(
            unsafe { center.playCommand().as_ref() },
            ctrl_tx.clone(),
            MprisControl::Play,
            &mut targets,
        );
        register_simple(
            unsafe { center.pauseCommand().as_ref() },
            ctrl_tx.clone(),
            MprisControl::Pause,
            &mut targets,
        );
        register_simple(
            unsafe { center.stopCommand().as_ref() },
            ctrl_tx.clone(),
            MprisControl::Stop,
            &mut targets,
        );
        register_simple(
            unsafe { center.nextTrackCommand().as_ref() },
            ctrl_tx.clone(),
            MprisControl::Next,
            &mut targets,
        );
        register_simple(
            unsafe { center.previousTrackCommand().as_ref() },
            ctrl_tx.clone(),
            MprisControl::Previous,
            &mut targets,
        );

        {
            let ctrl_tx = ctrl_tx.clone();
            let handler = RcBlock::new(move |event: NonNull<MPRemoteCommandEvent>| {
                let event_ref = unsafe { event.as_ref() };
                let Some(pos_event) =
                    event_ref.downcast_ref::<MPChangePlaybackPositionCommandEvent>()
                else {
                    return MPRemoteCommandHandlerStatus::CommandFailed;
                };
                let secs = unsafe { pos_event.positionTime() };
                if crate::debug::enabled() {
                    crate::debug::log(format!("now-playing: remote SetPosition secs={secs:.2}"));
                }
                let _ = ctrl_tx.send(MprisControl::SetPosition {
                    track_path: String::new(),
                    position_micros: (secs * 1_000_000.0).round() as i64,
                });
                MPRemoteCommandHandlerStatus::Success
            });
            let cmd = unsafe { center.changePlaybackPositionCommand() };
            let target = unsafe { cmd.as_super().addTargetWithHandler(&handler) };
            targets.push(target);
            unsafe { set_enabled(cmd.as_super(), true) };
        }

        // Keep handler targets alive for process lifetime.
        std::mem::forget(targets);
        crate::debug::log("now-playing: remote commands registered (main thread)");
    }

    /// Must run on the OS main thread before the tokio runtime takes it over.
    pub(super) fn prepare_appkit() {
        let mtm = MainThreadMarker::new().unwrap_or_else(|| {
            // SAFETY: caller guarantees this is the process main thread.
            unsafe { MainThreadMarker::new_unchecked() }
        });
        let ns_app = NSApplication::sharedApplication(mtm);
        let ok = ns_app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        ns_app.finishLaunching();
        if crate::debug::enabled() {
            let bundle_id = NSBundle::mainBundle()
                .bundleIdentifier()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "<none>".into());
            crate::debug::log(format!(
                "now-playing: NSApplication ready (accessory_policy={ok}, bundle_id={bundle_id})"
            ));
        }
        // Keep the shared app alive.
        std::mem::forget(ns_app);
    }

    pub(super) fn run_main_loop() {
        crate::debug::log("now-playing: entering main NSRunLoop");
        // Blocks until `stop_main_loop` (daemon exit).
        NSRunLoop::mainRunLoop().run();
    }

    pub(super) fn stop_main_loop() {
        if let Some(rl) = CFRunLoopGetMain() {
            CFRunLoopStop(&rl);
        }
    }

    pub(super) fn spawn_server(
        snapshot: Arc<RwLock<MprisSnapshot>>,
        ctrl_tx: mpsc::Sender<MprisControl>,
        mut notify_rx: UnboundedReceiver<MprisNotify>,
    ) -> JoinHandle<()> {
        // Register remote commands on the AppKit main thread.
        let ctrl_for_main = ctrl_tx;
        Queue::main().exec_sync(move || {
            register_all_commands(ctrl_for_main);
        });

        // Snapshot updates also must hit the main thread for MediaRemote.
        std::thread::Builder::new()
            .name("ratune-nowplaying".into())
            .spawn(move || {
                while let Some(msg) = notify_rx.blocking_recv() {
                    match msg {
                        MprisNotify::Shutdown => break,
                        MprisNotify::Refresh => {
                            let snap = snapshot.read().map(|s| s.clone()).unwrap_or_default();
                            Queue::main().exec_async(move || {
                                push_now_playing(&snap, true);
                                sync_command_availability(&snap);
                            });
                        }
                        MprisNotify::Seeked { .. } => {
                            let snap = snapshot.read().map(|s| s.clone()).unwrap_or_default();
                            Queue::main().exec_async(move || {
                                push_now_playing(&snap, false);
                            });
                        }
                    }
                }

                Queue::main().exec_sync(|| {
                    let info_center = unsafe { MPNowPlayingInfoCenter::defaultCenter() };
                    let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };
                    unsafe {
                        info_center.setNowPlayingInfo(None);
                        info_center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
                        set_enabled(center.playCommand().as_ref(), false);
                        set_enabled(center.pauseCommand().as_ref(), false);
                        set_enabled(center.togglePlayPauseCommand().as_ref(), false);
                        set_enabled(center.stopCommand().as_ref(), false);
                        set_enabled(center.nextTrackCommand().as_ref(), false);
                        set_enabled(center.previousTrackCommand().as_ref(), false);
                        set_enabled(center.changePlaybackPositionCommand().as_super(), false);
                    }
                    crate::debug::log("now-playing: shutdown");
                });
            })
            .expect("spawn now-playing thread")
    }
}

#[cfg(target_os = "macos")]
pub fn macos_prepare_appkit() {
    macos::prepare_appkit();
}

#[cfg(target_os = "macos")]
pub fn macos_run_main_loop() {
    macos::run_main_loop();
}

#[cfg(target_os = "macos")]
pub fn macos_stop_main_loop() {
    macos::stop_main_loop();
}

/// Snapshot of playback state read by the OS media thread (must stay cheap to clone for refresh).
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
    /// Cover art as a `file:///…` URL (MPRIS artUrl / macOS artwork source).
    pub art_url: Option<String>,
    pub playback_status: MediaPlaybackStatus,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_seek: bool,
    pub has_track: bool,
    /// MPRIS `xesam:userRating` (0.0–1.0); unset when the track is unrated.
    pub user_rating_mpris: Option<f64>,
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
            playback_status: MediaPlaybackStatus::Stopped,
            can_go_next: false,
            can_go_previous: false,
            can_play: false,
            can_pause: false,
            can_seek: false,
            has_track: false,
            user_rating_mpris: None,
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

/// Start OS media-key integration when `enabled`.
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

/// Start OS media-key integration when `enabled`.
#[cfg(target_os = "macos")]
pub fn setup(enabled: bool) -> Option<(MprisLink, mpsc::Receiver<MprisControl>)> {
    if !enabled {
        return None;
    }
    let (ctrl_tx, ctrl_rx) = mpsc::channel::<MprisControl>();
    let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel::<MprisNotify>();
    let snapshot = Arc::new(RwLock::new(MprisSnapshot::default()));
    let thread = macos::spawn_server(Arc::clone(&snapshot), ctrl_tx, notify_rx);
    let link = MprisLink {
        snapshot,
        notify_tx,
        thread: Some(thread),
    };
    Some((link, ctrl_rx))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn setup(_enabled: bool) -> Option<(MprisLink, mpsc::Receiver<MprisControl>)> {
    None
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
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

/// Object path / track id used for seek-position matching (MPRIS trackid; reused on macOS).
pub fn dbus_track_path_for_song_id(song_id: &str) -> String {
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
        s.user_rating_mpris = if app.config.ratings_enabled {
            ratune_subsonic::user_rating_mpris(track.user_rating)
        } else {
            None
        };
    }
    s.position_micros = app.playback.elapsed.as_micros() as i64;
    s.volume = app.config.default_volume as f64 / 100.0;
    let has_queue = !app.queue.songs.is_empty();
    s.can_go_next =
        has_queue && (app.queue.cursor + 1 < app.queue.songs.len() || app.queue.loop_enabled);
    s.can_go_previous = has_queue && (app.queue.cursor > 0 || app.queue.loop_enabled);
    s.can_play = app.queue.current().is_some();
    s.can_pause = app.playback.player_loaded;
    s.can_seek = app.playback.player_loaded && app.playback.total.is_some();

    s.art_url = {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            mpris_art::cover_art_url(app)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            None
        }
    };

    s.playback_status = if song.is_none() || !app.playback.player_loaded {
        MediaPlaybackStatus::Stopped
    } else if app.playback.paused {
        MediaPlaybackStatus::Paused
    } else {
        MediaPlaybackStatus::Playing
    };

    if let Ok(mut w) = snap.write() {
        *w = s;
    }
}
