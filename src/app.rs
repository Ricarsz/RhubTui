//! Application state: device list, selection, background battery reads and
//! mount actions.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::battery;
use crate::config::Config;
use crate::mchose::Status;
use crate::model::*;
use crate::mount;
use crate::scan;
use crate::theme::Palette;

pub enum Msg {
    Battery {
        node: String,
        result: Result<Status, String>,
    },
    Action {
        ok: bool,
        text: String,
    },
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

pub struct App {
    pub snap: scan::Snapshot,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub selected_key: Option<String>,
    pub batteries: HashMap<String, Battery>,
    pub part_sel: HashMap<String, usize>,
    pub status: String,
    pub status_error: bool,
    pub status_at: Instant,
    pub quitting: bool,
    pub show_help: bool,
    pub detail_scroll: u16,
    pub show_internal: bool,
    action_pending: bool,
    last_scan: Instant,
    last_battery_refresh: Instant,
    scan_interval: Duration,
    battery_refresh: Duration,
    pub theme: String,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    battery_tx: Sender<HidInfo>,
}

impl App {
    pub fn new(config: Config) -> Self {
        let (tx, rx) = mpsc::channel();
        let battery_tx = spawn_battery_worker(tx.clone());
        let snap = scan::scan();

        let mut app = Self {
            snap,
            rows: Vec::new(),
            selected: 0,
            selected_key: None,
            batteries: HashMap::new(),
            part_sel: HashMap::new(),
            status: String::new(),
            status_error: false,
            status_at: Instant::now(),
            quitting: false,
            show_help: false,
            detail_scroll: 0,
            show_internal: true,
            action_pending: false,
            last_scan: Instant::now(),
            last_battery_refresh: Instant::now(),
            scan_interval: config.scan_interval(),
            battery_refresh: config.battery_refresh(),
            theme: config.theme,
            tx,
            rx,
            battery_tx,
        };

        app.rebuild();
        app.sync_selection();
        app.request_all_batteries();
        app
    }

    fn refresh_ok_batteries(&mut self) {
        let nodes: Vec<String> = self
            .batteries
            .iter()
            .filter(|(_, battery)| matches!(battery, Battery::Ok { .. }))
            .map(|(node, _)| node.clone())
            .collect();
        for node in nodes {
            self.batteries.remove(&node);
        }
        self.request_all_batteries();
    }

    fn request_all_batteries(&mut self) {
        for peripheral in &self.snap.peripherals {
            let Some(hid) = &peripheral.hid else { continue };
            if self.batteries.contains_key(&hid.node) {
                continue;
            }
            if battery::supported(hid) {
                self.batteries.insert(hid.node.clone(), Battery::Loading);
                let _ = self.battery_tx.send(hid.clone());
            } else if !hid.mchose {
                self.batteries
                    .insert(hid.node.clone(), Battery::Unsupported);
            }
        }
    }

    // ─── state ─────────────────────────────────────────────────────────────

    pub fn palette(&self) -> Palette {
        Palette::from_name(&self.theme)
    }

    pub fn visible_count(&self) -> usize {
        self.rows.iter().filter(|row| row.device.is_some()).count()
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
    }

    pub fn status_line(&self) -> (&str, Color) {
        let palette = self.palette();
        if self.status.is_empty() {
            ("就绪", palette.faint)
        } else if self.status_error {
            (self.status.as_str(), palette.bad)
        } else {
            (self.status.as_str(), palette.good)
        }
    }

    // ─── list building ─────────────────────────────────────────────────────

    fn rebuild(&mut self) {
        let key = self.selected_key.clone();
        let palette = self.palette();
        let mut rows = Vec::new();

        for category in Category::ALL {
            let mut indices: Vec<usize> = self
                .snap
                .peripherals
                .iter()
                .enumerate()
                .filter(|(_, p)| p.category == category)
                .filter(|(_, p)| self.show_internal || !p.usb.internal)
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
            .position(|r| r.device.is_some())
            .unwrap_or(0);
    }

    fn device_line(&self, index: usize) -> Line<'static> {
        let peripheral = &self.snap.peripherals[index];
        let mut spans = vec![Span::styled(
            truncate(&peripheral.name(), 30),
            Style::default().fg(self.palette().text),
        )];
        if peripheral.usb.internal {
            spans.push(Span::styled(
                " 内置",
                Style::default().fg(self.palette().faint),
            ));
        }
        if let Some((text, color)) = self.badge(peripheral) {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(text, Style::default().fg(color)));
        }
        Line::from(spans)
    }

