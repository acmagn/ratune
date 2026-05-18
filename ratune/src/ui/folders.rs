use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, BrowserColumn};
use crate::config::BrowseMode;
use crate::state::{FolderBrowseState, LoadingState};

pub fn render(app: &mut App, frame: &mut Frame, area: Rect, is_active: bool) {
    let t = &app.theme;
    let border_color = if is_active { app.accent() } else { t.border };
    let title_color = if is_active { app.accent() } else { t.dimmed };

    let title = if app.browser_browse_mode == BrowseMode::Files {
        if app.folders.path.is_empty() {
            " Libraries ".to_string()
        } else {
            app.folders
                .path
                .last()
                .map(|(_, n)| format!(" {n} "))
                .unwrap_or_else(|| " Folders ".to_string())
        }
    } else {
        " Folders ".to_string()
    };

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
        .border_type(BorderType::Plain)
        .border_style(border_style)
        .style(Style::default().bg(t.surface));

    if app.browser_browse_mode != BrowseMode::Files {
        let list =
            List::new(vec![ListItem::new("—").style(Style::default().fg(t.dimmed))]).block(block);
        frame.render_widget(list, area);
        return;
    }

    let labels: Vec<String> = if app.folders.path.is_empty() {
        match &app.folders.roots {
            LoadingState::NotLoaded | LoadingState::Loading => {
                vec!["Loading…".into()]
            }
            LoadingState::Error(e) => {
                vec![format!("Error: {e}")]
            }
            LoadingState::Loaded(roots) => {
                if roots.is_empty() {
                    vec!["No music folders".into()]
                } else if let Some(q) = app.browser_column_filter(BrowserColumn::Artists) {
                    roots
                        .iter()
                        .filter(|r| r.name.to_lowercase().contains(q))
                        .map(|r| r.name.clone())
                        .collect()
                } else {
                    roots.iter().map(|r| r.name.clone()).collect()
                }
            }
        }
    } else {
        match app.folders.current_listing() {
            None | Some(LoadingState::NotLoaded) | Some(LoadingState::Loading) => {
                vec!["Loading…".into()]
            }
            Some(LoadingState::Error(e)) => {
                vec![format!("Error: {e}")]
            }
            Some(LoadingState::Loaded(listing)) => {
                let mut out = vec!["..".to_string()];
                let dirs: Vec<String> =
                    if let Some(q) = app.browser_column_filter(BrowserColumn::Artists) {
                        listing
                            .directories
                            .iter()
                            .filter(|(_, name)| name.to_lowercase().contains(q))
                            .map(|(_, name)| name.clone())
                            .collect()
                    } else {
                        listing
                            .directories
                            .iter()
                            .map(|(_, name)| name.clone())
                            .collect()
                    };
                out.extend(dirs);
                out
            }
        }
    };

    let items: Vec<ListItem> = labels
        .iter()
        .map(|label| {
            let style = if label == ".." {
                Style::default().fg(t.dimmed)
            } else {
                Style::default().fg(t.foreground)
            };
            ListItem::new(label.as_str()).style(style)
        })
        .collect();

    let sel = app.folders.selected_dir.filter(|&s| s < labels.len());

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
    if !labels.is_empty() {
        if let Some(sel_ix) = sel {
            FolderBrowseState::clamp_scroll(&mut app.folders.dirs_scroll, sel_ix, labels.len(), vh);
            state = ListState::default().with_offset(app.folders.dirs_scroll);
            state.select(Some(sel_ix));
        } else {
            let max_first = labels.len().saturating_sub(vh);
            app.folders.dirs_scroll = app.folders.dirs_scroll.min(max_first);
            state = ListState::default().with_offset(app.folders.dirs_scroll);
        }
    }
    frame.render_stateful_widget(list, area, &mut state);
}
