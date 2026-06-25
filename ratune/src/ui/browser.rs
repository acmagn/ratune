use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{albums, artists, favorites_overlay, folder_tracks, folders, playlist_overlay, tracks};
use crate::app::{App, BrowserColumn};
use crate::config::BrowseMode;

pub fn render(app: &mut App, frame: &mut Frame, area: Rect) {
    let accent = app.accent();
    let theme = app.theme.clone();
    match app.browser_browse_mode {
        BrowseMode::Genre => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Genre browsing is not implemented yet.\n\
                     Use [ui.browsetab] mode = \"artists\" or \"files\".",
                    Style::default().fg(Color::DarkGray),
                ))),
                area,
            );
            playlist_overlay::render_playlist_overlay(
                frame,
                area,
                &mut app.playlist_overlay,
                accent,
                &theme,
            );
            if let Some(picker) = app.playlist_picker.as_mut() {
                playlist_overlay::render_playlist_picker(frame, area, picker, accent, &theme);
            }
            favorites_overlay::render_favorites_overlay(
                frame,
                area,
                &mut app.favorites_overlay,
                accent,
                &theme,
            );
            return;
        }
        BrowseMode::Files => {
            let cols = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                .split(area);
            folders::render(
                app,
                frame,
                cols[0],
                matches!(app.browser_focus, BrowserColumn::Artists),
            );
            folder_tracks::render(
                app,
                frame,
                cols[1],
                matches!(app.browser_focus, BrowserColumn::Tracks),
            );
            playlist_overlay::render_playlist_overlay(
                frame,
                area,
                &mut app.playlist_overlay,
                accent,
                &theme,
            );
            if let Some(picker) = app.playlist_picker.as_mut() {
                playlist_overlay::render_playlist_picker(frame, area, picker, accent, &theme);
            }
            favorites_overlay::render_favorites_overlay(
                frame,
                area,
                &mut app.favorites_overlay,
                accent,
                &theme,
            );
            return;
        }
        BrowseMode::Artists => {}
    }

    let cols = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(35),
        Constraint::Percentage(35),
    ])
    .split(area);

    artists::render(
        app,
        frame,
        cols[0],
        matches!(app.browser_focus, BrowserColumn::Artists),
    );
    albums::render(
        app,
        frame,
        cols[1],
        matches!(app.browser_focus, BrowserColumn::Albums),
    );
    tracks::render(
        app,
        frame,
        cols[2],
        matches!(app.browser_focus, BrowserColumn::Tracks),
    );

    playlist_overlay::render_playlist_overlay(
        frame,
        area,
        &mut app.playlist_overlay,
        accent,
        &theme,
    );

    if let Some(picker) = app.playlist_picker.as_mut() {
        playlist_overlay::render_playlist_picker(frame, area, picker, accent, &theme);
    }

    favorites_overlay::render_favorites_overlay(
        frame,
        area,
        &mut app.favorites_overlay,
        accent,
        &theme,
    );
}
