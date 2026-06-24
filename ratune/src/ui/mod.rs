pub mod albums;
pub mod art_prepare;
pub mod artists;
pub mod browser;
pub mod favorites_overlay;
pub mod folder_tracks;
pub mod folders;
pub mod home_tab;
pub mod kitty_art;
pub mod layout;
pub mod now_playing;
pub mod now_playing_format;
pub mod nowplaying_tab;
pub mod playlist_overlay;
pub mod popup;
pub mod queue;
pub mod radio_nowplaying;
pub mod radio_popup;
pub mod status_bar;
pub mod tab_bar;
pub mod terminal_palette;
pub mod tracks;
pub mod visualizer;

use crate::app::{App, Tab};
use ratatui::Frame;

use home_tab::render_home_tab;

// ── Top-level render ──────────────────────────────────────────────────────────

pub fn render(app: &mut App, frame: &mut Frame) {
    let total_rows = frame.area().height;

    match app.active_tab {
        Tab::Home => {
            let areas = layout::build_layout(frame.area(), &layout::layout_options_for_app(app));
            render_home_tab(frame, areas.center, app, app.accent(), app.help_visible);
            now_playing::render(app, frame, areas.now_playing);
            status_bar::render(app, frame, areas.status_bar);
            if total_rows >= 20 {
                tab_bar::render_tab_bar(
                    frame,
                    areas.tab_bar,
                    app.active_tab,
                    app.accent(),
                    &app.theme,
                );
            }
        }
        Tab::Browser => {
            let areas = layout::build_layout(frame.area(), &layout::layout_options_for_app(app));
            browser::render(app, frame, areas.center);
            now_playing::render(app, frame, areas.now_playing);
            status_bar::render(app, frame, areas.status_bar);
            if total_rows >= 20 {
                tab_bar::render_tab_bar(
                    frame,
                    areas.tab_bar,
                    app.active_tab,
                    app.accent(),
                    &app.theme,
                );
            }
        }
        Tab::NowPlaying => {
            let areas = layout::build_layout(frame.area(), &layout::layout_options_for_app(app));
            nowplaying_tab::render(app, frame, areas.center);
            now_playing::render(app, frame, areas.now_playing);
            status_bar::render(app, frame, areas.status_bar);
            if total_rows >= 20 {
                tab_bar::render_tab_bar(
                    frame,
                    areas.tab_bar,
                    app.active_tab,
                    app.accent(),
                    &app.theme,
                );
            }
        }
    }
    if app.radio.picker_visible && app.config.radio_enabled {
        radio_popup::render(app, frame, frame.area());
    }
    if app.help_visible {
        popup::render_help(app, frame);
    }
}
