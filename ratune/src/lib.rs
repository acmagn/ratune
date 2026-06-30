mod action;
mod app;
mod cache;
mod color;
mod config;
mod debug;
mod desktop_notify;
mod favorites_cache;
mod fzf_picker;
mod history;
mod keybinds;
mod keyring_init;
mod library_index;
mod lyrics;
mod lyrics_cache;
mod mouse_click;
mod mpris;
mod persist;
mod scrobble;
mod scrobble_queue;
mod state;
mod theme;
mod tty;
mod ui;
mod visualizer;

use std::io;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture, Event,
    KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::Terminal;

use action::{Action, Direction};
use app::{App, BrowserColumn, Tab};
use config::{AlbumArtBackend, BrowseMode, Config, HomePanel};
use keybinds::Keybinds;
use state::{
    FavoritesFocus, GlobalConfirm, LoadingState, PlaylistFocus, PlaylistInputMode, RadioInputMode,
};

/// Entry point shared by the `ratune` binary and integration tests.
pub async fn run() -> Result<()> {
    keyring_init::install_default_keyring_store();

    let config = Config::load().unwrap_or_else(|e| {
        // `{:#}` prints the full cause chain (plain `{}` hides nested anyhow contexts).
        eprintln!("error: {e:#}");
        process::exit(1);
    });
    let mut app = App::new(config)?;

    if let Err(e) = app.subsonic.ping().await {
        if ratune_subsonic::is_auth_failure(e.as_ref()) {
            eprintln!(
                "error: authentication failed for Subsonic server at {}",
                app.config.subsonic_url
            );
            eprintln!(
                "Authentication failed (wrong username or password).\n\
                 Check [server] url, username, password, password_command, OS keyring, or SUBSONIC_PASS."
            );
            eprintln!("{e:#}");
            process::exit(1);
        }
        eprintln!(
            "warn: could not reach Subsonic server at {} — starting offline (cache and saved state)",
            app.config.subsonic_url
        );
        app.server_reachable = false;
        app.flash_status_secs("Server unreachable — offline (cached content only)", 8);
    }

    // Detect tmux first: $TMUX is set when running inside a tmux session.
    app.in_tmux = std::env::var("TMUX").is_ok();
    if app.in_tmux {
        app.tmux_status_offset = tmux_status_offset();
    }

    app.visualizer_gradient_rgb_cache = ui::terminal_palette::try_query_visualizer_gradient_cache(
        &app.theme,
        &app.config,
        app.in_tmux,
    );

    // kitty-apc backend: probe before raw mode / alternate screen (see `kitty_art`).
    // `ratatui-image` uses `Picker::from_query_stdio()` after the alternate screen.
    match app.config.album_art_backend {
        AlbumArtBackend::KittyApc => {
            app.kitty_supported = if app.in_tmux {
                true
            } else {
                ui::kitty_art::detect_kitty_support()
            };
            if app.kitty_apc_graphics_ready() {
                app.cell_px = ui::kitty_art::query_cell_pixel_size();
            }
        }
        AlbumArtBackend::RatatuiImage => {
            app.kitty_supported = false;
        }
    }

    // Restore previous session state (selections, queue) before first render.
    if let Err(e) = persist::restore_state(&mut app) {
        eprintln!("warn: could not restore state: {e}");
    }

    // Load play history.
    let history_path = history::history_path();
    match history::PlayHistory::load(&history_path) {
        Ok(h) => app.history = h,
        Err(e) => eprintln!("warn: could not load history: {e}"),
    }

    if app.config.scrobble_enabled && app.server_reachable {
        if !app.scrobble_queue.is_empty() {
            eprintln!(
                "scrobble: retrying {} queued scrobble(s)…",
                app.scrobble_queue.len()
            );
        }
        app.spawn_scrobble_queue_flush();
    }

    // `refresh_home_data()` only ran when navigating to Home — not on cold start. If we restore
    // or default to Home, populate lists and kick art fetches before the first frame.
    if app.active_tab == Tab::Home {
        app.refresh_home_data();
        app.home_art_needs_redraw = true;
    }

    if app.server_reachable {
        // Begin fetching library metadata for the browse tab.
        if app.browser_browse_mode == crate::config::BrowseMode::Files {
            app.fetch_music_folders();
        } else {
            app.fetch_artists();
        }
        // Background metadata index refresh when missing or stale (Milestone 2).
        app.spawn_library_index_refresh(false);
        app.fetch_starred();
        if app.config.radio_enabled && app.server_reachable {
            app.fetch_radio_stations();
        }
    } else {
        app.prepare_offline_browse();
        if app.browser_browse_mode != BrowseMode::Files {
            app.fetch_artists();
        } else {
            app.folders.roots = LoadingState::Error(
                "Folder browse requires server — switch to artists (config or toggle)".into(),
            );
        }
    }

    // Spawn a task that sets a flag on SIGTERM, SIGHUP, SIGPIPE, or SIGINT so the main loop
    // can shut down cleanly (same path as pressing `q`).
    let signal_quit = Arc::new(AtomicBool::new(false));
    {
        let flag = signal_quit.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm =
                signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
            let mut sighup =
                signal(SignalKind::hangup()).expect("failed to install SIGHUP handler");
            // SIGPIPE: stdout/stdin fd closed (e.g. tmux pane killed while piped).
            let mut sigpipe =
                signal(SignalKind::pipe()).expect("failed to install SIGPIPE handler");
            let mut sigint =
                signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
            tokio::select! {
                _ = sigterm.recv() => {}
                _ = sighup.recv()  => {}
                _ = sigpipe.recv() => {}
                _ = sigint.recv() => {}
            }
            flag.store(true, Ordering::Relaxed);
        });
    }

    // Set up terminal.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(EnableMouseCapture)?;
    // Enable focus-change reporting. Inside tmux the bare CSI sequence is
    // swallowed by tmux itself; wrap it in a DCS passthrough so the outer
    // terminal (Ghostty) receives it.  Outside tmux the crossterm helper is fine.
    if app.in_tmux {
        use std::io::Write;
        stdout.write_all(b"\x1bPtmux;\x1b\x1b[?1004h\x1b\\")?;
        stdout.flush()?;
    } else {
        stdout.execute(EnableFocusChange)?;
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    if matches!(app.config.album_art_backend, AlbumArtBackend::RatatuiImage) {
        // Offload NP resize+encode (Sixel etc.) so tab switches are not blocked on the main thread.
        let (tx_job, rx_job) = std::sync::mpsc::channel::<ratatui_image::thread::ResizeRequest>();
        let (tx_done, rx_done) = std::sync::mpsc::channel::<
            Result<ratatui_image::thread::ResizeResponse, ratatui_image::errors::Errors>,
        >();
        std::thread::spawn(move || {
            while let Ok(req) = rx_job.recv() {
                let _ = tx_done.send(req.resize_encode());
            }
        });
        app.ratatui_resize_tx = Some(tx_job);
        app.ratatui_resize_rx = Some(rx_done);

        match ratatui_image::picker::Picker::from_query_stdio() {
            Ok(mut p) => {
                p.set_background_color(theme::color_to_rgba(app.theme.surface));
                app.cell_px = Some(p.font_size());
                app.art_picker = Some(p);
            }
            Err(e) => {
                eprintln!("warn: album art (ratatui-image): terminal query failed: {e}");
            }
        }
    }

    #[cfg(target_os = "linux")]
    let mpris_ctrl_rx = if let Some((link, rx)) = mpris::setup(app.config.mpris_enabled) {
        app.mpris = Some(link);
        app.mpris_sync_now();
        Some(rx)
    } else {
        None
    };
    #[cfg(not(target_os = "linux"))]
    let mpris_ctrl_rx: Option<std::sync::mpsc::Receiver<crate::mpris::MprisControl>> = None;

    let result = run_loop(&mut terminal, &mut app, signal_quit, mpris_ctrl_rx).await;

    #[cfg(target_os = "linux")]
    if let Some(m) = app.mpris.take() {
        m.shutdown();
    }

    // Clear any Kitty APC placements before leaving the alternate screen.
    if app.kitty_apc_overlay_active() {
        let _ = ui::kitty_art::clear_image(app.in_tmux);
    }

    // Restore terminal regardless of errors.
    disable_raw_mode()?;
    terminal.backend_mut().execute(DisableMouseCapture)?;
    if app.in_tmux {
        use std::io::Write;
        terminal
            .backend_mut()
            .write_all(b"\x1bPtmux;\x1b\x1b[?1004l\x1b\\")?;
        std::io::Write::flush(terminal.backend_mut())?;
    } else {
        terminal.backend_mut().execute(DisableFocusChange)?;
    }
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Shut down the audio engine cleanly.
    // Send Quit so the thread stops playback and releases the audio device.
    // Then join with a 1-second timeout; if the thread is stuck on a network
    // fetch (blocking download), detach it — the OS will clean it up on exit.
    let _ = app.player_tx.send(ratune_player::PlayerCommand::Quit);
    if let Some(handle) = app.player_join.take() {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let _ = handle.join();
            let _ = done_tx.send(());
        });
        let _ = done_rx.recv_timeout(Duration::from_secs(1));
    }

    result
}

/// Suspend the TUI and run `fzf` over the local metadata index.
fn run_library_fzf_picker(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    last_rendered_art: &mut Option<(u64, Rect)>,
    art_displayed: &mut bool,
) -> anyhow::Result<()> {
    use crate::fzf_picker;
    use crate::library_index;

    app.pending_gg = false;

    if !app.config.library_index_enabled {
        app.flash_status("Library index is disabled in config");
        return Ok(());
    }
    if app.library_index_refreshing {
        app.flash_status_secs("Library index refresh in progress — try again shortly", 8);
        return Ok(());
    }
    if app.library_index_tracks.is_empty() {
        app.flash_status("Library index empty — wait for refresh or use the index refresh shortcut (default Ctrl+g)");
        return Ok(());
    }

    let tracks = if !app.server_reachable && app.config.cache_enabled {
        app.cache.filter_cached_tracks(&app.library_index_tracks)
    } else {
        app.library_index_tracks.clone()
    };
    if tracks.is_empty() {
        app.flash_status("No cached tracks — play online to download audio");
        return Ok(());
    }

    fzf_picker::suspend_tui(terminal, app.in_tmux)?;
    let cols = app.config.fzf.columns;
    let input = library_index::fzf_input_lines(&tracks, cols);
    let mut fzf_args = app.config.fzf.args.clone();
    if !fzf_args.iter().any(|a| a.starts_with("--header")) {
        fzf_args.insert(
            0,
            format!("--header={}", library_index::fzf_header_line(cols)),
        );
    }
    let fzf_args = fzf_picker::prepare_library_fuzzy_picker_args(&app.config.fzf.binary, fzf_args);
    let res = fzf_picker::run_fzf(&app.config.fzf.binary, &fzf_args, &input);
    if let Err(e) = fzf_picker::resume_tui(terminal, app.in_tmux) {
        eprintln!("resume terminal after fzf: {e}");
    }
    // Subprocess UI may leave the alternate buffer and Kitty graphics out of sync;
    // clear everything and force a full redraw on the next frame.
    if let Err(e) = terminal.clear() {
        eprintln!("terminal clear after fzf: {e}");
    }
    if app.kitty_apc_overlay_active() {
        let _ = ui::kitty_art::clear_image(app.in_tmux);
        *last_rendered_art = None;
        *art_displayed = false;
        if app.active_tab == Tab::Home {
            app.home_art_needs_redraw = true;
        }
    }
    if app.ratatui_art_ready() && !app.ratatui_uses_kitty_apc() {
        app.clear_ratatui_art_state();
    }
    match res {
        Ok(Some(lines)) => {
            let (replace, rows) = fzf_picker::parse_fzf_output_lines(&lines);
            let ids: Vec<String> = rows
                .iter()
                .filter_map(|line| library_index::parse_pick_line(line))
                .collect();
            if !ids.is_empty() {
                app.apply_library_index_picks(&ids, replace);
            }
        }
        Ok(None) => {}
        Err(e) => app.flash_status(format!("fzf: {e}")),
    }
    Ok(())
}

