//! 渲染：顶栏、设备列表、详情、状态行和帮助。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::App;
use crate::theme::Palette;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let palette = Palette::from_name(&app.theme);
    let area = frame.area();
    if area.width < 40 || area.height < 12 {
        frame.render_widget(
            Paragraph::new("终端太小，请放大窗口").style(Style::default().fg(palette.bad)),
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
        Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).areas(body);
    draw_list(frame, app, left, palette);
    draw_details(frame, app, right, palette);
    draw_status(frame, app, status, palette);
    draw_keys(frame, keys, palette);

    if app.show_help {
        draw_help(frame, area, palette);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect, palette: Palette) {
    let [left, right] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(36)]).areas(area);

    let title = Line::from(vec![
        Span::styled(
            " RhubTui ",
            Style::default()
                .fg(palette.surface)
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" 外设管理", Style::default().fg(palette.text)),
    ]);
    frame.render_widget(Paragraph::new(title), left);

    let info = format!("{} 个设备 · 自动刷新 ", app.snap.peripherals.len());
    frame.render_widget(
        Paragraph::new(info)
            .alignment(Alignment::Right)
            .style(Style::default().fg(palette.dim)),
        right,
    );
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect, palette: Palette) {
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| ListItem::new(row.line.clone()))
        .collect();

    let list = List::new(items)
        .block(panel(" 设备 ", palette))
        .highlight_symbol("▌ ")
        .highlight_style(
            Style::default()
                .bg(palette.selected)
                .fg(palette.text)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_details(frame: &mut Frame, app: &App, area: Rect, palette: Palette) {
    let paragraph = Paragraph::new(app.details())
        .block(panel(" 详情 ", palette))
        .scroll((app.detail_scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, palette: Palette) {
    let (text, color) = app.status_line();
    let color = if text == "就绪" {
        palette.faint
    } else {
        color
    };
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {text}"), Style::default().fg(color))),
        area,
    );
}

fn draw_keys(frame: &mut Frame, area: Rect, palette: Palette) {
    let hints = " ↑↓/jk 选择   ←→/hl 选分区   m 挂载   u 卸载   e 弹出   b 电量   PgUp/PgDn 滚动   ? 帮助   q 退出";
    frame.render_widget(
        Paragraph::new(hints).style(Style::default().fg(palette.faint)),
        area,
    );
}

fn draw_help(frame: &mut Frame, area: Rect, palette: Palette) {
    let popup = centered(area, 60, 14);
    frame.render_widget(Clear, popup);

    let lines = vec![
        Line::from(Span::styled(
            "按键",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  ↑ ↓ / k j     上下选择设备"),
        Line::from("  ← → / h l     选择存储分区"),
        Line::from("  m / u         挂载 / 卸载选中分区"),
        Line::from("  e             安全弹出（需先卸载）"),
        Line::from("  PgUp / PgDn   滚动详情"),
        Line::from("  b             重新读取电量"),
        Line::from("  r             立即重新扫描设备"),
        Line::from("  ?             显示 / 隐藏本帮助"),
        Line::from("  q / Esc       退出"),
        Line::from(""),
        Line::from(Span::styled(
            "电量仅对 MCHOSE 接收器与标准 HID 电量通道可用",
            Style::default().fg(palette.dim),
        )),
    ];

    let paragraph = Paragraph::new(lines).block(
        Block::bordered()
            .title(" 帮助 ")
            .border_style(Style::default().fg(palette.accent)),
    );
    frame.render_widget(paragraph, popup);
}

fn panel(title: &'static str, palette: Palette) -> Block<'static> {
    Block::bordered()
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
