//! Application state: device list, selection, background battery reads and
//! mount actions.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, KeyEventKind};
use ratatui::style::Color;

use crate::config::Config;
use crate::model::*;
use crate::scan;
use crate::theme::Palette;
use crate::view::{self, Row};

mod input;
mod threads;

pub(crate) use self::threads::Msg;
use self::threads::{snapshot_signature, spawn_battery_worker, spawn_scan_worker};

const RETRY_AFTER: Duration = Duration::from_secs(20);
const LOADING_TIMEOUT: Duration = Duration::from_secs(15);
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Eject { device: String, label: String },
}

pub struct App {
    pub snap: scan::Snapshot,
    pub rows: Vec<Row>,
    pub selected: usize,
    /// 渲染时记录的可视区域，用于鼠标滚轮/点击定位。
    pub list_area: ratatui::layout::Rect,
    pub details_area: ratatui::layout::Rect,
    pub list_offset: usize,
    pub selected_key: Option<String>,
    pub batteries: HashMap<String, Battery>,
    pub part_sel: HashMap<String, usize>,
    pub status: String,
    pub status_error: bool,
    pub status_at: Instant,
    pub quitting: bool,
    pub show_help: bool,
    /// 详情面板是否显示技术细节；默认关闭，按 `a` 切换。
    pub advanced: bool,
    pub help_scroll: u16,
    pub detail_scroll: u16,
    pub detail_max: u16,
    pub filter: String,
    pub filter_editing: bool,
    pub show_internal: bool,
    pub theme: String,
    pub dirty: bool,
    pub modal: Option<Confirm>,
    action_pending: Option<(u64, Instant)>,
    action_seq: u64,
    last_scan: Instant,
    scan_interval: Duration,
    battery_refresh: Duration,
    battery_at: HashMap<String, Instant>,
    battery_tries: HashMap<String, u32>,
    snapshot_signature: Vec<String>,
    scan_busy: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    battery_tx: Sender<HidInfo>,
    scan_tx: Sender<()>,
    pub mouse: bool,
}

impl App {
    pub fn new(config: Config) -> Self {
        let (tx, rx) = mpsc::channel();
        let battery_tx = spawn_battery_worker(tx.clone());
        let scan_tx = spawn_scan_worker(tx.clone());
        let snapshot = scan::scan();
        let mut app = Self::assemble(snapshot, config, tx, rx, battery_tx, scan_tx);
        app.request_all_batteries();
        app.rebuild();
        app
    }

    /// 用给定快照搭一个不启动后台线程的 App，供测试与将来的离线渲染使用。
    #[cfg(test)]
    pub fn for_test(snapshot: scan::Snapshot, config: Config) -> Self {
        let (tx, rx) = mpsc::channel();
        let (battery_tx, _battery_rx) = mpsc::channel();
        let (scan_tx, _scan_rx) = mpsc::channel();
        Self::assemble(snapshot, config, tx, rx, battery_tx, scan_tx)
    }

    fn assemble(
        snapshot: scan::Snapshot,
        config: Config,
        tx: Sender<Msg>,
        rx: Receiver<Msg>,
        battery_tx: Sender<HidInfo>,
        scan_tx: Sender<()>,
    ) -> Self {
        let scan_interval = config.scan_interval();
        let battery_refresh = config.battery_refresh();
        let mouse = config.mouse;
        let theme = config.theme.clone();
        let signature = snapshot_signature(&snapshot);
        let mut app = Self {
            snap: snapshot,
            rows: Vec::new(),
            selected: 0,
            list_area: ratatui::layout::Rect::default(),
            details_area: ratatui::layout::Rect::default(),
            list_offset: 0,
            selected_key: None,
            batteries: HashMap::new(),
            part_sel: HashMap::new(),
            status: String::new(),
            status_error: false,
            status_at: Instant::now(),
            quitting: false,
            show_help: false,
            advanced: false,
            help_scroll: 0,
            detail_scroll: 0,
            detail_max: 0,
            filter: String::new(),
            filter_editing: false,
            show_internal: true,
            theme,
            dirty: true,
            modal: None,
            action_pending: None,
            action_seq: 0,
            last_scan: Instant::now(),
            scan_interval,
            battery_refresh,
            battery_at: HashMap::new(),
            battery_tries: HashMap::new(),
            snapshot_signature: signature,
            scan_busy: false,
            tx,
            rx,
            battery_tx,
            scan_tx,
            mouse,
        };

        app.rebuild();
        app.select_first();
        app.rebuild();
        app
    }

