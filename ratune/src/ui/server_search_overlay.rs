//! Server `search3` overlay.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::state::{FavoritesCategory, ServerSearchFocus, ServerSearchOverlay};
use crate::theme::{style_with_bg, Theme};

pub struct PanelLayout {
    pub area: Rect,
    pub query_row: Rect,
    pub categories_col: Rect,
    pub items_col: Rect,
}

pub fn panel_layout(parent: Rect) -> PanelLayout {
    let area =
        Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).split(parent)[1];
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(area);
    let cols =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).split(rows[1]);
    PanelLayout {
        area,
        query_row: rows[0],
        categories_col: cols[0],
        items_col: cols[1],
    }
}

fn fmt_duration_ms(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}

pub fn render_server_search_overlay(
    frame: &mut Frame,
    area: Rect,
    overlay: &mut ServerSearchOverlay,
    accent: ratatui::style::Color,
    theme: &Theme,
) {
    if !overlay.visible {
        return;
    }

    let layout = panel_layout(area);
    frame.render_widget(Clear, layout.area);

    render_query(frame, layout.query_row, overlay, accent, theme);
    render_categories(
        frame,
        layout.categories_col,
        overlay,
        accent,
        theme,
        matches!(overlay.focus, ServerSearchFocus::Categories),
    );
    render_items(
        frame,
        layout.items_col,
        overlay,
        accent,
        theme,
        matches!(overlay.focus, ServerSearchFocus::Items),
    );
}

fn render_query(
    frame: &mut Frame,
    area: Rect,
    overlay: &ServerSearchOverlay,
    accent: ratatui::style::Color,
    theme: &Theme,
) {
    let active = matches!(overlay.focus, ServerSearchFocus::Query);
    let border_color = if active { accent } else { theme.border };
    let title_color = if active { accent } else { theme.dimmed };
    let border_style = if active {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(border_color)
    };

    let status = if overlay.loading {
        " searching…"
    } else if let Some(ref q) = overlay.last_query {
        if overlay.error.is_some() {
            " (error)"
        } else {
            let _ = q;
            ""
        }
    } else {
        " — type and Enter"
    };

    let block = Block::default()
        .title(format!(" Server Search{status} "))
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_set(theme.border_set)
        .border_style(border_style)
        .style(style_with_bg(theme.background));

    let cursor = if active { "▌" } else { "" };
    let text = format!("/{}{cursor}", overlay.query);
    let para = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(theme.foreground));
    frame.render_widget(para, area);
}

fn render_categories(
    frame: &mut Frame,
    area: Rect,
    overlay: &mut ServerSearchOverlay,
    accent: ratatui::style::Color,
    theme: &Theme,
    is_active: bool,
) {
    let border_color = if is_active { accent } else { theme.border };
    let title_color = if is_active { accent } else { theme.dimmed };
    let list_border = if is_active {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(border_color)
    };

    let block = Block::default()
        .title(" Results ")
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_set(theme.border_set)
        .border_style(list_border)
        .style(style_with_bg(theme.background));

    let counts = [
        overlay.songs.len(),
        overlay.albums.len(),
        overlay.artists.len(),
    ];
    let items: Vec<ListItem> = FavoritesCategory::ALL
        .iter()
        .enumerate()
        .map(|(i, cat)| {
            let label = format!("{} ({})", cat.label(), counts[i]);
            ListItem::new(label).style(Style::default().fg(theme.foreground))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(accent)
                .fg(theme.background)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ")
        .style(style_with_bg(theme.background));

    let len = FavoritesCategory::ALL.len();
    overlay.categories_viewport_rows = area.height.saturating_sub(2).max(1) as usize;
    frame.render_stateful_widget(
        list,
        area,
        &mut super::list_scroll::list_state_for_selection(
            area,
            Some(overlay.selected_category_index),
            len,
            &mut overlay.categories_scroll,
        ),
    );
}

fn render_items(
    frame: &mut Frame,
    area: Rect,
    overlay: &mut ServerSearchOverlay,
    accent: ratatui::style::Color,
    theme: &Theme,
    is_active: bool,
) {
    let border_color = if is_active { accent } else { theme.border };
    let title_color = if is_active { accent } else { theme.dimmed };
    let list_border = if is_active {
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(border_color)
    };

    let title = format!(" {} ", overlay.category.label());
    let block = Block::default()
        .title(title)
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_set(theme.border_set)
        .border_style(list_border)
        .style(style_with_bg(theme.background));

    if overlay.loading {
        let list = List::new(vec![
            ListItem::new("Searching…").style(Style::default().fg(theme.dimmed))
        ])
        .block(block);
        frame.render_widget(list, area);
        return;
    }

    if let Some(ref e) = overlay.error {
        let list = List::new(vec![
            ListItem::new(format!("Error: {e}")).style(Style::default().fg(accent))
        ])
        .block(block);
        frame.render_widget(list, area);
        return;
    }

    let empty_hint = if overlay.last_query.is_none() {
        "Enter a query above"
    } else {
        "No results"
    };

    let (items, empty_msg) = match overlay.category {
        FavoritesCategory::Songs => {
            if overlay.songs.is_empty() {
                (vec![], empty_hint)
            } else {
                let rows: Vec<ListItem> = overlay
                    .songs
                    .iter()
                    .map(|s| {
                        let dur = s
                            .duration
                            .map(fmt_duration_ms)
                            .map(|d| format!("  {d}"))
                            .unwrap_or_default();
                        let artist = s.artist.as_deref().unwrap_or("");
                        let label = if artist.is_empty() {
                            format!("{}{}", s.title, dur)
                        } else {
                            format!("{} — {}{}", s.title, artist, dur)
                        };
                        ListItem::new(label).style(Style::default().fg(theme.foreground))
                    })
                    .collect();
                (rows, "")
            }
        }
        FavoritesCategory::Albums => {
            if overlay.albums.is_empty() {
                (vec![], empty_hint)
            } else {
                let rows: Vec<ListItem> = overlay
                    .albums
                    .iter()
                    .map(|a| {
                        let artist = a.artist.as_deref().unwrap_or("");
                        let label = if artist.is_empty() {
                            a.name.clone()
                        } else {
                            format!("{} — {}", a.name, artist)
                        };
                        ListItem::new(label).style(Style::default().fg(theme.foreground))
                    })
                    .collect();
                (rows, "")
            }
        }
        FavoritesCategory::Artists => {
            if overlay.artists.is_empty() {
                (vec![], empty_hint)
            } else {
                let rows: Vec<ListItem> = overlay
                    .artists
                    .iter()
                    .map(|a| {
                        ListItem::new(a.name.as_str()).style(Style::default().fg(theme.foreground))
                    })
                    .collect();
                (rows, "")
            }
        }
    };

    let items = if items.is_empty() {
        vec![ListItem::new(empty_msg).style(Style::default().fg(theme.dimmed))]
    } else {
        items
    };

    let len = items.len();
    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(accent)
                .fg(theme.background)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ")
        .style(style_with_bg(theme.background));

    let item_count = overlay.item_count();
    let sel = if item_count > 0 {
        Some(
            overlay
                .selected_item_index
                .min(item_count.saturating_sub(1)),
        )
    } else {
        None
    };
    overlay.items_viewport_rows = area.height.saturating_sub(2).max(1) as usize;
    frame.render_stateful_widget(
        list,
        area,
        &mut super::list_scroll::list_state_for_selection(
            area,
            sel,
            len,
            &mut overlay.items_scroll,
        ),
    );
}
