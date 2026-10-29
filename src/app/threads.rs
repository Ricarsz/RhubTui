//! 后台扫描线程、消息处理与周期性刷新。

use std::collections::HashSet;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::model::*;
use crate::scan;

use super::{ACTION_TIMEOUT, App};

pub(crate) enum Msg {
    Action { seq: u64, ok: bool, text: String },
    Snapshot(Box<scan::Snapshot>),
}

impl App {
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
        // 有操作进行中时持续重绘，让状态行保持最新。
        if self.action_pending.is_some() {
            self.dirty = true;
        }
    }

    pub(crate) fn handle(&mut self, msg: Msg) {
        match msg {
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

    /// 拔掉的设备不再保留分区选择，避免缓存无限增长。
    pub(crate) fn prune_state(&mut self) {
        let devices: HashSet<String> = self.snap.peripherals.iter().map(Peripheral::key).collect();
        self.part_sel.retain(|key, _| devices.contains(key));
    }
}

/// 只比较影响界面/操作的字段，避免每秒重建列表和详情。
pub(crate) fn snapshot_signature(snapshot: &scan::Snapshot) -> Vec<String> {
    snapshot
        .peripherals
        .iter()
        .map(|peripheral| {
            let mut text = format!(
                "{}|{:?}|{}|{}|{}|{}|{}",
                peripheral.usb.sysfs,
                peripheral.category,
                peripheral.name(),
                peripheral.usb.place.label(),
                peripheral.usb.hub_kind.map(HubKind::label).unwrap_or(""),
                peripheral.hids.len(),
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
