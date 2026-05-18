use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::App;

/// Braille spinner — advances every ~80 ms for a visible “still working” cue.
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
    } else if app.library_index_refreshing {
        let w = area.width as usize;
        let shown = fit_status_bar_text(&library_index_refresh_status_text(app), w);
        Line::from(vec![Span::styled(shown, Style::default().fg(app.accent()))])
    } else if app.library_server_append_fetching {
        let w = area.width as usize;
        let shown = fit_status_bar_text(&library_fetch_status_text(app), w);
        Line::from(vec![Span::styled(shown, Style::default().fg(app.accent()))])
    } else if let Some((msg, _)) = &app.status_flash {
        // Flash message: left-aligned, truncated to the bar width (centred long
        // strings overflow and corrupt the TUI layout).
        let w = area.width as usize;
        let shown = fit_status_bar_text(msg, w);
        Line::from(vec![Span::styled(shown, Style::default().fg(app.accent()))])
    } else {
        let host = app
            .config
            .subsonic_url
            .trim_start_matches("http://")
            .trim_start_matches("https://");

        let hint = "i — help";
        let sep = "  ·  ";
        let vol_label = format!("{}%", app.config.default_volume);
        let right_w = if app.config.show_volume_indicator {
            vol_label.len() + sep.len() + hint.len()
        } else {
            hint.len()
        };
        let host_w = 2 + host.len(); // "● " + host
        let gap = (area.width as usize).saturating_sub(host_w + right_w);

        let mut spans = vec![
            Span::styled("● ", Style::default().fg(app.accent())),
            Span::styled(host.to_string(), Style::default().fg(t.dimmed)),
            Span::raw(" ".repeat(gap)),
        ];
        if app.config.show_volume_indicator {
            spans.push(Span::styled(vol_label, Style::default().fg(app.accent())));
            spans.push(Span::styled(sep, Style::default().fg(t.dimmed)));
        }
        spans.push(Span::styled(hint, Style::default().fg(t.dimmed)));
        Line::from(spans)
    };

    let para = Paragraph::new(line).style(Style::default().bg(t.background));
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
