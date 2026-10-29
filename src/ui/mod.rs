//! 渲染：顶栏、设备列表、详情、状态行和帮助。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::theme::Palette;
use crate::view::Tone;

mod details;
mod list;
mod modal;

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
    list::draw(frame, app, left, palette);
    details::draw(frame, app, right, palette);
    draw_status(frame, app, status);
    draw_keys(frame, app, keys, palette);

    if let Some(modal) = app.modal.clone() {
        modal::confirm(frame, area, &modal, palette);
    } else if app.show_help {
        modal::help(frame, app, area, palette);
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
    if app.busy() {
        parts.push("⟳ 刷新中".into());
    }
    frame.render_widget(
        Paragraph::new(format!("{} ", parts.join(" · ")))
            .alignment(Alignment::Right)
            .style(Style::default().fg(palette.dim)),
        right,
    );
}

pub(crate) fn tone_color(tone: Tone, palette: Palette) -> Color {
    match tone {
        Tone::Normal => palette.text,
        Tone::Good => palette.good,
        Tone::Warn => palette.warn,
        Tone::Dim => palette.dim,
        Tone::Faint => palette.faint,
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
    let mut hints = vec!["↑↓ 选择", "Tab 分类", "/ 过滤"];
    if storage {
        hints.extend(["m 挂载", "u 卸载", "e 弹出"]);
    }
    hints.extend(["? 帮助", "q 退出"]);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Confirm;
    use crate::testing;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn app() -> App {
        let mut app = App::for_test(testing::snapshot(), crate::config::Config::default());
        app.rebuild();
        app
    }

    fn dump(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..height {
            let mut skip = false;
            for x in 0..width {
                if skip {
                    skip = false;
                    continue;
                }
                let symbol = buffer[(x, y)].symbol();
                out.push_str(symbol);
                skip = UnicodeWidthStr::width(symbol) == 2;
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_fixture_overview() {
        let mut app = app();
        let text = dump(&mut app, 120, 30);
        assert!(text.contains("RhubTui"), "{text}");
        assert!(text.contains("4 个设备"), "{text}");
        assert!(text.contains("PSSD T7"), "{text}");
        assert!(text.contains("A5 Pro Max"), "{text}");
        assert!(text.contains("已挂载"), "{text}");
        assert!(text.contains("扩展坞"), "{text}");
        assert!(text.contains("详情"), "{text}");
    }

    #[test]
    fn renders_details_for_selected_device() {
        let mut app = app();
        let text = dump(&mut app, 120, 30);
        assert!(text.contains("详情"), "{text}");
        assert!(text.contains("外接"), "{text}");
        assert!(text.contains("位置"), "{text}");
    }

    #[test]
    fn renders_help_and_eject_modal() {
        let mut app = app();
        app.show_help = true;
        let text = dump(&mut app, 120, 34);
        assert!(text.contains("帮助"), "{text}");
        app.show_help = false;
        app.modal = Some(Confirm::Eject {
            device: "/dev/sda".into(),
            label: "T7".into(),
        });
        let text = dump(&mut app, 120, 30);
        assert!(text.contains("安全弹出"), "{text}");
        assert!(text.contains("/dev/sda"), "{text}");
    }

    #[test]
    fn tiny_terminal_gets_a_hint_instead_of_panicking() {
        let mut app = app();
        let text = dump(&mut app, 40, 10);
        assert!(text.contains("终端太小"), "{text}");
    }
}
