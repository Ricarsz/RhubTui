//! Data model shared by the scanner, the UI and the actions.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Storage,
    Input,
    Audio,
    Hub,
    Other,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Storage,
        Category::Input,
        Category::Audio,
        Category::Hub,
        Category::Other,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Category::Storage => "存储",
            Category::Input => "键鼠 / HID",
            Category::Audio => "音频",
            Category::Hub => "集线器",
            Category::Other => "其他",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Iface {
    pub number: u8,
    pub class: u8,
    pub driver: String,
}

#[derive(Debug, Clone, Default)]
pub struct UsbDev {
    pub sysfs: String,
    pub bus: u8,
    pub devnum: u8,
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
    pub speed: String,
    pub class: u8,
    pub power_ma: Option<u32>,
    pub maxchild: u8,
    /// 内置设备（焊接/固定端口），与外接 U 盘、扩展坞区分。
    pub internal: bool,
    /// USB 协议版本，例如 `2.10`。
    pub usb_version: String,
    /// 父设备的 sysfs 名（`7-1`），根端口设备为空。
    pub parent: Option<String>,
    /// 父设备的产品名，根端口设备为 `None`。
    pub parent_name: Option<String>,
    /// 该设备接在父设备的第几号口。
    pub port: Option<u8>,
    /// 直接下挂的设备 sysfs 名（Hub/扩展坞）。
    pub children: Vec<String>,
    pub ifaces: Vec<Iface>,
}

impl UsbDev {
    pub fn id(&self) -> String {
        format!("{:04x}:{:04x}", self.vid, self.pid)
    }

    pub fn name(&self) -> String {
        let product = self.product.trim();
        let manufacturer = self.manufacturer.trim();
        let generic = product.to_ascii_lowercase();
        let generic = generic.contains("composite device")
            || generic.contains("usb device")
            || generic == "device";

        if product.is_empty() {
            if manufacturer.is_empty() {
                format!("USB 设备 {}", self.sysfs)
            } else {
                manufacturer.to_string()
            }
        } else if generic && !manufacturer.is_empty() {
            format!("{manufacturer} {product}")
        } else {
            product.to_string()
        }
    }

    pub fn is_hub(&self) -> bool {
        self.class == 0x09
    }

    pub fn class_name(&self, class: u8) -> &'static str {
        match class {
            0x01 => "audio",
            0x02 => "cdc",
            0x03 => "hid",
            0x06 => "image",
            0x07 => "printer",
            0x08 => "storage",
            0x09 => "hub",
            0x0A => "cdc-data",
            0x0B => "smartcard",
            0x0D => "content-security",
            0x0E => "video",
            0x0F => "healthcare",
            0x10 => "av",
            0x11 => "billboard",
            0x12 => "bridge",
            0x13 => "smartcard",
            0xDC => "diagnostic",
            0xE0 => "wireless",
            0xEF => "misc",
            0xFE => "app-specific",
            0xFF => "vendor",
            _ => "?",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HidInfo {
    pub node: String,
    pub pages: Vec<u16>,
    pub mchose: bool,
    pub cmd_out: usize,
    pub data_out: usize,
    pub in_len: usize,
    pub battery_feature: Option<(u8, usize)>,
    /// MCHOSE keyboard config channel (usage page 0xFF31).
    pub keyboard: bool,
}

#[derive(Debug, Clone)]
pub struct MountPoint {
    pub path: String,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct Partition {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub fstype: String,
    pub label: String,
    pub mount: Option<MountPoint>,
}

#[derive(Debug, Clone)]
pub struct Disk {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub model: String,
    pub serial: String,
    pub removable: bool,
    /// Mount of the whole device (superfloppy without a partition table).
    pub mount: Option<MountPoint>,
    pub partitions: Vec<Partition>,
}

impl Disk {
    pub fn mounted(&self) -> bool {
        self.mount.is_some() || self.partitions.iter().any(|p| p.mount.is_some())
    }
}

#[derive(Debug, Clone, Default)]
pub struct AudioCard {
    pub index: u32,
    pub id: String,
    pub driver: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Battery {
    Unknown,
    Loading,
    Unsupported,
    Ok {
        pct: u8,
        charging: bool,
        hz: Option<u32>,
    },
    Err(String),
}

impl Battery {
    pub fn label(&self) -> String {
        match self {
            Battery::Ok { pct, charging, .. } => {
                if *charging {
                    format!("{pct}%+")
                } else {
                    format!("{pct}%")
                }
            }
            Battery::Loading => "…".into(),
            _ => String::new(),
        }
    }
}

#[derive(Clone)]
pub struct Peripheral {
    pub usb: UsbDev,
    pub hid: Option<HidInfo>,
    pub audio: Option<AudioCard>,
    pub disks: Vec<Disk>,
    pub category: Category,
}

impl Peripheral {
    pub fn key(&self) -> String {
        self.usb.sysfs.clone()
    }

    pub fn name(&self) -> String {
        self.usb.name()
    }
}

/// Shorten to `max` characters, ending with an ellipsis.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn speed_label(speed: &str) -> String {
    match speed {
        "1.5" => "Low-Speed (1.5 Mbps)".into(),
        "12" => "Full-Speed (12 Mbps)".into(),
        "480" => "High-Speed (480 Mbps)".into(),
        "5000" => "SuperSpeed (5 Gbps)".into(),
        "10000" => "SuperSpeed+ (10 Gbps)".into(),
        "20000" => "SuperSpeed+ (20 Gbps)".into(),
        other => format!("{other} Mbps"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(30 * 1024 * 1024 * 1024), "30.0 GiB");
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("中文设备名", 3), "中文…");
    }

    #[test]
    fn speeds_are_labelled() {
        assert!(speed_label("480").contains("High-Speed"));
        assert!(speed_label("999").contains("999"));
    }

    #[test]
    fn battery_label_marks_charging() {
        let b = Battery::Ok {
            pct: 88,
            charging: true,
            hz: Some(1000),
        };
        assert_eq!(b.label(), "88%+");
    }
}
