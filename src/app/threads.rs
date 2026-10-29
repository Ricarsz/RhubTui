//! 后台线程、消息处理与周期性刷新。

use std::collections::HashSet;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::battery;
use crate::mchose::Status;
use crate::model::*;
use crate::scan;

use super::{ACTION_TIMEOUT, App, LOADING_TIMEOUT, RETRY_AFTER};

pub(crate) enum Msg {
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

impl App {
    pub(crate) fn request_battery(&mut self) {
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

    pub(crate) fn queue_battery(&mut self, hid: &HidInfo) {
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

    pub(crate) fn request_all_batteries(&mut self) {
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

    pub(crate) fn refresh_due_batteries(&mut self) {
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

    /// 开始一个后台操作，返回本次序号（用于识别过期结果）。
    pub(crate) fn start_action(&mut self, text: String) -> u64 {
        self.action_seq = self.action_seq.wrapping_add(1);
        self.action_pending = Some((self.action_seq, Instant::now()));
        self.set_status(text, false);
        self.dirty = true;
        self.action_seq
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

    pub(crate) fn handle(&mut self, msg: Msg) {
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

    pub(crate) fn request_scan(&mut self) {
        if self.scan_busy {
            return;
        }
        self.last_scan = Instant::now();
        if self.scan_tx.send(()).is_ok() {
            self.scan_busy = true;
        }
    }

    pub(crate) fn apply_snapshot(&mut self, snapshot: scan::Snapshot) {
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
    pub(crate) fn prune_state(&mut self) {
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
}

/// 连续失败时退避：20s → 40s → 80s → 160s。
pub(crate) fn retry_interval(tries: u32) -> Duration {
    RETRY_AFTER * (1u32 << tries.min(3))
}

/// 只比较影响界面/操作的字段，避免每秒重建列表和详情。
pub(crate) fn snapshot_signature(snapshot: &scan::Snapshot) -> Vec<String> {
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

pub(crate) fn spawn_battery_worker(tx: Sender<Msg>) -> Sender<HidInfo> {
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

pub(crate) fn spawn_scan_worker(tx: Sender<Msg>) -> Sender<()> {
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
