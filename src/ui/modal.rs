//! 确认框与帮助弹窗。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Confirm};
use crate::theme::Palette;

use super::centered;

pub(super) fn confirm(frame: &mut Frame, area: Rect, modal: &Confirm, palette: Palette) {
    let (title, body) = match modal {
        Confirm::Eject { device, label } => (
            "安全弹出",
            format!("将停止 {device} 并切断供电：\n\n  {label}\n\n确认后请等待提示再拔线。"),
        ),
    };
    let hint = " Enter / y 确认    Esc / n 取消";
    let width = body
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
        .max(UnicodeWidthStr::width(hint))
        .max(40)
        .min(area.width.saturating_sub(4) as usize) as u16;
    let height = (body.lines().count() as u16 + 4).min(area.height.saturating_sub(2));
    let popup = centered(area, width + 4, height);

    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(vec![
            Span::styled("▍", Style::default().fg(palette.warn)),
            Span::styled(format!(" {title} "), Style::default().fg(palette.text)),
        ]))
        .border_style(Style::default().fg(palette.warn));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [content, footer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette.text)),
        content,
    );
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(palette.dim)),
        footer,
    );
}

pub(super) fn help(frame: &mut Frame, app: &mut App, area: Rect, palette: Palette) {
    let lines: Vec<Line<'static>> = crate::view::help::LINES
        .iter()
        .map(|line| match line {
            crate::view::help::HelpLine::Heading(text) => Line::from(Span::styled(
                *text,
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            crate::view::help::HelpLine::Row(text) => Line::from(*text),
            crate::view::help::HelpLine::Note(text) => {
                Line::from(Span::styled(*text, Style::default().fg(palette.dim)))
            }
            crate::view::help::HelpLine::Blank => Line::from(""),
        })
        .collect();

    let body_width = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0);
    let width = (body_width + 4).min(area.width.saturating_sub(4) as usize) as u16;
    let popup = centered(
        area,
        width,
        (lines.len() as u16 + 2).min(area.height.saturating_sub(2)),
    );
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::from(vec![
            Span::styled("▍", Style::default().fg(palette.accent)),
            Span::styled(" 帮助 ", Style::default().fg(palette.text)),
        ]))
        .border_style(Style::default().fg(palette.accent));
    let inner = block.inner(popup);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = paragraph.line_count(inner.width) as u16;
    let max_scroll = total.saturating_sub(inner.height);
    if app.help_scroll > max_scroll {
        app.help_scroll = max_scroll;
    }
    frame.render_widget(paragraph.block(block).scroll((app.help_scroll, 0)), popup);
    if max_scroll > 0 {
        let mut state = ScrollbarState::new(total as usize)
            .viewport_content_length(inner.height as usize)
            .position(app.help_scroll as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_style(Style::default().fg(palette.accent))
                .track_style(Style::default().fg(palette.faint)),
            popup,
            &mut state,
        );
    }
}
