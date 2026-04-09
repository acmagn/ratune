use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};

use crate::app::App;

const DEFAULT_QUEUE_TEMPLATE: &str = "{n}  {title:<40}  {artist:<25}  {duration:>5}";

pub fn render(app: &App, frame: &mut Frame, area: Rect, is_active: bool) {
    let t = &app.theme;
    let border_color = if is_active { t.border_active } else { t.border };
    let title_color  = if is_active { app.accent() }    else { t.dimmed };

    let count = app.queue.songs.len();
    let title = if count == 0 {
        " Queue ".to_string()
    } else {
        format!(" Queue ({count}) ")
    };

    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(title_color).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(t.surface));

    if app.queue.songs.is_empty() {
        let mut msg = "Queue is empty — press 'a' to add tracks".to_string();
        if app.config.show_fzf_hint && app.config.library_index_enabled && app.keybinds.library_fzf.is_some() {
            msg.push_str(" · Ctrl+f: library picker");
        }
        let item = ListItem::new(msg).style(Style::default().fg(t.dimmed));
        let list = List::new(vec![item]).block(block);
        frame.render_widget(list, area);
        return;
    }

    let template = if app.config.queue_template.trim().is_empty() {
        DEFAULT_QUEUE_TEMPLATE
    } else {
        app.config.queue_template.as_str()
    };

    let items: Vec<ListItem> = app.queue.songs.iter().enumerate().map(|(i, s)| {
        let label = format_queue_line(template, s);

        let style = if i == app.queue.cursor {
            Style::default().fg(app.accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.foreground)
        };
        ListItem::new(label).style(style)
    }).collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().fg(app.accent()).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(t.surface));

    let mut state = ListState::default().with_offset(app.queue.scroll);
    state.select(Some(app.queue.cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

fn format_queue_line(template: &str, s: &playterm_subsonic::Song) -> String {
    let n = s.track
        .map(|n| format!("{n:>3}."))
        .unwrap_or_else(|| "    ".to_string());
    let title = s.title.as_str();
    let artist = s.artist.as_deref().unwrap_or("");
    let album = s.album.as_deref().unwrap_or("");
    let duration = s.duration
        .map(|d| format!("{}:{:02}", d / 60, d % 60))
        .unwrap_or_else(|| "".to_string());

    render_template(template, |name| match name {
        "n" => Some(n.clone()),
        "title" => Some(title.to_string()),
        "artist" => Some(artist.to_string()),
        "album" => Some(album.to_string()),
        "duration" => Some(duration.clone()),
        _ => None,
    })
}

fn render_template<F>(template: &str, mut resolve: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    let mut out = String::with_capacity(template.len().saturating_add(16));
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '}').map(|p| i + 1 + p) {
                let inner: String = chars[i + 1..end].iter().collect();
                if let Some((name, spec)) = inner.split_once(':') {
                    out.push_str(&format_field(&mut resolve, name.trim(), Some(spec.trim())));
                } else {
                    out.push_str(&format_field(&mut resolve, inner.trim(), None));
                }
                i = end + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn format_field<F>(resolve: &mut F, name: &str, spec: Option<&str>) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    let raw = match resolve(name) {
        Some(v) => v,
        None => return format!("{{{name}}}"),
    };

    let (align, width) = parse_spec(spec);
    if let Some(w) = width {
        let truncated = trunc(&raw, w);
        match align {
            Align::Right => format!("{:>width$}", truncated, width = w),
            Align::Left => format!("{:<width$}", truncated, width = w),
        }
    } else {
        raw
    }
}

#[derive(Copy, Clone)]
enum Align {
    Left,
    Right,
}

fn parse_spec(spec: Option<&str>) -> (Align, Option<usize>) {
    let Some(spec) = spec else { return (Align::Left, None); };
    if spec.is_empty() {
        return (Align::Left, None);
    }
    let mut chars = spec.chars();
    let first = chars.next().unwrap_or('<');
    let (align, rest) = match first {
        '>' => (Align::Right, chars.collect::<String>()),
        '<' => (Align::Left, chars.collect::<String>()),
        _ => (Align::Left, spec.to_string()),
    };
    let width = rest.trim().parse::<usize>().ok().filter(|w| *w > 0);
    (align, width)
}

/// Truncate `s` to at most `max` Unicode characters, appending `…` if cut.
fn trunc(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut chars = s.chars();
    let mut result = String::with_capacity(max);
    let mut count = 0;
    for ch in chars.by_ref() {
        if count >= max - 1 {
            // Check if there are more characters coming.
            if chars.next().is_some() {
                result.push('…');
            } else {
                result.push(ch);
            }
            return result;
        }
        result.push(ch);
        count += 1;
    }
    result
}
