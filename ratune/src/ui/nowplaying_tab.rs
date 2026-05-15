use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use ratatui_image::picker::ProtocolType;
use ratatui_image::thread::ThreadProtocol;
use ratatui_image::StatefulImage;

use super::now_playing;
use super::queue;
use super::visualizer::render_visualizer_ex as render_visualizer;

use crate::app::App;

pub fn render(app: &mut App, frame: &mut Frame, area: Rect) {
    let boxed = app
        .config
        .now_playing_layout
        .trim()
        .eq_ignore_ascii_case("boxed");
    let show_art = app.config.nowplaying_show_art;
    let art_position = super::layout::placement_from_str(&app.config.nowplaying_art_position)
        .unwrap_or(super::layout::Placement::Left);
    let queue_position = super::layout::placement_from_str(&app.config.nowplaying_queue_position)
        .unwrap_or(super::layout::Placement::Right);
    let visualizer_position = super::layout::placement_from_str(&app.config.visualizer_location)
        .unwrap_or(super::layout::Placement::Right);
    let now_playing_position =
        super::layout::placement_from_str(&app.config.now_playing_box_location)
            .unwrap_or(super::layout::Placement::Right);
    let lyrics_position =
        super::layout::placement_from_str(&app.config.lyrics_location).unwrap_or(queue_position);

    let rects = super::layout::now_playing_rects(
        area,
        show_art,
        art_position,
        queue_position,
        app.config.nowplaying_left_width_percent,
        app.config.nowplaying_vertical_fill_top_percent,
        app.visualizer_visible,
        visualizer_position,
        app.lyrics_visible,
        lyrics_position,
        boxed,
        now_playing_position,
    );

    if let Some(r) = rects.art {
        render_art_placeholder(app, frame, r);
    }
    if let Some(r) = rects.queue {
        queue::render(app, frame, r, true);
    }
    if let Some(r) = rects.visualizer {
        render_visualizer_pane(app, frame, r);
    }
    if let Some(r) = rects.lyrics {
        render_lyrics_pane(app, frame, r);
    }
    if let Some(r) = rects.now_playing {
        now_playing::render_boxed_pane(app, frame, r);
    }
}

fn sync_np_ratatui_protocol(app: &mut App, inner: Rect) {
    if !app.ratatui_art_ready() || inner.width == 0 || inner.height == 0 {
        return;
    }
    if app.ratatui_uses_kitty_apc() {
        app.np_art_state = None;
        app.np_art_prep_key = None;
        return;
    }
    if let Some(p) = app.art_picker.as_mut() {
        p.set_background_color(crate::theme::color_to_rgba(app.theme.surface));
    }
    if app.art_cache.is_none() {
        app.np_art_state = None;
        app.np_art_prep_key = None;
        return;
    }
    let Some(fp) = app.art_cache_fingerprint else {
        app.np_art_state = None;
        app.np_art_prep_key = None;
        return;
    };
    let (_, bytes) = app.art_cache.as_ref().unwrap();
    let key = (fp, inner.width, inner.height);
    if app.np_art_prep_key.as_ref() == Some(&key) && app.np_art_state.is_some() {
        return;
    }
    let Some(picker) = app.art_picker.as_ref() else {
        return;
    };
    let Some(tx) = app.ratatui_resize_tx.clone() else {
        app.np_art_state = None;
        app.np_art_prep_key = None;
        return;
    };
    // Must match `Picker` / `ImageSource` font (same as Home strip ratatui prep).
    let fs = picker.font_size();
    let base_img = match app.art_cache_decoded.as_ref() {
        Some((cached_fp, img)) if *cached_fp == fp => img.clone(),
        _ => {
            let img = match image::load_from_memory(bytes) {
                Ok(i) => i,
                Err(_) => {
                    app.np_art_state = None;
                    app.np_art_prep_key = None;
                    app.art_cache_decoded = None;
                    return;
                }
            };
            app.art_cache_decoded = Some((fp, img.clone()));
            img
        }
    };
    let img = if matches!(picker.protocol_type(), ProtocolType::Sixel) {
        let pad = crate::theme::color_to_rgba(app.theme.surface);
        let fw = fs.0 as u32;
        let fh = fs.1 as u32;
        let need_w = inner.width as u32 * fw;
        let need_h = inner.height as u32 * fh;
        if (need_w as u128).saturating_mul(need_h as u128)
            <= crate::ui::art_prepare::MAX_SIXEL_PREP_PIXELS
        {
            crate::ui::art_prepare::prepare_art_image_for_exact_pixels_contain_centered(
                base_img, need_w, need_h, pad,
            )
        } else {
            crate::ui::art_prepare::prepare_art_image_for_rect_contain_centered(
                base_img, inner, fs, pad,
            )
        }
    } else {
        crate::ui::art_prepare::prepare_art_image_for_rect(base_img, inner, fs)
    };
    let proto = picker.new_resize_protocol(img);
    app.np_art_state = Some(ThreadProtocol::new(tx, Some(proto)));
    app.np_art_prep_key = Some(key);
}