/// Merge completed Now Playing `ThreadProtocol` encodes from the worker thread.
fn drain_ratatui_np_resize_completions(app: &mut App) {
    let Some(rx) = app.ratatui_resize_rx.as_ref() else {
        return;
    };
    while let Ok(done) = rx.try_recv() {
        match done {
            Ok(res) => {
                if let Some(np) = app.np_art_state.as_mut() {
                    let _ = np.update_resized_protocol(res);
                }
            }
            Err(e) => eprintln!("now playing art: {e}"),
        }
    }
}

fn terminal_size_or_quit(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<Option<ratatui::layout::Rect>> {
    loop {
        match terminal.size() {
            Ok(sz) if sz.width == 0 || sz.height == 0 => {
                app.should_quit = true;
                return Ok(None);
            }
            Ok(sz) => return Ok(Some(Rect::new(0, 0, sz.width, sz.height))),
            Err(e) if tty::io_disconnect(&e) => {
                app.should_quit = true;
                return Ok(None);
            }
            Err(e) if tty::io_interrupted(&e) => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    signal_quit: Arc<AtomicBool>,
    mpris_ctrl_rx: Option<std::sync::mpsc::Receiver<crate::mpris::MprisControl>>,
) -> Result<()> {
    // `last_rendered_art` — the (bytes_digest, rect) of the last full image
    // transmission.  Kept across tab switches so we can detect whether a
    // re-transmit is actually needed (digest matches identical pixels even if
    // `cover_id` differs per track).
    //
    // `art_displayed` — whether the image is currently visible on screen.
    // Set to false when switching away (ratatui overwrites those cells) but we
    // deliberately do NOT clear the image from the terminal's store, so we can
    // redisplay it instantly with `a=p,i=1` when switching back.
    let mut last_rendered_art: Option<(u64, Rect)> = None;
    let mut art_displayed = false;
    // Cover id for which Kitty transmit failed (e.g. undecodable bytes). Without
    // this latch the loop retries every frame and spams stderr.
    let mut kitty_cover_unrenderable: Option<String> = None;
    let mut last_tab = app.active_tab;

    // 2-second fallback: nudge Kitty art re-transmit when it is missing.
    // Checked once per loop iteration (see below).
    let mut last_art_recovery_fire = Instant::now();
    let connectivity_interval = if app.config.connection_check_interval_secs > 0 {
        Duration::from_secs(app.config.connection_check_interval_secs)
    } else {
        Duration::MAX
    };
    let mut last_connectivity_check = Instant::now();

    loop {
        let frame_t0 = Instant::now();
        let poll_ms = if app.visualizer_visible || app.accent_transition_active() {
            33
        } else {
            50
        };

        // Check for SIGTERM / SIGHUP / SIGPIPE / SIGINT from the signal handler task.
        if signal_quit.load(Ordering::Relaxed) {
            app.should_quit = true;
        }
        if tty::terminal_disconnected() {
            app.should_quit = true;
        }

        // Drain library updates from background tokio tasks.
        while let Ok(update) = app.library_rx.try_recv() {
            app.apply_library_update(update);
        }
        // Drain player events from the audio thread.
        while let Ok(event) = app.player_rx.try_recv() {
            app.handle_player_event(event);
        }

        if let Some(rx) = &mpris_ctrl_rx {
            while let Ok(c) = rx.try_recv() {
                app.handle_mpris_control(c);
            }
        }

        // Advance colour transition before drawing.
        app.tick_accent_transition();

        // Compute FFT bands for the visualizer (no-op when not visible).
        app.tick_visualizer();

        // Expire status flash messages.
        app.tick_status_flash();
        app.tick_playlist_tracks_fetch();

        if connectivity_interval != Duration::MAX
            && last_connectivity_check.elapsed() >= connectivity_interval
        {
            last_connectivity_check = Instant::now();
            app.spawn_connectivity_check(false);
        }

        // Apply completed NP encodes before draw (previous frame) and after draw (same-frame worker).
        drain_ratatui_np_resize_completions(app);

        match terminal.draw(|f| ui::render(app, f)) {
            Ok(_) => {}
            Err(e) if tty::io_disconnect(&e) => app.should_quit = true,
            // EINTR during SIGWINCH: skip this present; next frame redraws cleanly.
            Err(e) if tty::io_interrupted(&e) => {}
            Err(e) => return Err(e.into()),
        }
        match Backend::flush(terminal.backend_mut()) {
            Ok(()) => {}
            Err(e) if tty::io_disconnect(&e) => app.should_quit = true,
            Err(e) if tty::io_interrupted(&e) => {}
            Err(e) => return Err(e.into()),
        }

        app.apply_home_strip_resize_settle();

        drain_ratatui_np_resize_completions(app);

        if app.ratatui_art_ready() && !app.ratatui_uses_kitty_apc() {
            for (_id, st) in app.home_strip_art.iter_mut() {
                if let Some(Err(e)) = st.last_encoding_result() {
                    eprintln!("home strip art: {e}");
                }
            }
        }

        // ── Kitty APC album art (rendered after ratatui so it sits above text) ─
        if app.kitty_apc_overlay_active() {
            if app.active_tab == app::Tab::NowPlaying {
                // New cover id → drop any "unrenderable" latch from a previous track.
                match (&app.art_cache, &kitty_cover_unrenderable) {
                    (Some((cid, _)), Some(bad)) if bad != cid => {
                        kitty_cover_unrenderable = None;
                    }
                    _ => {}
                }

                // On every entry to NowPlaying (including initial load) drop any
                // cached render state so the art is fully re-transmitted this frame.
                // The fast display_image() path (a=p,i=1) can silently fail if the
                // terminal evicted the stored image; full re-transmit is always reliable.
                if last_tab != app::Tab::NowPlaying {
                    last_rendered_art = None;
                    art_displayed = false;
                }

                if app.help_visible {
                    // Popup is open — clear any displayed art so the Kitty
                    // image doesn't paint over the ratatui popup layer.
                    if art_displayed {
                        let _ = ui::kitty_art::clear_image(app.in_tmux);
                        art_displayed = false;
                    }
                } else {
                    let Some(terminal_rect) = terminal_size_or_quit(terminal, app)? else {
                        last_tab = app.active_tab;
                        if app.should_quit {
                            break;
                        }
                        continue;
                    };
                    if app.np_art_cache_matches() {
                        if let (Some(fp), Some(cover_id)) = (
                            app.art_cache_fingerprint,
                            app.art_cache.as_ref().map(|(id, _)| id.clone()),
                        ) {
                            let show_art = app.config.nowplaying_show_art;
                            let layout_opts = ui::layout::layout_options_for_app(app);
                            let center =
                                ui::layout::build_layout(terminal_rect, &layout_opts).center;

                            let boxed = app
                                .config
                                .now_playing_layout
                                .trim()
                                .eq_ignore_ascii_case("boxed");

                            let art_position =
                                ui::layout::placement_from_str(&app.config.nowplaying_art_position)
                                    .unwrap_or(ui::layout::Placement::Left);
                            let queue_position = ui::layout::placement_from_str(
                                &app.config.nowplaying_queue_position,
                            )
                            .unwrap_or(ui::layout::Placement::Right);
                            let visualizer_position =
                                ui::layout::placement_from_str(&app.config.visualizer_location)
                                    .unwrap_or(ui::layout::Placement::Right);
                            let now_playing_position = ui::layout::placement_from_str(
                                &app.config.now_playing_box_location,
                            )
                            .unwrap_or(ui::layout::Placement::Right);
                            let lyrics_position =
                                ui::layout::placement_from_str(&app.config.lyrics_location)
                                    .unwrap_or(queue_position);

                            let rects = ui::layout::now_playing_rects(
                                center,
                                show_art,
                                art_position,
                                queue_position,
                                app.config.nowplaying_left_width_percent,
                                app.config.nowplaying_vertical_fill_top_percent,
                                app.visualizer_visible,
                                visualizer_position,
                                app.lyrics_visible,
                                lyrics_position,
                                boxed,
                                now_playing_position,
                            );
                            let art_rect_opt = rects.art;

                            if kitty_cover_unrenderable.as_deref() == Some(cover_id.as_str()) {
                                if art_displayed {
                                    let _ = ui::kitty_art::clear_image(app.in_tmux);
                                    art_displayed = false;
                                }
                            } else if let Some(art_rect) = art_rect_opt {
                                let inner = ui::kitty_art::album_art_placeholder_inner(art_rect);
                                let font = app
                                    .art_picker
                                    .as_ref()
                                    .map(|p| p.font_size())
                                    .or(app.cell_px)
                                    .unwrap_or((10, 20));
                                let placement = if app.ensure_art_cache_decoded() {
                                    app.art_cache_decoded.as_ref().map(|(_, img)| {
                                        ui::art_prepare::contain_fit_rect_in_cells(img, inner, font)
                                    })
                                } else {
                                    None
                                }
                                .unwrap_or(inner);
                                if placement.width == 0 || placement.height == 0 {
                                    if art_displayed {
                                        let _ = ui::kitty_art::clear_image(app.in_tmux);
                                        art_displayed = false;
                                    }
                                    last_rendered_art = None;
                                } else {
                                    let stored_matches = last_rendered_art
                                        .as_ref()
                                        .map(|(last_fp, r)| *last_fp == fp && r == &placement)
                                        .unwrap_or(false);

                                    if stored_matches && art_displayed {
                                        // Image is already visible — nothing to do.
                                    } else {
                                        let prepared =
                                            app.ensure_np_kitty_prepared(placement, font).cloned();
                                        let in_tmux = app.in_tmux;
                                        let tmux_offset = app.tmux_status_offset;
                                        if let Some(prepared) = prepared {
                                            match ui::kitty_art::transmit_np_image(
                                                &prepared,
                                                placement,
                                                in_tmux,
                                                tmux_offset,
                                            ) {
                                                Ok(()) => {
                                                    last_rendered_art = Some((fp, placement));
                                                    art_displayed = true;
                                                }
                                                Err(e) => {
                                                    eprintln!("kitty render: {e}");
                                                    let _ = ui::kitty_art::clear_image(app.in_tmux);
                                                    kitty_cover_unrenderable =
                                                        Some(cover_id.clone());
                                                    last_rendered_art = None;
                                                    art_displayed = false;
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if art_displayed {
                                // Art column hidden — clear Kitty overlay.
                                let _ = ui::kitty_art::clear_image(app.in_tmux);
                                last_rendered_art = None;
                                art_displayed = false;
                            }
                        }
                    } else if art_displayed {
                        // Stale cover (e.g. queue album art while radio is playing).
                        let _ = ui::kitty_art::clear_image(app.in_tmux);
                        last_rendered_art = None;
                        art_displayed = false;
                    }
                }
            } else if last_tab != app.active_tab {
                // Switched away from any tab — clear any visible Kitty
                // placement so it doesn't float above the new tab's content.
                if art_displayed {
                    let _ = ui::kitty_art::clear_image(app.in_tmux);
                    art_displayed = false;
                }
            }

            // ── Home tab art strip redraw after popup close ───────────────────
            // When the `i` popup was closed on the Home tab, re-render the art
            // strip (it was cleared on popup-open to avoid overlapping the popup).
            if app.home_art_needs_redraw && app.active_tab == app::Tab::Home && !app.help_visible {
                if app.config.home_recent_albums_show_art {
                    if let Some(albums_inner) = app.home_recent_albums_inner {
                        ui::kitty_art::render_art_strip(
                            &app.home.recent_albums,
                            app.home.album_scroll_offset,
                            app.home.album_selected_index,
                            &app.home_art_cache,
                            &mut app.home_strip_thumb_prepared,
                            albums_inner,
                            app.cell_px,
                            albums_inner.x,
                            albums_inner.y,
                            app.in_tmux,
                            theme::surface_pad_rgba(app.theme.surface),
                        );
                    }
                } else if app.kitty_apc_overlay_active() {
                    // If Home is configured text-only, make sure no stale Kitty strip remains
                    // after popup close/tab return.
                    let _ = ui::kitty_art::clear_art_strip(app.in_tmux);
                }
                app.home_art_needs_redraw = false;
                if app.in_tmux {
                    app.home_art_last_tmux_render = Some(std::time::Instant::now());
                }
            }
        }
        last_tab = app.active_tab;

        // Block until the frame interval elapses *or* input is ready.  Previously we
        // slept first and only then polled stdin, which added a full period (50 ms)
        // of latency to every keypress.
        match tty::wait_for_input(poll_ms, &mut app.should_quit) {
            Err(e) => return Err(e),
            Ok(false) => {}
            Ok(true) => loop {
                let read_result = event::read();
                match read_result {
                    Err(e) if tty::io_disconnect(&e) => {
                        app.should_quit = true;
                    }
                    Err(e) if tty::io_interrupted(&e) => {}
                    Err(e) => return Err(e.into()),
                    Ok(ev) => match ev {
                        Event::Key(key)
                            // Only process key-press events; ignore release/repeat to avoid
                            // double-firing on terminals that send all event kinds (e.g. Kitty).
                            if key.kind == KeyEventKind::Press => {
                                if app.playlist_picker.is_some() && !app.help_visible {
                                    // Picker is open: highest priority — swallow all keys.
                                    let action = map_picker_key(key.code, key.modifiers);
                                    app.dispatch(action);
                                } else if app.radio.picker_visible
                                    && !app.radio.input_mode.is_normal()
                                    && !app.help_visible
                                {
                                    let action = map_radio_form_key(
                                        key.code,
                                        key.modifiers,
                                        &app.radio.input_mode,
                                    );
                                    app.dispatch(action);
                                } else if app.radio.picker_visible && !app.help_visible {
                                    let action = map_radio_picker_key(
                                        key.code,
                                        key.modifiers,
                                        &app.keybinds,
                                    );
                                    app.dispatch(action);
                                } else if app.favorites_overlay.visible
                                    && app.active_tab == Tab::Browser
                                    && !app.help_visible
                                {
                                    let is_tab_switch = matches!(
                                        key.code,
                                        KeyCode::Tab
                                            | KeyCode::BackTab
                                            |                                         KeyCode::Char('1')
                                            | KeyCode::Char('2')
                                            | KeyCode::Char('3')
                                    );
                                    let is_quit_in_normal =
                                        app.keybinds.quit.matches(key.code, key.modifiers);
                                    if is_tab_switch {
                                        app.favorites_overlay.visible = false;
                                        let action = map_key(
                                            key.code,
                                            key.modifiers,
                                            app.active_tab,
                                            &app.keybinds,
                                            &mut app.pending_gg,
                                        );
                                        app.dispatch(action);
                                    } else if is_quit_in_normal {
                                        app.favorites_overlay.visible = false;
                                    } else {
                                        let action = map_favorites_key(
                                            key.code,
                                            key.modifiers,
                                            &app.favorites_overlay.focus,
                                            &app.keybinds,
                                        );
                                        app.dispatch(action);
                                    }
                                } else if app.playlist_overlay.visible
                                    && app.active_tab == Tab::Browser
                                    && !app.help_visible
                                {
                                    // Tab-switch keys close the overlay and switch tabs.
                                    let is_tab_switch = matches!(
                                        key.code,
                                        KeyCode::Tab
                                            | KeyCode::BackTab
                                            |                                         KeyCode::Char('1')
                                            | KeyCode::Char('2')
                                            | KeyCode::Char('3')
                                    );
                                    // Quit key in Normal mode closes the overlay only;
                                    // the user must press q again (overlay closed) to quit.
                                    // In text-input modes q is a typed character — don't intercept.
                                    let is_quit_in_normal =
                                        app.keybinds.quit.matches(key.code, key.modifiers)
                                            && matches!(
                                                app.playlist_overlay.input_mode,
                                                PlaylistInputMode::Normal
                                            );
                                    if is_tab_switch {
                                        app.playlist_overlay.visible = false;
                                        let action = map_key(
                                            key.code,
                                            key.modifiers,
                                            app.active_tab,
                                            &app.keybinds,
                                            &mut app.pending_gg,
                                        );
                                        app.dispatch(action);
                                    } else if is_quit_in_normal {
                                        // Close overlay; do NOT quit.
                                        app.playlist_overlay.visible = false;
                                    } else {
                                        let action = map_playlist_key(
                                            key.code,
                                            key.modifiers,
                                            &app.playlist_overlay.focus,
                                            &app.playlist_overlay.input_mode,
                                            &app.keybinds,
                                        );
                                        app.dispatch(action);
                                    }
                                } else {
                                    let action = if app.help_visible {
                                        map_help_key(key.code, key.modifiers, &app.keybinds)
                                    } else if app.search_mode.active {
                                        map_search_key(key.code, key.modifiers)
                                    } else if app.search_filter.is_some()
                                        && search_clear_key(key.code, key.modifiers)
                                    {
                                        Action::SearchCancel
                                    } else if let Some(pending) = app.pending_global_confirm {
                                        match pending {
                                            GlobalConfirm::LibraryIndexRefresh => {
                                                match key.code {
                                                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                                                        Action::ConfirmLibraryIndexRefresh
                                                    }
                                                    KeyCode::Char('n')
                                                    | KeyCode::Char('N')
                                                    | KeyCode::Esc => Action::CancelGlobalConfirm,
                                                    _ => Action::None,
                                                }
                                            }
                                            GlobalConfirm::LibraryIndexAppendQueue => {
                                                match key.code {
                                                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                                                        Action::ConfirmLibraryIndexAppendQueue
                                                    }
                                                    KeyCode::Char('n')
                                                    | KeyCode::Char('N')
                                                    | KeyCode::Esc => Action::CancelGlobalConfirm,
                                                    _ => Action::None,
                                                }
                                            }
                                            GlobalConfirm::LibraryServerAppendQueue => {
                                                match key.code {
                                                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                                                        Action::ConfirmLibraryServerAppendQueue
                                                    }
                                                    KeyCode::Char('n')
                                                    | KeyCode::Char('N')
                                                    | KeyCode::Esc => Action::CancelGlobalConfirm,
                                                    _ => Action::None,
                                                }
                                            }
                                        }
                                    } else {
                                        map_key(
                                            key.code,
                                            key.modifiers,
                                            app.active_tab,
                                            &app.keybinds,
                                            &mut app.pending_gg,
                                        )
                                    };
                                    match action {
                                        Action::LibraryFzfPicker => {
                                            if let Err(e) = run_library_fzf_picker(
                                                app,
                                                terminal,
                                                &mut last_rendered_art,
                                                &mut art_displayed,
                                            ) {
                                                eprintln!("fzf picker: {e}");
                                            }
                                        }
                                        other => app.dispatch(other),
                                    }
                                }
                            }
                        Event::Mouse(mouse) => {
                            let Some(area) = terminal_size_or_quit(terminal, app)? else {
                                app.should_quit = true;
                                break;
                            };
                            match mouse.kind {
                                MouseEventKind::Down(MouseButton::Left) => {
                                    handle_mouse_click(mouse.column, mouse.row, app, area);
                                }
                                MouseEventKind::ScrollUp => {
                                    handle_mouse_wheel(
                                        mouse.column,
                                        mouse.row,
                                        Direction::Up,
                                        app,
                                        area,
                                    );
                                }
                                MouseEventKind::ScrollDown => {
                                    handle_mouse_wheel(
                                        mouse.column,
                                        mouse.row,
                                        Direction::Down,
                                        app,
                                        area,
                                    );
                                }
                                _ => {}
                            }
                        }
                        Event::Resize(_, _) => {
                            // Invalidate cached art geometry; re-encode on the next draw.
                            // Avoid clear_image here — clearing on every resize tick while
                            // dragging a window causes visible flicker (especially Kitty APC).
                            if app.kitty_apc_overlay_active() && art_displayed {
                                art_displayed = false;
                                last_rendered_art = None;
                            }
                            // Now Playing ratatui art — must rebuild for new layout.
                            if app.ratatui_art_ready() && !app.ratatui_uses_kitty_apc() {
                                app.clear_np_ratatui_art_state();
                            }
                            // Home strip: debounced — avoids re-encoding sixel/Kitt strip on every resize tick.
                            app.schedule_home_strip_resize_invalidate();
                        }
                        // tmux focus events (requires `focus-events on` in tmux.conf).
                        // Crossterm also reports WM focus (another app focused) when
                        // `EnableFocusChange` is on — do not treat that like a tmux pane
                        // switch: the alternate-screen buffer is unchanged, so clearing
                        // ratatui-image state would re-encode Sixel on every refocus.
                        //
                        // FocusLost  → tmux only: clear Kitty overlays / ratatui state so
                        //              graphics don't bleed into another pane.
                        // FocusGained → Kitty APC: always force re-transmit (terminal may
                        //              have evicted the stored image). Ratatui: same as
                        //              FocusLost — only under tmux.
                        //
                        Event::FocusLost => {
                            if app.kitty_apc_overlay_active() && app.in_tmux {
                                let _ = ui::kitty_art::clear_image(app.in_tmux);
                                let _ = ui::kitty_art::clear_art_strip(app.in_tmux);
                                art_displayed = false;
                            }
                            if app.ratatui_art_ready()
                                && !app.ratatui_uses_kitty_apc()
                                && app.in_tmux
                            {
                                app.clear_ratatui_art_state();
                            }
                        }
                        Event::FocusGained => {
                            if app.kitty_apc_overlay_active() {
                                // Force a full art re-transmit on the next frame — same
                                // mechanism as tab return (last_rendered_art = None makes
                                // stored_matches false, taking the re-encode path).
                                art_displayed = false;
                                last_rendered_art = None;
                            }
                            if app.ratatui_art_ready()
                                && !app.ratatui_uses_kitty_apc()
                                && app.in_tmux
                            {
                                app.clear_ratatui_art_state();
                            }
                        }
                        _ => {}
                    }, // end Ok(ev) match
                } // end read_result match

                if app.should_quit {
                    break;
                }

                if !tty::stdin_has_input() {
                    break;
                }
            },
        }

        if last_art_recovery_fire.elapsed() >= Duration::from_secs(2) {
            last_art_recovery_fire = Instant::now();
            let latched_bad = matches!(
                (&app.art_cache, &kitty_cover_unrenderable),
                (Some((cid, _)), Some(bad)) if bad == cid
            );
            if app.kitty_apc_overlay_active()
                && !art_displayed
                && app.np_art_cache_matches()
                && app.art_cache.is_some()
                && app.active_tab == app::Tab::NowPlaying
                && !app.help_visible
                && !latched_bad
            {
                last_rendered_art = None;
            }
        }

        // Drain once more so any triggered playback reflects on next frame.
        while let Ok(event) = app.player_rx.try_recv() {
            app.handle_player_event(event);
        }
        if let Some(rx) = &mpris_ctrl_rx {
            while let Ok(c) = rx.try_recv() {
                app.handle_mpris_control(c);
            }
        }

        if app.should_quit {
            break;
        }

        // When stdin had a burst (resize drag, key repeat), `wait_for_input` may return
        // immediately; cap the loop at ~20–30 FPS so we don't full-redraw as fast as possible.
        let frame_budget = Duration::from_millis(poll_ms);
        let spent = frame_t0.elapsed();
        if spent < frame_budget {
            std::thread::sleep(frame_budget - spent);
        }
    }
    // Persist UI state on clean quit.
    if let Err(e) = persist::save_state(app) {
        eprintln!("warn: could not save state: {e}");
    }
    // Persist play history.
    let history_path = history::history_path();
    if let Err(e) = app.history.save(&history_path) {
        eprintln!("warn: could not save history: {e}");
    }
    app.persist_scrobble_queue();
    Ok(())
}

/// Obtain a Last.fm / Libre.fm session key via the browser authorization flow.
pub async fn scrobble_auth(save_keyring: bool) -> Result<()> {
    use std::io::{self, Write};

    use ratune_scrobble::AuthClient;

    keyring_init::install_default_keyring_store();

    let (service, api_key, api_secret) = config::load_scrobble_app_credentials()?;
    let client = AuthClient::new(service, api_key, api_secret);

    eprintln!(
        "Requesting auth token from {}…",
        client.service().display_name()
    );
    let token = client.get_token().await?;
    let url = client.authorize_url(&token);

    eprintln!();
    eprintln!("Open this URL in a browser and approve access:");
    eprintln!("  {url}");
    eprintln!();
    eprint!("Press Enter after authorizing… ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;

    eprintln!("Exchanging token for session key…");
    let session = client.get_session(&token).await?;

    eprintln!();
    eprintln!("Authorized as: {}", session.username);
    eprintln!();

    if save_keyring {
        match config::store_scrobble_session_key(client.service(), &session.key) {
            Ok(()) => {
                eprintln!(
                    "Session key saved to the OS keyring (service \"ratune\", user \"{}\", {}).",
                    scrobble_keyring_user_label(client.service(), "session"),
                    keyring_init::KeyringBackend::scrobble().label()
                );
                eprintln!("You can leave session_key empty in config and set enabled = true.");
            }
            Err(e) => {
                eprintln!("warning: could not save session key to keyring: {e:#}");
                eprintln!("Add it manually to ~/.config/ratune/config.toml:");
                eprintln!("  session_key = \"{}\"", session.key);
            }
        }
    } else {
        eprintln!("Add to ~/.config/ratune/config.toml under [scrobble]:");
        eprintln!("  enabled = true");
        eprintln!("  session_key = \"{}\"", session.key);
        eprintln!();
        eprintln!("Or store in the OS keyring and leave session_key empty:");
        eprintln!("  ratune scrobble-auth --save-keyring");
        eprintln!();
        eprintln!("Or export for the current shell:");
        eprintln!("  export LASTFM_SESSION_KEY=\"{}\"", session.key);
    }

    Ok(())
}

fn scrobble_service_display(service: ratune_scrobble::ScrobbleService) -> &'static str {
    match service {
        ratune_scrobble::ScrobbleService::LastFm => "Last.fm",
        ratune_scrobble::ScrobbleService::LibreFm => "Libre.fm",
    }
}

fn scrobble_keyring_user_label(service: ratune_scrobble::ScrobbleService, kind: &str) -> &str {
    match (service, kind) {
        (ratune_scrobble::ScrobbleService::LastFm, "api_secret") => "lastfm|api_secret",
        (ratune_scrobble::ScrobbleService::LibreFm, "api_secret") => "librefm|api_secret",
        (ratune_scrobble::ScrobbleService::LastFm, "session") => "lastfm|session",
        (ratune_scrobble::ScrobbleService::LibreFm, "session") => "librefm|session",
        (_, other) => other,
    }
}

/// Prompt for the Last.fm / Libre.fm API shared secret.
pub fn scrobble_api_secret(save_keyring: bool) -> Result<()> {
    use inquire::Password;

    keyring_init::install_default_keyring_store();

    let (service, api_key) = config::load_scrobble_api_key()?;
    eprintln!(
        "{} API shared secret for application key {api_key}",
        scrobble_service_display(service)
    );

    let secret = Password::new("API shared secret:")
        .without_confirmation()
        .prompt()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let secret = secret.trim();
    if secret.is_empty() {
        anyhow::bail!("empty API secret");
    }

    if save_keyring {
        match config::store_scrobble_api_secret(service, secret) {
            Ok(()) => {
                eprintln!(
                    "API secret saved to the OS keyring (service \"ratune\", user \"{}\", {}).",
                    scrobble_keyring_user_label(service, "api_secret"),
                    keyring_init::KeyringBackend::scrobble().label()
                );
                eprintln!("You can leave api_secret empty in config and unset LASTFM_API_SECRET.");
            }
            Err(e) => {
                eprintln!("warning: could not save API secret to keyring: {e:#}");
                eprintln!("Add it manually to ~/.config/ratune/config.toml:");
                eprintln!("  api_secret = \"{secret}\"");
            }
        }
    } else {
        eprintln!();
        eprintln!("Add to ~/.config/ratune/config.toml under [scrobble]:");
        eprintln!("  api_secret = \"{secret}\"");
        eprintln!();
        eprintln!("Or store in the OS keyring and leave api_secret empty:");
        eprintln!("  ratune scrobble-api-secret --save-keyring");
        eprintln!();
        eprintln!("Or export for the current shell:");
        eprintln!("  export LASTFM_API_SECRET=\"{secret}\"");
    }

    Ok(())
}

/// Handle mouse clicks within the Home tab center area.
fn handle_home_click(x: u16, y: u16, app: &mut App, center: ratatui::layout::Rect) {
    use crate::ui::home_tab::compute_home_layout;

    if y < center.y || y >= center.y + center.height {
        return;
    }

    let Some(layout) = compute_home_layout(center, &app.config) else {
        return;
    };

    // ── Top band ─────────────────────────────────────────────────────────────
    if y >= layout.top.y && y < layout.top.y + layout.top.height {
        home_click_panel(x, y, layout.top, layout.top_panel, app);
        return;
    }

    if layout.bottom_h == 0 {
        return;
    }

    if x >= layout.bottom_left.x && x < layout.bottom_left.x + layout.bottom_left.width {
        home_click_panel(x, y, layout.bottom_left, layout.bottom_left_panel, app);
        return;
    }

    if x >= layout.bottom_right.x && x < layout.bottom_right.x + layout.bottom_right.width {
        home_click_panel(x, y, layout.bottom_right, layout.bottom_right_panel, app);
    }
}

fn home_click_panel(x: u16, y: u16, area: Rect, panel: HomePanel, app: &mut App) {
    match panel {
        HomePanel::RecentAlbums => {
            let inner = crate::ui::home_tab::home_panel_inner(area);
            let album_count = app.home.recent_albums.len();
            let Some(album_index) = crate::ui::home_tab::home_recent_album_index_at(
                x,
                y,
                inner,
                app.config.home_recent_albums_show_art,
                app.home.album_scroll_offset,
                album_count,
            ) else {
                return;
            };
            if mouse_click::is_double_click(
                app,
                mouse_click::MouseClickTarget::HomeRecentAlbum(album_index),
            ) {
                app.dispatch(Action::HomeAlbumAddToQueue);
            } else {
                app.home.active_section = app::HomeSection::RecentAlbums;
                app.home.selected_index = 0;
                app.home.album_selected_index = album_index;
            }
        }
        HomePanel::RecentTracks => {
            let inner_y = area.y + 1;
            let inner_h = area.height.saturating_sub(2);
            if y < inner_y || y >= inner_y + inner_h {
                return;
            }
            let row = (y - inner_y) as usize;
            if row < app.home.recent_tracks.len() {
                if mouse_click::is_double_click(
                    app,
                    mouse_click::MouseClickTarget::HomeRecentTrack(row),
                ) {
                    app.append_home_recent_track(row);
                } else {
                    app.home.active_section = app::HomeSection::RecentTracks;
                    app.home.selected_index = row;
                }
            }
        }
        HomePanel::Rediscover => {
            let inner_y = area.y + 1;
            let inner_h = area.height.saturating_sub(2);
            if y < inner_y || y >= inner_y + inner_h {
                return;
            }
            let row = (y - inner_y) as usize;
            if row < app.home.rediscover.len() {
                if mouse_click::is_double_click(
                    app,
                    mouse_click::MouseClickTarget::HomeRediscover(row),
                ) {
                    app.append_home_rediscover_artist(row);
                } else {
                    app.home.active_section = app::HomeSection::Rediscover;
                    app.home.selected_index = row;
                }
            }
        }
    }
}

/// Translate a key event into an `Action` when the favorites overlay is open.
fn map_favorites_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    _focus: &FavoritesFocus,
    kb: &Keybinds,
) -> Action {
    let shift = modifiers.intersects(KeyModifiers::SHIFT);
    match code {
        KeyCode::Esc => Action::ToggleFavoritesOverlay,
        _ if kb.favorites_overlay.matches(code, modifiers) => Action::ToggleFavoritesOverlay,
        _ if kb.toggle_favorite.matches(code, modifiers) => Action::ToggleFavorite,
        KeyCode::Char('k') | KeyCode::Up => Action::FavoritesScrollUp,
        KeyCode::Char('j') | KeyCode::Down => Action::FavoritesScrollDown,
        KeyCode::Char('h') => Action::FavoritesFocusCategories,
        KeyCode::Char('l') => Action::FavoritesFocusItems,
        KeyCode::Enter => Action::FavoritesPlay,
        _ => {
            if kb
                .add_all_replace_album
                .as_ref()
                .is_some_and(|spec| spec.matches(code, modifiers))
            {
                return Action::FavoritesPlay;
            }
            if code == KeyCode::Char('A') || (code == KeyCode::Char('a') && shift) {
                return Action::FavoritesAppend;
            }
            Action::None
        }
    }
}

fn map_radio_picker_key(code: KeyCode, modifiers: KeyModifiers, kb: &Keybinds) -> Action {
    if kb.toggle_radio.matches(code, modifiers) {
        return Action::ToggleRadioPicker;
    }
    if kb.home_refresh.matches(code, modifiers) {
        return Action::RadioRefresh;
    }
    let shift = modifiers.intersects(KeyModifiers::SHIFT);
    match code {
        KeyCode::Esc => Action::RadioPickerCancel,
        KeyCode::Enter => Action::RadioPickerSelect,
        KeyCode::Char('c') if !shift => Action::RadioCreate,
        KeyCode::Char('e') if !shift => Action::RadioEdit,
        KeyCode::Char('X') | KeyCode::Char('x') if code == KeyCode::Char('X') || shift => {
            Action::RadioDelete
        }
        KeyCode::Char('k') | KeyCode::Up => Action::Navigate(Direction::Up),
        KeyCode::Char('j') | KeyCode::Down => Action::Navigate(Direction::Down),
        _ => Action::None,
    }
}

fn map_radio_form_key(
    code: KeyCode,
    _modifiers: KeyModifiers,
    input_mode: &RadioInputMode,
) -> Action {
    match input_mode {
        RadioInputMode::ConfirmingDelete { .. } => match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Action::RadioConfirmYes,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Action::RadioConfirmNo,
            _ => Action::None,
        },
        RadioInputMode::Creating { .. } | RadioInputMode::Editing { .. } => match code {
            KeyCode::Esc => Action::RadioInputCancel,
            KeyCode::Enter => Action::RadioInputConfirm,
            KeyCode::Tab => Action::RadioFieldNext,
            KeyCode::BackTab => Action::RadioFieldPrev,
            KeyCode::Backspace => Action::RadioInputChar('\x08'),
            KeyCode::Char(ch) => Action::RadioInputChar(ch),
            _ => Action::None,
        },
        RadioInputMode::Normal => Action::None,
    }
}

/// Translate a key event into an `Action` when the playlist overlay is open.
///
/// Called instead of `map_key` whenever `playlist_overlay.visible` is true and
/// the active tab is Browser.  Every key that is not handled here produces
/// `Action::None`, so normal playback/volume keys are intentionally blocked
/// while the overlay is in the foreground.
fn map_playlist_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    focus: &PlaylistFocus,
    input_mode: &PlaylistInputMode,
    kb: &Keybinds,
) -> Action {
    match input_mode {
        // ── Text-input modes: feed characters into the buffer ──────────────
        PlaylistInputMode::Creating { .. } | PlaylistInputMode::Renaming { .. } => match code {
            KeyCode::Esc => Action::PlaylistInputCancel,
            KeyCode::Enter => Action::PlaylistInputConfirm,
            KeyCode::Backspace => Action::PlaylistInputChar('\x08'),
            KeyCode::Char(ch) => Action::PlaylistInputChar(ch),
            _ => Action::None,
        },
        // ── Confirmation prompt: y/n ───────────────────────────────────────
        PlaylistInputMode::Confirming { .. } => match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Action::PlaylistConfirmYes,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Action::PlaylistConfirmNo,
            _ => Action::None,
        },
        // ── Normal navigation / mutation ───────────────────────────────────
        PlaylistInputMode::Normal => {
            match code {
                KeyCode::Esc => Action::TogglePlaylistOverlay,
                _ if kb.playlist_overlay.matches(code, modifiers) => Action::TogglePlaylistOverlay,
                KeyCode::Char('k') | KeyCode::Up => Action::PlaylistScrollUp,
                KeyCode::Char('j') | KeyCode::Down => Action::PlaylistScrollDown,
                _ if kb.column_left.matches(code, modifiers)
                    || kb.seek_backward.matches(code, modifiers) =>
                {
                    Action::PlaylistFocusList
                }
                _ if kb.column_right.matches(code, modifiers)
                    || kb.seek_forward.matches(code, modifiers) =>
                {
                    Action::PlaylistFocusTracks
                }
                KeyCode::Char('h') => Action::PlaylistFocusList,
                KeyCode::Char('l') => Action::PlaylistFocusTracks,
                // c: create new playlist
                KeyCode::Char('c') => Action::PlaylistCreate,
                // X (Shift+x): delete selected playlist
                KeyCode::Char('X') | KeyCode::Char('x')
                    if code == KeyCode::Char('X') || modifiers.intersects(KeyModifiers::SHIFT) =>
                {
                    Action::PlaylistDelete
                }
                _ if kb.remove_from_playlist.matches(code, modifiers)
                    && matches!(focus, PlaylistFocus::Tracks) =>
                {
                    Action::PlaylistRemoveTrack
                }
                _ if kb
                    .add_all_replace_album
                    .as_ref()
                    .is_some_and(|spec| spec.matches(code, modifiers)) =>
                {
                    match focus {
                        PlaylistFocus::List => Action::PlaylistPlayAll,
                        PlaylistFocus::Tracks => Action::PlaylistPlayTrack,
                    }
                }
                // r: rename selected playlist (list pane)
                KeyCode::Char('r')
                    if modifiers.is_empty() && matches!(focus, PlaylistFocus::List) =>
                {
                    Action::PlaylistRename
                }
                KeyCode::Enter => match focus {
                    PlaylistFocus::List => Action::PlaylistPlayAll,
                    PlaylistFocus::Tracks => Action::PlaylistPlayTrack,
                },
                _ if kb.add_track.matches(code, modifiers)
                    && matches!(focus, PlaylistFocus::Tracks) =>
                {
                    Action::PlaylistAppendTrack
                }
                _ if kb.add_all.matches(code, modifiers)
                    && matches!(focus, PlaylistFocus::List) =>
                {
                    Action::PlaylistAppendAll
                }
                _ => Action::None,
            }
        }
    }
}

/// Translate a key event into an `Action` when the playlist picker popup is open.
fn map_picker_key(code: KeyCode, _modifiers: KeyModifiers) -> Action {
    match code {
        KeyCode::Esc => Action::PlaylistPickerCancel,
        KeyCode::Enter => Action::PlaylistPickerSelect,
        KeyCode::Char('k') | KeyCode::Up => Action::PlaylistPickerScrollUp,
        KeyCode::Char('j') | KeyCode::Down => Action::PlaylistPickerScrollDown,
        _ => Action::None,
    }
}

fn map_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    active_tab: Tab,
    kb: &Keybinds,
    pending_gg: &mut bool,
) -> Action {
    // Second `g` after a lone `g`: vim-style `gg` → top.
    if *pending_gg {
        *pending_gg = false;
        if code == KeyCode::Char('g') && modifiers.is_empty() {
            return Action::Navigate(Direction::Top);
        }
    }

    // ── Browser-tab-specific keys ─────────────────────────────────────────────
    if active_tab == Tab::Browser {
        if kb.playlist_overlay.matches(code, modifiers) {
            return Action::TogglePlaylistOverlay;
        }
        if kb.favorites_overlay.matches(code, modifiers) {
            return Action::ToggleFavoritesOverlay;
        }
        if kb.browser_add_to_playlist.matches(code, modifiers) {
            return Action::BrowserAddToPlaylist;
        }
    }

    // ── Home-tab-specific keys ────────────────────────────────────────────────
    if active_tab == Tab::Home {
        if kb.home_section_next.matches(code, modifiers) {
            return Action::HomeSectionNext;
        }
        // Extra Home-section aliases: Shift+h / Shift+l (sent as H/L or h/l+SHIFT).
        if (code == KeyCode::Char('H') && modifiers.is_empty())
            || (code == KeyCode::Char('h') && modifiers.intersects(KeyModifiers::SHIFT))
        {
            return Action::HomeSectionPrev;
        }
        if (code == KeyCode::Char('L') && modifiers.is_empty())
            || (code == KeyCode::Char('l') && modifiers.intersects(KeyModifiers::SHIFT))
        {
            return Action::HomeSectionNext;
        }
        if kb.home_section_prev.matches(code, modifiers) {
            return Action::HomeSectionPrev;
        }
        if kb.home_refresh.matches(code, modifiers) {
            return Action::HomeRefresh;
        }
        // Home: Ctrl+r (same bind as Browser replace-album) replaces the queue with the selected album.
        if let Some(spec) = &kb.add_all_replace_album {
            if spec.matches(code, modifiers) {
                return Action::HomeAlbumPlay;
            }
        }
        if kb.column_left.matches(code, modifiers) {
            return Action::HomeAlbumLeft;
        }
        if kb.column_right.matches(code, modifiers) {
            return Action::HomeAlbumRight;
        }
        if kb.add_track.matches(code, modifiers) {
            return Action::HomeAlbumAddToQueue;
        }
    }

    // ── Always-on / non-configurable ─────────────────────────────────────────
    // G: jump to bottom — not exposed in config. Top is `gg` (handled via pending_gg).
    // Terminals usually send Shift+G as `Char('G')` with SHIFT set, not bare `G`.
    if code == KeyCode::Char('G')
        && !modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::HYPER,
        )
    {
        return Action::Navigate(Direction::Bottom);
    }
    // Enter / Esc — not configurable
    if code == KeyCode::Enter {
        return Action::Select;
    }
    if code == KeyCode::Esc {
        return Action::Back;
    }
    // Space alone is an alias for play_pause.
    if code == KeyCode::Char(' ') && modifiers.is_empty() {
        return Action::PlayPause;
    }
    // '=' is always a secondary alias for volume_up (easy to hit with +)
    if code == KeyCode::Char('=') {
        return Action::VolumeUp;
    }
    if kb.toggle_help.matches(code, modifiers) {
        return Action::ToggleHelp;
    }
    if kb.toggle_favorite.matches(code, modifiers) {
        return Action::ToggleFavorite;
    }
    if kb.toggle_dynamic_theme.matches(code, modifiers) {
        return Action::ToggleDynamicTheme;
    }
    if kb.toggle_lyrics.matches(code, modifiers) {
        return Action::ToggleLyrics;
    }
    if kb.toggle_visualizer.matches(code, modifiers)
        || code == KeyCode::Char('V')
        || (code == KeyCode::Char('v') && modifiers.intersects(KeyModifiers::SHIFT))
    {
        return Action::ToggleVisualizer;
    }
    // Up/Down arrows are always secondary scroll aliases
    if code == KeyCode::Up {
        return Action::Navigate(Direction::Up);
    }
    if code == KeyCode::Down {
        return Action::Navigate(Direction::Down);
    }
    // PageUp/PageDown and vim-style Ctrl+u / Ctrl+d: lists on these tabs.
    if matches!(active_tab, Tab::Browser | Tab::Home | Tab::NowPlaying) {
        let ctrl = modifiers.intersects(KeyModifiers::CONTROL)
            && !modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        if code == KeyCode::PageUp
            || (ctrl && matches!(code, KeyCode::Char('u') | KeyCode::Char('U')))
        {
            return Action::Navigate(Direction::PageUp);
        }
        if code == KeyCode::PageDown
            || (ctrl && matches!(code, KeyCode::Char('d') | KeyCode::Char('D')))
        {
            return Action::Navigate(Direction::PageDown);
        }
    }

    // ── Configurable keybinds ─────────────────────────────────────────────────
    if kb.quit.matches(code, modifiers) {
        return Action::Quit;
    }
    if kb.tab_switch.matches(code, modifiers) {
        return Action::SwitchTab;
    }
    if kb.tab_switch_reverse.matches(code, modifiers) {
        return Action::SwitchTabReverse;
    }
    // BackTab (Shift-Tab) is always an alias for reverse tab cycle.
    if code == KeyCode::BackTab {
        return Action::SwitchTabReverse;
    }
    if kb.go_to_home.matches(code, modifiers) {
        return Action::GoToHome;
    }
    if kb.go_to_browser.matches(code, modifiers) {
        return Action::GoToBrowser;
    }
    if kb.go_to_nowplaying.matches(code, modifiers) {
        return Action::GoToNowPlaying;
    }
    // Legacy tab jump: `4` used to open Now Playing before Radio became a popup.
    if code == KeyCode::Char('4') && modifiers.is_empty() {
        return Action::GoToNowPlaying;
    }
    if kb.toggle_radio.matches(code, modifiers) {
        return Action::ToggleRadioPicker;
    }
    if let Some(spec) = &kb.toggle_folder_browse {
        if spec.matches(code, modifiers) {
            return Action::ToggleBrowserFolder;
        }
    }

    // seek_forward / seek_backward are tab-aware: they also act as column
    // navigation in the Browser tab so Right/Left keep working there.
    if kb.seek_forward.matches(code, modifiers) {
        return match active_tab {
            Tab::NowPlaying => Action::SeekForward,
            Tab::Browser | Tab::Home => Action::FocusRight,
        };
    }
    if kb.seek_backward.matches(code, modifiers) {
        return match active_tab {
            Tab::NowPlaying => Action::SeekBackward,
            Tab::Browser | Tab::Home => Action::FocusLeft,
        };
    }

    if kb.column_left.matches(code, modifiers) {
        return Action::FocusLeft;
    }
    if kb.column_right.matches(code, modifiers) {
        return Action::FocusRight;
    }
    if kb.scroll_up.matches(code, modifiers) {
        return Action::Navigate(Direction::Up);
    }
    if kb.scroll_down.matches(code, modifiers) {
        return Action::Navigate(Direction::Down);
    }

    if kb.play_pause.matches(code, modifiers) {
        return Action::PlayPause;
    }
    if kb.next_track.matches(code, modifiers) {
        return Action::NextTrack;
    }
    if kb.prev_track.matches(code, modifiers) {
        return Action::PrevTrack;
    }

    // add_all variants must be checked before add_track (superset keys).
    if let Some(spec) = &kb.add_all_replace_artist {
        if spec.matches(code, modifiers) {
            return Action::AddAllToQueueReplaceArtist;
        }
    }
    if let Some(spec) = &kb.add_all_replace_album {
        if spec.matches(code, modifiers) {
            return Action::AddAllToQueueReplaceAlbum;
        }
    }
    if let Some(spec) = &kb.add_all_prepend {
        if spec.matches(code, modifiers) {
            return Action::AddAllToQueuePrepend;
        }
    }
    if kb.add_all.matches(code, modifiers) {
        return Action::AddAllToQueue;
    }
    if kb.add_track.matches(code, modifiers) {
        return Action::AddToQueue;
    }

    if kb.shuffle.matches(code, modifiers) {
        return Action::Shuffle;
    }
    if kb.unshuffle.matches(code, modifiers) {
        return Action::Unshuffle;
    }
    if kb.toggle_queue_loop.matches(code, modifiers) {
        return Action::ToggleQueueLoop;
    }
    if active_tab == Tab::NowPlaying && kb.np_focus_queue.matches(code, modifiers) {
        return Action::ToggleNpPaneFocus;
    }
    if kb.clear_queue.matches(code, modifiers) {
        return Action::ClearQueue;
    }
    if active_tab == Tab::NowPlaying && kb.remove_from_queue.matches(code, modifiers) {
        return Action::RemoveFromQueue;
    }
    if kb.search.matches(code, modifiers) {
        return Action::SearchStart;
    }
    if kb.volume_up.matches(code, modifiers) {
        return Action::VolumeUp;
    }
    if kb.volume_down.matches(code, modifiers) {
        return Action::VolumeDown;
    }

    if let Some(spec) = &kb.library_fzf {
        if spec.matches(code, modifiers) {
            return Action::LibraryFzfPicker;
        }
    }
    if let Some(spec) = &kb.library_refresh {
        if spec.matches(code, modifiers) {
            return Action::LibraryIndexRefresh;
        }
    }
    if let Some(spec) = &kb.connection_check {
        if spec.matches(code, modifiers) {
            return Action::CheckConnection;
        }
    }
    if let Some(spec) = &kb.library_index_append_queue {
        if spec.matches(code, modifiers) {
            return Action::LibraryIndexAppendQueue;
        }
    }

    // Lone `g`: wait for second `g` (`gg`) to go to top (vim-style).
    if code == KeyCode::Char('g') && modifiers.is_empty() {
        *pending_gg = true;
        return Action::None;
    }

    Action::None
}

