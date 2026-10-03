use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::App;
use crate::theme::style_with_bg;

/// Braille spinner. Advances every ~80 ms for a visible “still working” cue.
const LIB_SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn library_index_refresh_status_text(app: &App) -> String {
    let (idx, secs) = match app.library_index_refresh_started {
        Some(start) => {
            let idx = (Instant::now().duration_since(start).as_millis() / 80) as usize
                % LIB_SPINNER.len();
            let secs = start.elapsed().as_secs();
            (idx, secs)
        }
        None => (0, 0),
    };
    let sp = LIB_SPINNER[idx];
    format!("Refreshing library index {sp}  ·  {secs}s")
}

fn library_fetch_status_text(app: &App) -> String {
    let (idx, secs) = match app.library_server_append_started {
        Some(start) => {
            let idx = (Instant::now().duration_since(start).as_millis() / 80) as usize
                % LIB_SPINNER.len();
            let secs = start.elapsed().as_secs();
            (idx, secs)
        }
        None => (0, 0),
    };
    let sp = LIB_SPINNER[idx];
    format!("Fetching full library {sp}  ·  {secs}s")
}

fn scrobble_service_name(app: &App) -> &'static str {
    match app.config.scrobble_service {
        ratune_scrobble::ScrobbleService::LastFm => "Last.fm",
        ratune_scrobble::ScrobbleService::LibreFm => "Libre.fm",
    }
}

fn scrobble_status_width(app: &App) -> usize {
    if !app.config.scrobble_enabled {
        return 0;
    }
    let mut w = scrobble_service_name(app).len();
    if app.scrobble_recently_ok() {
        w += " ✓".len();
    }
    if !app.scrobble_queue.is_empty() {
        w += format!(" ({})", app.scrobble_queue.len()).len();
    }
    w
}

fn push_scrobble_status_spans(
    app: &App,
    spans: &mut Vec<Span>,
    accent: ratatui::style::Color,
    dimmed: ratatui::style::Color,
) {
    if !app.config.scrobble_enabled {
        return;
    }
    spans.push(Span::styled(
        scrobble_service_name(app).to_string(),
        Style::default().fg(dimmed),
    ));
    if app.scrobble_recently_ok() {
        spans.push(Span::styled(" ✓", Style::default().fg(accent)));
    }
    if !app.scrobble_queue.is_empty() {
        spans.push(Span::styled(
            format!(" ({})", app.scrobble_queue.len()),
            Style::default().fg(dimmed),
        ));
    }
}

/// Right-side status chrome: shuffle mode, volume, help hint.
struct RightChrome {
    shuffle_mode_label: Option<String>,
    vol_label: String,
    show_volume: bool,
    width: usize,
}

const STATUS_SEP: &str = "  ·  ";
const HELP_HINT: &str = "i — help";

fn right_chrome(app: &App) -> RightChrome {
    let t = &app.theme;
    let vol_label = format!("{}%", app.config.default_volume);
    let shuffle_mode_label = if app.shuffle_mode {
        Some(format!("{} add", t.icons.mode_shuffle))
    } else {
        None
    };

    let mut width = HELP_HINT.len();
    if app.config.show_volume_indicator {
        width += STATUS_SEP.len() + vol_label.len();
    }
    if let Some(ref label) = shuffle_mode_label {
        width += STATUS_SEP.len() + label.chars().count();
    }

    RightChrome {
        shuffle_mode_label,
        vol_label,
        show_volume: app.config.show_volume_indicator,
        width,
    }
}

fn push_right_chrome(
    spans: &mut Vec<Span>,
    chrome: &RightChrome,
    accent: ratatui::style::Color,
    dimmed: ratatui::style::Color,
) {
    if let Some(ref label) = chrome.shuffle_mode_label {
        spans.push(Span::styled(label.clone(), Style::default().fg(accent)));
        spans.push(Span::styled(STATUS_SEP, Style::default().fg(dimmed)));
    }
    if chrome.show_volume {
        spans.push(Span::styled(
            chrome.vol_label.clone(),
            Style::default().fg(accent),
        ));
        spans.push(Span::styled(STATUS_SEP, Style::default().fg(dimmed)));
    }
    spans.push(Span::styled(HELP_HINT, Style::default().fg(dimmed)));
}

