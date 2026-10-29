//! Data model shared by the scanner, the UI and the actions.

use unicode_width::UnicodeWidthStr;

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
            Category::Hub => "扩展坞 / Hub",
            Category::Other => "其他",
        }
    }
}

/// 设备在物理位置上的归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// 主板/机身内置（摄像头、蓝牙、读卡器、焊接的接收器……）。
    Internal,
    /// 外部接口上的设备。
    #[default]
    External,
}

impl Placement {
    pub fn is_internal(self) -> bool {
        self == Placement::Internal
    }

    pub fn label(self) -> &'static str {
        match self {
            Placement::Internal => "内置",
            Placement::External => "外接",
        }
    }
}

/// Hub 的形态。只有拿到足够证据时才标记为扩展坞。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubKind {
    Hub,
    Dock,
}

impl HubKind {
    pub fn label(self) -> &'static str {
        match self {
            HubKind::Hub => "Hub",
            HubKind::Dock => "扩展坞",
        }
    }
}

/// USB 控制器（PCI 功能），同一控制器通常带 USB2/USB3 两条根总线。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Controller {
    pub slot: String,
    pub driver: String,
    pub vendor: u16,
    pub device: u16,
    /// 该控制器下的根总线号，例如 `[7, 8]`。
    pub buses: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Iface {
    pub number: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: u8,
    pub driver: String,
}

