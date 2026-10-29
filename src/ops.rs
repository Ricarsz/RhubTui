//! 挂载/卸载/弹出的目标解析。TUI 与 CLI 共用同一套语义，避免两边各写一遍。

use anyhow::{Result, bail};

use crate::model::{Disk, Peripheral};
use crate::scan::Snapshot;

/// 一次挂载的目标：设备节点 + 给用户看的标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountTarget {
    pub device: String,
    pub label: String,
}

/// 设备的所有分区，按磁盘顺序摊平。
pub fn partitions(peripheral: &Peripheral) -> Vec<&crate::model::Partition> {
    peripheral
        .disks
        .iter()
        .flat_map(|disk| disk.partitions.iter())
        .collect()
}

fn partition_label(partition: &crate::model::Partition) -> String {
    if partition.label.is_empty() {
        partition.name.clone()
    } else {
        partition.label.clone()
    }
}

fn disk_label(disk: &Disk) -> String {
    if disk.model.is_empty() {
        disk.name.clone()
    } else {
        disk.model.clone()
    }
}

pub fn matches_device(selector: &str, path: &str, name: &str) -> bool {
    let selector = selector.trim();
    selector == path || selector == name || selector.trim_start_matches("/dev/") == name
}

/// TUI：当前选中分区（或整盘）的挂载目标。
pub fn mount_target(peripheral: &Peripheral, part: usize) -> Option<MountTarget> {
    let partitions = partitions(peripheral);
    if let Some(partition) = partitions.get(part) {
        if partition.mount.is_some() {
            return None;
        }
        return Some(MountTarget {
            device: partition.path.clone(),
            label: partition_label(partition),
        });
    }
    let disk = peripheral.disks.first()?;
    if disk.mount.is_some() {
        return None;
    }
    Some(MountTarget {
        device: disk.path.clone(),
        label: disk_label(disk),
    })
}

/// CLI：按选择器找未挂载的挂载目标。
pub fn mount_target_by(snapshot: &Snapshot, selector: &str) -> Result<MountTarget> {
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            for partition in &disk.partitions {
                if matches_device(selector, &partition.path, &partition.name) {
                    if partition.mount.is_some() {
                        bail!("{} 已经挂载", partition.path);
                    }
                    return Ok(MountTarget {
                        device: partition.path.clone(),
                        label: partition_label(partition),
                    });
                }
            }
            if matches_device(selector, &disk.path, &disk.name) {
                if disk.partitions.is_empty() {
                    return Ok(MountTarget {
                        device: disk.path.clone(),
                        label: disk_label(disk),
                    });
                }
                let example = disk
                    .partitions
                    .first()
                    .map(|partition| partition.path.clone())
                    .unwrap_or_else(|| format!("{}{}", disk.path, 1));
                bail!("{} 有分区，请指定分区（如 {}）", disk.path, example);
            }
        }
    }
    bail!("没有找到设备 {selector}")
}

/// CLI：第一个可挂载目标（优先分区，其次无分区整盘）。
pub fn first_mount_target(snapshot: &Snapshot) -> Result<MountTarget> {
    let mut whole_disk = None;
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            for partition in &disk.partitions {
                if partition.mount.is_none() {
                    return Ok(MountTarget {
                        device: partition.path.clone(),
                        label: partition_label(partition),
                    });
                }
            }
            if disk.partitions.is_empty() && disk.mount.is_none() && disk.size > 0 {
                whole_disk.get_or_insert(MountTarget {
                    device: disk.path.clone(),
                    label: disk_label(disk),
                });
            }
        }
    }
    whole_disk.ok_or_else(|| anyhow::anyhow!("没有可挂载的存储设备"))
}

/// TUI：当前分区/首个已挂载分区的卸载目标；最后回退到整盘挂载。
pub fn unmount_target(peripheral: &Peripheral, part: usize) -> Option<String> {
    let partitions = partitions(peripheral);
    partitions
        .get(part)
        .copied()
        .filter(|partition| partition.mount.is_some())
        .or_else(|| {
            partitions
                .iter()
                .copied()
                .find(|partition| partition.mount.is_some())
        })
        .map(|partition| partition.path.clone())
        .or_else(|| {
            peripheral
                .disks
                .iter()
                .find(|disk| disk.mount.is_some())
                .map(|disk| disk.path.clone())
        })
}

/// CLI：按选择器找已挂载的设备。
pub fn mounted_by(snapshot: &Snapshot, selector: &str) -> Result<String> {
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            for partition in &disk.partitions {
                if matches_device(selector, &partition.path, &partition.name) {
                    if partition.mount.is_some() {
                        return Ok(partition.path.clone());
                    }
                    bail!("{} 没有挂载", partition.path);
                }
            }
            if matches_device(selector, &disk.path, &disk.name) {
                if disk.mount.is_some() {
                    return Ok(disk.path.clone());
                }
                bail!("{} 没有挂载", disk.path);
            }
        }
    }
    bail!("没有找到设备 {selector}")
}

