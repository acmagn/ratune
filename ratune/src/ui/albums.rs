use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, BrowserColumn};
use crate::state::{LibraryState, LoadingState};
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
    let block = Block::default()
        .title(" Albums ")
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(border_style)
        .style(Style::default().bg(t.surface));

    let artist_id = match app.library.current_artist() {
        Some(a) => a.id.clone(),
        None => {
            let list = List::new(vec![
                ListItem::new("← Select an artist").style(Style::default().fg(t.dimmed))
            ])
            .block(block);
            frame.render_widget(list, area);
            return;
        }
    };

    match app.library.albums.get(&artist_id) {
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
        Some(LoadingState::Loaded(albums)) => {
            let make_label = |a: &Album| match a.year {
                Some(y) => format!("{} ({})", a.name, y),
                None => a.name.clone(),
            };

            let visible: Vec<(usize, String)> =
                if let Some(q) = app.browser_column_filter(BrowserColumn::Albums) {
                    albums
                        .iter()
                        .enumerate()
                        .filter(|(_, a)| a.name.to_lowercase().contains(q))
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
                .style(Style::default().bg(t.surface));

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
