mod action;
mod app;
mod cache;
mod color;
mod config;
mod desktop_notify;
mod fzf_picker;
mod history;
mod keybinds;
mod keyring_init;
mod library_index;
mod lyrics;
mod mpris;
mod persist;
mod state;
mod theme;
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
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use action::{Action, Direction};
use app::{App, BrowserColumn, Tab};
use config::{AlbumArtBackend, BrowseMode, Config, HomePanel};
use keybinds::Keybinds;
use state::{GlobalConfirm, PlaylistFocus, PlaylistInputMode};

/// Entry point shared by the `ratune` binary and integration tests.
pub async fn run() -> Result<()> {
    keyring_init::install_default_keyring_store();

    let config = Config::load().unwrap_or_else(|e| {
        // `{:#}` prints the full cause chain (plain `{}` hides nested anyhow contexts).
        eprintln!("error: {e:#}");
        process::exit(1);
    });
    let mut app = App::new(config)?;

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

    // `refresh_home_data()` only ran when navigating to Home — not on cold start. If we restore
    // or default to Home, populate lists and kick art fetches before the first frame.
    if app.active_tab == Tab::Home {
        app.refresh_home_data();
        app.home_art_needs_redraw = true;
    }

    // Begin fetching library metadata for the browse tab.
    if app.browser_browse_mode == crate::config::BrowseMode::Files {
        app.fetch_music_folders();
    } else {
        app.fetch_artists();
    }
    // Background metadata index refresh when missing or stale (Milestone 2).
    app.spawn_library_index_refresh(false);

    // Spawn a task that sets a flag on SIGTERM or SIGHUP so the main loop
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
            tokio::select! {
                _ = sigterm.recv() => {}
                _ = sighup.recv()  => {}
                _ = sigpipe.recv() => {}
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
        terminal.backend_mut().flush()?;
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

    fzf_picker::suspend_tui(terminal, app.in_tmux)?;
    let input = library_index::fzf_input_lines(&app.library_index_tracks);
    let mut fzf_args = app.config.fzf_args.clone();
    if !fzf_args.iter().any(|a| a.starts_with("--header")) {
        fzf_args.insert(0, format!("--header={}", library_index::fzf_header_line()));
    }
    let fzf_args = fzf_picker::prepare_library_fuzzy_picker_args(&app.config.fzf_binary, fzf_args);
    let res = fzf_picker::run_fzf(&app.config.fzf_binary, &fzf_args, &input);
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
    // Cover id for which `render_image` failed (e.g. undecodable bytes). Without
    // this latch the loop retries every frame and spams stderr.
    let mut kitty_cover_unrenderable: Option<String> = None;
    let mut last_tab = app.active_tab;

    // 2-second fallback: nudge Kitty art re-transmit when it is missing.
    // Checked once per loop iteration (see below).
    let mut last_art_recovery_fire = Instant::now();

    loop {
        // Check for SIGTERM / SIGHUP from the signal handler task.
        if signal_quit.load(Ordering::Relaxed) {
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

        // Apply completed NP encodes before draw (previous frame) and after draw (same-frame worker).
        drain_ratatui_np_resize_completions(app);

        terminal.draw(|f| ui::render(app, f))?;

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
                // terminal evicted the stored image; render_image() is always reliable.
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
                } else if let (Some((cover_id, bytes)), Some(fp)) =
                    (app.art_cache.as_ref(), app.art_cache_fingerprint)
                {
                    let sz = terminal.size()?;
                    let show_art = app.config.nowplaying_show_art;
                    let terminal_rect = Rect::new(0, 0, sz.width, sz.height);
                    let layout_opts = ui::layout::layout_options_for_app(app);
                    let center = ui::layout::build_layout(terminal_rect, &layout_opts).center;

                    let boxed = app
                        .config
                        .now_playing_layout
                        .trim()
                        .eq_ignore_ascii_case("boxed");

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
                        let placement = ui::kitty_art::album_art_placeholder_inner(art_rect);
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
                                // Album changed, first display, tab return, or terminal
                                // was resized — full re-encode and re-transmit.
                                match ui::kitty_art::render_image(
                                    bytes,
                                    placement,
                                    app.in_tmux,
                                    app.tmux_status_offset,
                                    app.cell_px,
                                    crate::theme::color_to_rgba(app.theme.surface),
                                ) {
                                    Ok(()) => {
                                        last_rendered_art = Some((fp, placement));
                                        art_displayed = true;
                                    }
                                    Err(e) => {
                                        eprintln!("kitty render: {e}");
                                        let _ = ui::kitty_art::clear_image(app.in_tmux);
                                        kitty_cover_unrenderable = Some(cover_id.clone());
                                        last_rendered_art = None;
                                        art_displayed = false;
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
                } else if art_displayed {
                    // In NowPlaying tab but no art — clear any stale image.
                    let _ = ui::kitty_art::clear_image(app.in_tmux);
                    last_rendered_art = None;
                    art_displayed = false;
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
                            crate::theme::color_to_rgba(app.theme.surface),
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
        let poll_ms = if app.visualizer_visible || app.accent_transition_active() {
            33
        } else {
            50
        };
        match event::poll(Duration::from_millis(poll_ms)) {
            Err(e)
                if e.kind() == std::io::ErrorKind::BrokenPipe
                    || e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                app.should_quit = true;
            }
            Err(e) => return Err(e.into()),
            Ok(false) => {}
            Ok(true) => loop {
                let read_result = event::read();
                match read_result {
                    Err(e)
                        if e.kind() == std::io::ErrorKind::BrokenPipe
                            || e.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        app.should_quit = true;
                    }
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
                                } else if app.playlist_overlay.visible
                                    && app.active_tab == Tab::Browser
                                    && !app.help_visible
                                {
                                    // Tab-switch keys close the overlay and switch tabs.
                                    let is_tab_switch = matches!(
                                        key.code,
                                        KeyCode::Tab
                                            | KeyCode::BackTab
                                            | KeyCode::Char('1')
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
                        Event::Mouse(mouse)
                            if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                                let sz = terminal.size()?;
                                let area = ratatui::layout::Rect::new(0, 0, sz.width, sz.height);
                                handle_mouse_click(mouse.column, mouse.row, app, area);
                            }
                        Event::Resize(_, _) => {
                            // Terminal resized — clear any displayed image and reset
                            // stored state so the art is re-encoded at the new size.
                            // last_rendered_art rect will no longer match the new
                            // art_rect, so the full render path is taken on next frame.
                            if app.kitty_apc_overlay_active() && art_displayed {
                                let _ = ui::kitty_art::clear_image(app.in_tmux);
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

                match event::poll(Duration::ZERO) {
                    Err(e)
                        if e.kind() == std::io::ErrorKind::BrokenPipe
                            || e.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        app.should_quit = true;
                        break;
                    }
                    Err(e) => return Err(e.into()),
                    Ok(false) => break,
                    Ok(true) => {}
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
    use crate::ui::kitty_art::{art_strip_layout, art_strip_thumb_hit, KITTY_STRIP_MAX_SLOTS};

    match panel {
        HomePanel::RecentAlbums => {
            let inner = Rect {
                x: area.x + 1,
                y: area.y + 1,
                width: area.width.saturating_sub(2),
                height: area.height.saturating_sub(2),
            };
            if y < inner.y
                || y >= inner.y + inner.height
                || x < inner.x
                || x >= inner.x + inner.width
            {
                return;
            }
            app.home.active_section = app::HomeSection::RecentAlbums;
            app.home.selected_index = 0;

            let layout = art_strip_layout(inner.width, inner.height);
            let rel_x = x.saturating_sub(inner.x);
            let rel_y = y.saturating_sub(inner.y);
            let Some((row_in_grid, col_in_grid)) = art_strip_thumb_hit(&layout, rel_x, rel_y)
            else {
                return;
            };
            let slot = (row_in_grid as usize) * layout.per_row + (col_in_grid as usize);
            let max_slots = layout.total_visible.min(KITTY_STRIP_MAX_SLOTS);
            if slot >= max_slots {
                return;
            }
            let album_index = app.home.album_scroll_offset + slot;
            if album_index < app.home.recent_albums.len() {
                if app.home.album_selected_index == album_index {
                    if app.kitty_apc_overlay_active() {
                        let _ = crate::ui::kitty_art::clear_image(app.in_tmux);
                        let _ = crate::ui::kitty_art::clear_art_strip(app.in_tmux);
                    }
                    if app.ratatui_art_ready() && !app.ratatui_uses_kitty_apc() {
                        app.clear_ratatui_art_state();
                    }
                    app.active_tab = app::Tab::Browser;
                    app.clear_browser_search();
                } else {
                    app.home.album_selected_index = album_index;
                }
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
                if app.home.active_section == app::HomeSection::RecentTracks
                    && app.home.selected_index == row
                {
                    app.dispatch(Action::Select);
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
                if app.home.active_section == app::HomeSection::Rediscover
                    && app.home.selected_index == row
                {
                    app.dispatch(Action::Select);
                } else {
                    app.home.active_section = app::HomeSection::Rediscover;
                    app.home.selected_index = row;
                }
            }
        }
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
            let shift = modifiers.intersects(KeyModifiers::SHIFT);
            match code {
                KeyCode::Esc => Action::TogglePlaylistOverlay,
                _ if kb.playlist_overlay.matches(code, modifiers) => Action::TogglePlaylistOverlay,
                KeyCode::Char('k') | KeyCode::Up => Action::PlaylistScrollUp,
                KeyCode::Char('j') | KeyCode::Down => Action::PlaylistScrollDown,
                KeyCode::Char('h') => Action::PlaylistFocusList,
                KeyCode::Char('l') => Action::PlaylistFocusTracks,
                // c: create new playlist
                KeyCode::Char('c') => Action::PlaylistCreate,
                // r: rename selected playlist (list pane)
                KeyCode::Char('r') => Action::PlaylistRename,
                // X (Shift+x): delete selected playlist
                KeyCode::Char('X') | KeyCode::Char('x') if code == KeyCode::Char('X') || shift => {
                    Action::PlaylistDelete
                }
                // <: remove highlighted track from playlist (tracks pane)
                KeyCode::Char('<') if matches!(focus, PlaylistFocus::Tracks) => {
                    Action::PlaylistRemoveTrack
                }
                KeyCode::Enter => match focus {
                    PlaylistFocus::List => Action::PlaylistPlayAll,
                    PlaylistFocus::Tracks => Action::PlaylistPlayTrack,
                },
                // Shift+A — append all / append track
                KeyCode::Char('A') | KeyCode::Char('a') if code == KeyCode::Char('A') || shift => {
                    match focus {
                        PlaylistFocus::List => Action::PlaylistAppendAll,
                        PlaylistFocus::Tracks => Action::PlaylistAppendTrack,
                    }
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
    if kb.clear_queue.matches(code, modifiers) {
        return Action::ClearQueue;
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
    use ratatui::layout::{Constraint, Layout};
    use state::LoadingState;

    // Always use build_layout: the renderer uses it for all three tabs.
    let areas = ui::layout::build_layout(terminal_size, &ui::layout::layout_options_for_app(app));
    let center = areas.center;
    let now_playing = areas.now_playing;

    // ── Tab bar: dispatch GoToHome / GoToBrowser / GoToNowPlaying ────────────
    if y == areas.tab_bar.y {
        // The labels are:  " Home "  " │ "  " Browse "  " │ "  " Now Playing "
        // Measure cumulative widths to decide which label was clicked.
        // Label widths (chars): Home=6, sep=3, Browse=8, sep=3, NowPlaying=13
        let home_end: u16 = 6;
        let browser_start: u16 = 9; // 6+3
        let browser_end: u16 = 17; // 9+8
        let np_start: u16 = 20; // 17+3

        let action = if x < home_end {
            Action::GoToHome
        } else if x >= browser_start && x < browser_end {
            Action::GoToBrowser
        } else if x >= np_start {
            Action::GoToNowPlaying
        } else {
            Action::None // clicked a separator
        };
        app.dispatch(action);
        return;
    }

    // ── Now-playing bar (layout matches `ui::now_playing::render`) ───────────
    let chrome = ui::now_playing::interaction_rects(app, now_playing);

    if let Some(controls_area) = chrome.controls {
        if y >= controls_area.y
            && y < controls_area.y + controls_area.height
            && x >= controls_area.x
            && x < controls_area.x + controls_area.width
        {
            let rel_x = x - controls_area.x;
            let third = controls_area.width / 3;
            if rel_x < third {
                app.dispatch(Action::PrevTrack);
            } else if rel_x < 2 * third {
                app.dispatch(Action::PlayPause);
            } else {
                app.dispatch(Action::NextTrack);
            }
            return;
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
            let files_mode = app.browser_browse_mode == BrowseMode::Files;
            let browser_cols = if files_mode {
                Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                    .split(center)
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
            // Ignore clicks on the border rows.
            if y <= col_area.y || y >= col_area.y + col_area.height - 1 {
                return;
            }

            let visible_row = (y - col_area.y - 1) as usize;
            let visible_height = col_area.height.saturating_sub(2) as usize;

            // Switch focus to the clicked column.
            app.browser_focus = if files_mode {
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

            if files_mode {
                match col_idx {
                    0 => {
                        let visible_pos = {
                            let scroll = app.folders.dirs_scroll;
                            scroll + visible_row
                        };
                        app.click_folder_dir(visible_pos);
                    }
                    _ => {
                        app.click_folder_preview_row(visible_row);
                        app.folder_activate_preview_selection();
                    }
                }
                return;
            }

            match col_idx {
                0 => {
                    // Artists: compute ratatui's auto-scroll offset and map click.
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
                            let sel_pos = app
                                .library
                                .selected_artist
                                .and_then(|s| visible.iter().position(|&i| i == s))
                                .unwrap_or(0);
                            // ratatui scrolls to keep selection visible from below:
                            // scroll = max(0, sel_pos - (visible_height - 1))
                            let scroll = sel_pos.saturating_sub(visible_height.saturating_sub(1));
                            let clicked = scroll + visible_row;
                            visible.get(clicked).copied()
                        } else {
                            None
                        }
                    };
                    if let Some(idx) = orig_idx {
                        app.click_browser_artist(idx);
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
                            let sel_pos = app
                                .library
                                .selected_album
                                .and_then(|s| visible.iter().position(|&i| i == s))
                                .unwrap_or(0);
                            let scroll = sel_pos.saturating_sub(visible_height.saturating_sub(1));
                            let clicked = scroll + visible_row;
                            visible.get(clicked).copied()
                        } else {
                            None
                        }
                    };
                    if let Some(idx) = orig_idx {
                        app.click_browser_album(idx);
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
                            let sel_pos = app
                                .library
                                .selected_track
                                .and_then(|s| visible.iter().position(|&i| i == s))
                                .unwrap_or(0);
                            let scroll = sel_pos.saturating_sub(visible_height.saturating_sub(1));
                            let clicked = scroll + visible_row;
                            visible.get(clicked).copied()
                        } else {
                            None
                        }
                    };
                    if let Some(idx) = orig_idx {
                        app.click_browser_track(idx);
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
                        if y >= controls_area.y
                            && y < controls_area.y + controls_area.height
                            && x >= controls_area.x
                            && x < controls_area.x + controls_area.width
                        {
                            let rel_x = x - controls_area.x;
                            let third = controls_area.width / 3;
                            if rel_x < third {
                                app.dispatch(Action::PrevTrack);
                            } else if rel_x < 2 * third {
                                app.dispatch(Action::PlayPause);
                            } else {
                                app.dispatch(Action::NextTrack);
                            }
                            return;
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
            let clicked_idx = app.queue.scroll + visible_row;
            app.set_queue_cursor(clicked_idx);
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
