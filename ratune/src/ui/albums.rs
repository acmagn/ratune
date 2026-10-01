use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, BrowserColumn};
use crate::state::{LibraryState, LoadingState};
use crate::theme::style_with_bg;
use ratune_subsonic::Album;

pub fn render(app: &mut App, frame: &mut Frame, area: Rect, is_active: bool) {
    let t = &app.theme;
    let border_color = if is_active { app.accent() } else { t.border };
    let title_color = if is_active { app.accent() } else { t.dimmed };

    let border_style = if is_active {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(border_color)
    };

    let title = if app.library.use_flat_albums {
        format!(" Albums ({}) ", app.album_list_sort.label())
    } else {
        " Albums ".to_string()
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

    let albums_state: &LoadingState<Vec<Album>> = if app.library.use_flat_albums {
        &app.library.flat_albums
    } else {
        let Some(artist) = app.library.current_artist() else {
            let list = List::new(vec![
                ListItem::new("← Select an artist").style(Style::default().fg(t.dimmed))
            ])
            .block(block);
            frame.render_widget(list, area);
            return;
        };
        match app.library.albums.get(&artist.id) {
            Some(state) => state,
            None => {
                let list = List::new(vec![
                    ListItem::new("← Select an artist").style(Style::default().fg(t.dimmed))
                ])
                .block(block);
                frame.render_widget(list, area);
                return;
            }
        }
    };

    match albums_state {
        LoadingState::NotLoaded | LoadingState::Loading => {
            let item = ListItem::new("Loading…").style(Style::default().fg(t.dimmed));
            let list = List::new(vec![item]).block(block);
            frame.render_widget(list, area);
        }
        LoadingState::Error(e) => {
            let item =
                ListItem::new(format!("Error: {e}")).style(Style::default().fg(app.accent()));
            let list = List::new(vec![item]).block(block);
            frame.render_widget(list, area);
        }
        LoadingState::Loaded(albums) => {
            let flat = app.library.use_flat_albums;
            let make_label = |a: &Album| {
                let star = if a.starred.is_some() {
                    app.theme.icons.favorite_prefix()
                } else {
                    String::new()
                };
                let rating_suffix = if app.config.ratings_enabled {
                    let rating = app.config.rating_stars.format(a.user_rating);
                    if rating.is_empty() {
                        String::new()
                    } else {
                        format!("  {rating}")
                    }
                } else {
                    String::new()
                };
                if flat {
                    let artist = a.artist.as_deref().unwrap_or("Unknown Artist");
                    match a.year {
                        Some(y) => {
                            format!("{}{} — {} ({}){}", star, a.name, artist, y, rating_suffix)
                        }
                        None => format!("{}{} — {}{}", star, a.name, artist, rating_suffix),
                    }
                } else {
                    match a.year {
                        Some(y) => format!("{}{} ({}){}", star, a.name, y, rating_suffix),
                        None => format!("{}{}{}", star, a.name, rating_suffix),
                    }
                }
            };

            let visible: Vec<(usize, String)> =
                if let Some(q) = app.browser_column_filter(BrowserColumn::Albums) {
                    albums
                        .iter()
                        .enumerate()
                        .filter(|(_, a)| {
                            let name_hit = a.name.to_lowercase().contains(q);
                            let artist_hit = a
                                .artist
                                .as_deref()
                                .map(|s| s.to_lowercase().contains(q))
                                .unwrap_or(false);
                            name_hit || artist_hit
                        })
                        .map(|(i, a)| (i, make_label(a)))
                        .collect()
                } else {
                    albums
                        .iter()
                        .enumerate()
                        .map(|(i, a)| (i, make_label(a)))
                        .collect()
                };

            let items: Vec<ListItem> = if visible.is_empty() {
                vec![ListItem::new("No matches").style(Style::default().fg(t.dimmed))]
            } else {
                visible
                    .iter()
                    .map(|(_, label)| {
                        ListItem::new(label.as_str()).style(Style::default().fg(t.foreground))
                    })
                    .collect()
            };

            let sel = app
                .library
                .selected_album
                .and_then(|s| visible.iter().position(|(i, _)| *i == s));

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
                        &mut app.library.albums_scroll,
                        sel_ix,
                        visible.len(),
                        vh,
                    );
                    state = ListState::default().with_offset(app.library.albums_scroll);
                    state.select(Some(sel_ix));
                } else {
                    let max_first = visible.len().saturating_sub(vh);
                    app.library.albums_scroll = app.library.albums_scroll.min(max_first);
                    state = ListState::default().with_offset(app.library.albums_scroll);
                }
            }
            frame.render_stateful_widget(list, area, &mut state);
        }
    }
}
