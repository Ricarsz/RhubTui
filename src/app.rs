//! Application state: device list, selection, background battery reads and
//! mount actions.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::Color;

use crate::battery;
use crate::config::Config;
use crate::mchose::Status;
use crate::model::*;
use crate::mount;
use crate::scan;
use crate::theme::Palette;
use crate::view::{self, Row};

const RETRY_AFTER: Duration = Duration::from_secs(20);
const LOADING_TIMEOUT: Duration = Duration::from_secs(15);
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);

pub enum Msg {
    Battery {
        node: String,
        result: Result<Status, String>,
    },
    Action {
        seq: u64,
        ok: bool,
        text: String,
    },
    Snapshot(Box<scan::Snapshot>),
}

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

    fn select_first(&mut self) {
        if let Some(position) = self.rows.iter().position(|row| row.device.is_some()) {
            self.selected = position;
        }
        self.sync_selection();
    }

    fn sync_selection(&mut self) {
        let key = self.selected_peripheral().map(Peripheral::key);
        if key == self.selected_key {
            return;
        }
        self.selected_key = key;
        self.detail_scroll = 0;
        self.dirty = true;
        self.request_battery();
    }

    fn select(&mut self, index: usize) {
        if index < self.rows.len() && self.rows[index].device.is_some() {
            self.selected = index;
            self.sync_selection();
        }
    }

    fn request_battery(&mut self) {
        let Some(peripheral) = self.selected_peripheral() else {
            return;
        };
        let hids: Vec<HidInfo> = peripheral
            .hids
            .iter()
            .filter(|hid| hid.has_battery())
            .filter(|hid| !self.batteries.contains_key(&hid.node))
            .cloned()
            .collect();
        for hid in hids {
            self.queue_battery(&hid);
        }
    }

    fn queue_battery(&mut self, hid: &HidInfo) {
        if !battery::supported(hid) {
            return;
        }
        self.battery_at.insert(hid.node.clone(), Instant::now());
        self.batteries
            .entry(hid.node.clone())
            .or_insert(Battery::Loading);
        let _ = self.battery_tx.send(hid.clone());
        self.dirty = true;
    }

    fn request_all_batteries(&mut self) {
        let nodes: Vec<HidInfo> = self
            .snap
            .peripherals
            .iter()
            .flat_map(|peripheral| peripheral.hids.iter())
            .filter(|hid| hid.has_battery())
            .cloned()
            .collect();
        for hid in nodes {
            self.queue_battery(&hid);
        }
    }

    fn refresh_due_batteries(&mut self) {
        let now = Instant::now();
        let nodes: Vec<HidInfo> = self
            .snap
            .peripherals
            .iter()
            .flat_map(|peripheral| peripheral.hids.iter().filter(|hid| hid.has_battery()))
            .cloned()
            .collect();
        let mut due = Vec::new();
        for hid in nodes {
            let state = self.batteries.get(&hid.node);
            let tries = self.battery_tries.get(&hid.node).copied().unwrap_or(0);
            let elapsed = self
                .battery_at
                .get(&hid.node)
                .map(|at| now.duration_since(*at))
                .unwrap_or(Duration::MAX);
            let interval = match state {
                Some(Battery::Ok { .. }) => self.battery_refresh,
                Some(Battery::Loading) => LOADING_TIMEOUT,
                Some(Battery::Err(_)) => retry_interval(tries),
                None => Duration::ZERO,
                Some(Battery::Unknown) => continue,
            };
            if elapsed >= interval {
                due.push(hid);
            }
        }
        for hid in due {
            *self.battery_tries.entry(hid.node.clone()).or_insert(0) += 1;
            self.queue_battery(&hid);
        }
    }

    fn move_by(&mut self, delta: isize) {
        let len = self.rows.len() as isize;
        let mut index = self.selected as isize;
        for _ in 0..len {
            index += delta;
            if index < 0 || index >= len {
                break;
            }
            if self.rows[index as usize].device.is_some() {
                self.select(index as usize);
                return;
            }
        }
    }

    fn move_to_edge(&mut self, last: bool) {
        let position = if last {
            self.rows.iter().rposition(|row| row.device.is_some())
        } else {
            self.rows.iter().position(|row| row.device.is_some())
        };
        if let Some(position) = position {
            self.select(position);
        }
    }

    /// 跳到上一/下一分类的第一个设备。
    fn jump_category(&mut self, delta: isize) {
        let headers: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.device.is_none())
            .map(|(index, _)| index)
            .collect();
        if headers.is_empty() {
            return;
        }
        let current = headers
            .iter()
            .filter(|index| **index <= self.selected)
            .count()
            .saturating_sub(1);
        let target = (current as isize + delta).clamp(0, headers.len() as isize - 1) as usize;
        let start = headers[target];
        if let Some(position) = self.rows[start..]
            .iter()
            .position(|row| row.device.is_some())
        {
            self.select(start + position);
        }
    }

    // ─── filter ────────────────────────────────────────────────────────────

    fn begin_filter(&mut self) {
        self.filter_editing = true;
        self.dirty = true;
        self.set_status("输入关键字过滤，Enter 确认，Esc 清空", false);
    }

    fn push_filter(&mut self, ch: char) {
        self.filter.push(ch);
        self.rebuild();
        self.sync_selection();
        self.dirty = true;
    }

    fn filter_backspace(&mut self) {
        self.filter.pop();
        self.rebuild();
        self.sync_selection();
        self.dirty = true;
    }

    fn clear_filter(&mut self) {
        if !self.filter.is_empty() {
            self.filter.clear();
            self.rebuild();
            self.sync_selection();
            self.dirty = true;
            self.set_status("已清除过滤", false);
        }
        self.filter_editing = false;
    }

    // ─── partitions & actions ──────────────────────────────────────────────

    fn partitions(peripheral: &Peripheral) -> Vec<&Partition> {
        peripheral
            .disks
            .iter()
            .flat_map(|disk| disk.partitions.iter())
            .collect()
    }

    fn part_index(&self, peripheral: &Peripheral) -> usize {
        let total: usize = peripheral.disks.iter().map(|d| d.partitions.len()).sum();
        if total == 0 {
            return 0;
        }
        self.part_sel.get(&peripheral.key()).copied().unwrap_or(0) % total
    }

    fn cycle_partition(&mut self, delta: isize) {
        let Some(peripheral) = self.selected_peripheral().cloned() else {
            return;
        };
        let total: usize = peripheral.disks.iter().map(|d| d.partitions.len()).sum();
        if total == 0 {
            self.set_status("该设备没有分区（整盘可直接挂载）", true);
            return;
        }
        let next = (self.part_index(&peripheral) as isize + delta).rem_euclid(total as isize);
        self.part_sel.insert(peripheral.key(), next as usize);
        self.dirty = true;
    }

    fn mount_selected(&mut self) {
        if self.action_pending.is_some() {
            self.set_status("上一个操作还没完成", true);
            return;
        }
        let Some(peripheral) = self.selected_peripheral().cloned() else {
            return;
        };
        let index = self.part_index(&peripheral);
        let partitions = Self::partitions(&peripheral);

        let (device, label, mounted) = match partitions.get(index) {
            Some(partition) => (
                partition.path.clone(),
                if partition.label.is_empty() {
                    partition.name.clone()
                } else {
                    partition.label.clone()
                },
                partition.mount.is_some(),
            ),
            None => match peripheral.disks.first() {
                // No partition table: mount the whole device (superfloppy).
                Some(disk) => (
                    disk.path.clone(),
                    if disk.model.is_empty() {
                        disk.name.clone()
                    } else {
                        disk.model.clone()
                    },
                    disk.mount.is_some(),
                ),
                None => {
                    self.set_status("该设备不是存储设备", true);
                    return;
                }
            },
        };

        if mounted {
            self.set_status("已经挂载了", true);
            return;
        }

        let seq = self.start_action(format!("挂载 {device} …"));
        let tx = self.tx.clone();
        thread::spawn(move || {
            let (ok, text) = match mount::mount(&device, &label) {
                Ok(point) => (true, format!("已挂载 {device} → {point}")),
                Err(error) => (false, format!("挂载失败：{error:#}")),
            };
            let _ = tx.send(Msg::Action { seq, ok, text });
        });
    }

    fn unmount_selected(&mut self) {
        if self.action_pending.is_some() {
            self.set_status("上一个操作还没完成", true);
            return;
        }
        let Some(peripheral) = self.selected_peripheral().cloned() else {
            return;
        };
        let partitions = Self::partitions(&peripheral);
        let current = self.part_index(&peripheral);
        let target = partitions
            .get(current)
            .filter(|p| p.mount.is_some())
            .or_else(|| partitions.iter().find(|p| p.mount.is_some()));

        let device = match target {
            Some(partition) => partition.path.clone(),
            None => match peripheral.disks.iter().find(|d| d.mount.is_some()) {
                Some(disk) => disk.path.clone(),
                None => {
                    self.set_status("没有已挂载的分区", true);
                    return;
                }
            },
        };

        let seq = self.start_action(format!("卸载 {device} …"));
        let tx = self.tx.clone();
        thread::spawn(move || {
            let (ok, text) = match mount::unmount(&device) {
                Ok(()) => (true, format!("已卸载 {device}")),
                Err(error) => (false, format!("卸载失败：{error:#}")),
            };
            let _ = tx.send(Msg::Action { seq, ok, text });
        });
    }

    fn eject_selected(&mut self) {
        if self.action_pending.is_some() {
            self.set_status("上一个操作还没完成", true);
            return;
        }
        let Some(peripheral) = self.selected_peripheral().cloned() else {
            return;
        };
        // Power off the disk that owns the selected partition.
        let selected = self.part_index(&peripheral);
        let mut offset = 0;
        let disk = peripheral
            .disks
            .iter()
            .find(|disk| {
                let count = disk.partitions.len();
                let hit = selected >= offset && selected < offset + count;
                offset += count;
                hit
            })
            .or_else(|| peripheral.disks.first());

        let Some(disk) = disk else {
            self.set_status("该设备不是存储设备", true);
            return;
        };
        if disk.mounted() {
            self.set_status("请先卸载再弹出", true);
            return;
        }

        self.modal = Some(Confirm::Eject {
            device: disk.path.clone(),
            label: disk.descriptor(),
        });
        self.set_status("确认弹出？y 确认 · Esc 取消", false);
        self.dirty = true;
    }

    fn confirm(&mut self, confirmed: bool) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        self.dirty = true;
        if !confirmed {
            self.set_status("已取消", false);
            return;
        }
        match modal {
            Confirm::Eject { device, label } => self.perform_eject(device, label),
        }
    }

    fn perform_eject(&mut self, device: String, label: String) {
        let seq = self.start_action(format!("弹出 {label} …"));
        let tx = self.tx.clone();
        thread::spawn(move || {
            let (ok, text) = match mount::power_off(&device) {
                Ok(()) => (true, format!("已安全弹出 {device}，可以拔线")),
                Err(error) => (false, format!("弹出失败：{error:#}")),
            };
            let _ = tx.send(Msg::Action { seq, ok, text });
        });
    }

    /// 开始一个后台操作，返回本次序号（用于识别过期结果）。
    fn start_action(&mut self, text: String) -> u64 {
        self.action_seq = self.action_seq.wrapping_add(1);
        self.action_pending = Some((self.action_seq, Instant::now()));
        self.set_status(text, false);
        self.dirty = true;
        self.action_seq
    }

    // ─── event handling ────────────────────────────────────────────────────

    pub fn on_key(&mut self, key: KeyEvent) {
        let code = key.code;
        let modifiers = key.modifiers;
        self.dirty = true;

        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            self.quitting = true;
            return;
        }

        if self.modal.is_some() {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.confirm(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.confirm(false)
                }
                _ => {}
            }
            return;
        }

        if self.show_help {
            match code {
                KeyCode::PageDown | KeyCode::Char('j') | KeyCode::Down => {
                    self.help_scroll = self.help_scroll.saturating_add(3)
                }
                KeyCode::PageUp | KeyCode::Char('k') | KeyCode::Up => {
                    self.help_scroll = self.help_scroll.saturating_sub(3)
                }
                _ => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
            }
            return;
        }

        if self.filter_editing {
            match code {
                KeyCode::Enter => {
                    self.filter_editing = false;
                    self.set_status(
                        if self.filter.is_empty() {
                            "过滤已结束".to_string()
                        } else {
                            format!("过滤：{}", self.filter)
                        },
                        false,
                    );
                }
                KeyCode::Esc => {
                    if self.filter.is_empty() {
                        self.filter_editing = false;
                    } else {
                        self.clear_filter();
                    }
                }
                KeyCode::Backspace => {
                    if self.filter.is_empty() {
                        self.filter_editing = false;
                    } else {
                        self.filter_backspace();
                    }
                }
                KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                    self.clear_filter();
                }
                KeyCode::Up => self.move_by(-1),
                KeyCode::Down => self.move_by(1),
                KeyCode::Char(ch) if !modifiers.contains(KeyModifiers::CONTROL) => {
                    self.push_filter(ch)
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Home | KeyCode::Char('g') => self.move_to_edge(false),
            KeyCode::End | KeyCode::Char('G') => self.move_to_edge(true),
            KeyCode::Tab => self.jump_category(1),
            KeyCode::BackTab => self.jump_category(-1),
            KeyCode::Left | KeyCode::Char('h') => self.cycle_partition(-1),
            KeyCode::Right | KeyCode::Char('l') => self.cycle_partition(1),
            KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_details(10)
            }
            KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_details(-10)
            }
            KeyCode::Char('m') => self.mount_selected(),
            KeyCode::Char('u') => self.unmount_selected(),
            KeyCode::Char('e') => self.eject_selected(),
            KeyCode::PageDown => self.scroll_details(5),
            KeyCode::PageUp => self.scroll_details(-5),
            KeyCode::Char('J') => self.scroll_details(1),
            KeyCode::Char('K') => self.scroll_details(-1),
            KeyCode::Char('/') => self.begin_filter(),
            KeyCode::Char('b') => self.refresh_batteries(),
            KeyCode::Char('a') => {
                self.advanced = !self.advanced;
                self.detail_scroll = 0;
                let text = if self.advanced {
                    "详情：高级信息"
                } else {
                    "详情：概览"
                };
                self.set_status(text, false);
            }
            KeyCode::Char('r') => {
                self.request_scan();
                self.set_status("正在刷新设备…", false);
            }
            KeyCode::Char('t') => self.cycle_theme(),
            KeyCode::Char('i') => {
                self.show_internal = !self.show_internal;
                self.rebuild();
                self.sync_selection();
                let text = if self.show_internal {
                    "显示内置设备"
                } else {
                    "隐藏内置设备"
                };
                self.set_status(text, false);
            }
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
                self.help_scroll = 0;
            }
            KeyCode::Esc => {
                if self.filter.is_empty() {
                    self.quitting = true;
                } else {
                    self.clear_filter();
                }
            }
            KeyCode::Char('q') => self.quitting = true,
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, mouse: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let inside = |area: ratatui::layout::Rect, column: u16, row: u16| {
            column >= area.x
                && column < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
        };
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if inside(self.details_area, mouse.column, mouse.row) {
                    self.scroll_details(3);
                } else {
                    self.move_by(1);
                }
            }
            MouseEventKind::ScrollUp => {
                if inside(self.details_area, mouse.column, mouse.row) {
                    self.scroll_details(-3);
                } else {
                    self.move_by(-1);
                }
            }
            MouseEventKind::Down(MouseButton::Left)
                if inside(self.list_area, mouse.column, mouse.row) =>
            {
                // 去掉上边框，再按列表滚动偏移换算成行号。
                let offset = mouse.row.saturating_sub(self.list_area.y + 1) as usize;
                let index = self.list_offset + offset;
                if index < self.rows.len() && self.rows[index].device.is_some() {
                    self.select(index);
                }
            }
            _ => {}
        }
    }

    pub fn scroll_details(&mut self, delta: i32) {
        let max = self.detail_max;
        let next = (self.detail_scroll as i32 + delta).clamp(0, max as i32) as u16;
        if next != self.detail_scroll {
            self.detail_scroll = next;
            self.dirty = true;
        }
    }

    fn refresh_batteries(&mut self) {
        self.batteries.clear();
        self.battery_at.clear();
        self.battery_tries.clear();
        self.request_all_batteries();
        self.rebuild();
        self.set_status("重新读取电量", false);
    }

    fn cycle_theme(&mut self) {
        let names = Palette::NAMES;
        let current = names
            .iter()
            .position(|name| *name == self.theme)
            .unwrap_or(0);
        self.theme = names[(current + 1) % names.len()].to_string();
        self.rebuild();
        self.dirty = true;
        self.set_status(format!("主题：{}", self.theme), false);
    }

    pub fn tick(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }

        if self.last_scan.elapsed() >= self.scan_interval {
            self.request_scan();
        }
        self.refresh_due_batteries();

        if let Some((_, started)) = self.action_pending
            && started.elapsed() > ACTION_TIMEOUT
        {
            self.action_pending = None;
            self.set_status("操作超时，未收到结果", true);
        }

        if self.action_pending.is_none()
            && !self.status.is_empty()
            && self.status_at.elapsed() > Duration::from_secs(6)
        {
            self.status.clear();
            self.dirty = true;
        }
        // 有读取中/操作中的状态时持续重绘，让状态行保持最新。
        if self.action_pending.is_some()
            || self
                .batteries
                .values()
                .any(|battery| matches!(battery, Battery::Loading))
        {
            self.dirty = true;
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Battery { node, result } => {
                let state = match result {
                    Ok(status) => Battery::Ok {
                        pct: status.battery,
                        charging: status.charging,
                        hz: status.polling_hz,
                        dpi: status.dpi,
                        wireless: status.wireless,
                    },
                    Err(error) => Battery::Err(error),
                };
                if matches!(state, Battery::Ok { .. }) {
                    self.battery_tries.insert(node.clone(), 0);
                }
                self.batteries.insert(node, state);
                self.rebuild();
                self.dirty = true;
            }
            Msg::Action { seq, ok, text } => {
                // 超时后到达的旧结果不再覆盖当前状态。
                if self.action_pending.map(|(current, _)| current) != Some(seq) {
                    return;
                }
                self.action_pending = None;
                self.set_status(text, !ok);
                self.last_scan = Instant::now() - Duration::from_secs(2);
                self.request_scan();
            }
            Msg::Snapshot(snapshot) => {
                self.scan_busy = false;
                self.apply_snapshot(*snapshot);
            }
        }
    }

    fn request_scan(&mut self) {
        if self.scan_busy {
            return;
        }
        self.last_scan = Instant::now();
        if self.scan_tx.send(()).is_ok() {
            self.scan_busy = true;
        }
    }

    fn apply_snapshot(&mut self, snapshot: scan::Snapshot) {
        let signature = snapshot_signature(&snapshot);
        let unchanged = signature == self.snapshot_signature;
        let old_keys: HashSet<String> = self.snap.peripherals.iter().map(Peripheral::key).collect();
        let new_storage = snapshot
            .peripherals
            .iter()
            .find(|p| p.category == Category::Storage && !old_keys.contains(&p.key()))
            .map(Peripheral::key);

        self.snap = snapshot;
        self.snapshot_signature = signature;
        self.prune_state();

        if let Some(key) = new_storage
            && self.filter.is_empty()
        {
            self.selected_key = Some(key);
            self.set_status("检测到新的存储设备", false);
        }

        if !unchanged {
            self.rebuild();
            self.sync_selection();
        }
        self.last_scan = Instant::now();
        self.dirty = true;
    }

    /// 拔掉的设备不再保留电量/分区选择，避免缓存无限增长。
    fn prune_state(&mut self) {
        let present: HashSet<String> = self
            .snap
            .peripherals
            .iter()
            .flat_map(|peripheral| peripheral.hids.iter())
            .map(|hid| hid.node.clone())
            .collect();
        self.batteries.retain(|node, _| present.contains(node));
        self.battery_at.retain(|node, _| present.contains(node));
        self.battery_tries.retain(|node, _| present.contains(node));

        let devices: HashSet<String> = self.snap.peripherals.iter().map(Peripheral::key).collect();
        self.part_sel.retain(|key, _| devices.contains(key));
    }

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

