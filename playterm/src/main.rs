mod action;
mod app;
mod cache;
mod fzf_picker;
mod library_index;
mod mpris;
mod color;
mod config;
mod desktop_notify;
mod history;
mod keybinds;
mod lyrics;
mod persist;
mod state;
mod theme;
mod ui;
mod visualizer;

use std::io;
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
use config::Config;
use keybinds::Keybinds;
use state::{PlaylistFocus, PlaylistInputMode};

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    });
    let mut app = App::new(config)?;

    // Detect tmux first: $TMUX is set when running inside a tmux session.
    app.in_tmux = std::env::var("TMUX").is_ok();
    if app.in_tmux {
        app.tmux_status_offset = tmux_status_offset();
    }

    // Detect Kitty graphics support before entering raw mode / alternate screen.
    // Inside tmux the probe fails — tmux intercepts the APC response before it
    // reaches us. Assume the outer terminal supports Kitty when in_tmux is true;
    // the user would not have enabled passthrough otherwise.
    app.kitty_supported = if app.in_tmux {
        true
    } else {
        ui::kitty_art::detect_kitty_support()
    };

    // Query cell pixel dimensions (used for art strip sizing).
    // Attempted only if Kitty is supported — non-Kitty terminals may not respond.
    if app.kitty_supported {
        app.cell_px = ui::kitty_art::query_cell_pixel_size();
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

    // Begin fetching artists immediately.
    app.fetch_artists();
    // Background metadata index refresh when missing or stale (Milestone 2).
    app.spawn_library_index_refresh(false);

    // Spawn a task that sets a flag on SIGTERM or SIGHUP so the main loop
    // can shut down cleanly (same path as pressing `q`).
    let signal_quit = Arc::new(AtomicBool::new(false));
    {
        let flag = signal_quit.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate())
                .expect("failed to install SIGTERM handler");
            let mut sighup = signal(SignalKind::hangup())
                .expect("failed to install SIGHUP handler");
            // SIGPIPE: stdout/stdin fd closed (e.g. tmux pane killed while piped).
            let mut sigpipe = signal(SignalKind::pipe())
                .expect("failed to install SIGPIPE handler");
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

    // Clear any Kitty images before leaving the alternate screen.
    if app.kitty_supported {
        let _ = ui::kitty_art::clear_image(app.in_tmux);
    }

    // Restore terminal regardless of errors.
    disable_raw_mode()?;
    terminal.backend_mut().execute(DisableMouseCapture)?;
    if app.in_tmux {
        use std::io::Write;
        terminal.backend_mut().write_all(b"\x1bPtmux;\x1b\x1b[?1004l\x1b\\")?;
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
    let _ = app.player_tx.send(playterm_player::PlayerCommand::Quit);
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
    last_rendered_art: &mut Option<(String, Rect)>,
    art_displayed: &mut bool,
) -> anyhow::Result<()> {
    use crate::fzf_picker;
    use crate::library_index;

    if !app.config.library_index_enabled {
        app.flash_status("Library index is disabled in config");
        return Ok(());
    }
    if app.library_index_refreshing {
        app.flash_status_secs("Library index refresh in progress — try again shortly", 8);
        return Ok(());
    }
    if app.library_index_tracks.is_empty() {
        app.flash_status("Library index empty — wait for refresh or press Ctrl+r");
        return Ok(());
    }

    fzf_picker::suspend_tui(terminal, app.in_tmux)?;
    let input = library_index::fzf_input_lines(&app.library_index_tracks);
    let mut fzf_args = app.config.fzf_args.clone();
    if !fzf_args.iter().any(|a| a.starts_with("--header")) {
        fzf_args.insert(0, format!("--header={}", library_index::fzf_header_line()));
    }
    let res = fzf_picker::run_fzf(&app.config.fzf_binary, &fzf_args, &input);
    if let Err(e) = fzf_picker::resume_tui(terminal, app.in_tmux) {
        eprintln!("resume terminal after fzf: {e}");
    }
    // Subprocess UI may leave the alternate buffer and Kitty graphics out of sync;
    // clear everything and force a full redraw on the next frame.
    if let Err(e) = terminal.clear() {
        eprintln!("terminal clear after fzf: {e}");
    }
    if app.kitty_supported {
        let _ = ui::kitty_art::clear_image(app.in_tmux);
        *last_rendered_art = None;
        *art_displayed = false;
        if app.active_tab == Tab::Home {
            app.home_art_needs_redraw = true;
        }
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

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    signal_quit: Arc<AtomicBool>,
    mpris_ctrl_rx: Option<std::sync::mpsc::Receiver<crate::mpris::MprisControl>>,
) -> Result<()> {
    // `last_rendered_art` — the (cover_id, rect) of the last full image
    // transmission.  Kept across tab switches so we can detect whether a
    // re-transmit is actually needed.
    //
    // `art_displayed` — whether the image is currently visible on screen.
    // Set to false when switching away (ratatui overwrites those cells) but we
    // deliberately do NOT clear the image from the terminal's store, so we can
    // redisplay it instantly with `a=p,i=1` when switching back.
    let mut last_rendered_art: Option<(String, Rect)> = None;
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

        terminal.draw(|f| ui::render(app, f))?;

        // ── Kitty album art (rendered after ratatui so it sits above text) ──────
        if app.kitty_supported {
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
                } else if let Some((cover_id, bytes)) = &app.art_cache {
                    let sz = terminal.size()?;
                    let art_rect = ui::layout::art_rect(Rect::new(0, 0, sz.width, sz.height));

                    if kitty_cover_unrenderable.as_deref() == Some(cover_id.as_str()) {
                        if art_displayed {
                            let _ = ui::kitty_art::clear_image(app.in_tmux);
                            art_displayed = false;
                        }
                    } else {
                        let stored_matches = last_rendered_art
                            .as_ref()
                            .map(|(id, r)| id == cover_id && r == &art_rect)
                            .unwrap_or(false);

                        if stored_matches && art_displayed {
                            // Image is already visible — nothing to do.
                        } else {
                            // Album changed, first display, tab return, or terminal
                            // was resized — full re-encode and re-transmit.
                            match ui::kitty_art::render_image(
                                bytes,
                                art_rect,
                                app.in_tmux,
                                app.tmux_status_offset,
                            ) {
                                Ok(()) => {
                                    last_rendered_art = Some((cover_id.clone(), art_rect));
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
            if app.home_art_needs_redraw
                && app.active_tab == app::Tab::Home
                && !app.help_visible
            {
                let sz = terminal.size()?;
                let area = Rect::new(0, 0, sz.width, sz.height);
                // Replicate the strip area used in home_tab.rs render.
                let content_area = ui::layout::build_layout(area).center;
                let half = (content_area.height / 2).max(3);
                let top_area = Rect { height: half, ..content_area };
                // albums_inner = top_area inset by 1 on each side (block border).
                let albums_inner = Rect {
                    x: top_area.x + 1,
                    y: top_area.y + 1,
                    width: top_area.width.saturating_sub(2),
                    height: top_area.height.saturating_sub(2),
                };
                let thumb_area_h = albums_inner.height.saturating_sub(2).max(1);
                let strip_rect = Rect { height: thumb_area_h, ..albums_inner };
                ui::kitty_art::render_art_strip(
                    &app.home.recent_albums,
                    app.home.album_scroll_offset,
                    app.home.album_selected_index,
                    &app.home_art_cache,
                    strip_rect,
                    app.cell_px,
                    albums_inner.x,
                    albums_inner.y,
                    app.in_tmux,
                );
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
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe
                || e.kind() == std::io::ErrorKind::UnexpectedEof => {
                app.should_quit = true;
            }
            Err(e) => return Err(e.into()),
            Ok(false) => {}
            Ok(true) => loop {
                let read_result = event::read();
                match read_result {
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe
                        || e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        app.should_quit = true;
                    }
                    Err(e) => return Err(e.into()),
                    Ok(ev) => match ev {
                Event::Key(key) => {
                    // Only process key-press events; ignore release/repeat to avoid
                    // double-firing on terminals that send all event kinds (e.g. Kitty).
                    if key.kind == KeyEventKind::Press {
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
                            let is_quit_in_normal = app.keybinds.quit.matches(key.code, key.modifiers)
                                && matches!(app.playlist_overlay.input_mode, PlaylistInputMode::Normal);
                            if is_tab_switch {
                                app.playlist_overlay.visible = false;
                                let action = map_key(
                                    key.code,
                                    key.modifiers,
                                    app.active_tab,
                                    &app.keybinds,
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
                                );
                                app.dispatch(action);
                            }
                        } else {
                            let action = if app.help_visible {
                                map_help_key(key.code, key.modifiers, &app.keybinds)
                            } else if app.search_mode.active {
                                map_search_key(key.code)
                            } else {
                                map_key(key.code, key.modifiers, app.active_tab, &app.keybinds)
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
                }
                Event::Mouse(mouse) => {
                    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                        let sz = terminal.size()?;
                        let area = ratatui::layout::Rect::new(0, 0, sz.width, sz.height);
                        handle_mouse_click(mouse.column, mouse.row, app, area);
                    }
                }
                Event::Resize(_, _) => {
                    // Terminal resized — clear any displayed image and reset
                    // stored state so the art is re-encoded at the new size.
                    // last_rendered_art rect will no longer match the new
                    // art_rect, so the full render path is taken on next frame.
                    if app.kitty_supported && art_displayed {
                        let _ = ui::kitty_art::clear_image(app.in_tmux);
                        art_displayed = false;
                        last_rendered_art = None;
                    }
                    // Clear art strip thumbnails on resize so they re-render at the new size.
                    if app.kitty_supported && app.active_tab == app::Tab::Home {
                        let _ = ui::kitty_art::clear_art_strip(app.in_tmux);
                    }
                }
                // tmux focus events (requires `focus-events on` in tmux.conf).
                // FocusLost  → user switched to another tmux window/pane; clear
                //              Kitty images so they don't bleed into that pane.
                // FocusGained → we're visible again; force a full redraw.
                //
                Event::FocusLost => {
                    if app.kitty_supported && app.in_tmux {
                        let _ = ui::kitty_art::clear_image(app.in_tmux);
                        let _ = ui::kitty_art::clear_art_strip(app.in_tmux);
                        art_displayed = false;
                    }
                }
                Event::FocusGained => {
                    if app.kitty_supported {
                        // Force a full art re-transmit on the next frame — same
                        // mechanism as tab return (last_rendered_art = None makes
                        // stored_matches false, taking the re-encode path).
                        art_displayed = false;
                        last_rendered_art = None;
                    }
                }
                _ => {}
                    } // end Ok(ev) match
                }     // end read_result match

                if app.should_quit {
                    break;
                }

                match event::poll(Duration::ZERO) {
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe
                        || e.kind() == std::io::ErrorKind::UnexpectedEof => {
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
            let latched_bad = match (&app.art_cache, &kitty_cover_unrenderable) {
                (Some((cid, _)), Some(bad)) if bad == cid => true,
                _ => false,
            };
            if app.kitty_supported
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
    use ratatui::layout::{Constraint, Layout};
    use crate::ui::kitty_art::art_strip_thumbnail_size;

    if y < center.y || y >= center.y + center.height {
        return;
    }

    // Replicate the home_tab layout: top 50% = recently played, bottom 50% = tracks+rediscover.
    let half = (center.height / 2).max(3);
    let bottom_h = center.height.saturating_sub(half);

    let top_area = ratatui::layout::Rect {
        x: center.x,
        y: center.y,
        width: center.width,
        height: half,
    };
    let bottom_area = ratatui::layout::Rect {
        x: center.x,
        y: center.y + half,
        width: center.width,
        height: bottom_h,
    };

    // ── Top block: Recently Played ────────────────────────────────────────────
    if y >= top_area.y && y < top_area.y + top_area.height {
        // Inner area (subtract 1-cell border on all sides).
        let inner = ratatui::layout::Rect {
            x: top_area.x + 1,
            y: top_area.y + 1,
            width: top_area.width.saturating_sub(2),
            height: top_area.height.saturating_sub(2),
        };
        if y >= inner.y && y < inner.y + inner.height && x >= inner.x && x < inner.x + inner.width {
            // Focus the RecentAlbums section.
            app.home.active_section = app::HomeSection::RecentAlbums;
            app.home.selected_index = 0;

            // Compute which thumbnail was clicked.
            let thumb_area_h = inner.height.saturating_sub(2).max(1);
            let (thumb_cols, _) = art_strip_thumbnail_size(app.cell_px, thumb_area_h);
            let gap = 1u16;
            let rel_x = x.saturating_sub(inner.x);
            let slot = (rel_x / (thumb_cols + gap)) as usize;
            let album_index = app.home.album_scroll_offset + slot;
            if album_index < app.home.recent_albums.len() {
                if app.home.album_selected_index == album_index {
                    // Second click on already-selected album: navigate to Browser.
                    if app.kitty_supported {
                        let _ = crate::ui::kitty_art::clear_image(app.in_tmux);
                        let _ = crate::ui::kitty_art::clear_art_strip(app.in_tmux);
                    }
                    app.active_tab = app::Tab::Browser;
                    app.search_filter = None;
                } else {
                    // First click: just select.
                    app.home.album_selected_index = album_index;
                }
            }
        }
        return;
    }

    // ── Bottom blocks ─────────────────────────────────────────────────────────
    if bottom_h == 0 || y < bottom_area.y || y >= bottom_area.y + bottom_area.height {
        return;
    }

    let bottom_cols = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .split(bottom_area);

    let tracks_area    = bottom_cols[0];
    let rediscover_area = bottom_cols[1];

    // Recent Tracks block.
    if x >= tracks_area.x && x < tracks_area.x + tracks_area.width {
        let inner_y = tracks_area.y + 1;
        let inner_h = tracks_area.height.saturating_sub(2);
        if y >= inner_y && y < inner_y + inner_h {
            let row = (y - inner_y) as usize;
            if row < app.home.recent_tracks.len() {
                if app.home.active_section == app::HomeSection::RecentTracks
                    && app.home.selected_index == row
                {
                    // Second click on already-selected row: play it.
                    app.dispatch(Action::Select);
                } else {
                    app.home.active_section = app::HomeSection::RecentTracks;
                    app.home.selected_index = row;
                }
            }
        }
        return;
    }

    // Rediscover block.
    if x >= rediscover_area.x && x < rediscover_area.x + rediscover_area.width {
        let inner_y = rediscover_area.y + 1;
        let inner_h = rediscover_area.height.saturating_sub(2);
        if y >= inner_y && y < inner_y + inner_h {
            let row = (y - inner_y) as usize;
            if row < app.home.rediscover.len() {
                if app.home.active_section == app::HomeSection::Rediscover
                    && app.home.selected_index == row
                {
                    // Second click: navigate to Browser.
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
) -> Action {
    match input_mode {
        // ── Text-input modes: feed characters into the buffer ──────────────
        PlaylistInputMode::Creating { .. } | PlaylistInputMode::Renaming { .. } => match code {
            KeyCode::Esc       => Action::PlaylistInputCancel,
            KeyCode::Enter     => Action::PlaylistInputConfirm,
            KeyCode::Backspace => Action::PlaylistInputChar('\x08'),
            KeyCode::Char(ch)  => Action::PlaylistInputChar(ch),
            _                  => Action::None,
        },
        // ── Confirmation prompt: y/n ───────────────────────────────────────
        PlaylistInputMode::Confirming { .. } => match code {
            KeyCode::Char('y') | KeyCode::Char('Y')                  => Action::PlaylistConfirmYes,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc   => Action::PlaylistConfirmNo,
            _                                                          => Action::None,
        },
        // ── Normal navigation / mutation ───────────────────────────────────
        PlaylistInputMode::Normal => {
            let shift = modifiers.intersects(KeyModifiers::SHIFT);
            match code {
                KeyCode::Esc   => Action::TogglePlaylistOverlay,
                // Shift+P toggles the overlay closed (mirrors the map_key open binding).
                KeyCode::Char('P') | KeyCode::Char('p') if code == KeyCode::Char('P') || shift => {
                    Action::TogglePlaylistOverlay
                }
                KeyCode::Char('k') | KeyCode::Up   => Action::PlaylistScrollUp,
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
                    PlaylistFocus::List   => Action::PlaylistPlayAll,
                    PlaylistFocus::Tracks => Action::PlaylistPlayTrack,
                },
                // Shift+A — append all / append track
                KeyCode::Char('A') | KeyCode::Char('a') if code == KeyCode::Char('A') || shift => {
                    match focus {
                        PlaylistFocus::List   => Action::PlaylistAppendAll,
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
        KeyCode::Esc                          => Action::PlaylistPickerCancel,
        KeyCode::Enter                        => Action::PlaylistPickerSelect,
        KeyCode::Char('k') | KeyCode::Up      => Action::PlaylistPickerScrollUp,
        KeyCode::Char('j') | KeyCode::Down    => Action::PlaylistPickerScrollDown,
        _                                     => Action::None,
    }
}

fn map_key(code: KeyCode, modifiers: KeyModifiers, active_tab: Tab, kb: &Keybinds) -> Action {
    // ── Browser-tab-specific keys ─────────────────────────────────────────────
    if active_tab == Tab::Browser {
        // Shift+P — open/close the playlist overlay
        let shift = modifiers.intersects(KeyModifiers::SHIFT);
        if code == KeyCode::Char('P') || (code == KeyCode::Char('p') && shift) {
            return Action::TogglePlaylistOverlay;
        }
        // > — open playlist picker to add focused track to a playlist
        if code == KeyCode::Char('>') {
            return Action::BrowserAddToPlaylist;
        }
    }

    // ── Home-tab-specific keys ────────────────────────────────────────────────
    if active_tab == Tab::Home {
        // J / Shift+j: move to next section.
        // Handle both KeyCode::Char('J') (most terminals) and
        // KeyCode::Char('j')+SHIFT (Ghostty / kitty keyboard protocol).
        let shift = modifiers.intersects(KeyModifiers::SHIFT);
        if code == KeyCode::Char('J') || (code == KeyCode::Char('j') && shift) {
            return Action::HomeSectionNext;
        }
        // K / Shift+k: move to previous section.
        if code == KeyCode::Char('K') || (code == KeyCode::Char('k') && shift) {
            return Action::HomeSectionPrev;
        }
        // r: re-roll rediscover / refresh data
        if code == KeyCode::Char('r') && modifiers.is_empty() { return Action::HomeRefresh; }
        // h/l: navigate album strip left/right (only active when RecentAlbums section is focused)
        if code == KeyCode::Char('h') && modifiers.is_empty() { return Action::HomeAlbumLeft; }
        if code == KeyCode::Char('l') && modifiers.is_empty() { return Action::HomeAlbumRight; }
        // a: append selected album to queue
        if code == KeyCode::Char('a') && modifiers.is_empty() { return Action::HomeAlbumAddToQueue; }
    }

    // ── Always-on / non-configurable ─────────────────────────────────────────
    // g / G: jump to top/bottom — not exposed in config
    if code == KeyCode::Char('g') && modifiers.is_empty() { return Action::Navigate(Direction::Top);    }
    if code == KeyCode::Char('G') && modifiers.is_empty() { return Action::Navigate(Direction::Bottom); }
    // Enter / Esc — not configurable
    if code == KeyCode::Enter { return Action::Select; }
    if code == KeyCode::Esc   { return Action::Back;   }
    // Space is always an alias for play_pause
    if code == KeyCode::Char(' ') { return Action::PlayPause; }
    // '=' is always a secondary alias for volume_up (easy to hit with +)
    if code == KeyCode::Char('=') { return Action::VolumeUp; }
    // 'i' toggles the keybind help popup
    if code == KeyCode::Char('i') && modifiers.is_empty() { return Action::ToggleHelp; }
    // 't' toggles dynamic accent colour
    if code == KeyCode::Char('t') && modifiers.is_empty() { return Action::ToggleDynamicTheme; }
    // 'L' toggles lyrics overlay (NowPlaying tab only)
    if code == KeyCode::Char('L') { return Action::ToggleLyrics; }
    // 'V' toggles spectrum visualizer (NowPlaying tab only)
    let shift = modifiers.intersects(KeyModifiers::SHIFT);
    if code == KeyCode::Char('V') || (code == KeyCode::Char('v') && shift) {
        return Action::ToggleVisualizer;
    }
    // Up/Down arrows are always secondary scroll aliases
    if code == KeyCode::Up   { return Action::Navigate(Direction::Up);   }
    if code == KeyCode::Down { return Action::Navigate(Direction::Down); }

    // ── Configurable keybinds ─────────────────────────────────────────────────
    if kb.quit.matches(code, modifiers)              { return Action::Quit;             }
    if kb.tab_switch.matches(code, modifiers)        { return Action::SwitchTab;        }
    if kb.tab_switch_reverse.matches(code, modifiers){ return Action::SwitchTabReverse; }
    // BackTab (Shift-Tab) is always an alias for reverse tab cycle.
    if code == KeyCode::BackTab                      { return Action::SwitchTabReverse; }
    if kb.go_to_home.matches(code, modifiers)        { return Action::GoToHome;         }
    if kb.go_to_browser.matches(code, modifiers)     { return Action::GoToBrowser;      }
    if kb.go_to_nowplaying.matches(code, modifiers)  { return Action::GoToNowPlaying;   }

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

    if kb.column_left.matches(code, modifiers)  { return Action::FocusLeft;  }
    if kb.column_right.matches(code, modifiers) { return Action::FocusRight; }
    if kb.scroll_up.matches(code, modifiers)    { return Action::Navigate(Direction::Up);   }
    if kb.scroll_down.matches(code, modifiers)  { return Action::Navigate(Direction::Down); }

    if kb.play_pause.matches(code, modifiers)   { return Action::PlayPause;    }
    if kb.next_track.matches(code, modifiers)   { return Action::NextTrack;    }
    if kb.prev_track.matches(code, modifiers)   { return Action::PrevTrack;    }

    // add_all must be checked before add_track (it's typically a superset key).
    if kb.add_all.matches(code, modifiers)      { return Action::AddAllToQueue; }
    if kb.add_track.matches(code, modifiers)    { return Action::AddToQueue;    }

    if kb.shuffle.matches(code, modifiers)      { return Action::Shuffle;      }
    if kb.unshuffle.matches(code, modifiers)    { return Action::Unshuffle;    }
    if kb.clear_queue.matches(code, modifiers)  { return Action::ClearQueue;   }
    if kb.search.matches(code, modifiers)       { return Action::SearchStart;  }
    if kb.volume_up.matches(code, modifiers)    { return Action::VolumeUp;     }
    if kb.volume_down.matches(code, modifiers)  { return Action::VolumeDown;   }

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

    Action::None
}

fn map_search_key(code: KeyCode) -> Action {
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
    if code == KeyCode::Char('i') && modifiers.is_empty() { return Action::ToggleHelp; }
    if code == KeyCode::Esc                               { return Action::ToggleHelp; }
    if kb.quit.matches(code, modifiers)                   { return Action::ToggleHelp; }
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
// For 16 albums this is multiple seconds of CPU-bound work every ~50 ms poll tick.
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
    let areas = ui::layout::build_layout(terminal_size);
    let center = areas.center;
    let now_playing = areas.now_playing;

    // ── Tab bar: dispatch GoToHome / GoToBrowser / GoToNowPlaying ────────────
    if y == areas.tab_bar.y {
        // The labels are:  " Home "  " │ "  " Browse "  " │ "  " Now Playing "
        // Measure cumulative widths to decide which label was clicked.
        // Label widths (chars): Home=6, sep=3, Browse=8, sep=3, NowPlaying=13
        let home_end:      u16 = 6;
        let browser_start: u16 = 9;   // 6+3
        let browser_end:   u16 = 17;  // 9+8
        let np_start:      u16 = 20;  // 17+3

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

    // ── Now-playing bar: [30% info | 40% controls | 30% progress] ────────────
    let np_cols = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(40),
        Constraint::Percentage(30),
    ])
    .split(now_playing);

    let controls_area = np_cols[1];
    let progress_area = np_cols[2];

    // Controls: row 1 of the now-playing bar (0-indexed).
    if y == now_playing.y + 1
        && x >= controls_area.x
        && x < controls_area.x + controls_area.width
    {
        // Divide controls into three equal zones: prev | play-pause | next.
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

    // Progress bar: row 2 of the now-playing bar.
    if y == now_playing.y + 2
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
            let bar_end = (progress_area.x + progress_area.width)
                .saturating_sub(total_str_len + 2);

            if x >= bar_start && bar_end > bar_start {
                let bar_w = (bar_end - bar_start) as f64;
                let ratio = (x - bar_start) as f64 / bar_w;
                let seek_secs = (ratio * ts as f64) as u64;
                app.dispatch(Action::SeekTo(std::time::Duration::from_secs(seek_secs)));
            }
        }
        return;
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
            // 3 columns: [30% artists | 35% albums | 35% tracks]
            let browser_cols = Layout::horizontal([
                Constraint::Percentage(30),
                Constraint::Percentage(35),
                Constraint::Percentage(35),
            ])
            .split(center);

            let col_idx = if x < browser_cols[1].x {
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
            app.browser_focus = match col_idx {
                0 => BrowserColumn::Artists,
                1 => BrowserColumn::Albums,
                _ => BrowserColumn::Tracks,
            };

            match col_idx {
                0 => {
                    // Artists: compute ratatui's auto-scroll offset and map click.
                    let orig_idx: Option<usize> = {
                        if let LoadingState::Loaded(artists) = &app.library.artists {
                            let visible: Vec<usize> = if let Some(q) = &app.search_filter {
                                artists.iter().enumerate()
                                    .filter(|(_, a)| a.name.to_lowercase().contains(q.as_str()))
                                    .map(|(i, _)| i)
                                    .collect()
                            } else {
                                (0..artists.len()).collect()
                            };
                            let sel_pos = app.library.selected_artist
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
                            let visible: Vec<usize> = if let Some(q) = &app.search_filter {
                                albums.iter().enumerate()
                                    .filter(|(_, a)| a.name.to_lowercase().contains(q.as_str()))
                                    .map(|(i, _)| i)
                                    .collect()
                            } else {
                                (0..albums.len()).collect()
                            };
                            let sel_pos = app.library.selected_album
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
                        if let Some(LoadingState::Loaded(songs)) =
                            app.library.tracks.get(&album_id)
                        {
                            let visible: Vec<usize> = if let Some(q) = &app.search_filter {
                                songs.iter().enumerate()
                                    .filter(|(_, s)| s.title.to_lowercase().contains(q.as_str()))
                                    .map(|(i, _)| i)
                                    .collect()
                            } else {
                                (0..songs.len()).collect()
                            };
                            let sel_pos = app.library.selected_track
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
            let queue_area = ui::layout::now_playing_queue_widget_rect(
                center,
                app.lyrics_visible,
                app.visualizer_visible,
            );
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
            if s.starts_with("on") && s.ends_with("top") { 1 } else { 0 }
        }
        Err(_) => 1, // safe default: assume top status bar
    }
}
