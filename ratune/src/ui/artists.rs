use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, BrowserColumn};
use crate::state::{LibraryState, LoadingState};

pub fn render(app: &mut App, frame: &mut Frame, area: Rect, is_active: bool) {
    let t = &app.theme;
    // Focused column: accent border (same idea as Visualizer / NP boxed panes), not a gray border_active.
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
        .title(" Artists ")
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(border_style)
        .style(Style::default().bg(t.surface));

    match &app.library.artists {
        LoadingState::NotLoaded | LoadingState::Loading => {
            let item = ListItem::new("Loading…").style(Style::default().fg(t.dimmed));
            let list = List::new(vec![item])
                .block(block)
                .style(Style::default().bg(t.background));
            frame.render_widget(list, area);
        }
        LoadingState::Error(e) => {
            let item =
                ListItem::new(format!("Error: {e}")).style(Style::default().fg(app.accent()));
            let list = List::new(vec![item])
                .block(block)
                .style(Style::default().bg(t.background));
            frame.render_widget(list, area);
        }
        LoadingState::Loaded(artists) => {
            // Build (original_index, name) pairs, filtered when search is active.
            let visible: Vec<(usize, &str)> =
                if let Some(q) = app.browser_column_filter(BrowserColumn::Artists) {
                    artists
                        .iter()
                        .enumerate()
                        .filter(|(_, a)| a.name.to_lowercase().contains(q))
                        .map(|(i, a)| (i, a.name.as_str()))
                        .collect()
                } else {
                    artists
                        .iter()
                        .enumerate()
                        .map(|(i, a)| (i, a.name.as_str()))
                        .collect()
                };

            let items: Vec<ListItem> = if visible.is_empty() {
                vec![ListItem::new("No matches").style(Style::default().fg(t.dimmed))]
            } else {
                visible
                    .iter()
                    .map(|(_, name)| ListItem::new(*name).style(Style::default().fg(t.foreground)))
                    .collect()
            };

            // Find where the currently selected artist sits in the visible list.
            let sel = app
                .library
                .selected_artist
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
                        &mut app.library.artists_scroll,
                        sel_ix,
                        visible.len(),
                        vh,
                    );
                    state = ListState::default().with_offset(app.library.artists_scroll);
                    state.select(Some(sel_ix));
                } else {
                    let max_first = visible.len().saturating_sub(vh);
                    app.library.artists_scroll = app.library.artists_scroll.min(max_first);
                    state = ListState::default().with_offset(app.library.artists_scroll);
                }
            }
            frame.render_stateful_widget(list, area, &mut state);
        }
    }
}
