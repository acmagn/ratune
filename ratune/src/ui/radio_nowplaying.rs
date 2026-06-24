//! Radio station list pane on the Now Playing tab (while live radio is active).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::state::{LibraryState, LoadingState};
use crate::theme::style_with_bg;

pub fn render(app: &mut App, frame: &mut Frame, area: Rect, is_active: bool) {
    let t = &app.theme;
    let accent = app.accent();
    let border_color = if is_active { t.border_active } else { t.border };
    let title_color = if is_active { accent } else { t.dimmed };

    let queue_n = app.queue.songs.len();
    let title = if queue_n == 0 {
        " Radio ".to_string()
    } else {
        format!(" Radio · {queue_n} queued ")
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
        .border_style(Style::default().fg(border_color))
        .style(style_with_bg(t.surface));

    let playing_id = app
        .playback
        .current_song
        .as_ref()
        .filter(|s| App::is_radio_song(s))
        .and_then(|s| s.id.strip_prefix("radio:"));

    match &app.radio.stations {
        LoadingState::NotLoaded | LoadingState::Loading => {
            let list = List::new(vec![
                ListItem::new("Loading stations…").style(Style::default().fg(t.dimmed))
            ])
            .block(block);
            frame.render_widget(list, area);
        }
        LoadingState::Error(e) => {
            let list = List::new(vec![
                ListItem::new(format!("Error: {e}")).style(Style::default().fg(accent))
            ])
            .block(block);
            frame.render_widget(list, area);
        }
        LoadingState::Loaded(stations) => {
            if stations.is_empty() {
                let list =
                    List::new(vec![ListItem::new("No stations configured")
                        .style(Style::default().fg(t.dimmed))])
                    .block(block);
                frame.render_widget(list, area);
                return;
            }

            let visible_rows = area.height.saturating_sub(2) as usize;
            const HINT_ROWS: usize = 1;
            let list_rows = visible_rows.saturating_sub(HINT_ROWS).max(1);
            app.queue_viewport_rows = list_rows;
            LibraryState::clamp_vertical_scroll(
                &mut app.radio.scroll,
                app.radio.selected,
                stations.len(),
                list_rows,
            );

            let items: Vec<ListItem> = stations
                .iter()
                .enumerate()
                .skip(app.radio.scroll)
                .take(list_rows)
                .map(|(i, station)| {
                    let global_idx = app.radio.scroll + i;
                    let buffering =
                        playing_id == Some(station.id.as_str()) && app.radio_buffering();
                    let playing = playing_id == Some(station.id.as_str())
                        && !app.playback.paused
                        && app.playback.player_loaded;
                    let marker = if playing {
                        "▶ "
                    } else if buffering {
                        "◌ "
                    } else {
                        "  "
                    };
                    let label = format!("{marker}{}", station.name);
                    let style = if global_idx == app.radio.selected {
                        Style::default().fg(accent).add_modifier(Modifier::BOLD)
                    } else if playing {
                        Style::default().fg(t.foreground)
                    } else {
                        Style::default().fg(t.dimmed)
                    };
                    ListItem::new(label).style(style)
                })
                .collect();

            let list = List::new(items)
                .block(block)
                .highlight_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
                .style(style_with_bg(t.surface));

            let mut state = ListState::default();
            let selected = app
                .radio
                .selected
                .saturating_sub(app.radio.scroll)
                .min(list_rows.saturating_sub(1));
            state.select(Some(selected));
            frame.render_stateful_widget(list, area, &mut state);

            if area.height > 4 {
                render_radio_hints(app, frame, area, queue_n);
            }
        }
    }
}

fn render_radio_hints(app: &App, frame: &mut Frame, area: Rect, queue_n: usize) {
    let t = &app.theme;
    let hint = if queue_n > 0 {
        "Ctrl+g library queue · Enter play queue"
    } else {
        "Ctrl+g library queue"
    };
    let hint_area = Rect {
        x: area.x + 1,
        y: area.y + area.height.saturating_sub(1),
        width: area.width.saturating_sub(2),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            hint,
            Style::default().fg(t.dimmed),
        )])),
        hint_area,
    );
}