fn search_clear_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    (modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c'))
        || (code == KeyCode::Esc && modifiers.is_empty())
}

fn map_search_key(code: KeyCode, modifiers: KeyModifiers) -> Action {
    if search_clear_key(code, modifiers) {
        return Action::SearchCancel;
    }
    match code {
        KeyCode::Esc => Action::SearchCancel,
        KeyCode::Enter => Action::SearchConfirm,
        KeyCode::Backspace => Action::SearchBackspace,
        KeyCode::Char(ch) => Action::SearchInput(ch),
        _ => Action::None,
    }
}

/// Key handler when the help popup is open.
/// Only `i`, `Esc`, and the configured quit key close the popup — everything
/// else is suppressed so no accidental navigation occurs.
fn map_help_key(code: KeyCode, modifiers: KeyModifiers, kb: &Keybinds) -> Action {
    if kb.toggle_help.matches(code, modifiers) {
        return Action::ToggleHelp;
    }
    if code == KeyCode::Esc {
        return Action::ToggleHelp;
    }
    if kb.quit.matches(code, modifiers) {
        return Action::ToggleHelp;
    }
    if code == KeyCode::Char('k') || code == KeyCode::Up {
        return Action::HelpScrollUp;
    }
    if code == KeyCode::Char('j') || code == KeyCode::Down {
        return Action::HelpScrollDown;
    }
    Action::None
}

