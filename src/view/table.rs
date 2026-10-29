//! `rhubtui list` 的纯文本表格，给人和脚本共用的稳定投影。

use unicode_width::UnicodeWidthStr;

use crate::model::{HidInfo, Peripheral};
use crate::scan::Snapshot;

pub fn rows(snapshot: &Snapshot) -> Vec<Vec<String>> {
    let mut rows = vec![vec![
        "PORT".to_string(),
        "LINK".to_string(),
        "ID".to_string(),
        "TYPE".to_string(),
        "PLACE".to_string(),
        "NAME".to_string(),
        "EXTRA".to_string(),
    ]];
    for peripheral in &snapshot.peripherals {
        rows.push(vec![
            peripheral.usb.sysfs.clone(),
            peripheral.usb.link_label(),
            peripheral.usb.id(),
            super::json::category_tag(peripheral.category).to_string(),
            place_tag(peripheral),
            peripheral.name(),
            extra_summary(peripheral),
        ]);
    }
    rows
}

/// 对齐成纯文本表格；返回不含结尾换行的多行字符串。
pub fn render(rows: &[Vec<String>]) -> String {
    let columns = rows.first().map_or(0, Vec::len);
    let mut widths = vec![0usize; columns];
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(UnicodeWidthStr::width(cell.as_str()));
        }
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let mut line = String::new();
            for (index, cell) in row.iter().enumerate() {
                if index + 1 == row.len() {
                    line.push_str(cell);
                } else {
                    line.push_str(&pad(cell, widths[index] + 2));
                }
            }
            line.trim_end().to_string()
        })
        .collect();
    lines.join("\n")
}

fn pad(text: &str, width: usize) -> String {
    let mut text = text.to_string();
    let used = UnicodeWidthStr::width(text.as_str());
    if used < width {
        text.push_str(&" ".repeat(width - used));
    }
    text
}

fn place_tag(peripheral: &Peripheral) -> String {
    let mut tag = peripheral.usb.place.label().to_string();
    if peripheral.usb.on_dock() {
        tag.push_str("/坞上");
    }
    tag
}

fn extra_summary(peripheral: &Peripheral) -> String {
    let mut extra = Vec::new();
    let supported: Vec<&HidInfo> = peripheral
        .hids
        .iter()
        .filter(|hid| hid.has_battery())
        .collect();
    for hid in &supported {
        extra.push(format!("{} {}", hid.node, hid.protocol_name()));
    }
    if peripheral.hids.len() > supported.len() {
        extra.push(format!("共 {} 个 HID 接口", peripheral.hids.len()));
    }
    for disk in &peripheral.disks {
        for partition in &disk.partitions {
            let state = match &partition.mount {
                Some(mount) => format!("mounted {}", mount.path),
                None => "unmounted".to_string(),
            };
            extra.push(format!("{} {}", partition.name, state));
        }
        if let Some(mount) = &disk.mount {
            extra.push(format!("{} mounted {}", disk.name, mount.path));
        }
        if disk.partitions.is_empty() {
            extra.push(format!("{} 无介质", disk.name));
        }
    }
    if let Some(audio) = &peripheral.audio {
        extra.push(format!("card {} {}", audio.index, audio.id));
    }
    for net in &peripheral.nets {
        extra.push(net.label());
    }
    if !peripheral.videos.is_empty() {
        extra.push(peripheral.videos.join(" "));
    }
    if !peripheral.ttys.is_empty() {
        extra.push(peripheral.ttys.join(" "));
    }
    if !peripheral.printers.is_empty() {
        extra.push(peripheral.printers.join(" "));
    }
    if peripheral.usb.is_hub() {
        extra.push(format!(
            "{} · {} 口 · {} 台",
            peripheral
                .usb
                .hub_kind
                .unwrap_or(crate::model::HubKind::Hub)
                .label(),
            peripheral.usb.maxchild,
            peripheral.usb.children.len()
        ));
    }
    if extra.is_empty() {
        "-".to_string()
    } else {
        extra.join(" · ")
    }
}
