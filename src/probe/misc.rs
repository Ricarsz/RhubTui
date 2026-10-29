//! 摄像头（UVC）信息。

use std::collections::HashMap;
use std::fs;

use super::usb_of;

pub(crate) fn scan_video() -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(entries) = fs::read_dir("/sys/class/video4linux") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let device = entry.path().join("device");
        if !device.exists() {
            continue;
        }
        let Ok(real) = device.canonicalize() else {
            continue;
        };
        let Some(sysfs) = usb_of(&real) else {
            continue;
        };
        let label = fs::read_to_string(device.join("name"))
            .map(|text| text.trim().to_string())
            .unwrap_or(name.clone());
        if label.is_empty() {
            continue;
        }
        out.entry(sysfs).or_default().push(label);
    }
    for names in out.values_mut() {
        names.sort();
        names.dedup();
    }
    out
}