// ── Browser column hit-test (shared by click + wheel) ─────────────────────────

struct BrowserColumnHit {
    focus: BrowserColumn,
    col_idx: usize,
    col_area: Rect,
}

fn browser_column_hit(
    x: u16,
    y: u16,
    center: Rect,
    browse_mode: BrowseMode,
) -> Option<BrowserColumnHit> {
    use ratatui::layout::{Constraint, Layout};

    if y < center.y || y >= center.y + center.height {
        return None;
    }

    let files_mode = browse_mode == BrowseMode::Files;
    let browser_cols = if files_mode {
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(center)
    } else {
        Layout::horizontal([
            Constraint::Percentage(30),
            Constraint::Percentage(35),
            Constraint::Percentage(35),
        ])
        .split(center)
    };

    let col_idx = if files_mode {
        if x < browser_cols[1].x {
            0usize
        } else {
            1
        }
    } else if x < browser_cols[1].x {
        0usize
    } else if x < browser_cols[2].x {
        1
    } else {
        2
    };

    let col_area = browser_cols[col_idx];
    if y <= col_area.y || y >= col_area.y + col_area.height - 1 {
        return None;
    }

    let focus = if files_mode {
        match col_idx {
            0 => BrowserColumn::Artists,
            _ => BrowserColumn::Tracks,
        }
    } else {
        match col_idx {
            0 => BrowserColumn::Artists,
            1 => BrowserColumn::Albums,
            _ => BrowserColumn::Tracks,
        }
    };

    Some(BrowserColumnHit {
        focus,
        col_idx,
        col_area,
    })
}

