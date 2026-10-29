//! 网络接口、视频、串口与打印机。

use std::collections::HashMap;
use std::fs;


use super::*;


pub(crate) fn scan_net() -> HashMap<String, Vec<NetIf>> {
    let mut out: HashMap<String, Vec<NetIf>> = HashMap::new();
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(real) = fs::canonicalize(entry.path().join("device")) else {
            continue;
        };
        let Some(usb) = usb_of(&real) else {
            continue;
        };
        let base = entry.path();
        let speed = read_str(base.join("speed"));
        out.entry(usb).or_default().push(NetIf {
            name,
            mac: read_str(base.join("address")),
            speed_mbps: speed
                .parse::<i64>()
                .ok()
                .filter(|speed| *speed > 0)
                .map(|s| s as u64),
            state: read_str(base.join("operstate")),
            driver: driver_of(&real),
        });
    }
    for nets in out.values_mut() {
        nets.sort_by(|a, b| a.name.cmp(&b.name));
    }
    out
}

pub(crate) fn scan_video() -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(entries) = fs::read_dir("/sys/class/video4linux") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(real) = fs::canonicalize(entry.path().join("device")) else {
            continue;
        };
        let Some(usb) = usb_of(&real) else {
            continue;
        };
        out.entry(usb).or_default().push(format!("/dev/{name}"));
    }
    for nodes in out.values_mut() {
        nodes.sort();
    }
    out
}

pub(crate) fn scan_tty() -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(entries) = fs::read_dir("/sys/class/tty") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with("ttyUSB") || name.starts_with("ttyACM")) {
            continue;
        }
        let Ok(real) = fs::canonicalize(entry.path().join("device")) else {
            continue;
        };
        let Some(usb) = usb_of(&real) else {
            continue;
        };
        out.entry(usb).or_default().push(format!("/dev/{name}"));
    }
    for nodes in out.values_mut() {
        nodes.sort();
    }
    out
}

pub(crate) fn scan_printers() -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(entries) = fs::read_dir("/sys/class/usbmisc") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("lp") {
            continue;
        }
        let Ok(real) = fs::canonicalize(entry.path().join("device")) else {
            continue;
        };
        let Some(usb) = usb_of(&real) else {
            continue;
        };
        out.entry(usb).or_default().push(format!("/dev/usb/{name}"));
    }
    for nodes in out.values_mut() {
        nodes.sort();
    }
    out
}