    // ─── state ─────────────────────────────────────────────────────────────

    pub fn palette(&self) -> Palette {
        Palette::from_name(&self.theme)
    }

    pub fn visible_count(&self) -> usize {
        self.rows.iter().filter(|row| row.device.is_some()).count()
    }

    pub fn mounted_count(&self) -> usize {
        self.snap
            .peripherals
            .iter()
            .filter(|peripheral| peripheral.disks.iter().any(Disk::mounted))
            .count()
    }

    pub fn low_battery(&self) -> Vec<(String, u8)> {
        self.snap
            .peripherals
            .iter()
            .filter(|peripheral| !peripheral.usb.place.is_internal())
            .filter_map(|peripheral| {
                let hid = peripheral.battery_hid()?;
                let pct = self.battery_of(hid).pct()?;
                (pct <= 20).then(|| (peripheral.name(), pct))
            })
            .collect()
    }

    pub fn selected_peripheral(&self) -> Option<&Peripheral> {
        let index = self.rows.get(self.selected)?.device?;
        self.snap.peripherals.get(index)
    }

    pub fn battery_of(&self, hid: &HidInfo) -> Battery {
        self.batteries
            .get(&hid.node)
            .cloned()
            .unwrap_or(Battery::Unknown)
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = text.into();
        self.status_error = error;
        self.status_at = Instant::now();
        self.dirty = true;
    }

    pub fn status_line(&self) -> (&str, Color) {
        let palette = self.palette();
        if self.action_pending.is_some() {
            (self.status.as_str(), palette.warn)
        } else if self.status.is_empty() {
            ("就绪", palette.faint)
        } else if self.status_error {
            (self.status.as_str(), palette.bad)
        } else {
            (self.status.as_str(), palette.good)
        }
    }

    pub fn busy(&self) -> bool {
        self.scan_busy
            || self.action_pending.is_some()
            || self
                .batteries
                .values()
                .any(|battery| matches!(battery, Battery::Loading))
    }

    // ─── list building ─────────────────────────────────────────────────────

    pub(crate) fn rebuild(&mut self) {
        let key = self.selected_key.clone();
        let mut rows = Vec::new();
        let needle = self.filter.trim().to_lowercase();

        for category in Category::ALL {
            let mut indices: Vec<usize> = self
                .snap
                .peripherals
                .iter()
                .enumerate()
                .filter(|(_, p)| p.category == category)
                .filter(|(_, p)| self.show_internal || !p.usb.place.is_internal())
                .filter(|(_, p)| needle.is_empty() || p.matches(&needle))
                .map(|(i, _)| i)
                .collect();
            if indices.is_empty() {
                continue;
            }
            indices.sort_by_key(|i| self.snap.peripherals[*i].name().to_lowercase());

            rows.push(Row::section(category.title()));
            for index in indices {
                let peripheral = &self.snap.peripherals[index];
                let battery = peripheral.battery_hid().map(|hid| self.battery_of(hid));
                rows.push(Row::device(index, peripheral, battery.as_ref()));
            }
        }

        self.rows = rows;

        if let Some(key) = key
            && let Some(position) = self
                .rows
                .iter()
                .position(|r| r.device.map(|i| self.snap.peripherals[i].key()) == Some(key.clone()))
        {
            self.selected = position;
            return;
        }
        self.selected = self
            .rows
            .iter()
            .position(|row| row.device.is_some())
            .unwrap_or(0);
        self.dirty = true;
    }

    // ─── selection ─────────────────────────────────────────────────────────

    // ─── filter ────────────────────────────────────────────────────────────

    // ─── partitions & actions ──────────────────────────────────────────────

    // ─── event handling ────────────────────────────────────────────────────

    // ─── details pane ──────────────────────────────────────────────────────

    /// 默认展示一眼能看懂的概览，按 `a` 切到技术细节。
    pub fn details(&self) -> view::Details {
        let index = self.rows.get(self.selected).and_then(|row| row.device);
        view::details(
            &self.snap,
            index,
            &self.batteries,
            &self.part_sel,
            self.advanced,
        )
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// 启动 TUI。配置在启动时读取一次。
pub fn run(config: Config) -> Result<()> {
    let mouse = config.mouse;
    let mut terminal = ratatui::init();
    if mouse {
        let mut stdout = std::io::stdout();
        ratatui::crossterm::execute!(stdout, ratatui::crossterm::event::EnableMouseCapture)?;
    }
    let mut app = App::new(config);

    let result = loop {
        if app.dirty {
            terminal.draw(|frame| crate::ui::draw(frame, &mut app))?;
            app.dirty = false;
        }

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                event::Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                event::Event::Mouse(mouse_event) if app.mouse => app.on_mouse(mouse_event),
                event::Event::Resize(_, _) => app.dirty = true,
                _ => {}
            }
        }

        app.tick();
        if app.quitting {
            break Ok(());
        }
    };