fn rect_contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

/// Visible list row inside a bordered column (skips top/bottom border rows).
fn list_row_at(col: Rect, x: u16, y: u16) -> Option<usize> {
    if x < col.x || x >= col.x + col.width {
        return None;
    }
    if y <= col.y || y >= col.y + col.height.saturating_sub(1) {
        return None;
    }
    Some((y - col.y - 1) as usize)
}

fn picker_list_row_at(popup: Rect, x: u16, y: u16) -> Option<usize> {
    if x < popup.x || x >= popup.x + popup.width {
        return None;
    }
    if y <= popup.y || y >= popup.y + popup.height.saturating_sub(1) {
        return None;
    }
    Some((y - popup.y - 1) as usize)
}

/// Returns true when the click was consumed by a browser-tab overlay or popup.
fn handle_browser_overlay_click(x: u16, y: u16, app: &mut App, center: Rect) -> bool {
    use crate::state::LoadingState;
    use crate::ui::{favorites_overlay, list_scroll, playlist_overlay};

    // Render order: playlist overlay → picker → favorites (topmost last).
    if app.favorites_overlay.visible {
        let layout = favorites_overlay::panel_layout(center);
        if rect_contains(layout.area, x, y) {
            let cat_len = crate::state::FavoritesCategory::ALL.len();
            if let Some(idx) = list_scroll::list_index_at_click(
                layout.categories_col,
                x,
                y,
                app.favorites_overlay.categories_scroll,
                cat_len,
            ) {
                let target = mouse_click::MouseClickTarget::FavoritesCategory(idx);
                if mouse_click::is_double_click(app, target) {
                    app.double_click_favorites_category(idx);
                } else {
                    app.click_favorites_category(idx);
                }
            } else {
                let item_len = app.favorites_overlay.item_count();
                if let Some(idx) = list_scroll::list_index_at_click(
                    layout.items_col,
                    x,
                    y,
                    app.favorites_overlay.items_scroll,
                    item_len,
                ) {
                    let target = mouse_click::MouseClickTarget::FavoritesItem(idx);
                    if mouse_click::is_double_click(app, target) {
                        app.double_click_favorites_item(idx);
                    } else {
                        app.click_favorites_item(idx);
                    }
                }
            }
            return true;
        }
    }

    if app.playlist_picker.is_some() {
        let popup = playlist_overlay::picker_rect(center);
        if rect_contains(popup, x, y) {
            if let Some(ref picker) = app.playlist_picker {
                let len = picker.playlists.len();
                if let Some(idx) = list_scroll::list_index_at_click(popup, x, y, picker.scroll, len)
                {
                    app.click_playlist_picker_row(idx);
                }
            }
            return true;
        }
    }

    if app.playlist_overlay.visible {
        let layout = playlist_overlay::panel_layout(center, &app.playlist_overlay.input_mode);
        if rect_contains(layout.area, x, y) {
            let list_len = match &app.playlist_overlay.playlists {
                LoadingState::Loaded(playlists) => playlists.len(),
                _ => 0,
            };
            if let Some(idx) = list_scroll::list_index_at_click(
                layout.list_rows,
                x,
                y,
                app.playlist_overlay.list_scroll,
                list_len,
            ) {
                let target = mouse_click::MouseClickTarget::PlaylistList(idx);
                if mouse_click::is_double_click(app, target) {
                    app.double_click_playlist_overlay_list(idx);
                } else {
                    app.click_playlist_overlay_list(idx);
                }
            } else {
                let track_len = match &app.playlist_overlay.tracks {
                    LoadingState::Loaded(songs) => songs.len(),
                    _ => 0,
                };
                if let Some(idx) = list_scroll::list_index_at_click(
                    layout.tracks_col,
                    x,
                    y,
                    app.playlist_overlay.tracks_scroll,
                    track_len,
                ) {
                    let target = mouse_click::MouseClickTarget::PlaylistTrack(idx);
                    if mouse_click::is_double_click(app, target) {
                        app.double_click_playlist_overlay_track(idx);
                    } else {
                        app.click_playlist_overlay_track(idx);
                    }
                }
            }
            return true;
        }
    }

    false
}

