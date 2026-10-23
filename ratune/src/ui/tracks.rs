use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, BrowserColumn};
use crate::state::{
    album_track_rows, disc_header_label, AlbumTrackRow, LibraryState, LoadingState,
};
use crate::theme::style_with_bg;

pub fn render(app: &mut App, frame: &mut Frame, area: Rect, is_active: bool) {
    let t = &app.theme;
    let border_color = if is_active { app.accent() } else { t.border };
    let title_color = if is_active { app.accent() } else { t.dimmed };

    let title = app
        .library
        .current_album()
        .map(|a| format!(" {} ", a.name))
        .unwrap_or_else(|| " Tracks ".to_string());

    let border_style = if is_active {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(border_color)
    };
    let block = Block::default()
        .title(title)
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_set(t.border_set)
        .border_style(border_style)
        .style(style_with_bg(t.surface));

    let album_id = match app.library.current_album() {
        Some(a) => a.id.clone(),
        None => {
            let list = List::new(vec![
                ListItem::new("← Select an album").style(Style::default().fg(t.dimmed))
            ])
            .block(block);
            frame.render_widget(list, area);
            return;
        }
    };

    match app.library.tracks.get(&album_id) {
        None | Some(LoadingState::NotLoaded) | Some(LoadingState::Loading) => {
            let item = ListItem::new("Loading…").style(Style::default().fg(t.dimmed));
            let list = List::new(vec![item]).block(block);
            frame.render_widget(list, area);
        }
        Some(LoadingState::Error(e)) => {
            let item =
                ListItem::new(format!("Error: {e}")).style(Style::default().fg(app.accent()));
            let list = List::new(vec![item]).block(block);
            frame.render_widget(list, area);
        }
        Some(LoadingState::Loaded(songs)) => {
            let make_label = |s: &ratune_subsonic::Song| {
                let num = s.track.map(|n| format!("{n:>2}. ")).unwrap_or_default();
                let star = if s.starred.is_some() {
                    app.theme.icons.favorite_prefix()
                } else {
                    String::new()
                };
                let rating_suffix = if app.config.ratings_enabled {
                    let rating = app.config.rating_stars.format(s.user_rating);
                    if rating.is_empty() {
                        String::new()
                    } else {
                        format!("  {rating}")
                    }
                } else {
                    String::new()
                };
                let dur = s
                    .duration
                    .map(|d| {
                        let m = d / 60;
                        let sec = d % 60;
                        format!("  {m}:{sec:02}")
                    })
                    .unwrap_or_default();
                format!("{}{}{}{}{}", num, star, s.title, dur, rating_suffix)
            };

            let disc_titles = app
                .library
                .disc_titles
                .get(&album_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let visible = album_track_rows(songs, app.browser_column_filter(BrowserColumn::Tracks));

            let items: Vec<ListItem> = if visible.is_empty() {
                vec![ListItem::new("No matches").style(Style::default().fg(t.dimmed))]
            } else {
                visible
                    .iter()
                    .map(|row| match *row {
                        AlbumTrackRow::Disc(n) => ListItem::new(disc_header_label(n, disc_titles))
                            .style(Style::default().fg(t.dimmed).add_modifier(Modifier::BOLD)),
                        AlbumTrackRow::Track(i) => ListItem::new(make_label(&songs[i]))
                            .style(Style::default().fg(t.foreground)),
                    })
                    .collect()
            };

            let sel = app
                .library
                .selected_track
                .and_then(|s| visible.iter().position(|r| *r == AlbumTrackRow::Track(s)));

            let list = List::new(items)
                .block(block)
                .highlight_style(
                    Style::default()
                        .bg(app.accent())
                        .fg(t.background)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("▶ ")
                .style(style_with_bg(t.surface));

            let vh = area.height.saturating_sub(2) as usize;
            let vh = vh.max(1);
            app.browser_list_viewport_rows = vh;

            let mut state = ListState::default();
            if !visible.is_empty() {
                if let Some(sel_ix) = sel {
                    LibraryState::clamp_vertical_scroll(
                        &mut app.library.tracks_scroll,
                        sel_ix,
                        visible.len(),
                        vh,
                    );
                    // Keep a disc's header on screen when its first track is selected.
                    if sel_ix > 0
                        && vh > 1
                        && app.library.tracks_scroll == sel_ix
                        && matches!(visible[sel_ix - 1], AlbumTrackRow::Disc(_))
                    {
                        app.library.tracks_scroll = sel_ix - 1;
                    }
                    state = ListState::default().with_offset(app.library.tracks_scroll);
                    state.select(Some(sel_ix));
                } else {
                    let max_first = visible.len().saturating_sub(vh);
                    app.library.tracks_scroll = app.library.tracks_scroll.min(max_first);
                    state = ListState::default().with_offset(app.library.tracks_scroll);
                }
            }
            frame.render_stateful_widget(list, area, &mut state);
        }
    }
}
