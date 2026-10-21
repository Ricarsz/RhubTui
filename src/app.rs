//! Application state: device list, selection, background battery reads and
//! mount actions.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::battery;
use crate::config::Config;
use crate::mchose::Status;
use crate::model::*;
use crate::mount;
use crate::scan;
use crate::theme::Palette;

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

pub struct Row {
    pub line: Line<'static>,
    pub device: Option<usize>,
}

impl Row {
    fn section(title: &str, palette: Palette) -> Self {
        Self {
            line: Line::from(Span::styled(
                format!(" {title} "),
                Style::default()
                    .fg(palette.surface)
                    .bg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            device: None,
        }
    }
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
    /// 列表里设备名可用的显示列数，随窗口宽度调整。
    pub name_cols: usize,
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
        let scan_interval = config.scan_interval();
        let battery_refresh = config.battery_refresh();
        let mouse = config.mouse;
        let theme = config.theme.clone();
        let (tx, rx) = mpsc::channel();
        let battery_tx = spawn_battery_worker(tx.clone());
        let scan_tx = spawn_scan_worker(tx.clone());
        let snap = scan::scan();
        let signature = snapshot_signature(&snap);
        let mut app = Self {
            snap,
            rows: Vec::new(),
            selected: 0,
            list_area: ratatui::layout::Rect::default(),
            details_area: ratatui::layout::Rect::default(),
            list_offset: 0,
            name_cols: 26,
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
        app.request_all_batteries();
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

    /// 窗口宽度变化时调整名称列，必要时重建带样式的行。
    pub fn set_name_width(&mut self, cols: usize) {
        let cols = cols.clamp(8, 34);
        if self.name_cols != cols {
            self.name_cols = cols;
            self.rebuild();
            self.dirty = true;
        }
    }

    fn rebuild(&mut self) {
        let key = self.selected_key.clone();
        let palette = self.palette();
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

            rows.push(Row::section(category.title(), palette));
            for index in indices {
                let line = self.device_line(index);
                rows.push(Row {
                    line,
                    device: Some(index),
                });
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

    fn device_line(&self, index: usize) -> Line<'static> {
        let peripheral = &self.snap.peripherals[index];
        let palette = self.palette();
        let mut spans = vec![Span::styled(
            truncate_cols(&peripheral.name(), self.name_cols),
            Style::default().fg(palette.text),
        )];
        if peripheral.usb.place.is_internal() {
            spans.push(Span::styled(" 内置", Style::default().fg(palette.faint)));
        } else if peripheral.usb.on_dock() {
            spans.push(Span::styled(" 坞上", Style::default().fg(palette.dim)));
        }
        if let Some((text, color)) = self.badge(peripheral) {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(text, Style::default().fg(color)));
        }
        Line::from(spans)
    }

    fn badge(&self, peripheral: &Peripheral) -> Option<(String, Color)> {
        let palette = self.palette();
        if let Some(hid) = peripheral.battery_hid() {
            match self.battery_of(hid) {
                Battery::Ok { pct, .. } => {
                    return Some((self.battery_of(hid).label(), battery_color(pct, palette)));
                }
                Battery::Loading => return Some(("读取中".into(), palette.faint)),
                Battery::Err(_) => return Some(("电量?".into(), palette.bad)),
                Battery::Unknown => {}
            }
        }

        if !peripheral.disks.is_empty() {
            let partitions: usize = peripheral.disks.iter().map(|d| d.partitions.len()).sum();
            if peripheral.disks.iter().any(Disk::mounted) {
                return Some(("已挂载".into(), palette.good));
            }
            if partitions > 0 {
                return Some(("未挂载".into(), palette.warn));
            }
            return Some(("无介质".into(), palette.faint));
        }

        if peripheral.usb.is_hub() {
            let text = match peripheral.usb.hub_kind.unwrap_or(HubKind::Hub) {
                HubKind::Dock => format!("扩展坞 · {} 口", peripheral.usb.maxchild),
                HubKind::Hub => {
                    let mut text = format!("{} 口", peripheral.usb.maxchild);
                    if !peripheral.usb.children.is_empty() {
                        text.push_str(&format!(" · {} 台", peripheral.usb.children.len()));
                    }
                    text
                }
            };
            return Some((text, palette.faint));
        }

        if peripheral.category == Category::Input
            && !peripheral.hids.is_empty()
            && peripheral.battery_hid().is_none()
        {
            return Some(("无电量".into(), palette.faint));
        }

        let functions = peripheral.functions();
        if !functions.is_empty() && peripheral.category != Category::Other {
            return Some((functions.join(" · "), palette.faint));
        }
        None
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
    pub fn details(&self) -> Vec<Line<'static>> {
        if self.advanced {
            self.details_advanced()
        } else {
            self.details_overview()
        }
    }

    fn details_advanced(&self) -> Vec<Line<'static>> {
        let palette = self.palette();
        let Some(peripheral) = self.selected_peripheral() else {
            return vec![Line::from(Span::styled(
                "没有检测到外设",
                Style::default().fg(palette.faint),
            ))];
        };

        let usb = &peripheral.usb;
        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            peripheral.name(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        let mut tags = vec![peripheral.category.title().to_string()];
        tags.push(usb.place.label().to_string());
        if usb.on_dock() {
            tags.push("经扩展坞".into());
        }
        lines.push(Line::from(Span::styled(
            tags.join(" · "),
            Style::default().fg(palette.accent),
        )));
        lines.push(Line::from(""));

        // 连接方式
        lines.push(section_title("连接", palette));
        lines.push(kv(
            "归属",
            format!("{} · {}", usb.place.label(), usb.place_reason),
        ));
        lines.push(kv("位置", usb.attachment()));
        if let Some(dock) = &usb.dock
            && usb.hub_kind != Some(HubKind::Dock)
        {
            lines.push(kv("扩展坞", dock.clone()));
        }
        if let Some(controller) = &usb.controller {
            lines.push(kv("控制器", controller.label()));
        }
        lines.push(kv("链路", self.chain_of(peripheral)));
        let version = if usb.usb_version.is_empty() {
            String::new()
        } else {
            format!("USB {} · ", usb.usb_version)
        };
        lines.push(kv("速率", format!("{version}{}", speed_label(&usb.speed))));
        lines.push(kv(
            "地址",
            format!(
                "bus {} · dev {} · {}（端口 {}）",
                usb.bus,
                usb.devnum,
                usb.sysfs,
                usb.port.unwrap_or(0)
            ),
        ));
        if let Some(power) = usb.power_ma {
            lines.push(kv("供电", format!("{power} mA（配置上限）")));
        }
        lines.push(kv("VID:PID", usb.id()));
        lines.push(kv(
            "类别",
            format!(
                "{:02x}/{:02x}/{:02x} {}",
                usb.class,
                usb.subclass,
                usb.protocol,
                usb.class_name(usb.class)
            ),
        ));
        if !usb.manufacturer.is_empty() {
            lines.push(kv("厂商", usb.manufacturer.clone()));
        }
        if !usb.product.is_empty() {
            lines.push(kv("型号", usb.product.clone()));
        }
        if !usb.bcd_device.is_empty() {
            lines.push(kv("版本", format!("bcdDevice {}", usb.bcd_device)));
        }
        if !usb.serial.is_empty() {
            lines.push(kv("序列号", usb.serial.clone()));
        }
        lines.push(kv(
            "配置",
            format!(
                "{} 个接口 · {} 组配置 · LTM {} · {}",
                usb.num_interfaces,
                usb.num_configs,
                if usb.ltm_capable {
                    "支持"
                } else {
                    "不支持"
                },
                if usb.authorized {
                    "已授权"
                } else {
                    "未授权"
                }
            ),
        ));

        if !usb.ifaces.is_empty() {
            lines.push(Line::from(""));
            lines.push(section_title("USB 接口", palette));
            for iface in &usb.ifaces {
                let mut text = format!(
                    "  {:<2} {} {:02x}/{:02x}/{:02x}  {} 端点",
                    iface.number,
                    pad_right(iface.function(), 12),
                    iface.class,
                    iface.subclass,
                    iface.protocol,
                    iface.endpoints
                );
                if !iface.driver.is_empty() {
                    text.push_str(&format!("  → {}", iface.driver));
                }
                lines.push(Line::from(text));
            }
        }

        if !peripheral.hids.is_empty() {
            lines.push(Line::from(""));
            lines.push(section_title("HID", palette));
            for hid in &peripheral.hids {
                let iface = hid
                    .iface
                    .map(|iface| format!("iface {iface}"))
                    .unwrap_or_else(|| "iface ?".into());
                let pages: Vec<String> = hid
                    .pages
                    .iter()
                    .map(|page| format!("0x{page:04X} {}", page_name(*page)))
                    .collect();
                lines.push(Line::from(format!(
                    "  {iface} · {} · {}",
                    hid.node,
                    hid.protocol_name()
                )));
                lines.push(Line::from(Span::styled(
                    format!("       {}", pages.join(" ")),
                    Style::default().fg(palette.dim),
                )));
                if hid.has_battery() {
                    lines.push(self.battery_lines(hid, palette));
                }
            }
        }

        if !peripheral.disks.is_empty() {
            lines.push(Line::from(""));
            lines.push(section_title("存储", palette));
            let selected = self.part_index(peripheral);
            let mut flat = 0;
            for disk in &peripheral.disks {
                let size = if disk.size == 0 {
                    "无介质".to_string()
                } else {
                    human_size(disk.size)
                };
                let mut header = format!("  {}  {}", disk.path, size);
                if !disk.model.is_empty() {
                    header.push_str(&format!("  {}", disk.model));
                }
                if !disk.ptable.is_empty() {
                    header.push_str(&format!("  {}", disk.ptable.to_uppercase()));
                }
                if disk.size > 0 {
                    header.push_str(if disk.rotational {
                        "  · 机械"
                    } else {
                        "  · 固态"
                    });
                }
                if !disk.transport.is_empty() {
                    header.push_str(&format!("  · {}", disk.transport));
                }
                if disk.removable {
                    header.push_str("  · 可移动");
                }
                lines.push(Line::from(header));
                if !disk.serial.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("      序列号 {}", disk.serial),
                        Style::default().fg(palette.faint),
                    )));
                }
                if let Some(mount) = &disk.mount {
                    lines.push(kv_indent("挂载", mount_label(mount), palette));
                }
                for partition in &disk.partitions {
                    let marker = if flat == selected { "▸" } else { " " };
                    let mut text = format!(
                        "  {marker} {}  {}",
                        partition.name,
                        human_size(partition.size)
                    );
                    if !partition.fstype.is_empty() {
                        text.push_str(&format!("  {}", partition.fstype));
                    }
                    if !partition.label.is_empty() {
                        text.push_str(&format!("  {}", partition.label));
                    }
                    if let Some(mount) = &partition.mount {
                        text.push_str(&format!("  → {}", mount.path));
                        if mount.read_only {
                            text.push_str(" (ro)");
                        }
                    }
                    let style = if flat == selected {
                        Style::default().fg(palette.accent)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(text, style)));
                    if !partition.uuid.is_empty() {
                        lines.push(Line::from(Span::styled(
                            format!("      UUID {}", partition.uuid),
                            Style::default().fg(palette.faint),
                        )));
                    }
                    if let Some(mount) = &partition.mount
                        && let (Some(pct), Some(label)) = (mount.used_pct(), mount.usage_label())
                    {
                        lines.push(Line::from(Span::styled(
                            format!("      {}  {label}", usage_bar(pct, 8)),
                            Style::default().fg(usage_color(pct, palette)),
                        )));
                    }
                    flat += 1;
                }
            }
            if peripheral.disks.iter().all(|d| d.partitions.is_empty()) {
                let hint = if peripheral.disks.iter().any(|d| d.size > 0) {
                    "  整盘无分区表 · m 直接挂载整盘"
                } else {
                    "  没有分区（未插介质？）"
                };
                lines.push(Line::from(Span::styled(
                    hint,
                    Style::default().fg(palette.faint),
                )));
            }
            lines.push(Line::from(Span::styled(
                "  h/l 选分区 · m 挂载 · u 卸载 · e 弹出",
                Style::default().fg(palette.faint),
            )));
        }

        if let Some(audio) = &peripheral.audio {
            lines.push(Line::from(""));
            lines.push(section_title("音频", palette));
            lines.push(kv(
                "声卡",
                format!(
                    "card {} [{}] · {} · {}",
                    audio.index, audio.id, audio.driver, audio.name
                ),
            ));
            for pcm in &audio.pcms {
                lines.push(Line::from(format!("  {pcm}")));
            }
        }

        if !peripheral.nets.is_empty() {
            lines.push(Line::from(""));
            lines.push(section_title("网络", palette));
            for net in &peripheral.nets {
                let mut text = net.label();
                if !net.driver.is_empty() {
                    text.push_str(&format!(" · {}", net.driver));
                }
                lines.push(Line::from(format!("  {text}")));
            }
        }

        let mut nodes = Vec::new();
        if !peripheral.videos.is_empty() {
            nodes.push(("视频", peripheral.videos.join(" ")));
        }
        if !peripheral.ttys.is_empty() {
            nodes.push(("串口", peripheral.ttys.join(" ")));
        }
        if !peripheral.printers.is_empty() {
            nodes.push(("打印", peripheral.printers.join(" ")));
        }
        if !nodes.is_empty() {
            lines.push(Line::from(""));
            for (label, value) in nodes {
                lines.push(kv(label, value));
            }
        }

        if usb.is_hub() {
            lines.push(Line::from(""));
            lines.push(section_title("Hub", palette));
            lines.push(kv(
                "形态",
                format!(
                    "{} · {} 个下行口",
                    usb.hub_kind.unwrap_or(HubKind::Hub).label(),
                    usb.maxchild
                ),
            ));
            if !usb.children.is_empty() {
                let names: Vec<String> = usb
                    .children
                    .iter()
                    .map(|child| self.name_of(child))
                    .collect();
                lines.push(kv("下挂", names.join("、")));
            }
        }

        lines
    }

    /// 面向普通用户的概览：设备名、电量、连接方式，以及少量关键信息。
    fn details_overview(&self) -> Vec<Line<'static>> {
        let palette = self.palette();
        let Some(peripheral) = self.selected_peripheral() else {
            return vec![Line::from(Span::styled(
                "没有检测到外设",
                Style::default().fg(palette.faint),
            ))];
        };

        let usb = &peripheral.usb;
        let battery_hid = peripheral.battery_hid();
        let wireless = battery_hid.and_then(|hid| self.battery_of(hid).wireless());
        let connection = match wireless {
            Some(true) => Some("2.4G 无线"),
            Some(false) => Some("USB 有线"),
            None => peripheral.connection(),
        };

        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            peripheral.name(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        let mut tags = vec![peripheral.kind_label().to_string()];
        if let Some(connection) = connection {
            tags.push(connection.to_string());
        }
        tags.push(usb.place.label().to_string());
        if usb.on_dock() {
            tags.push("经扩展坞".into());
        }
        lines.push(Line::from(Span::styled(
            tags.join(" · "),
            Style::default().fg(palette.accent),
        )));
        lines.push(Line::from(""));

        if let Some(hid) = battery_hid {
            lines.push(section_title("电量", palette));
            match self.battery_of(hid) {
                Battery::Ok { pct, charging, .. } => {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            battery_bar(pct, 16),
                            Style::default()
                                .fg(battery_color(pct, palette))
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    let (state, color) = if charging {
                        ("充电中", palette.good)
                    } else {
                        ("使用中", palette.dim)
                    };
                    lines.push(Line::from(Span::styled(
                        format!("  {state}"),
                        Style::default().fg(color),
                    )));
                }
                Battery::Loading => lines.push(Line::from(Span::styled(
                    "  读取中…",
                    Style::default().fg(palette.faint),
                ))),
                Battery::Unknown => lines.push(Line::from(Span::styled(
                    "  未检测到电量通道",
                    Style::default().fg(palette.faint),
                ))),
                Battery::Err(error) => lines.push(Line::from(Span::styled(
                    format!("  读取失败：{error}"),
                    Style::default().fg(palette.bad),
                ))),
            }
            lines.push(Line::from(""));
        }

        let mut rows: Vec<(&str, String)> = Vec::new();
        rows.push(("类型", peripheral.kind_label().to_string()));
        if let Some(connection) = connection {
            rows.push(("连接", connection.to_string()));
        }
        if let Some(dpi) = battery_hid.and_then(|hid| self.battery_of(hid).dpi()) {
            rows.push(("DPI档位", format!("{dpi} 档")));
        }
        if let Some(hz) = battery_hid.and_then(|hid| self.battery_of(hid).hz())
            && wireless != Some(true)
        {
            rows.push(("回报率", format!("{hz} Hz")));
        }
        rows.push(("位置", usb.friendly_attachment()));

        if !peripheral.disks.is_empty() {
            let partitions: usize = peripheral
                .disks
                .iter()
                .map(|disk| disk.partitions.len())
                .sum();
            let mounted = peripheral.disks.iter().any(Disk::mounted);
            rows.push((
                "存储",
                format!(
                    "{partitions} 个分区 · {}",
                    if mounted { "已挂载" } else { "未挂载" }
                ),
            ));
            let mount_point = peripheral.disks.iter().find_map(|disk| {
                disk.mount
                    .as_ref()
                    .or_else(|| disk.partitions.iter().find_map(|part| part.mount.as_ref()))
            });
            if let Some(mount) = mount_point {
                let mut value = mount.path.clone();
                if let Some(usage) = mount.usage_label() {
                    value.push_str(&format!(" · {usage}"));
                }
                if mount.read_only {
                    value.push_str(" (ro)");
                }
                rows.push(("挂载", value));
            }
        }

        if let Some(audio) = &peripheral.audio {
            rows.push(("音频", format!("card {} · {}", audio.index, audio.name)));
        }
        if let Some(net) = peripheral.nets.first() {
            rows.push(("网络", net.label()));
        }
        if usb.is_hub() {
            rows.push((
                "扩展坞",
                format!(
                    "{} · {} 台下挂",
                    usb.hub_kind.unwrap_or(HubKind::Hub).label(),
                    usb.children.len()
                ),
            ));
        }

        if !rows.is_empty() {
            lines.push(section_title("关键信息", palette));
            for (key, value) in rows {
                lines.push(kv(key, value));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  a 查看高级详情 · b 重新读取电量 · r 重新扫描",
            Style::default().fg(palette.faint),
        )));
        lines
    }

    fn chain_of(&self, peripheral: &Peripheral) -> String {
        let usb = &peripheral.usb;
        let mut parts = Vec::new();
        if let Some(controller) = &usb.controller {
            parts.push(controller.slot.clone());
        }
        let root_port = usb
            .root_port
            .map(|port| format!("根端口 {port}"))
            .unwrap_or_else(|| "根端口".into());
        parts.push(root_port);
        for ancestor in &usb.chain {
            parts.push(format!("{}「{}」", ancestor, self.name_of(ancestor)));
        }
        parts.push(usb.sysfs.clone());
        parts.join(" → ")
    }

    fn name_of(&self, sysfs: &str) -> String {
        self.snap
            .peripherals
            .iter()
            .find(|peripheral| peripheral.usb.sysfs == sysfs)
            .map(Peripheral::name)
            .unwrap_or_else(|| sysfs.to_string())
    }

    fn battery_lines(&self, hid: &HidInfo, palette: Palette) -> Line<'static> {
        let mut spans = vec![Span::styled(
            "      电量 ",
            Style::default().fg(palette.dim),
        )];
        match self.battery_of(hid) {
            Battery::Ok {
                pct, charging, hz, ..
            } => {
                spans.push(Span::styled(
                    battery_bar(pct, 10),
                    Style::default().fg(battery_color(pct, palette)),
                ));
                let mut extra = Vec::new();
                if charging {
                    extra.push("充电中".to_string());
                }
                if let Some(hz) = hz {
                    extra.push(format!("{hz} Hz"));
                }
                if !extra.is_empty() {
                    spans.push(Span::styled(
                        format!("  {}", extra.join(" · ")),
                        Style::default().fg(palette.dim),
                    ));
                }
            }
            Battery::Loading => {
                spans.push(Span::styled("读取中…", Style::default().fg(palette.faint)))
            }
            Battery::Unknown => spans.push(Span::styled(
                "—（该接口无已知协议）",
                Style::default().fg(palette.faint),
            )),
            Battery::Err(error) => {
                spans.push(Span::styled(
                    format!("读取失败：{error}"),
                    Style::default().fg(palette.bad),
                ));
                if error.contains("Permission denied") {
                    spans.push(Span::styled(
                        "（见 README 的 udev 规则）",
                        Style::default().fg(palette.faint),
                    ));
                }
            }
        }
        Line::from(spans)
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// 连续失败时退避：20s → 40s → 80s → 160s。
fn retry_interval(tries: u32) -> Duration {
    RETRY_AFTER * (1u32 << tries.min(3))
}