fn handle_browser_overlay_wheel(
    x: u16,
    y: u16,
    dir: Direction,
    app: &mut App,
    center: Rect,
) -> bool {
    use crate::state::{FavoritesFocus, PlaylistFocus};
    use crate::ui::{favorites_overlay, playlist_overlay};

    // Render order: playlist overlay → picker → favorites (topmost last).
    if app.favorites_overlay.visible {
        let layout = favorites_overlay::panel_layout(center);
        if rect_contains(layout.area, x, y) {
            if list_row_at(layout.categories_col, x, y).is_some() {
                app.favorites_overlay.focus = FavoritesFocus::Categories;
            } else if list_row_at(layout.items_col, x, y).is_some() {
                app.favorites_overlay.focus = FavoritesFocus::Items;
            }
            app.navigate_favorites_wheel(dir);
            return true;
        }
    }

    if app.playlist_picker.is_some() {
        let popup = playlist_overlay::picker_rect(center);
        if rect_contains(popup, x, y) {
            app.navigate_playlist_picker_wheel(dir);
            return true;
        }
    }

    if app.playlist_overlay.visible {
        let layout = playlist_overlay::panel_layout(center, &app.playlist_overlay.input_mode);
        if rect_contains(layout.area, x, y) {
            if list_row_at(layout.list_rows, x, y).is_some() {
                app.playlist_overlay.focus = PlaylistFocus::List;
            } else if list_row_at(layout.tracks_col, x, y).is_some() {
                app.playlist_overlay.focus = PlaylistFocus::Tracks;
            }
            app.navigate_playlist_wheel(dir);
            return true;
        }
    }

    false
}