/// 连续失败时退避：20s → 40s → 80s → 160s。
fn retry_interval(tries: u32) -> Duration {
    RETRY_AFTER * (1u32 << tries.min(3))
}

/// 只比较影响界面/操作的字段，避免每秒重建列表和详情。
fn snapshot_signature(snapshot: &scan::Snapshot) -> Vec<String> {
    snapshot
        .peripherals
        .iter()
        .map(|peripheral| {
            let mut text = format!(
                "{}|{:?}|{}|{}|{}|{}|{}|{}",
                peripheral.usb.sysfs,
                peripheral.category,
                peripheral.name(),
                peripheral.usb.place.label(),
                peripheral.usb.hub_kind.map(HubKind::label).unwrap_or(""),
                peripheral.hids.len(),
                peripheral.nets.len(),
                peripheral.audio.is_some()
            );
            for disk in &peripheral.disks {
                text.push_str(&format!("|{}{}", disk.name, disk.mount.is_some()));
                for partition in &disk.partitions {
                    text.push_str(&format!(
                        "|{}{}{}",
                        partition.name,
                        partition.fstype,
                        partition
                            .mount
                            .as_ref()
                            .map(|mount| mount.path.as_str())
                            .unwrap_or("")
                    ));
                }
            }
            text
        })
        .collect()
}

fn spawn_battery_worker(tx: Sender<Msg>) -> Sender<HidInfo> {
    let (request_tx, request_rx) = mpsc::channel::<HidInfo>();
    thread::spawn(move || {
        while let Ok(hid) = request_rx.recv() {
            // 每个设备独立线程：休眠设备超时不会拖住其他设备。
            let tx = tx.clone();
            thread::spawn(move || {
                let result = battery::read(&hid).map_err(|error| format!("{error:#}"));
                let _ = tx.send(Msg::Battery {
                    node: hid.node,
                    result,
                });
            });
        }
    });
    request_tx
}

fn spawn_scan_worker(tx: Sender<Msg>) -> Sender<()> {
    let (request_tx, request_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        while request_rx.recv().is_ok() {
            let snapshot = Box::new(scan::scan());
            if tx.send(Msg::Snapshot(snapshot)).is_err() {
                break;
            }
        }
    });
    request_tx
}

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
    use super::*;

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
