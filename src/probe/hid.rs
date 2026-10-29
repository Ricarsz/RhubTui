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

        let info = HidInfo {
            node,
            iface: iface_of(&real),
            pages: desc.usage_pages.iter().copied().collect(),
        };

        let list = out.entry(usb).or_default();
        // 一个 USB 接口可能导出多个 hidraw 节点，按节点去重。
        if !list.iter().any(|existing| existing.node == info.node) {
            list.push(info);
        }
    }

    // 只影响展示顺序：按接口号排列，缺少接口号的排最后。
    for list in out.values_mut() {
        list.sort_by_key(|info| (info.iface.is_none(), info.iface.unwrap_or(0)));
    }
    out
}