/// CLI：首个已挂载的设备（分区优先，其次整盘）。
pub fn first_mounted(snapshot: &Snapshot) -> Result<String> {
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            for partition in &disk.partitions {
                if partition.mount.is_some() {
                    return Ok(partition.path.clone());
                }
            }
            if disk.mount.is_some() {
                return Ok(disk.path.clone());
            }
        }
    }
    bail!("没有已挂载的存储设备")
}

/// CLI：选择器对应的磁盘。
pub fn disk_by<'a>(snapshot: &'a Snapshot, selector: &str) -> Result<&'a Disk> {
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            if matches_device(selector, &disk.path, &disk.name) {
                return Ok(disk);
            }
            for partition in &disk.partitions {
                if matches_device(selector, &partition.path, &partition.name) {
                    return Ok(disk);
                }
            }
        }
    }
    bail!("没有找到磁盘 {selector}")
}

/// CLI：首个可弹出的磁盘（未挂载、有介质优先）。
pub fn first_idle_disk(snapshot: &Snapshot) -> Result<&Disk> {
    let mut empty = None;
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            if disk.mounted() {
                continue;
            }
            if disk.size > 0 {
                return Ok(disk);
            }
            empty.get_or_insert(disk);
        }
    }
    empty.ok_or_else(|| anyhow::anyhow!("没有可弹出的磁盘"))
}

/// TUI：弹出所选分区所在的整盘；已挂载时拒绝。
pub fn eject_disk(peripheral: &Peripheral, part: usize) -> Result<&Disk> {
    let mut offset = 0;
    let disk = peripheral
        .disks
        .iter()
        .find(|disk| {
            let count = disk.partitions.len();
            let hit = part >= offset && part < offset + count;
            offset += count;
            hit
        })
        .or_else(|| peripheral.disks.first())
        .ok_or_else(|| anyhow::anyhow!("该设备不是存储设备"))?;
    if disk.mounted() {
        bail!("请先卸载再弹出");
    }
    Ok(disk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn snapshot() -> Snapshot {
        testing::snapshot()
    }

    #[test]
    fn mounted_partition_is_not_a_mount_target() {
        let snapshot = snapshot();
        let storage = &snapshot.peripherals[0];
        assert!(mount_target(storage, 0).is_none());
        assert_eq!(
            mount_target_by(&snapshot, "/dev/sda1")
                .unwrap_err()
                .to_string(),
            "/dev/sda1 已经挂载"
        );
    }

    #[test]
    fn whole_disk_without_partitions_uses_model() {
        let mut snapshot = snapshot();
        let storage = &mut snapshot.peripherals[0];
        storage.disks[0].partitions.clear();
        storage.disks[0].mount = None;
        storage.disks[0].model = "Portable SSD T7".into();
        let target = mount_target(storage, 0).expect("整盘应可直接挂载");
        assert_eq!(target.device, "/dev/sda");
        assert_eq!(target.label, "Portable SSD T7");
    }

    #[test]
    fn disk_with_partitions_requires_a_partition() {
        let snapshot = snapshot();
        let error = mount_target_by(&snapshot, "/dev/sda")
            .unwrap_err()
            .to_string();
        assert!(error.contains("有分区，请指定分区"), "{error}");
    }

    #[test]
    fn first_target_prefers_unmounted_partition() {
        let mut snapshot = snapshot();
        snapshot.peripherals[0].disks[0].partitions[0].mount = None;
        let target = first_mount_target(&snapshot).unwrap();
        assert_eq!(target.device, "/dev/sda1");
        assert_eq!(target.label, "T7");
    }

    #[test]
    fn unmount_falls_back_to_first_mounted_partition() {
        let snapshot = snapshot();
        let storage = &snapshot.peripherals[0];
        assert_eq!(unmount_target(storage, 0).as_deref(), Some("/dev/sda1"));
    }

    #[test]
    fn eject_rejects_mounted_disk() {
        let snapshot = snapshot();
        let storage = &snapshot.peripherals[0];
        assert_eq!(
            eject_disk(storage, 0).unwrap_err().to_string(),
            "请先卸载再弹出"
        );
    }

    #[test]
    fn selectors_accept_partition_name_and_path() {
        let snapshot = snapshot();
        assert_eq!(disk_by(&snapshot, "sda1").unwrap().path, "/dev/sda");
        assert_eq!(
            mounted_by(&snapshot, "/dev/sda").unwrap_err().to_string(),
            "/dev/sda 没有挂载"
        );
        assert!(mounted_by(&snapshot, "sda1").is_ok());
    }
}
