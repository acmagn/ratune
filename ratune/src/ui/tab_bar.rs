use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::Tab;

// ── Tab indicator bar ─────────────────────────────────────────────────────────

/// Render a single-line tab indicator bar.
///
/// Active tab: `accent` background, `Color::Black` foreground, bold.
/// Inactive tabs: `Color::DarkGray` foreground, no background.
/// Separator: ` │ ` in `Color::DarkGray`.
pub fn render_tab_bar(f: &mut Frame, area: Rect, active_tab: Tab, accent: Color) {
    let separator = Span::styled(" │ ", Style::default().fg(Color::DarkGray));

    let label_home = " Home ";
    let label_browser = " Browse ";
    let label_nowplaying = " Now Playing ";

    let span_home = if active_tab == Tab::Home {
        Span::styled(
            label_home,
            Style::default()
                .bg(accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(label_home, Style::default().fg(Color::DarkGray))
    };

    let span_browser = if active_tab == Tab::Browser {
        Span::styled(
            label_browser,
            Style::default()
                .bg(accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(label_browser, Style::default().fg(Color::DarkGray))
    };

    let span_nowplaying = if active_tab == Tab::NowPlaying {
        Span::styled(
            label_nowplaying,
            Style::default()
                .bg(accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(label_nowplaying, Style::default().fg(Color::DarkGray))
    };

    let line = Line::from(vec![
        span_home,
        separator.clone(),
        span_browser,
        separator,
        span_nowplaying,
    ]);

    let para = Paragraph::new(line);
    f.render_widget(para, area);
}