impl Iface {
    /// 比 class 更细的用途描述，便于用户一眼判断接口功能。
    pub fn function(&self) -> &'static str {
        match (self.class, self.subclass, self.protocol) {
            (0x01, _, _) => "音频",
            (0x02, _, _) => "串口 / CDC",
            (0x03, 0x01, 0x01) => "键盘",
            (0x03, 0x01, 0x02) => "鼠标",
            (0x03, 0x00, 0x00) => "HID",
            (0x03, _, _) => "HID",
            (0x06, _, _) => "图像",
            (0x07, _, _) => "打印机",
            (0x08, _, _) => "存储",
            (0x09, _, _) => "Hub",
            (0x0A, _, _) => "网络",
            (0x0B, _, _) => "智能卡",
            (0x0E, _, _) => "摄像头",
            (0x11, _, _) => "USB-C 告示牌",
            (0xE0, 0x01, _) => "蓝牙",
            (0xE0, _, _) => "无线",
            (0xEF, 0x02, 0x01) => "复合设备 (IAD)",
            (0xFF, _, _) => "厂商自定义",
            _ => "其他",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsbDev {
    pub sysfs: String,
    pub bus: u8,
    pub devnum: u8,
    pub vid: u16,
    pub pid: u16,
    pub bcd_device: String,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
    pub speed: String,
    pub class: u8,
    pub power_ma: Option<u32>,
    pub maxchild: u8,
    pub ltm_capable: bool,
    pub authorized: bool,
    /// USB 协议版本，例如 `2.10`。
    pub usb_version: String,
    /// 物理归属与判定依据。
    pub place: Placement,
    pub place_reason: String,
    /// 根总线号与根端口号（顶层设备的端口号）。
    pub root_bus: u8,
    pub root_port: Option<u8>,
    /// 0 = 直连主板根端口；1 = 经过一级 Hub；依此类推。
    pub depth: u8,
    /// 从根端口到直接父设备的 sysfs 名。
    pub chain: Vec<String>,
    /// 父设备的 sysfs 名（`7-1`），根端口设备为空。
    pub parent: Option<String>,
    /// 父设备的产品名，根端口设备为 `None`。
    pub parent_name: Option<String>,
    /// 该设备接在父设备的第几号口。
    pub port: Option<u8>,
    /// 直接下挂的设备 sysfs 名（Hub/扩展坞）。
    pub children: Vec<String>,
    /// 所属 Hub 的形态；自身是 Hub 时表示自身形态。
    pub hub_kind: Option<HubKind>,
    /// 最近一层扩展坞的名称（自身是扩展坞时也会填）。
    pub dock: Option<String>,
    pub controller: Option<Controller>,
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

    /// 该设备是否接在扩展坞上。
    pub fn on_dock(&self) -> bool {
        self.dock.is_some()
    }

    /// 连接位置的一句话描述：主板根端口 / Hub 的 N 号口 / 扩展坞的 N 号口。
    pub fn attachment(&self) -> String {
        let head = match self.place {
            Placement::Internal => "主板内置",
            Placement::External => "外接",
        };
        if self.depth == 0 {
            let port = self
                .root_port
                .map(|port| format!("根端口 {port}"))
                .unwrap_or_else(|| "根端口".to_string());
            return format!("{head} · 直连{port}");
        }
        let parent = self
            .parent_name
            .clone()
            .unwrap_or_else(|| self.parent.clone().unwrap_or_else(|| "Hub".to_string()));
        let kind = match self.hub_kind {
            Some(HubKind::Dock) => "扩展坞",
            _ => "Hub",
        };
        let port = self
            .port
            .map(|port| format!("{port} 号口"))
            .unwrap_or_else(|| "下行口".to_string());
        let sep = if kind == "Hub" { " " } else { "" };
        format!(
            "{head} · 经{sep}{kind}「{parent}」{port}（{} 级）",
            self.depth
        )
    }

    /// 面向普通用户的位置描述：不暴露父设备名和层级。
    pub fn friendly_attachment(&self) -> String {
        if self.place.is_internal() {
            return "主板内置".into();
        }
        if self.depth == 0 {
            return "直连主机".into();
        }
        let port = self
            .port
            .map(|port| format!("{port} 号口"))
            .unwrap_or_else(|| "下行口".into());
        match self.hub_kind {
            Some(HubKind::Dock) => format!("扩展坞 {port}"),
            _ => format!("Hub {port}"),
        }
    }

    /// 列表中使用的短连接描述：`根端口 2` / `Hub 7-1 · 4 号口`。
    pub fn link_label(&self) -> String {
        if self.depth == 0 {
            return self
                .root_port
                .map(|port| format!("根端口 {port}"))
                .unwrap_or_else(|| "根端口".to_string());
        }
        let kind = match self.hub_kind {
            Some(HubKind::Dock) => "扩展坞",
            _ => "Hub",
        };
        match (&self.parent, self.port) {
            (Some(parent), Some(port)) => format!("{kind} {parent} · {port} 号口"),
            (Some(parent), None) => format!("{kind} {parent}"),
            _ => format!("{kind} · {} 级", self.depth),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HidInfo {
    pub node: String,
    /// 所在 USB 接口号（`bInterfaceNumber`）。
    pub iface: Option<u8>,
    pub pages: Vec<u16>,
}

#[derive(Debug, Clone)]
pub struct MountPoint {
    pub path: String,
    pub read_only: bool,
    /// 文件系统总容量与可用空间（statvfs），仅在扫描时补全。
    pub total: Option<u64>,
    pub free: Option<u64>,
}

impl MountPoint {
    /// 已用百分比（0–100），拿不到 statvfs 数据时为 `None`。
    pub fn used_pct(&self) -> Option<u8> {
        let (total, free) = (self.total?, self.free?);
        if total == 0 {
            return None;
        }
        Some(
            (total.saturating_sub(free))
                .saturating_mul(100)
                .div_ceil(total) as u8,
        )
    }
}

#[derive(Debug, Clone)]
pub struct Partition {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub fstype: String,
    pub label: String,
    pub uuid: String,
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
    /// 内核驱动，例如 `usb-storage` / `uas`。
    pub transport: String,
    pub rotational: bool,
    /// 分区表类型（gpt / dos / 空）。
    pub ptable: String,
    /// Mount of the whole device (superfloppy without a partition table).
    pub mount: Option<MountPoint>,
    pub partitions: Vec<Partition>,
}

impl Disk {
    pub fn mounted(&self) -> bool {
        self.mount.is_some() || self.partitions.iter().any(|p| p.mount.is_some())
    }

    pub fn descriptor(&self) -> String {
        let mut text = self.name.clone();
        if !self.model.is_empty() {
            text.push_str(&format!(" · {}", self.model));
        }
        text
    }
}

#[derive(Debug, Clone, Default)]
pub struct AudioCard {
    pub index: u32,
    pub id: String,
    pub driver: String,
    pub name: String,
    pub pcms: Vec<String>,
}

/// 一个 USB 设备以及挂在它上面的所有功能节点。
#[derive(Clone)]
pub struct Peripheral {
    pub usb: UsbDev,
    /// 同一接收器可能暴露多个 HID 接口（键鼠组合、厂商页），按接口号排列。
    pub hids: Vec<HidInfo>,
    pub audio: Option<AudioCard>,
    pub videos: Vec<String>,
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

    /// 面向用户的设备类型：优先按 HID 用途细分键鼠，拿不到时退回大类。
    pub fn kind_label(&self) -> &'static str {
        let has_iface = |subclass: u8, protocol: u8| {
            self.usb.ifaces.iter().any(|iface| {
                iface.class == 0x03 && iface.subclass == subclass && iface.protocol == protocol
            })
        };
        if has_iface(0x01, 0x01) {
            "键盘"
        } else if has_iface(0x01, 0x02) {
            "鼠标"
        } else {
            self.category.title()
        }
    }

    /// 所有功能节点的一行摘要，用于列表徽标。
    pub fn functions(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.disks.is_empty() {
            out.push("存储");
        }
        if !self.hids.is_empty() {
            out.push("HID");
        }
        if self.audio.is_some() {
            out.push("音频");
        }
        if !self.videos.is_empty() {
            out.push("摄像头");
        }
        out
    }

    /// 大小写无关的模糊匹配：名称、端口、VID:PID、序列号、分区卷标。
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let haystack = [
            self.name(),
            self.usb.sysfs.clone(),
            self.usb.id(),
            self.usb.serial.clone(),
            self.category.title().to_string(),
            self.usb.place.label().to_string(),
        ];
        if haystack
            .iter()
            .any(|field| field.to_lowercase().contains(&query))
        {
            return true;
        }
        self.disks.iter().any(|disk| {
            disk.name.to_lowercase().contains(&query)
                || disk.model.to_lowercase().contains(&query)
                || disk.partitions.iter().any(|part| {
                    part.name.to_lowercase().contains(&query)
                        || part.label.to_lowercase().contains(&query)
                })
        })
    }
}

/// 按终端列宽截断（中文占两列）。
pub fn truncate_cols(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width + 1 > max {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push('…');
    out
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
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
    fn truncates_by_display_width() {
        assert_eq!(truncate_cols("abc", 5), "abc");
        assert_eq!(truncate_cols("中文设备", 5), "中文…");
        assert_eq!(truncate_cols("abcdef", 4), "abc…");
    }

    #[test]
    fn usage_percent_rounds_up() {
        let mount = MountPoint {
            path: "/run/media/u/disk".into(),
            read_only: false,
            total: Some(1000),
            free: Some(620),
        };
        assert_eq!(mount.used_pct(), Some(38));
    }

    #[test]
    fn attachment_describes_topology() {
        let dev = UsbDev {
            parent: Some("7-1".into()),
            parent_name: Some("USB2.1 Hub".into()),
            depth: 1,
            port: Some(4),
            ..Default::default()
        };
        assert!(dev.attachment().contains("经 Hub「USB2.1 Hub」4 号口"));
        assert_eq!(dev.link_label(), "Hub 7-1 · 4 号口");
        let dev = UsbDev {
            root_port: Some(2),
            ..Default::default()
        };
        assert!(dev.attachment().contains("直连根端口 2"));
        assert_eq!(dev.link_label(), "根端口 2");
    }

    fn hid() -> HidInfo {
        HidInfo {
            node: "/dev/hidraw0".into(),
            iface: None,
            pages: Vec::new(),
        }
    }

    fn input(hids: Vec<HidInfo>, ifaces: Vec<Iface>) -> Peripheral {
        Peripheral {
            usb: UsbDev {
                ifaces,
                ..Default::default()
            },
            hids,
            audio: None,
            videos: Vec::new(),
            disks: Vec::new(),
            category: Category::Input,
        }
    }

    #[test]
    fn friendly_attachment_hides_bus_names() {
        let dev = UsbDev {
            depth: 1,
            port: Some(4),
            parent: Some("7-1".into()),
            parent_name: Some("USB2.1 Hub".into()),
            ..Default::default()
        };
        assert_eq!(dev.friendly_attachment(), "Hub 4 号口");
        let dev = UsbDev {
            depth: 1,
            port: Some(1),
            hub_kind: Some(HubKind::Dock),
            ..Default::default()
        };
        assert_eq!(dev.friendly_attachment(), "扩展坞 1 号口");
        let dev = UsbDev {
            root_port: Some(2),
            ..Default::default()
        };
        assert_eq!(dev.friendly_attachment(), "直连主机");
    }

    #[test]
    fn overview_labels_keyboard_and_mouse() {
        let keyboard = input(
            vec![hid()],
            vec![Iface {
                number: 0,
                class: 0x03,
                subclass: 0x01,
                protocol: 0x01,
                endpoints: 1,
                driver: String::new(),
            }],
        );
        assert_eq!(keyboard.kind_label(), "键盘");

        let mouse = input(
            vec![hid()],
            vec![Iface {
                number: 0,
                class: 0x03,
                subclass: 0x01,
                protocol: 0x02,
                endpoints: 1,
                driver: String::new(),
            }],
        );
        assert_eq!(mouse.kind_label(), "鼠标");
    }

    #[test]
    fn overview_falls_back_to_interfaces() {
        let ifaces = vec![Iface {
            number: 0,
            class: 0x03,
            subclass: 0x01,
            protocol: 0x02,
            endpoints: 1,
            driver: String::new(),
        }];
        let mouse = input(Vec::new(), ifaces);
        assert_eq!(mouse.kind_label(), "鼠标");
    }
}