/// Left-aligned message plus persistent right chrome (shuffle / volume / help).
fn line_left_message_with_chrome(app: &App, message: &str, area_width: usize) -> Line<'static> {
    let t = &app.theme;
    let chrome = right_chrome(app);
    let max_left = area_width.saturating_sub(chrome.width);
    let shown = fit_status_bar_text(message, max_left);
    let left_w = shown.chars().count();
    let gap = area_width.saturating_sub(left_w + chrome.width);

    let mut spans = vec![Span::styled(shown, Style::default().fg(app.accent()))];
    spans.push(Span::raw(" ".repeat(gap)));
    push_right_chrome(&mut spans, &chrome, app.accent(), t.dimmed);
    Line::from(spans)
}

// ── Public render ─────────────────────────────────────────────────────────────

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;

    let line = if app.search_mode.active {
        Line::from(vec![
            Span::styled("Search: ", Style::default().fg(app.accent())),
            Span::styled(
                app.search_mode.query.as_str(),
                Style::default().fg(t.foreground),
            ),
            Span::styled("_", Style::default().fg(app.accent())),
            Span::raw("   "),
            Span::styled("Enter", Style::default().fg(t.dimmed)),
            Span::raw(" apply  "),
            Span::styled("Esc", Style::default().fg(t.dimmed)),
            Span::raw(" / "),
            Span::styled("Ctrl+C", Style::default().fg(t.dimmed)),
            Span::raw(" cancel"),
        ])
    } else if app.search_filter.is_some() {
        let q = app.search_filter.as_deref().unwrap_or("");
        Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(app.accent())),
            Span::styled(q, Style::default().fg(t.foreground)),
            Span::raw("   "),
            Span::styled("Esc", Style::default().fg(t.dimmed)),
            Span::raw(" / "),
            Span::styled("Ctrl+C", Style::default().fg(t.dimmed)),
            Span::raw(" clear"),
        ])
    } else if let Some((msg, _)) = &app.status_flash {
        // Flashes win over long-running refresh/fetch text so notices (e.g. bad cache
        // recovery) are visible even while the library index is refreshing.
        line_left_message_with_chrome(app, msg, area.width as usize)
    } else if app.library_index_refreshing {
        line_left_message_with_chrome(
            app,
            &library_index_refresh_status_text(app),
            area.width as usize,
        )
    } else if app.library_server_append_fetching {
        line_left_message_with_chrome(app, &library_fetch_status_text(app), area.width as usize)
    } else {
        let server = app.server_label();
        let host_label = if app.server_reachable {
            server
        } else {
            format!("{server} (offline)")
        };

        let chrome = right_chrome(app);
        let conn_icon = if app.server_reachable {
            t.icons.online.as_str()
        } else {
            t.icons.offline.as_str()
        };
        let conn_label = format!("{conn_icon} ");
        let scrobble_w = scrobble_status_width(app);
        let mut left_w = conn_label.chars().count() + host_label.chars().count();
        if scrobble_w > 0 {
            left_w += STATUS_SEP.len() + scrobble_w;
        }
        let gap = (area.width as usize).saturating_sub(left_w + chrome.width);

        let conn_style = if app.server_reachable {
            Style::default().fg(app.accent())
        } else {
            Style::default().fg(t.dimmed)
        };
        let mut spans = vec![
            Span::styled(conn_label, conn_style),
            Span::styled(host_label, Style::default().fg(t.dimmed)),
        ];
        if scrobble_w > 0 {
            spans.push(Span::styled(STATUS_SEP, Style::default().fg(t.dimmed)));
            push_scrobble_status_spans(app, &mut spans, app.accent(), t.dimmed);
        }
        spans.push(Span::raw(" ".repeat(gap)));
        push_right_chrome(&mut spans, &chrome, app.accent(), t.dimmed);
        Line::from(spans)
    };

    let para = Paragraph::new(line).style(style_with_bg(t.status_bar));
    frame.render_widget(para, area);
}

/// Truncate `s` to at most `max_cols` Unicode scalars (status bar is one row).
fn fit_status_bar_text(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let n = s.chars().count();
    if n <= max_cols {
        return s.to_string();
    }
    if max_cols <= 1 {
        return "…".to_string();
    }
    s.chars().take(max_cols - 1).collect::<String>() + "…"
}
