//! 渲染：顶栏、设备列表、详情、状态行和帮助。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Confirm};
use crate::theme::Palette;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let palette = app.palette();
    let area = frame.area();
    if area.width < 60 || area.height < 16 {
        frame.render_widget(
            Paragraph::new("终端太小（至少 60×16），请放大窗口")
                .style(Style::default().fg(palette.bad)),
            area,
        );
        return;
    }

    let [header, body, status, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header, palette);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(body);
    draw_list(frame, app, left, palette);
    draw_details(frame, app, right, palette);
    draw_status(frame, app, status);
    draw_keys(frame, app, keys, palette);

    if let Some(modal) = app.modal.clone() {
        draw_confirm(frame, area, &modal, palette);
    } else if app.show_help {
        draw_help(frame, app, area, palette);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect, palette: Palette) {
    let [left, right] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(42)]).areas(area);

    let mut title = vec![
        Span::styled(
            " RhubTui ",
            Style::default()
                .fg(palette.surface)
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" 外设管理", Style::default().fg(palette.text)),
    ];
    if !app.filter.is_empty() {
        title.push(Span::styled(
            format!("  /{}", app.filter),
            Style::default().fg(palette.warn),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(title)), left);

    let mut parts = vec![format!("{} 个设备", app.visible_count())];
    let mounted = app.mounted_count();
    if mounted > 0 {
        parts.push(format!("{mounted} 已挂载"));
    }
    let low = app.low_battery();
    if !low.is_empty() {
        parts.push(format!("{} 低电", low.len()));
    }
    if app.busy() {
        parts.push("⟳ 刷新中".into());
    }
    parts.push("自动刷新".into());
    frame.render_widget(
        Paragraph::new(format!("{} ", parts.join(" · ")))
            .alignment(Alignment::Right)
            .style(Style::default().fg(palette.dim)),
        right,
    );
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect, palette: Palette) {
    // 边框、高亮符号和徽标大约占 16 列，其余留给设备名。
    app.set_name_width(area.width.saturating_sub(16) as usize);
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| ListItem::new(row.line.clone()))
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

fn draw_details(frame: &mut Frame, app: &mut App, area: Rect, palette: Palette) {
    let block = panel(" 详情 ", palette);
    let inner = block.inner(area);
    let lines = app.details();
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

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (text, color) = app.status_line();
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {text}"), Style::default().fg(color))),
        area,
    );
}

fn draw_keys(frame: &mut Frame, app: &App, area: Rect, palette: Palette) {
    let storage = app
        .selected_peripheral()
        .is_some_and(|peripheral| !peripheral.disks.is_empty());
    let mut hints = vec![
        "↑↓ 选择",
        "Tab 分类",
        "/ 过滤",
        "i 内置",
        "b 电量",
        "t 主题",
        "? 帮助",
        "q 退出",
    ];
    if storage {
        hints.insert(3, "m 挂载");
        hints.insert(4, "u 卸载");
        hints.insert(5, "e 弹出");
    } else {
        hints.insert(3, "PgUp/PgDn 滚动");
    }
    let mut line = String::from(" ");
    for hint in hints {
        if UnicodeWidthStr::width(line.as_str()) + UnicodeWidthStr::width(hint) + 3
            > area.width as usize
        {
            break;
        }
        if line.len() > 1 {
            line.push_str("   ");
        }
        line.push_str(hint);
    }
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(palette.faint)),
        area,
    );
}

fn draw_confirm(frame: &mut Frame, area: Rect, modal: &Confirm, palette: Palette) {
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

fn draw_help(frame: &mut Frame, app: &mut App, area: Rect, palette: Palette) {
    let lines = vec![
        Line::from(Span::styled(
            "按键",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  ↑ ↓ / k j       选择设备"),
        Line::from("  Tab / Shift+Tab 跳到上/下一分类"),
        Line::from("  ← → / h l       选择存储分区"),
        Line::from("  m / u           挂载 / 卸载选中分区"),
        Line::from("  e               安全弹出（弹确认框）"),
        Line::from("  PgUp/PgDn J/K     滚动详情"),
        Line::from("  Ctrl+D / Ctrl+U   滚动详情"),
        Line::from("  /               按名称/端口/VID:PID 过滤"),
        Line::from("  b               重新读取电量"),
        Line::from("  r               立即重新扫描设备"),
        Line::from("  i               显示 / 隐藏内置设备"),
        Line::from("  t               切换主题"),
        Line::from("  ?               显示 / 隐藏本帮助"),
        Line::from("  q / Esc / Ctrl+C  退出（有过滤时先清除）"),
        Line::from(""),
        Line::from(Span::styled(
            "鼠标：滚轮滚动详情或列表，左键点击选择设备。",
            Style::default().fg(palette.dim),
        )),
        Line::from(Span::styled(
            "电量仅对 MCHOSE 接收器和标准 HID 电量通道可用。",
            Style::default().fg(palette.dim),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  ? / Esc 关闭",
            Style::default().fg(palette.faint),
        )),
    ];

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

fn panel<'a>(title: impl Into<Line<'a>>, palette: Palette) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .border_style(Style::default().fg(palette.border))
        .title_style(Style::default().fg(palette.dim))
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}