fn render_art_placeholder(app: &mut App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;
    let block = crate::ui::kitty_art::album_art_block()
        .title_style(Style::default().fg(t.dimmed).add_modifier(Modifier::BOLD))
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.surface));
    frame.render_widget(block, area);

    if app.ratatui_art_ready()
        && !app.ratatui_uses_kitty_apc()
        && !app.help_visible
        && app.config.nowplaying_show_art
    {
        let inner = crate::ui::kitty_art::album_art_placeholder_inner(area);
        if inner.width > 0 && inner.height > 0 {
            if app
                .art_picker
                .as_ref()
                .is_some_and(|p| matches!(p.protocol_type(), ProtocolType::Sixel))
            {
                frame.render_widget(
                    Block::default().style(Style::default().bg(app.theme.surface)),
                    inner,
                );
            }
            sync_np_ratatui_protocol(app, inner);
            let img_resize = app.ratatui_stateful_resize();
            if let Some(ref mut state) = app.np_art_state {
                // Source is pre-fitted in `art_prepare`; `ratatui_stateful_resize` fills cells (see App).
                let w = StatefulImage::default().resize(img_resize);
                frame.render_stateful_widget(w, inner, state);
            }
        }
    }
}

// ── Visualizer pane ───────────────────────────────────────────────────────────

fn render_visualizer_pane(app: &App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;
    let accent = app.accent();

    let block = Block::default()
        .title(" Visualizer ")
        .title_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(t.surface));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    render_visualizer(
        frame,
        inner,
        &app.theme,
        &app.config.visualizer_type,
        &app.spectrum_bands,
        &app.waveform,
        &app.config.visualizer_color_mode,
        &app.config.visualizer_colors,
        accent,
        app.visualizer_gradient_rgb_cache.as_ref(),
    );
}

// ── Lyrics pane ───────────────────────────────────────────────────────────────

fn render_lyrics_pane(app: &App, frame: &mut Frame, area: Rect) {
    let t = &app.theme;
    let accent = app.accent();

    let block = Block::default()
        .title(" Lyrics ")
        .title_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(t.surface));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let inner_h = inner.height as usize;
    let inner_w = inner.width as usize;

    let current_song_id = app.playback.current_song.as_ref().map(|s| s.id.as_str());

    let cache_match = current_song_id.and_then(|sid| {
        app.lyrics_cache.as_ref().and_then(|(cached_id, lines)| {
            if cached_id.as_str() == sid {
                Some(lines.as_slice())
            } else {
                None
            }
        })
    });

    match cache_match {
        None => {
            render_centered_msg(frame, inner, "Loading…", t.dimmed);
        }
        Some(lines) => {
            if lines.is_empty() {
                render_centered_msg(frame, inner, "No lyrics available", t.dimmed);
            } else {
                let is_synced = lines.iter().any(|l| l.time.is_some());
                if is_synced {
                    render_synced(app, frame, inner, lines, inner_h, inner_w, accent);
                } else {
                    render_unsynced(app, frame, inner, lines, inner_h, inner_w);
                }
            }
        }
    }
}

fn render_centered_msg(
    frame: &mut Frame,
    area: Rect,
    msg: &'static str,
    color: ratatui::style::Color,
) {
    let para = Paragraph::new(msg)
        .style(Style::default().fg(color))
        .alignment(Alignment::Center);
    frame.render_widget(para, area);
}

fn render_synced(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    lines: &[ratune_subsonic::LyricLine],
    inner_h: usize,
    _inner_w: usize,
    accent: ratatui::style::Color,
) {
    let t = &app.theme;
    let elapsed = app.playback.elapsed;

    let current_idx: Option<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.time.map(|ts| ts <= elapsed).unwrap_or(false))
        .map(|(i, _)| i)
        .next_back();

    let scroll: usize = current_idx
        .map(|ci| ci.saturating_sub(inner_h / 2))
        .unwrap_or(0);

    let display: Vec<Line> = lines
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_h)
        .map(|(i, l)| {
            let style = match current_idx {
                Some(ci) if i == ci => Style::default().fg(accent).add_modifier(Modifier::BOLD),
                Some(ci) if i < ci => Style::default().fg(t.dimmed),
                _ => Style::default().fg(t.foreground),
            };
            Line::from(Span::styled(l.text.as_str(), style))
        })
        .collect();

    let para = Paragraph::new(display)
        .style(Style::default().bg(t.surface))
        .alignment(Alignment::Center);
    frame.render_widget(para, area);
}

fn render_unsynced(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    lines: &[ratune_subsonic::LyricLine],
    inner_h: usize,
    inner_w: usize,
) {
    let t = &app.theme;

    let wrapped: Vec<String> = lines
        .iter()
        .flat_map(|l| wrap_text(&l.text, inner_w))
        .collect();

    let scroll = app.lyrics_scroll.min(wrapped.len().saturating_sub(1));

    let display: Vec<Line> = wrapped
        .iter()
        .skip(scroll)
        .take(inner_h)
        .map(|row| {
            Line::from(Span::styled(
                row.as_str(),
                Style::default().fg(t.foreground),
            ))
        })
        .collect();

    let para = Paragraph::new(display).style(Style::default().bg(t.surface));
    frame.render_widget(para, area);
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    if chars.len() <= width {
        return vec![chars.iter().collect()];
    }

    let mut lines = Vec::new();
    let mut start = 0;

    while start < chars.len() {
        let end = (start + width).min(chars.len());
        let break_at = if end < chars.len() {
            chars[start..end]
                .iter()
                .rposition(|&c| c == ' ')
                .map(|i| start + i)
                .unwrap_or(end)
        } else {
            end
        };
        lines.push(chars[start..break_at].iter().collect());
        start = break_at;
        while start < chars.len() && chars[start] == ' ' {
            start += 1;
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
