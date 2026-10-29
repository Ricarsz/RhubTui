//! Read the peripheral state out of sysfs/procfs.
//!
//! Everything here is unprivileged: USB topology and HID report descriptors live
//! in sysfs (world readable), mounts in `/proc/self/mountinfo`, filesystem
//! metadata is taken from `lsblk`, ALSA cards from `/proc/asound/cards`.

use std::fs;
use std::path::Path;

use crate::model::*;

pub struct Snapshot {
    pub peripherals: Vec<Peripheral>,
}

pub mod audio;
pub mod hid;
pub mod misc;
pub mod storage;
pub mod usb;

use self::audio::scan_audio;
use self::hid::scan_hidraw;
use self::misc::{scan_net, scan_printers, scan_tty, scan_video};
use self::storage::{lsblk_meta, read_mounts, scan_disks};
use self::usb::scan_usb;

pub fn scan() -> Snapshot {
    let usbs = scan_usb();
    let hid = scan_hidraw();
    let mounts = read_mounts();
    let fsmeta = lsblk_meta();
    let disks = scan_disks(&mounts, &fsmeta);
    let audio = scan_audio();
    let nets = scan_net();
    let videos = scan_video();
    let ttys = scan_tty();
    let printers = scan_printers();

    let mut peripherals = Vec::new();
    for usb in usbs {
        let hids = hid.get(&usb.sysfs).cloned().unwrap_or_default();
        let audio_info = audio.get(&usb.sysfs).cloned();
        let nets_info = nets.get(&usb.sysfs).cloned().unwrap_or_default();
        let video_info = videos.get(&usb.sysfs).cloned().unwrap_or_default();
        let tty_info = ttys.get(&usb.sysfs).cloned().unwrap_or_default();
        let printer_info = printers.get(&usb.sysfs).cloned().unwrap_or_default();

        let mut dev_disks: Vec<Disk> = disks
            .iter()
            .filter(|(owner, _)| owner.as_deref() == Some(usb.sysfs.as_str()))
            .map(|(_, disk)| disk.clone())
            .collect();
        // 存储传输层是 usb-storage 还是 uas，取 USB 存储接口的驱动名。
        for disk in &mut dev_disks {
            if let Some(iface) = usb
                .ifaces
                .iter()
                .find(|iface| iface.class == 0x08 && !iface.driver.is_empty())
            {
                disk.transport = iface.driver.clone();
            }
        }
        dev_disks.sort_by(|a, b| a.name.cmp(&b.name));

        let category = classify(
            !dev_disks.is_empty(),
            !hids.is_empty(),
            audio_info.is_some(),
            !nets_info.is_empty(),
            usb.is_hub(),
            &usb.ifaces
                .iter()
                .map(|iface| iface.class)
                .collect::<Vec<_>>(),
        );

        peripherals.push(Peripheral {
            usb,
            hids,
            audio: audio_info,
            nets: nets_info,
            videos: video_info,
            ttys: tty_info,
            printers: printer_info,
            disks: dev_disks,
            category,
        });
    }

    peripherals.sort_by(|a, b| {
        (a.category, a.name().to_lowercase()).cmp(&(b.category, b.name().to_lowercase()))
    });

    Snapshot { peripherals }
}

/// 设备分类：多功能设备取「用户最可能想要操作」的那一类。
fn classify(
    has_disk: bool,
    has_hid: bool,
    has_audio: bool,
    has_net: bool,
    is_hub: bool,
    classes: &[u8],
) -> Category {
    if has_disk || classes.contains(&0x08) {
        Category::Storage
    } else if has_net {
        Category::Network
    } else if has_audio {
        Category::Audio
    } else if has_hid {
        Category::Input
    } else if is_hub {
        Category::Hub
    } else {
        Category::Other
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

pub(crate) fn read_str(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub(crate) fn read_hex(path: impl AsRef<Path>) -> u16 {
    let s = read_str(path);
    u16::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0)
}

pub(crate) fn read_u64(path: impl AsRef<Path>) -> u64 {
    read_str(path).parse().unwrap_or(0)
}

/// Last path component of a sysfs `driver` symlink.
pub(crate) fn driver_of(dir: &Path) -> String {
    fs::read_link(dir.join("driver"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default()
}

/// USB device sysfs name (`7-1.4`) owning `path`.
///
/// Interfaces are named `<device>:<config>.<interface>` (e.g. `7-1.4:1.2`), so
/// the first ancestor of that shape tells us the device — no need to walk all
/// the way up to `usbN`, which would return the hub for devices behind a hub.
pub(crate) fn usb_of(path: &Path) -> Option<String> {
    for ancestor in path.ancestors() {
        let name = ancestor.file_name()?.to_string_lossy().to_string();
        if let Some((device, _)) = name.split_once(':')
            && device.contains('-')
        {
            return Some(device.to_string());
        }
    }
    None
}

/// 接口号（`7-1.4:1.2` → 2）。
pub(crate) fn iface_of(path: &Path) -> Option<u8> {
    for ancestor in path.ancestors() {
        let name = ancestor.file_name()?.to_string_lossy().to_string();
        let Some((device, config)) = name.split_once(':') else {
            continue;
        };
        if !device.contains('-') {
            continue;
        }
        let Some((_, interface)) = config.split_once('.') else {
            continue;
        };
        return interface.parse().ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorises_multifunction_devices() {
        assert_eq!(
            classify(true, true, true, true, true, &[0x01, 0x08]),
            Category::Storage
        );
        assert_eq!(
            classify(false, true, false, false, false, &[0x03]),
            Category::Input
        );
        assert_eq!(
            classify(false, false, false, true, false, &[0x0A]),
            Category::Network
        );
        assert_eq!(
            classify(false, false, false, false, true, &[0x09]),
            Category::Hub
        );
    }
}
