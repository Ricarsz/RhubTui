//! hidraw 设备与报告描述符。

use std::collections::HashMap;
use std::fs;


use super::*;
use crate::desc::ReportDescriptor;


pub(crate) fn scan_hidraw() -> HashMap<String, Vec<HidInfo>> {
    let mut out: HashMap<String, Vec<HidInfo>> = HashMap::new();
    let Ok(dir) = fs::read_dir("/sys/class/hidraw") else {
        return out;
    };

    for entry in dir.flatten() {
        let node = format!("/dev/{}", entry.file_name().to_string_lossy());
        let Ok(real) = fs::canonicalize(entry.path().join("device")) else {
            continue;
        };
        let Some(usb) = usb_of(&real) else {
            continue;
        };

        let desc = fs::read(real.join("report_descriptor"))
            .map(|bytes| ReportDescriptor::parse(&bytes))
            .unwrap_or_default();
        let mchose =
            desc.has_page(0xFF01) && desc.report(0x11).is_some() && desc.report(0x13).is_some();
        let keyboard = desc.has_page(0xFF31)
            && desc
                .report(0)
                .map(|r| r.input_bytes >= 41 && r.output_bytes >= 64)
                .unwrap_or(false);

        let info = HidInfo {
            node,
            iface: iface_of(&real),
            pages: desc.usage_pages.iter().copied().collect(),
            mchose,
            cmd_out: desc
                .report(0x11)
                .filter(|report| report.output_bytes > 0)
                .map(|report| report.output_bytes)
                .unwrap_or(20),
            data_out: desc
                .report(0x12)
                .filter(|report| report.output_bytes > 0)
                .map(|report| report.output_bytes)
                .unwrap_or(64),
            in_len: desc
                .report(0x13)
                .map(|report| report.input_bytes)
                .unwrap_or(20)
                .max(17),
            battery_feature: desc.battery_feature_report(),
            keyboard,
        };

        let list = out.entry(usb).or_default();
        // 一个 USB 接口可能导出多个 hidraw 节点，按节点去重。
        if !list.iter().any(|existing| existing.node == info.node) {
            list.push(info);
        }
    }

    // 协议优先级：MCHOSE 鼠标 > MCHOSE 键盘 > 标准电量 > 厂商页 > 普通 HID。
    for list in out.values_mut() {
        list.sort_by_key(|info| (std::cmp::Reverse(score(info)), info.iface.unwrap_or(0)));
    }
    out
}

fn score(info: &HidInfo) -> u8 {
    if info.mchose {
        4
    } else if info.keyboard {
        3
    } else if info.battery_feature.is_some() {
        2
    } else if info.pages.iter().any(|page| *page >= 0xFF00) {
        1
    } else {
        0
    }
}