fn section_title(title: &'static str, palette: Palette) -> Line<'static> {
    Line::from(Span::styled(
        title,
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
    ))
}

fn pad_right(text: &str, width: usize) -> String {
    let mut text = text.to_string();
    let used = UnicodeWidthStr::width(text.as_str());
    if used < width {
        text.push_str(&" ".repeat(width - used));
    }
    text
}

fn kv(key: &str, value: String) -> Line<'static> {
    kv_styled(key, value, Color::Reset)
}

fn kv_styled(key: &str, value: String, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{} ", pad_right(key, 8)),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(value, Style::default().fg(color)),
    ])
}

fn kv_indent(key: &str, value: String, palette: Palette) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("      {} ", pad_right(key, 5)),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(value, Style::default().fg(palette.dim)),
    ])
}

fn mount_label(mount: &MountPoint) -> String {
    let mut text = mount.path.clone();
    if mount.read_only {
        text.push_str(" (ro)");
    }
    if let Some(usage) = mount.usage_label() {
        text.push_str(&format!(" · {usage}"));
    }
    text
}

fn page_name(page: u16) -> &'static str {
    match page {
        0x0001 => "generic",
        0x0002 => "simulation",
        0x0005 => "game",
        0x0006 => "device",
        0x0007 => "keyboard",
        0x0008 => "led",
        0x0009 => "button",
        0x000A => "ordinal",
        0x000B => "telephony",
        0x000C => "consumer",
        0x000D => "digitizer",
        0x000E => "haptics",
        0x0012 => "oscilloscope",
        0x0014 => "barcode",
        0x0020 => "sensors",
        0x0084 => "power",
        0xFF00..=0xFFFF => "vendor",
        _ => "",
    }
}

fn battery_color(pct: u8, palette: Palette) -> Color {
    if pct <= 20 {
        palette.bad
    } else if pct <= 40 {
        palette.warn
    } else {
        palette.good
    }
}

fn usage_color(pct: u8, palette: Palette) -> Color {
    if pct >= 90 {
        palette.bad
    } else if pct >= 75 {
        palette.warn
    } else {
        palette.good
    }
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

    #[test]
    fn pads_by_display_width() {
        assert_eq!(pad_right("abc", 5), "abc  ");
        assert_eq!(pad_right("中文", 6), "中文  ");
        assert_eq!(pad_right("abcdef", 3), "abcdef");
    }
}
