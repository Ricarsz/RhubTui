//! 详情面板。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};

use crate::app::App;
use crate::theme::Palette;
use crate::view::Details;

use super::{panel, tone_color};

pub(super) fn draw(frame: &mut Frame, app: &mut App, area: Rect, palette: Palette) {
    let title = " 详情 ";
    let block = panel(title, palette);
    let inner = block.inner(area);
    let lines = details_lines(&app.details(), palette);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = paragraph.line_count(inner.width) as u16;
    let max_scroll = total.saturating_sub(inner.height);
    app.details_area = area;
    app.detail_max = max_scroll;
    if app.detail_scroll > max_scroll {
        app.detail_scroll = max_scroll;
    }

    frame.render_widget(paragraph.block(block).scroll((app.detail_scroll, 0)), area);

    if max_scroll > 0 && !app.show_help && app.modal.is_none() {
        let mut state = ScrollbarState::new(total as usize)
            .viewport_content_length(inner.height as usize)
            .position(app.detail_scroll as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_style(Style::default().fg(palette.accent))
                .track_style(Style::default().fg(palette.faint)),
            area,
            &mut state,
        );
    }
}

pub(super) fn details_lines(details: &Details, palette: Palette) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match details {
        Details::Empty => lines.push(Line::from(Span::styled(
            "没有检测到外设",
            Style::default().fg(palette.faint),
        ))),
        Details::Device {
            title,
            tags,
            lines: body,
        } => {
            lines.push(Line::from(Span::styled(
                title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                tags.join(" · "),
                Style::default().fg(palette.accent),
            )));
            lines.push(Line::from(""));
            for line in body {
                let spans: Vec<Span> = line
                    .spans
                    .iter()
                    .map(|span| {
                        let mut style = Style::default().fg(tone_color(span.tone, palette));
                        if span.bold {
                            style = style.add_modifier(Modifier::BOLD);
                        }
                        Span::styled(span.text.clone(), style)
                    })
                    .collect();
                lines.push(Line::from(spans));
            }
        }
    }
    lines
}