fn handle_mouse_wheel(x: u16, y: u16, dir: Direction, app: &mut App, terminal_size: Rect) {
    if app.active_tab != Tab::Browser {
        return;
    }

    let areas = ui::layout::build_layout(terminal_size, &ui::layout::layout_options_for_app(app));
    if handle_browser_overlay_wheel(x, y, dir, app, areas.center) {
        return;
    }

    let Some(hit) = browser_column_hit(x, y, areas.center, app.browser_browse_mode) else {
        return;
    };

    app.browser_focus = hit.focus;
    app.navigate_browser_wheel(dir);
}

// ── Mouse click handler ───────────────────────────────────────────────────────
//
// CALL PATH DIAGNOSIS (tab-bar freeze, 2026-03-28)
// ─────────────────────────────────────────────────
// Render uses build_layout() for ALL three tabs (center | now_playing | tab_bar | status_bar).
// Previously this function used build_browser() / build_nowplaying() for the Browser /
// NowPlaying tabs — those layouts omit the tab_bar row, so their `now_playing` started 1
// row lower and their `center` was 1 row taller than what was actually drawn on screen.
//
// Consequence 1 — no tab-bar click handler existed at all.
// Consequence 2 — the coordinate mismatch meant clicks on the rendered now-playing bar
//   rows 0 and 1 could silently fall through rather than hitting the controls check.
//
// The freeze itself came from render_art_strip() being called on *every* ratatui frame
// inside render_home_tab().  That function does, per visible thumbnail:
//   image::load_from_memory → resize_exact(Lanczos3) → zlib compress → base64 encode
//   → Kitty protocol write to stdout
// For a full strip this is multiple seconds of CPU-bound work every ~50 ms poll tick.
//
// Fixes applied:
//   1. Use build_layout() for all tabs here so geometry matches the renderer.
//   2. Add a tab_bar hit-test that dispatches GoToHome / GoToBrowser / GoToNowPlaying.
//      The dispatch completes in <1 ms (refresh_home_data() is in-memory + tokio::spawn).
//   3. render_art_strip() removed from render_home_tab() (per-frame path).
//      It is now driven exclusively by the home_art_needs_redraw flag in main.rs,
//      set only when: entering Home tab, a HomeArt cache update arrives, or
//      the album scroll / selection changes.

