use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, BrowserColumn};
use crate::config::BrowseMode;
use super::{artists, albums, playlist_overlay, tracks};

pub fn render(app: &mut App, frame: &mut Frame, area: Rect) {
    match app.config.browse_mode {
        BrowseMode::Artists => {}
        BrowseMode::Genre => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Genre browsing is not implemented yet.\n\
                     Use [ui.browsetab] mode = \"artists\" for the artist/album/track browser.",
                    Style::default().fg(Color::DarkGray),
                ))),
                area,
            );
            return;
        }
        BrowseMode::Files => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "File browsing is not implemented yet.\n\
                     Use [ui.browsetab] mode = \"artists\" for the artist/album/track browser.",
                    Style::default().fg(Color::DarkGray),
                ))),
                area,
            );
            return;
        }
    }

    let cols = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(35),
        Constraint::Percentage(35),
    ])
    .split(area);

    artists::render(app, frame, cols[0], matches!(app.browser_focus, BrowserColumn::Artists));
    albums::render(app, frame, cols[1], matches!(app.browser_focus, BrowserColumn::Albums));
    tracks::render(app, frame, cols[2], matches!(app.browser_focus, BrowserColumn::Tracks));

    playlist_overlay::render_playlist_overlay(
        frame,
        area,
        &app.playlist_overlay,
        app.accent(),
        &app.theme,
    );

    if let Some(picker) = &app.playlist_picker {
        playlist_overlay::render_playlist_picker(frame, area, picker, app.accent(), &app.theme);
    }
}