    if mouse {
        let mut stdout = std::io::stdout();
        let _ =
            ratatui::crossterm::execute!(stdout, ratatui::crossterm::event::DisableMouseCapture);
    }
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::threads::retry_interval;
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn battery_retry_backs_off() {
        assert_eq!(retry_interval(0), Duration::from_secs(20));
        assert_eq!(retry_interval(1), Duration::from_secs(40));
        assert_eq!(retry_interval(2), Duration::from_secs(80));
        assert_eq!(retry_interval(3), Duration::from_secs(160));
        assert_eq!(retry_interval(99), Duration::from_secs(160));
    }

    fn fixture_app() -> App {
        App::for_test(crate::testing::snapshot(), Config::default())
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn navigation_skips_category_sections() {
        let mut app = fixture_app();
        assert_eq!(app.selected_peripheral().unwrap().name(), "PSSD T7");
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected_peripheral().unwrap().name(), "A5 Pro Max");
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected_peripheral().unwrap().name(), "USB4 Dock");
        app.on_key(key(KeyCode::Char('G')));
        assert_eq!(
            app.selected_peripheral().unwrap().name(),
            "Integrated Camera"
        );
        app.on_key(key(KeyCode::Char('g')));
        assert_eq!(app.selected_peripheral().unwrap().name(), "PSSD T7");
    }

    #[test]
    fn filter_mode_narrows_the_list() {
        let mut app = fixture_app();
        app.on_key(key(KeyCode::Char('/')));
        for ch in "usb4".chars() {
            app.on_key(key(KeyCode::Char(ch)));
        }
        assert!(app.filter_editing);
        assert_eq!(app.visible_count(), 1);
        assert_eq!(app.selected_peripheral().unwrap().name(), "USB4 Dock");
        app.on_key(key(KeyCode::Enter));
        assert!(!app.filter_editing);
        assert_eq!(app.filter, "usb4");
    }

    #[test]
    fn internal_devices_can_be_hidden() {
        let mut app = fixture_app();
        assert_eq!(app.visible_count(), 4);
        app.on_key(key(KeyCode::Char('i')));
        assert!(!app.show_internal);
        assert_eq!(app.visible_count(), 3);
        app.on_key(key(KeyCode::Char('i')));
        assert_eq!(app.visible_count(), 4);
    }

    #[test]
    fn help_and_details_advanced_toggle() {
        let mut app = fixture_app();
        app.on_key(key(KeyCode::Char('?')));
        assert!(app.show_help);
        app.on_key(key(KeyCode::Char('j')));
        assert!(app.show_help);
        assert_eq!(app.help_scroll, 3);
        app.on_key(key(KeyCode::Char('x')));
        assert!(!app.show_help);
        assert_eq!(app.help_scroll, 0);

        assert!(!app.advanced);
        app.on_key(key(KeyCode::Char('a')));
        assert!(app.advanced);
        app.on_key(key(KeyCode::Char('a')));
        assert!(!app.advanced);
    }

    #[test]
    fn eject_requires_unmounted_disk_and_confirms() {
        let mut app = fixture_app();
        app.on_key(key(KeyCode::Char('e')));
        assert!(app.modal.is_none(), "挂载中的磁盘不应进入弹出确认");
        assert!(app.status_line().0.contains("先卸载"));

        app.snap.peripherals[0].disks[0].partitions[0].mount = None;
        app.rebuild();
        app.on_key(key(KeyCode::Char('e')));
        match app.modal.as_ref() {
            Some(Confirm::Eject { device, .. }) => assert_eq!(device, "/dev/sda"),
            other => panic!("应打开弹出确认，实际 {other:?}"),
        }
        app.on_key(key(KeyCode::Esc));
        assert!(app.modal.is_none());
        assert!(!app.quitting);
    }

    #[test]
    fn theme_cycles_through_known_names() {
        let mut app = fixture_app();
        assert_eq!(app.theme, "ocean");
        app.on_key(key(KeyCode::Char('t')));
        assert_eq!(app.theme, "forest");
        app.on_key(key(KeyCode::Char('t')));
        assert_eq!(app.theme, "sunset");
        app.on_key(key(KeyCode::Char('t')));
        assert_eq!(app.theme, "ocean");
    }
}
