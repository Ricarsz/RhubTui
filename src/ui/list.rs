//! 设备列表。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};

use crate::app::App;
use crate::theme::Palette;
use crate::view::{Row, RowText};

use super::{panel, tone_color};

pub(super) fn draw(frame: &mut Frame, app: &mut App, area: Rect, palette: Palette) {
    // 边框、高亮符号和徽标大约占 16 列，其余留给设备名。
    let name_cols = area.width.saturating_sub(16) as usize;
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| ListItem::new(row_line(row, name_cols, palette)))
        .collect();

    let title = if app.filter_editing {
        Line::from(vec![
            Span::styled(" 过滤 ", Style::default().fg(palette.accent)),
            Span::styled(
                format!("{}▏", app.filter),
                Style::default().fg(palette.warn),
            ),
            Span::styled(" ", Style::default()),
        ])
    } else if !app.filter.is_empty() {
        Line::from(format!(" 设备 · /{} ", app.filter))
    } else {
        Line::from(" 设备 ")
    };

    let list = List::new(items)
        .block(panel(title, palette))
        .highlight_symbol("▌ ")
        .highlight_style(
            Style::default()
                .bg(palette.selected)
                .fg(palette.text)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
    app.list_area = area;
    app.list_offset = state.offset();
}

pub(super) fn row_line(row: &Row, name_cols: usize, palette: Palette) -> Line<'static> {
    match &row.text {
        RowText::Section(title) => Line::from(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(palette.surface)
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD),
        )),
        RowText::Device { name, flags, badge } => {
            let mut spans = vec![Span::styled(
                crate::model::truncate_cols(name, name_cols),
                Style::default().fg(palette.text),
            )];
            for (text, tone) in flags {
                spans.push(Span::styled(
                    format!(" {text}"),
                    Style::default().fg(tone_color(*tone, palette)),
                ));
            }
            if let Some(badge) = badge {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    badge.text.clone(),
                    Style::default().fg(tone_color(badge.tone, palette)),
                ));
            }
            Line::from(spans)
        }
    }
}