    fn badge(&self, peripheral: &Peripheral) -> Option<(String, Color)> {
        let palette = self.palette();
        if let Some(hid) = &peripheral.hid {
            match self.battery_of(hid) {
                Battery::Ok { pct, .. } => {
                    return Some((self.battery_of(hid).label(), battery_color(pct, palette)));
                }
                Battery::Loading => return Some(("读取中".into(), palette.faint)),
                Battery::Err(_) => return Some(("电量?".into(), palette.bad)),
                Battery::Unsupported if peripheral.category == Category::Input => {
                    return Some(("无电量".into(), palette.faint));
                }
                _ => {}
            }
            if hid.mchose {
                return Some(("…".into(), palette.faint));
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
            let mut text = format!("{} 端口", peripheral.usb.maxchild);
            if !peripheral.usb.children.is_empty() {
                text.push_str(&format!(" · {} 台", peripheral.usb.children.len()));
            }
            return Some((text, palette.faint));
        }
        None
    }

    // ─── selection ─────────────────────────────────────────────────────────

    fn sync_selection(&mut self) {
        self.selected_key = self.selected_peripheral().map(Peripheral::key);
        self.detail_scroll = 0;
        self.request_battery();
    }

    fn request_battery(&mut self) {
        let hid = match self.selected_peripheral() {
            Some(peripheral) => peripheral.hid.clone(),
            None => return,
        };
        let Some(hid) = hid else { return };

        if self.batteries.contains_key(&hid.node) {
            return;
        }
        if battery::supported(&hid) {
            self.batteries.insert(hid.node.clone(), Battery::Loading);
            let _ = self.battery_tx.send(hid);
        } else if !hid.mchose {
            self.batteries.insert(hid.node, Battery::Unsupported);
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
                self.selected = index as usize;
                self.sync_selection();
                return;
            }
        }
    }

