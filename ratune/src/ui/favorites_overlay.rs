//! Favorites (starred) browser overlay. Songs, albums, and artists from `getStarred2`.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::keybinds::{format_spec, Keybinds};
use crate::state::{FavoritesCategory, FavoritesFocus, FavoritesOverlay};
use crate::text_width::str_width;
use crate::theme::{style_with_bg, Theme};

pub struct PanelLayout {
    pub area: Rect,
    pub categories_col: Rect,
    pub items_col: Rect,
}

pub fn panel_layout(parent: Rect) -> PanelLayout {
    let area =
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).split(parent)[1];
    let cols =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).split(area);
    PanelLayout {
        area,
        categories_col: cols[0],
        items_col: cols[1],
    }
}

fn overlay_title(overlay: &FavoritesOverlay) -> String {
    if overlay.offline_snapshot {
        let age = overlay
            .snapshot_refreshed_at
            .map(fmt_snapshot_age)
            .unwrap_or_else(|| "offline".to_string());
        format!(" Favorites ({age}) ")
    } else {
        " Favorites ".to_string()
    }
}

fn fmt_snapshot_age(refreshed_at_unix: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(refreshed_at_unix);
    let age = now.saturating_sub(refreshed_at_unix);
    if age < 60 {
        "offline · just now".to_string()
    } else if age < 3600 {
        format!("offline · {}m ago", age / 60)
    } else if age < 86_400 {
        format!("offline · {}h ago", age / 3600)
    } else {
        format!("offline · {}d ago", age / 86_400)
    }
}

fn fmt_duration_ms(secs: u32) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}

pub fn render_favorites_overlay(
    frame: &mut Frame,
    area: Rect,
    overlay: &mut FavoritesOverlay,
    accent: ratatui::style::Color,
    theme: &Theme,
    kb: &Keybinds,
) {
    if !overlay.visible {
        return;
    }

    let layout = panel_layout(area);
    frame.render_widget(Clear, layout.area);

    let left_active = matches!(overlay.focus, FavoritesFocus::Categories);
    let right_active = !left_active;

    render_categories(
        frame,
        layout.categories_col,
        overlay,
        accent,
        theme,
        left_active,
    );
    render_items(
        frame,
        layout.items_col,
        overlay,
        accent,
        theme,
        right_active,
    );
    render_hints(frame, layout.items_col, theme, kb);
}

/// Key hints drawn over the items column's bottom border; trailing hints are dropped
/// when the column is too narrow.
fn render_hints(frame: &mut Frame, area: Rect, theme: &Theme, kb: &Keybinds) {
    if area.height <= 4 {
        return;
    }
    let mut hints = vec![
        format!("{} add & close", format_spec(&kb.favorites_add_and_close)),
        format!("{} add", format_spec(&kb.favorites_add)),
        format!("{} add all", format_spec(&kb.favorites_add_all)),
    ];
    if let Some(spec) = kb.add_all_replace_album.as_ref() {
        hints.push(format!("{} play", format_spec(spec)));
    }
    hints.push("Esc close".to_string());

    let max = area.width.saturating_sub(6) as usize;
    let mut text = String::new();
    for hint in hints {
        let candidate = if text.is_empty() {
            hint
        } else {
            format!("{text} · {hint}")
        };
        if str_width(&candidate) > max {
            break;
        }
        text = candidate;
    }
    if text.is_empty() {
        return;
    }
    let hint_area = Rect {
        x: area.x + 2,
        y: area.y + area.height - 1,
        width: area.width.saturating_sub(4),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {text} "),
            Style::default().fg(theme.dimmed),
        ))),
        hint_area,
    );
}

fn render_categories(
    frame: &mut Frame,
    area: Rect,
    overlay: &mut FavoritesOverlay,
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
        .title(overlay_title(overlay))
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
            let label = format!(
                "{}{} ({})",
                theme.icons.favorite_prefix(),
                cat.label(),
                counts[i]
            );
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
    overlay: &mut FavoritesOverlay,
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
            ListItem::new("Loading…").style(Style::default().fg(theme.dimmed))
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

    let (items, empty_msg) = match overlay.category {
        FavoritesCategory::Songs => {
            if overlay.songs.is_empty() {
                (vec![], "No starred songs")
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
                (vec![], "No starred albums")
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
                (vec![], "No starred artists")
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
