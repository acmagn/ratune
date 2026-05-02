//! Minimal harness to exercise [ratatui-image] in the same terminal setup as ratune.
//!
//! ```text
//! cargo run -p ratune --example art_image_test -- /path/to/cover.jpg
//! ```
//!
//! Startup order matches [`main.rs`]: alternate screen and [`ratatui::Terminal`] are set up
//! before [`Picker::from_query_stdio`]. Rendering uses [`ThreadProtocol`] and a resize worker,
//! like the Now Playing tab, so the first frames are not blank on Kitty/Sixel-heavy setups.
//!
//! Press `q` or Esc to exit.

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use image::{DynamicImage, Rgba};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use ratatui_image::picker::Picker;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};
use ratatui_image::{Resize, StatefulImage};
use std::io::{stdout, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use image::imageops::FilterType;

/// Default panel surface (#161616) — same order of magnitude as ratune's default theme.
const SURFACE: Rgba<u8> = Rgba([22, 22, 22, 255]);

fn fit_image_for_test_harness(img: DynamicImage, max_w: u32, max_h: u32) -> DynamicImage {
    let (iw, ih) = (img.width(), img.height());
    if iw == 0 || ih == 0 {
        return img;
    }
    let wratio = max_w as f64 / iw as f64;
    let hratio = max_h as f64 / ih as f64;
    let ratio = f64::min(wratio, hratio);
    let tw = ((iw as f64 * ratio).round() as u32).max(1);
    let th = ((ih as f64 * ratio).round() as u32).max(1);
    if (tw, th) == (iw, ih) {
        return img;
    }
    img.resize_exact(tw, th, FilterType::Triangle)
}

fn drain_resize(
    rx: &mpsc::Receiver<Result<ResizeResponse, ratatui_image::errors::Errors>>,
    state: &mut ThreadProtocol,
) {
    while let Ok(done) = rx.try_recv() {
        match done {
            Ok(res) => {
                let _ = state.update_resized_protocol(res);
            }
            Err(e) => eprintln!("art_image_test: encode: {e}"),
        }
    }
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: cargo run -p ratune --example art_image_test -- <image path>")?;

    let dyn_img = image::open(&path).with_context(|| format!("open image: {path}"))?;
    let dyn_img = fit_image_for_test_harness(dyn_img, 1024, 1024);

    let in_tmux = std::env::var("TMUX").is_ok();

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;

    if in_tmux {
        stdout.write_all(b"\x1bPtmux;\x1b\x1b[?1004h\x1b\\")?;
        stdout.flush()?;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut picker =
        Picker::from_query_stdio().context("terminal capability query (ratatui-image)")?;
    picker.set_background_color(SURFACE);

    let protocol_str = format!("{:?}", picker.protocol_type());
    let font_str = format!("{:?}", picker.font_size());
    let caps = picker
        .capabilities()
        .iter()
        .map(|c| format!("{c:?}"))
        .collect::<Vec<_>>()
        .join(", ");

    let (tx_job, rx_job) = mpsc::channel::<ResizeRequest>();
    let (tx_done, rx_done) =
        mpsc::channel::<Result<ResizeResponse, ratatui_image::errors::Errors>>();
    thread::spawn(move || {
        while let Ok(req) = rx_job.recv() {
            let _ = tx_done.send(req.resize_encode());
        }
    });

    let initial = picker.new_resize_protocol(dyn_img);
    let mut image_state = ThreadProtocol::new(tx_job, Some(initial));

    loop {
        drain_resize(&rx_done, &mut image_state);

        terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

            let block = Block::default()
                .title(" Album Art ")
                .borders(Borders::ALL)
                .title_style(Style::default().add_modifier(Modifier::BOLD));

            let inner = block.inner(chunks[0]);
            f.render_widget(block, chunks[0]);

            let img_widget = StatefulImage::default().resize(Resize::Fit(None));
            f.render_stateful_widget(img_widget, inner, &mut image_state);

            let status = format!(
                "protocol={protocol_str}  font_px={font_str}  caps=[{caps}]  |  q or Esc quit"
            );
            f.render_widget(Paragraph::new(Span::raw(status)), chunks[1]);
        })?;

        drain_resize(&rx_done, &mut image_state);

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press
                    && matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
                {
                    break;
                }
            }
        }
    }

    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    if in_tmux {
        terminal
            .backend_mut()
            .write_all(b"\x1bPtmux;\x1b\x1b[?1004l\x1b\\")?;
        terminal.backend_mut().flush()?;
    }
    disable_raw_mode()?;
    Ok(())
}