    fn move_to_edge(&mut self, last: bool) {
        let position = if last {
            self.rows.iter().rposition(|r| r.device.is_some())
        } else {
            self.rows.iter().position(|r| r.device.is_some())
        };
        if let Some(position) = position {
            self.selected = position;
            self.sync_selection();
        }
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
            self.set_status("该设备没有分区", true);
            return;
        }
        let next = (self.part_index(&peripheral) as isize + delta).rem_euclid(total as isize);
        self.part_sel.insert(peripheral.key(), next as usize);
    }

    fn mount_selected(&mut self) {
        if self.action_pending {
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

        let tx = self.tx.clone();
        self.set_status(format!("挂载 {device} …"), false);
        self.action_pending = true;

        thread::spawn(move || {
            let (ok, text) = match mount::mount(&device, &label) {
                Ok(point) => (true, format!("已挂载 {device} → {point}")),
                Err(error) => (false, format!("挂载失败：{error:#}")),
            };
            let _ = tx.send(Msg::Action { ok, text });
        });
    }

    fn unmount_selected(&mut self) {
        if self.action_pending {
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
        let tx = self.tx.clone();
        self.set_status(format!("卸载 {device} …"), false);
        self.action_pending = true;

        thread::spawn(move || {
            let (ok, text) = match mount::unmount(&device) {
                Ok(()) => (true, format!("已卸载 {device}")),
                Err(error) => (false, format!("卸载失败：{error:#}")),
            };
            let _ = tx.send(Msg::Action { ok, text });
        });
    }

    fn eject_selected(&mut self) {
        if self.action_pending {
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

        let device = disk.path.clone();
        let tx = self.tx.clone();
        self.set_status(format!("弹出 {device} …"), false);
        self.action_pending = true;

        thread::spawn(move || {
            let (ok, text) = match mount::power_off(&device) {
                Ok(()) => (true, format!("已安全弹出 {device}")),
                Err(error) => (false, format!("弹出失败：{error:#}")),
            };
            let _ = tx.send(Msg::Action { ok, text });
        });
    }

    // ─── event handling ────────────────────────────────────────────────────

    pub fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Home | KeyCode::Char('g') => self.move_to_edge(false),
            KeyCode::End | KeyCode::Char('G') => self.move_to_edge(true),
            KeyCode::Left | KeyCode::Char('h') => self.cycle_partition(-1),
            KeyCode::Right | KeyCode::Char('l') => self.cycle_partition(1),
            KeyCode::Char('m') => self.mount_selected(),
            KeyCode::Char('u') => self.unmount_selected(),
            KeyCode::Char('e') => self.eject_selected(),
            KeyCode::PageDown => {
                let max = self.details().len().saturating_sub(1) as u16;
                self.detail_scroll = (self.detail_scroll + 5).min(max);
            }
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(5),
            KeyCode::Char('b') => {
                self.batteries.clear();
                self.request_all_batteries();
                self.last_battery_refresh = Instant::now();
                self.set_status("重新读取电量", false);
            }
            KeyCode::Char('r') => {
                self.rescan();
                self.set_status("已刷新", false);
            }
            KeyCode::Char('i') => {
                self.show_internal = !self.show_internal;
                self.rebuild();
                let text = if self.show_internal {
                    "显示内置设备"
                } else {
                    "隐藏内置设备"
                };
                self.set_status(text, false);
            }
            KeyCode::Char('?') => self.show_help = !self.show_help,
            KeyCode::Char('q') | KeyCode::Esc => self.quitting = true,
            _ => {}
        }
    }

    pub fn tick(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }

        if self.last_scan.elapsed() >= self.scan_interval {
            self.rescan();
        }
        // Keep known battery values fresh without waking devices that failed.
        if self.last_battery_refresh.elapsed() >= self.battery_refresh {
            self.refresh_ok_batteries();
            self.last_battery_refresh = Instant::now();
        }
        if !self.status.is_empty() && self.status_at.elapsed() > Duration::from_secs(6) {
            self.status.clear();
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
                    },
                    Err(error) => Battery::Err(error),
                };
                self.batteries.insert(node, state);
                self.rebuild();
            }
            Msg::Action { ok, text } => {
                self.action_pending = false;
                self.set_status(text, !ok);
                self.last_scan = Instant::now() - Duration::from_secs(2);
            }
        }
    }

    fn rescan(&mut self) {
        let old: Vec<String> = self.snap.peripherals.iter().map(Peripheral::key).collect();
        let snapshot = scan::scan();

        let new_storage = snapshot
            .peripherals
            .iter()
            .find(|p| p.category == Category::Storage && !old.contains(&p.key()))
            .map(Peripheral::key);

        self.snap = snapshot;

        if let Some(key) = new_storage {
            self.selected_key = Some(key);
            self.set_status("检测到新的存储设备", false);
        }

        self.rebuild();
        self.sync_selection();
        self.last_scan = Instant::now();
    }

    // ─── details pane ──────────────────────────────────────────────────────

    pub fn details(&self) -> Vec<Line<'static>> {
        let palette = self.palette();
        let Some(peripheral) = self.selected_peripheral() else {
            return vec![Line::from(Span::styled(
                "没有检测到外设",
                Style::default().fg(palette.faint),
            ))];
        };

        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            peripheral.name(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            peripheral.category.title(),
            Style::default().fg(palette.accent),
        )));
        lines.push(Line::from(""));

        lines.push(kv("VID:PID", peripheral.usb.id()));
        lines.push(kv(
            "归属",
            if peripheral.usb.internal {
                "内置（固定端口）".to_string()
            } else {
                "外接".to_string()
            },
        ));
        if !peripheral.usb.manufacturer.is_empty() {
            lines.push(kv("厂商", peripheral.usb.manufacturer.clone()));
        }
        if !peripheral.usb.product.is_empty() {
            lines.push(kv("型号", peripheral.usb.product.clone()));
        }
        lines.push(kv(
            "位置",
            format!(
                "bus {} · dev {} · {}",
                peripheral.usb.bus, peripheral.usb.devnum, peripheral.usb.sysfs
            ),
        ));
        if let Some(port) = peripheral.usb.port {
            let parent = peripheral
                .usb
                .parent_name
                .clone()
                .unwrap_or_else(|| format!("总线 {}", peripheral.usb.bus));
            lines.push(kv("端口", format!("{parent} · {port} 号口")));
        }
        let version = if peripheral.usb.usb_version.is_empty() {
            String::new()
        } else {
            format!("USB {} · ", peripheral.usb.usb_version)
        };
        lines.push(kv(
            "速率",
            format!("{version}{}", speed_label(&peripheral.usb.speed)),
        ));
        if let Some(power) = peripheral.usb.power_ma {
            lines.push(kv("供电", format!("{power} mA")));
        }
        if !peripheral.usb.serial.is_empty() {
            lines.push(kv("序列号", peripheral.usb.serial.clone()));
        }
        if peripheral.usb.is_hub() {
            lines.push(kv("端口数", peripheral.usb.maxchild.to_string()));
            if !peripheral.usb.children.is_empty() {
                let names: Vec<String> = peripheral
                    .usb
                    .children
                    .iter()
                    .map(|child| {
                        self.snap
                            .peripherals
                            .iter()
                            .find(|p| &p.usb.sysfs == child)
                            .map(|p| p.name())
                            .unwrap_or_else(|| child.clone())
                    })
                    .collect();
                lines.push(kv("下挂", names.join("、")));
            }
        }

        if let Some(hid) = &peripheral.hid {
            let pages: Vec<String> = hid.pages.iter().map(|p| format!("0x{p:04X}")).collect();
            lines.push(kv("HID", format!("{} · {}", hid.node, pages.join(" "))));
            match self.battery_of(hid) {
                Battery::Ok { pct, charging, hz } => {
                    let mut text = format!("{pct}%");
                    if charging {
                        text.push_str(" · 充电中");
                    }
                    if let Some(hz) = hz {
                        text.push_str(&format!(" · {hz} Hz"));
                    }
                    lines.push(kv_styled("电量", text, battery_color(pct, palette)));
                }
                Battery::Loading => lines.push(kv("电量", "读取中…".into())),
                Battery::Unsupported => lines.push(kv("电量", "—（该接口无已知协议）".into())),
                Battery::Err(error) => {
                    let permission = error.contains("Permission denied");
                    lines.push(kv_styled("电量", format!("读取失败：{error}"), palette.bad));
                    if permission {
                        lines.push(Line::from(Span::styled(
                            "  需要 /dev/hidraw* 权限，见 README 的 udev 规则",
                            Style::default().fg(palette.faint),
                        )));
                    }
                }
                Battery::Unknown => lines.push(kv("电量", "—".into())),
            }
        }

        if !peripheral.usb.ifaces.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "接口",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            for iface in &peripheral.usb.ifaces {
                let driver = if iface.driver.is_empty() {
                    "—".to_string()
                } else {
                    iface.driver.clone()
                };
                lines.push(Line::from(format!(
                    "  {}  {:<8} {}",
                    iface.number,
                    peripheral.usb.class_name(iface.class),
                    driver
                )));
            }
        }

        if !peripheral.disks.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "存储",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            let selected = self.part_index(peripheral);
            let mut flat = 0;
            for disk in &peripheral.disks {
                let mut header = format!("  {}  {}", disk.path, human_size(disk.size));
                if !disk.model.is_empty() {
                    header.push_str(&format!("  {}", disk.model));
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
                    lines.push(Line::from(format!("      挂载点 {}", mount.path)));
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
                "  h/l 选分区 · m 挂载 · u 卸载",
                Style::default().fg(palette.faint),
            )));
        }

        if let Some(audio) = &peripheral.audio {
            lines.push(Line::from(""));
            lines.push(kv(
                "音频",
                format!(
                    "card {} [{}] · {} · {}",
                    audio.index, audio.id, audio.driver, audio.name
                ),
            ));
        }

        lines
    }
}

fn kv(key: &str, value: String) -> Line<'static> {
    kv_styled(key, value, Color::Reset)
}

fn kv_styled(key: &str, value: String, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<8}"), Style::default().fg(Color::DarkGray)),
        Span::styled(value, Style::default().fg(color)),
    ])
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

/// 启动 TUI。配置在启动时读取一次。
pub fn run(config: Config) -> Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new(config);

    let result = loop {
        terminal.draw(|frame| crate::ui::draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        app.quitting = true;
                    } else {
                        app.on_key(key.code);
                    }
                }
                _ => {}
            }
        }

        app.tick();
        if app.quitting {
            break Ok(());
        }
    };

    ratatui::restore();
    result
}
