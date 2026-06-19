//! Favorites (starred) browser overlay — songs, albums, and artists from `getStarred2`.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState};
use ratatui::Frame;

use crate::state::{FavoritesCategory, FavoritesFocus, FavoritesOverlay};
use crate::theme::{style_with_bg, Theme};

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
    overlay: &FavoritesOverlay,
    accent: ratatui::style::Color,
    theme: &Theme,
) {
    if !overlay.visible {
        return;
    }

    let split =
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).split(area);
    let overlay_area = split[1];
    frame.render_widget(Clear, overlay_area);

    let cols = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(overlay_area);

    let left_active = matches!(overlay.focus, FavoritesFocus::Categories);
    let right_active = !left_active;

    render_categories(frame, cols[0], overlay, accent, theme, left_active);
    render_items(frame, cols[1], overlay, accent, theme, right_active);
}

fn render_categories(
    frame: &mut Frame,
    area: Rect,
    overlay: &FavoritesOverlay,
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
        .border_type(BorderType::Plain)
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
            let label = format!("★ {} ({})", cat.label(), counts[i]);
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

    let mut state = ListState::default();
    state.select(Some(overlay.selected_category_index));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_items(
    frame: &mut Frame,
    area: Rect,
    overlay: &FavoritesOverlay,
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
        .border_type(BorderType::Plain)
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

    let mut state = ListState::default();
    if overlay.item_count() > 0 {
        state.select(Some(
            overlay
                .selected_item_index
                .min(overlay.item_count().saturating_sub(1)),
        ));
    }
    frame.render_stateful_widget(list, area, &mut state);
}