fn handle_mouse_click(x: u16, y: u16, app: &mut App, terminal_size: ratatui::layout::Rect) {
    use state::LoadingState;

    // Always use build_layout: the renderer uses it for all three tabs.
    let areas = ui::layout::build_layout(terminal_size, &ui::layout::layout_options_for_app(app));
    let center = areas.center;
    let now_playing = areas.now_playing;

    // ── Tab bar: dispatch GoToHome / GoToBrowser / GoToNowPlaying ──
    if y == areas.tab_bar.y {
        // Label widths (chars): Home=6, sep=3, Browse=8, sep=3, NowPlaying=13
        let home_end: u16 = 6;
        let browser_start: u16 = 9;
        let browser_end: u16 = 17;
        let np_start: u16 = 20;

        let action = if x < home_end {
            Action::GoToHome
        } else if x >= browser_start && x < browser_end {
            Action::GoToBrowser
        } else if x >= np_start {
            Action::GoToNowPlaying
        } else {
            Action::None
        };
        app.dispatch(action);
        mouse_click::clear_pending_click(app);
        return;
    }

    if app.help_visible {
        let popup = ui::popup::help_popup_rect(terminal_size, app.config.radio_enabled);
        if rect_contains(popup, x, y) {
            mouse_click::clear_pending_click(app);
            return;
        }
    }

    if app.radio.picker_visible && app.radio.input_mode.is_normal() {
        let popup = ui::radio_popup::popup_rect(terminal_size);
        if rect_contains(popup, x, y) {
            if let Some(visible_row) = picker_list_row_at(popup, x, y) {
                app.click_radio_row(visible_row);
            }
            mouse_click::clear_pending_click(app);
            return;
        }
    }

    // ── Now-playing bar (layout matches `ui::now_playing::render`) ───────────
    let chrome = ui::now_playing::interaction_rects(app, now_playing);

    if let Some(controls_area) = chrome.controls {
        if let Some(action) = ui::now_playing::controls_click_action(app, controls_area, x) {
            let row = ui::now_playing::controls_row_rect(controls_area);
            if y >= row.y && y < row.y + row.height {
                app.dispatch(action);
                mouse_click::clear_pending_click(app);
                return;
            }
        }
    }

    if let Some(progress_area) = chrome.progress {
        if y >= progress_area.y
            && y < progress_area.y + progress_area.height
            && x >= progress_area.x
            && x < progress_area.x + progress_area.width
            && app.playback.current_song.is_some()
        {
            if let Some(total) = app.playback.total {
                let e = app.playback.elapsed.as_secs();
                let ts = total.as_secs();
                let elapsed_str_len = format!("{}:{:02}", e / 60, e % 60).len() as u16;
                let total_str_len = format!("{}:{:02}", ts / 60, ts % 60).len() as u16;
                let bar_start = progress_area.x + elapsed_str_len + 2;
                let bar_end =
                    (progress_area.x + progress_area.width).saturating_sub(total_str_len + 2);

                if x >= bar_start && bar_end > bar_start {
                    let bar_w = (bar_end - bar_start) as f64;
                    let ratio = (x - bar_start) as f64 / bar_w;
                    let seek_secs = (ratio * ts as f64) as u64;
                    app.dispatch(Action::SeekTo(std::time::Duration::from_secs(seek_secs)));
                }
            }
            mouse_click::clear_pending_click(app);
            return;
        }
    }

    // ── Center area ───────────────────────────────────────────────────────────
    if y < center.y || y >= center.y + center.height {
        return;
    }

    match app.active_tab {
        Tab::Home => {
            handle_home_click(x, y, app, center);
        }
        Tab::Browser => {
            if handle_browser_overlay_click(x, y, app, center) {
                return;
            }
            let Some(hit) = browser_column_hit(x, y, center, app.browser_browse_mode) else {
                return;
            };
            let visible_row = (y - hit.col_area.y - 1) as usize;
            app.browser_focus = hit.focus;
            let col_idx = hit.col_idx;
            let files_mode = app.browser_browse_mode == BrowseMode::Files;

            if files_mode {
                match col_idx {
                    0 => {
                        let visible_pos = {
                            let scroll = app.folders.dirs_scroll;
                            scroll + visible_row
                        };
                        let target = mouse_click::MouseClickTarget::FolderDir(visible_pos);
                        if mouse_click::is_double_click(app, target) {
                            app.double_click_folder_dir(visible_pos);
                        } else {
                            app.click_folder_dir(visible_pos);
                        }
                    }
                    _ => {
                        if let Some(row_ix) = app.folder_preview_row_index(visible_row) {
                            let target = mouse_click::MouseClickTarget::FolderPreview(row_ix);
                            if mouse_click::is_double_click(app, target) {
                                app.double_click_folder_preview_row(visible_row);
                            } else {
                                app.click_folder_preview_row(visible_row);
                            }
                        }
                    }
                }
                return;
            }

            match col_idx {
                0 => {
                    let orig_idx: Option<usize> = {
                        if let LoadingState::Loaded(artists) = &app.library.artists {
                            let visible: Vec<usize> = if let Some(q) =
                                app.browser_column_filter(crate::app::BrowserColumn::Artists)
                            {
                                artists
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, a)| a.name.to_lowercase().contains(q))
                                    .map(|(i, _)| i)
                                    .collect()
                            } else {
                                (0..artists.len()).collect()
                            };
                            let clicked = app.library.artists_scroll + visible_row;
                            visible.get(clicked).copied()
                        } else {
                            None
                        }
                    };
                    if let Some(idx) = orig_idx {
                        let target = mouse_click::MouseClickTarget::BrowserArtist(idx);
                        if mouse_click::is_double_click(app, target) {
                            app.double_click_browser_artist(idx);
                        } else {
                            app.click_browser_artist(idx);
                        }
                    }
                }
                1 => {
                    let orig_idx: Option<usize> = {
                        let artist_id = match app.library.current_artist() {
                            Some(a) => a.id.clone(),
                            None => return,
                        };
                        if let Some(LoadingState::Loaded(albums)) =
                            app.library.albums.get(&artist_id)
                        {
                            let visible: Vec<usize> = if let Some(q) =
                                app.browser_column_filter(crate::app::BrowserColumn::Albums)
                            {
                                albums
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, a)| a.name.to_lowercase().contains(q))
                                    .map(|(i, _)| i)
                                    .collect()
                            } else {
                                (0..albums.len()).collect()
                            };
                            let clicked = app.library.albums_scroll + visible_row;
                            visible.get(clicked).copied()
                        } else {
                            None
                        }
                    };
                    if let Some(idx) = orig_idx {
                        let target = mouse_click::MouseClickTarget::BrowserAlbum(idx);
                        if mouse_click::is_double_click(app, target) {
                            app.double_click_browser_album(idx);
                        } else {
                            app.click_browser_album(idx);
                        }
                    }
                }
                _ => {
                    let orig_idx: Option<usize> = {
                        let album_id = match app.library.current_album() {
                            Some(a) => a.id.clone(),
                            None => return,
                        };
                        if let Some(LoadingState::Loaded(songs)) = app.library.tracks.get(&album_id)
                        {
                            let visible: Vec<usize> = if let Some(q) =
                                app.browser_column_filter(crate::app::BrowserColumn::Tracks)
                            {
                                songs
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, s)| s.title.to_lowercase().contains(q))
                                    .map(|(i, _)| i)
                                    .collect()
                            } else {
                                (0..songs.len()).collect()
                            };
                            let clicked = app.library.tracks_scroll + visible_row;
                            visible.get(clicked).copied()
                        } else {
                            None
                        }
                    };
                    if let Some(idx) = orig_idx {
                        let target = mouse_click::MouseClickTarget::BrowserTrack(idx);
                        if mouse_click::is_double_click(app, target) {
                            app.double_click_browser_track(idx);
                        } else {
                            app.click_browser_track(idx);
                        }
                    }
                }
            }
        }
        Tab::NowPlaying => {
            let show_art = app.config.nowplaying_show_art;
            let boxed_np = app
                .config
                .now_playing_layout
                .trim()
                .eq_ignore_ascii_case("boxed");

            if boxed_np {
                let art_position =
                    ui::layout::placement_from_str(&app.config.nowplaying_art_position)
                        .unwrap_or(ui::layout::Placement::Left);
                let queue_position =
                    ui::layout::placement_from_str(&app.config.nowplaying_queue_position)
                        .unwrap_or(ui::layout::Placement::Right);
                let visualizer_position =
                    ui::layout::placement_from_str(&app.config.visualizer_location)
                        .unwrap_or(ui::layout::Placement::Right);
                let now_playing_position =
                    ui::layout::placement_from_str(&app.config.now_playing_box_location)
                        .unwrap_or(ui::layout::Placement::Right);
                let lyrics_position = ui::layout::placement_from_str(&app.config.lyrics_location)
                    .unwrap_or(queue_position);
                let rects = ui::layout::now_playing_rects(
                    center,
                    show_art,
                    art_position,
                    queue_position,
                    app.config.nowplaying_left_width_percent,
                    app.config.nowplaying_vertical_fill_top_percent,
                    app.visualizer_visible,
                    visualizer_position,
                    app.lyrics_visible,
                    lyrics_position,
                    boxed_np,
                    now_playing_position,
                );
                if let Some(pane) = rects.now_playing {
                    let chrome = ui::now_playing::interaction_rects_pane(app, pane);
                    if let Some(controls_area) = chrome.controls {
                        if let Some(action) =
                            ui::now_playing::controls_click_action(app, controls_area, x)
                        {
                            let row = ui::now_playing::controls_row_rect(controls_area);
                            if y >= row.y && y < row.y + row.height {
                                app.dispatch(action);
                                return;
                            }
                        }
                    }
                    if let Some(progress_area) = chrome.progress {
                        if y >= progress_area.y
                            && y < progress_area.y + progress_area.height
                            && x >= progress_area.x
                            && x < progress_area.x + progress_area.width
                            && app.playback.current_song.is_some()
                        {
                            if let Some(total) = app.playback.total {
                                let e = app.playback.elapsed.as_secs();
                                let ts = total.as_secs();
                                let elapsed_str_len =
                                    format!("{}:{:02}", e / 60, e % 60).len() as u16;
                                let total_str_len =
                                    format!("{}:{:02}", ts / 60, ts % 60).len() as u16;
                                let bar_start = progress_area.x + elapsed_str_len + 2;
                                let bar_end = (progress_area.x + progress_area.width)
                                    .saturating_sub(total_str_len + 2);

                                if x >= bar_start && bar_end > bar_start {
                                    let bar_w = (bar_end - bar_start) as f64;
                                    let ratio = (x - bar_start) as f64 / bar_w;
                                    let seek_secs = (ratio * ts as f64) as u64;
                                    app.dispatch(Action::SeekTo(std::time::Duration::from_secs(
                                        seek_secs,
                                    )));
                                }
                            }
                            return;
                        }
                    }
                }
            }

            let art_position = ui::layout::placement_from_str(&app.config.nowplaying_art_position)
                .unwrap_or(ui::layout::Placement::Left);
            let queue_position =
                ui::layout::placement_from_str(&app.config.nowplaying_queue_position)
                    .unwrap_or(ui::layout::Placement::Right);
            let visualizer_position =
                ui::layout::placement_from_str(&app.config.visualizer_location)
                    .unwrap_or(ui::layout::Placement::Right);
            let now_playing_position =
                ui::layout::placement_from_str(&app.config.now_playing_box_location)
                    .unwrap_or(ui::layout::Placement::Right);
            let lyrics_position = ui::layout::placement_from_str(&app.config.lyrics_location)
                .unwrap_or(queue_position);
            let rects = ui::layout::now_playing_rects(
                center,
                show_art,
                art_position,
                queue_position,
                app.config.nowplaying_left_width_percent,
                app.config.nowplaying_vertical_fill_top_percent,
                app.visualizer_visible,
                visualizer_position,
                app.lyrics_visible,
                lyrics_position,
                boxed_np,
                now_playing_position,
            );
            let Some(queue_area) = rects.queue else {
                return;
            };
            if x < queue_area.x || x >= queue_area.x + queue_area.width {
                return;
            }
            // Ignore border rows.
            if y <= queue_area.y || y >= queue_area.y + queue_area.height - 1 {
                return;
            }
            let visible_row = (y - queue_area.y - 1) as usize;
            if app.np_radio_pane_available()
                && app.np_pane_focus == crate::state::NowPlayingPaneFocus::Radio
            {
                let visible_rows = queue_area.height.saturating_sub(2) as usize;
                let hint_rows = if app.queue.songs.is_empty() { 1 } else { 2 };
                let list_rows = visible_rows.saturating_sub(hint_rows).max(1);
                if visible_row >= list_rows {
                    return;
                }
                app.select_radio_visible_row(visible_row);
                return;
            }
            let clicked_idx = app.queue.scroll + visible_row;
            let target = mouse_click::MouseClickTarget::QueueRow(clicked_idx);
            if mouse_click::is_double_click(app, target) {
                app.double_click_queue_row(clicked_idx);
            } else {
                app.click_queue_row(clicked_idx);
            }
        }
    }
}

/// Query the tmux status bar position and return a row offset (0 or 1).
///
/// Returns 1 when the tmux status bar is enabled and positioned at the top,
/// because the pane's row 0 maps to Ghostty's row 1 (the status bar occupies row 0).
/// Returns 0 in all other cases (bottom bar, disabled, or not in tmux).
fn tmux_status_offset() -> u16 {
    if std::env::var("TMUX").is_err() {
        return 0;
    }
    let output = std::process::Command::new("tmux")
        .args(["display-message", "-p", "#{status}#{status-position}"])
        .output();
    match output {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            let s = s.trim();
            // "ontop" = status on, position top → offset 1
            // "offbottom" / "offtop" / "onbottom" = no offset
            if s.starts_with("on") && s.ends_with("top") {
                1
            } else {
                0
            }
        }
        Err(_) => 1, // safe default: assume top status bar
    }
}
