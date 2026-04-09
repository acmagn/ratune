pub mod albums;
pub mod artists;
pub mod browser;
pub mod home_tab;
pub mod kitty_art;
pub mod layout;
pub mod now_playing;
pub mod now_playing_format;
pub mod nowplaying_tab;
pub mod playlist_overlay;
pub mod popup;
pub mod queue;
pub mod status_bar;
pub mod tab_bar;
pub mod tracks;
pub mod visualizer;

use ratatui::Frame;
use crate::app::{App, Tab};

use home_tab::render_home_tab;

// Colour palette constants removed — all colours are now accessed via
// `app.theme.*` (see `playterm/src/theme.rs`) so that the [theme] config
// section can override them at runtime.

// ── Top-level render ──────────────────────────────────────────────────────────

pub fn render(app: &App, frame: &mut Frame) {
    let total_rows = frame.area().height;

    match app.active_tab {
        Tab::Home => {
            let areas = layout::build_layout(frame.area(), &layout::layout_options_for_app(app));
            render_home_tab(
                frame,
                areas.center,
                &app.home,
                &app.config,
                app.accent(),
                app.kitty_supported,
                app.help_visible,
                app.cell_px,
                &app.theme,
            );
            now_playing::render(app, frame, areas.now_playing);
            status_bar::render(app, frame, areas.status_bar);
            // Tab bar (skip if terminal is too small).
            if total_rows >= 20 {
                tab_bar::render_tab_bar(frame, areas.tab_bar, app.active_tab, app.accent());
            }
        }
        Tab::Browser => {
            let areas = layout::build_layout(frame.area(), &layout::layout_options_for_app(app));
            browser::render(app, frame, areas.center);
            now_playing::render(app, frame, areas.now_playing);
            status_bar::render(app, frame, areas.status_bar);
            // Tab bar (skip if terminal is too small).
            if total_rows >= 20 {
                tab_bar::render_tab_bar(frame, areas.tab_bar, app.active_tab, app.accent());
            }
        }
        Tab::NowPlaying => {
            let areas = layout::build_layout(frame.area(), &layout::layout_options_for_app(app));
            nowplaying_tab::render(app, frame, areas.center);
            now_playing::render(app, frame, areas.now_playing);
            status_bar::render(app, frame, areas.status_bar);
            // Tab bar (skip if terminal is too small).
            if total_rows >= 20 {
                tab_bar::render_tab_bar(frame, areas.tab_bar, app.active_tab, app.accent());
            }
        }
    }
    // Popup renders last so it layers on top of everything.
    if app.help_visible {
        popup::render_help(app, frame);
    }
}
