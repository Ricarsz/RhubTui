//! 与终端无关的展示模型：TUI 负责上色与排版，CLI/JSON 复用同一份语义。

use std::collections::HashMap;

use unicode_width::UnicodeWidthStr;

use crate::model::{
    Battery, Category, Disk, HidInfo, HubKind, MountPoint, Peripheral, battery_bar, human_size,
    speed_label, usage_bar,
};
use crate::scan::Snapshot;

pub mod json;
pub mod table;

/// 语义色调，由具体主题映射成颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Accent,
    Good,
    Warn,
    Bad,
    Dim,
    Faint,
}

#[derive(Debug, Clone)]
pub struct Badge {
    pub text: String,
    pub tone: Tone,
}

#[derive(Debug, Clone)]
pub enum RowText {
    /// 分类分组标题。
    Section(String),
    Device {
        name: String,
        /// 内置 / 坞上等位置标记。
        flags: Vec<(String, Tone)>,
        badge: Option<Badge>,
    },
}

#[derive(Debug, Clone)]
pub struct Row {
    pub text: RowText,
    pub device: Option<usize>,
}

impl Row {
    pub fn section(title: impl Into<String>) -> Self {
        Self {
            text: RowText::Section(title.into()),
            device: None,
        }
    }

    pub fn device(index: usize, peripheral: &Peripheral, battery: Option<&Battery>) -> Self {
        let mut flags = Vec::new();
        if peripheral.usb.place.is_internal() {
            flags.push(("内置".to_string(), Tone::Faint));
        } else if peripheral.usb.on_dock() {
            flags.push(("坞上".to_string(), Tone::Dim));
        }
        Self {
            device: Some(index),
            text: RowText::Device {
                name: peripheral.name(),
                flags,
                badge: badge(peripheral, battery),
            },
        }
    }
}

/// 列表徽标：电量优先，其次存储状态、Hub 端口、功能摘要。
pub fn badge(peripheral: &Peripheral, battery: Option<&Battery>) -> Option<Badge> {
    if let Some(battery) = battery {
        match battery {
            Battery::Ok { pct, .. } => {
                return Some(Badge {
                    text: battery.label(),
                    tone: if *pct <= 20 { Tone::Bad } else { Tone::Good },
                });
            }
            Battery::Loading => {
                return Some(Badge {
                    text: "读取中".into(),
                    tone: Tone::Faint,
                });
            }
            Battery::Err(_) => {
                return Some(Badge {
                    text: "电量?".into(),
                    tone: Tone::Bad,
                });
            }
            Battery::Unknown => {}
        }
    }

    if !peripheral.disks.is_empty() {
        let partitions: usize = peripheral.disks.iter().map(|d| d.partitions.len()).sum();
        if peripheral.disks.iter().any(Disk::mounted) {
            return Some(Badge {
                text: "已挂载".into(),
                tone: Tone::Good,
            });
        }
        if partitions > 0 {
            return Some(Badge {
                text: "未挂载".into(),
                tone: Tone::Warn,
            });
        }
        return Some(Badge {
            text: "无介质".into(),
            tone: Tone::Faint,
        });
    }

    if peripheral.usb.is_hub() {
        let text = match peripheral.usb.hub_kind.unwrap_or(HubKind::Hub) {
            HubKind::Dock => format!("扩展坞 · {} 口", peripheral.usb.maxchild),
            HubKind::Hub => {
                let mut text = format!("{} 口", peripheral.usb.maxchild);
                if !peripheral.usb.children.is_empty() {
                    text.push_str(&format!(" · {} 台", peripheral.usb.children.len()));
                }
                text
            }
        };
        return Some(Badge {
            text,
            tone: Tone::Faint,
        });
    }

    if peripheral.category == Category::Input
        && !peripheral.hids.is_empty()
        && peripheral.hids.iter().all(|hid| !hid.has_battery())
    {
        return Some(Badge {
            text: "无电量".into(),
            tone: Tone::Faint,
        });
    }

    let functions = peripheral.functions();
    if !functions.is_empty() && peripheral.category != Category::Other {
        return Some(Badge {
            text: functions.join(" · "),
            tone: Tone::Faint,
        });
    }
    None
}

// ─── 详情 ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub tone: Tone,
    pub bold: bool,
}

impl Span {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            bold: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    pub fn blank() -> Self {
        Self::default()
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self {
            spans: vec![Span::new(text, Tone::Normal)],
        }
    }

    pub fn tone(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            spans: vec![Span::new(text, tone)],
        }
    }

    pub fn heading(text: impl Into<String>) -> Self {
        let mut span = Span::new(text, Tone::Accent);
        span.bold = true;
        Self { spans: vec![span] }
    }

    pub fn kv(label: &str, value: impl Into<String>) -> Self {
        Self::kv_tone(label, value, Tone::Normal)
    }

    pub fn kv_tone(label: &str, value: impl Into<String>, tone: Tone) -> Self {
        Self {
            spans: vec![
                Span::new(format!("{} ", pad_right(label, 8)), Tone::Dim),
                Span::new(value, tone),
            ],
        }
    }

    pub fn kv_indent(label: &str, value: impl Into<String>) -> Self {
        Self {
            spans: vec![
                Span::new(format!("      {} ", pad_right(label, 5)), Tone::Dim),
                Span::new(value, Tone::Dim),
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub enum Details {
    Empty,
    Device {
        title: String,
        tags: String,
        lines: Vec<Line>,
    },
}

/// 详情视图。`advanced` 为假时给普通用户看的概览。
pub fn details(
    snapshot: &Snapshot,
    index: Option<usize>,
    batteries: &HashMap<String, Battery>,
    part_sel: &HashMap<String, usize>,
    advanced: bool,
) -> Details {
    let Some(peripheral) = index.and_then(|index| snapshot.peripherals.get(index)) else {
        return Details::Empty;
    };
    if advanced {
        advanced_details(snapshot, peripheral, batteries, part_sel)
    } else {
        overview(snapshot, peripheral, batteries)
    }
}

fn overview(
    _snapshot: &Snapshot,
    peripheral: &Peripheral,
    batteries: &HashMap<String, Battery>,
) -> Details {
    let usb = &peripheral.usb;
    let battery_hid = peripheral.battery_hid();
    let battery = battery_hid.map(|hid| battery_of(batteries, hid));
    let wireless = battery.as_ref().and_then(Battery::wireless);
    let connection = match wireless {
        Some(true) => Some("2.4G 无线"),
        Some(false) => Some("USB 有线"),
        None => peripheral.connection(),
    };

    let mut tags = vec![peripheral.kind_label().to_string()];
    if let Some(connection) = connection {
        tags.push(connection.to_string());
    }
    tags.push(usb.place.label().to_string());
    if usb.on_dock() {
        tags.push("经扩展坞".into());
    }

    let mut lines = Vec::new();

    if let Some(hid) = battery_hid {
        let status = battery_of(batteries, hid);
        lines.push(Line::heading("电量"));
        match &status {
            Battery::Ok { pct, charging, .. } => {
                let mut span = Span::new(battery_bar(*pct, 16), battery_tone(*pct));
                span.bold = true;
                lines.push(Line {
                    spans: vec![Span::new("  ", Tone::Normal), span],
                });
                let (state, tone) = if *charging {
                    ("充电中", Tone::Good)
                } else {
                    ("使用中", Tone::Dim)
                };
                lines.push(Line::tone(format!("  {state}"), tone));
            }
            Battery::Loading => lines.push(Line::tone("  读取中…", Tone::Faint)),
            Battery::Unknown => lines.push(Line::tone("  未检测到电量通道", Tone::Faint)),
            Battery::Err(error) => {
                lines.push(Line::tone(format!("  读取失败：{error}"), Tone::Bad))
            }
        }
        lines.push(Line::blank());
    }

    let mut rows: Vec<(&str, String)> = Vec::new();
    rows.push(("类型", peripheral.kind_label().to_string()));
    if let Some(connection) = connection {
        rows.push(("连接", connection.to_string()));
    }
    if let Some(dpi) = battery.as_ref().and_then(Battery::dpi) {
        rows.push(("DPI档位", format!("{dpi} 档")));
    }
    if let Some(hz) = battery.as_ref().and_then(Battery::hz)
        && wireless != Some(true)
    {
        rows.push(("回报率", format!("{hz} Hz")));
    }
    rows.push(("位置", usb.friendly_attachment()));

    if !peripheral.disks.is_empty() {
        let partitions: usize = peripheral
            .disks
            .iter()
            .map(|disk| disk.partitions.len())
            .sum();
        let mounted = peripheral.disks.iter().any(Disk::mounted);
        rows.push((
            "存储",
            format!(
                "{partitions} 个分区 · {}",
                if mounted { "已挂载" } else { "未挂载" }
            ),
        ));
        let mount_point = peripheral.disks.iter().find_map(|disk| {
            disk.mount
                .as_ref()
                .or_else(|| disk.partitions.iter().find_map(|part| part.mount.as_ref()))
        });
        if let Some(mount) = mount_point {
            let mut value = mount.path.clone();
            if let Some(usage) = mount.usage_label() {
                value.push_str(&format!(" · {usage}"));
            }
            if mount.read_only {
                value.push_str(" (ro)");
            }
            rows.push(("挂载", value));
        }
    }

    if let Some(audio) = &peripheral.audio {
        rows.push(("音频", format!("card {} · {}", audio.index, audio.name)));
    }
    if let Some(net) = peripheral.nets.first() {
        rows.push(("网络", net.label()));
    }
    if usb.is_hub() {
        rows.push((
            "扩展坞",
            format!(
                "{} · {} 台下挂",
                usb.hub_kind.unwrap_or(HubKind::Hub).label(),
                usb.children.len()
            ),
        ));
    }

    if !rows.is_empty() {
        lines.push(Line::heading("关键信息"));
        for (key, value) in rows {
            lines.push(Line::kv(key, value));
        }
    }

    lines.push(Line::blank());
    lines.push(Line::tone(
        "  a 查看高级详情 · b 重新读取电量 · r 重新扫描",
        Tone::Faint,
    ));

    Details::Device {
        title: peripheral.name(),
        tags: tags.join(" · "),
        lines,
    }
}

fn advanced_details(
    snapshot: &Snapshot,
    peripheral: &Peripheral,
    batteries: &HashMap<String, Battery>,
    part_sel: &HashMap<String, usize>,
) -> Details {
    let usb = &peripheral.usb;
    let mut tags = vec![peripheral.category.title().to_string()];
    tags.push(usb.place.label().to_string());
    if usb.on_dock() {
        tags.push("经扩展坞".into());
    }

    let mut lines = Vec::new();
    lines.push(Line::heading("连接"));
    lines.push(Line::kv(
        "归属",
        format!("{} · {}", usb.place.label(), usb.place_reason),
    ));
    lines.push(Line::kv("位置", usb.attachment()));
    if let Some(dock) = &usb.dock
        && usb.hub_kind != Some(HubKind::Dock)
    {
        lines.push(Line::kv("扩展坞", dock.clone()));
    }
    if let Some(controller) = &usb.controller {
        lines.push(Line::kv("控制器", controller.label()));
    }
    lines.push(Line::kv("链路", chain_of(snapshot, peripheral)));
    let version = if usb.usb_version.is_empty() {
        String::new()
    } else {
        format!("USB {} · ", usb.usb_version)
    };
    lines.push(Line::kv(
        "速率",
        format!("{version}{}", speed_label(&usb.speed)),
    ));
    lines.push(Line::kv(
        "地址",
        format!(
            "bus {} · dev {} · {}（端口 {}）",
            usb.bus,
            usb.devnum,
            usb.sysfs,
            usb.port.unwrap_or(0)
        ),
    ));
    if let Some(power) = usb.power_ma {
        lines.push(Line::kv("供电", format!("{power} mA（配置上限）")));
    }
    lines.push(Line::kv("VID:PID", usb.id()));
    lines.push(Line::kv(
        "类别",
        format!(
            "{:02x}/{:02x}/{:02x} {}",
            usb.class,
            usb.subclass,
            usb.protocol,
            usb.class_name(usb.class)
        ),
    ));
    if !usb.manufacturer.is_empty() {
        lines.push(Line::kv("厂商", usb.manufacturer.clone()));
    }
    if !usb.product.is_empty() {
        lines.push(Line::kv("型号", usb.product.clone()));
    }
    if !usb.bcd_device.is_empty() {
        lines.push(Line::kv("版本", format!("bcdDevice {}", usb.bcd_device)));
    }
    if !usb.serial.is_empty() {
        lines.push(Line::kv("序列号", usb.serial.clone()));
    }
    lines.push(Line::kv(
        "配置",
        format!(
            "{} 个接口 · {} 组配置 · LTM {} · {}",
            usb.num_interfaces,
            usb.num_configs,
            if usb.ltm_capable {
                "支持"
            } else {
                "不支持"
            },
            if usb.authorized {
                "已授权"
            } else {
                "未授权"
            }
        ),
    ));

    if !usb.ifaces.is_empty() {
        lines.push(Line::blank());
        lines.push(Line::heading("USB 接口"));
        for iface in &usb.ifaces {
            let mut text = format!(
                "  {:<2} {} {:02x}/{:02x}/{:02x}  {} 端点",
                iface.number,
                pad_right(iface.function(), 12),
                iface.class,
                iface.subclass,
                iface.protocol,
                iface.endpoints
            );
            if !iface.driver.is_empty() {
                text.push_str(&format!("  → {}", iface.driver));
            }
            lines.push(Line::text(text));
        }
    }

    if !peripheral.hids.is_empty() {
        lines.push(Line::blank());
        lines.push(Line::heading("HID"));
        for hid in &peripheral.hids {
            let iface = hid
                .iface
                .map(|iface| format!("iface {iface}"))
                .unwrap_or_else(|| "iface ?".into());
            let pages: Vec<String> = hid
                .pages
                .iter()
                .map(|page| format!("0x{page:04X} {}", page_name(*page)))
                .collect();
            lines.push(Line::text(format!(
                "  {iface} · {} · {}",
                hid.node,
                hid.protocol_name()
            )));
            lines.push(Line::tone(format!("       {}", pages.join(" ")), Tone::Dim));
            if hid.has_battery() {
                lines.push(battery_line(batteries, hid));
            }
        }
    }

    if !peripheral.disks.is_empty() {
        lines.push(Line::blank());
        lines.push(Line::heading("存储"));
        let selected = part_index(peripheral, part_sel);
        let mut flat = 0;
        for disk in &peripheral.disks {
            let size = if disk.size == 0 {
                "无介质".to_string()
            } else {
                human_size(disk.size)
            };
            let mut header = format!("  {}  {}", disk.path, size);
            if !disk.model.is_empty() {
                header.push_str(&format!("  {}", disk.model));
            }
            if !disk.ptable.is_empty() {
                header.push_str(&format!("  {}", disk.ptable.to_uppercase()));
            }
            if disk.size > 0 {
                header.push_str(if disk.rotational {
                    "  · 机械"
                } else {
                    "  · 固态"
                });
            }
            if !disk.transport.is_empty() {
                header.push_str(&format!("  · {}", disk.transport));
            }
            if disk.removable {
                header.push_str("  · 可移动");
            }
            lines.push(Line::text(header));
            if !disk.serial.is_empty() {
                lines.push(Line::tone(
                    format!("      序列号 {}", disk.serial),
                    Tone::Faint,
                ));
            }
            if let Some(mount) = &disk.mount {
                lines.push(Line::kv_indent("挂载", mount_label(mount)));
            }
            for partition in &disk.partitions {
                let marker = if flat == selected { "▸" } else { " " };
                let mut text = format!(
                    "  {marker} {}  {}",
                    partition.name,
                    human_size(partition.size)
                );
                if !partition.fstype.is_empty() {
                    text.push_str(&format!("  {}", partition.fstype));
                }
                if !partition.label.is_empty() {
                    text.push_str(&format!("  {}", partition.label));
                }
                if let Some(mount) = &partition.mount {
                    text.push_str(&format!("  → {}", mount.path));
                    if mount.read_only {
                        text.push_str(" (ro)");
                    }
                }
                let tone = if flat == selected {
                    Tone::Accent
                } else {
                    Tone::Normal
                };
                lines.push(Line::tone(text, tone));
                if !partition.uuid.is_empty() {
                    lines.push(Line::tone(
                        format!("      UUID {}", partition.uuid),
                        Tone::Faint,
                    ));
                }
                if let Some(mount) = &partition.mount
                    && let (Some(pct), Some(label)) = (mount.used_pct(), mount.usage_label())
                {
                    lines.push(Line::tone(
                        format!("      {}  {label}", usage_bar(pct, 8)),
                        usage_tone(pct),
                    ));
                }
                flat += 1;
            }
        }
        if peripheral.disks.iter().all(|d| d.partitions.is_empty()) {
            let hint = if peripheral.disks.iter().any(|d| d.size > 0) {
                "  整盘无分区表 · m 直接挂载整盘"
            } else {
                "  没有分区（未插介质？）"
            };
            lines.push(Line::tone(hint, Tone::Faint));
        }
        lines.push(Line::tone(
            "  h/l 选分区 · m 挂载 · u 卸载 · e 弹出",
            Tone::Faint,
        ));
    }

    if let Some(audio) = &peripheral.audio {
        lines.push(Line::blank());
        lines.push(Line::heading("音频"));
        lines.push(Line::kv(
            "声卡",
            format!(
                "card {} [{}] · {} · {}",
                audio.index, audio.id, audio.driver, audio.name
            ),
        ));
        for pcm in &audio.pcms {
            lines.push(Line::text(format!("  {pcm}")));
        }
    }

    if !peripheral.nets.is_empty() {
        lines.push(Line::blank());
        lines.push(Line::heading("网络"));
        for net in &peripheral.nets {
            let mut text = net.label();
            if !net.driver.is_empty() {
                text.push_str(&format!(" · {}", net.driver));
            }
            lines.push(Line::text(format!("  {text}")));
        }
    }

    let mut nodes = Vec::new();
    if !peripheral.videos.is_empty() {
        nodes.push(("视频", peripheral.videos.join(" ")));
    }
    if !peripheral.ttys.is_empty() {
        nodes.push(("串口", peripheral.ttys.join(" ")));
    }
    if !peripheral.printers.is_empty() {
        nodes.push(("打印", peripheral.printers.join(" ")));
    }
    if !nodes.is_empty() {
        lines.push(Line::blank());
        for (label, value) in nodes {
            lines.push(Line::kv(label, value));
        }
    }

    if usb.is_hub() {
        lines.push(Line::blank());
        lines.push(Line::heading("Hub"));
        lines.push(Line::kv(
            "形态",
            format!(
                "{} · {} 个下行口",
                usb.hub_kind.unwrap_or(HubKind::Hub).label(),
                usb.maxchild
            ),
        ));
        if !usb.children.is_empty() {
            let names: Vec<String> = usb
                .children
                .iter()
                .map(|child| name_of(snapshot, child))
                .collect();
            lines.push(Line::kv("下挂", names.join("、")));
        }
    }

    Details::Device {
        title: peripheral.name(),
        tags: tags.join(" · "),
        lines,
    }
}

fn battery_line(batteries: &HashMap<String, Battery>, hid: &HidInfo) -> Line {
    let mut spans = vec![Span::new("      电量 ", Tone::Dim)];
    match battery_of(batteries, hid) {
        Battery::Ok {
            pct, charging, hz, ..
        } => {
            spans.push(Span::new(battery_bar(pct, 10), battery_tone(pct)));
            let mut extra = Vec::new();
            if charging {
                extra.push("充电中".to_string());
            }
            if let Some(hz) = hz {
                extra.push(format!("{hz} Hz"));
            }
            if !extra.is_empty() {
                spans.push(Span::new(format!("  {}", extra.join(" · ")), Tone::Dim));
            }
        }
        Battery::Loading => spans.push(Span::new("读取中…", Tone::Faint)),
        Battery::Unknown => spans.push(Span::new("—（该接口无已知协议）", Tone::Faint)),
        Battery::Err(error) => {
            spans.push(Span::new(format!("读取失败：{error}"), Tone::Bad));
            if error.contains("Permission denied") {
                spans.push(Span::new("（见 README 的 udev 规则）", Tone::Faint));
            }
        }
    }
    Line { spans }
}

fn battery_of(batteries: &HashMap<String, Battery>, hid: &HidInfo) -> Battery {
    batteries
        .get(&hid.node)
        .cloned()
        .unwrap_or(Battery::Unknown)
}

fn battery_tone(pct: u8) -> Tone {
    if pct <= 20 {
        Tone::Bad
    } else if pct <= 40 {
        Tone::Warn
    } else {
        Tone::Good
    }
}

fn usage_tone(pct: u8) -> Tone {
    if pct >= 90 {
        Tone::Bad
    } else if pct >= 75 {
        Tone::Warn
    } else {
        Tone::Good
    }
}

fn part_index(peripheral: &Peripheral, part_sel: &HashMap<String, usize>) -> usize {
    let total: usize = peripheral.disks.iter().map(|d| d.partitions.len()).sum();
    if total == 0 {
        return 0;
    }
    part_sel.get(&peripheral.key()).copied().unwrap_or(0) % total
}

fn chain_of(snapshot: &Snapshot, peripheral: &Peripheral) -> String {
    let usb = &peripheral.usb;
    let mut parts = Vec::new();
    if let Some(controller) = &usb.controller {
        parts.push(controller.slot.clone());
    }
    let root_port = usb
        .root_port
        .map(|port| format!("根端口 {port}"))
        .unwrap_or_else(|| "根端口".into());
    parts.push(root_port);
    for ancestor in &usb.chain {
        parts.push(format!("{}「{}」", ancestor, name_of(snapshot, ancestor)));
    }
    parts.push(usb.sysfs.clone());
    parts.join(" → ")
}

fn name_of(snapshot: &Snapshot, sysfs: &str) -> String {
    snapshot
        .peripherals
        .iter()
        .find(|peripheral| peripheral.usb.sysfs == sysfs)
        .map(Peripheral::name)
        .unwrap_or_else(|| sysfs.to_string())
}

fn pad_right(text: &str, width: usize) -> String {
    let mut text = text.to_string();
    let used = UnicodeWidthStr::width(text.as_str());
    if used < width {
        text.push_str(&" ".repeat(width - used));
    }
    text
}

fn mount_label(mount: &MountPoint) -> String {
    let mut text = mount.path.clone();
    if mount.read_only {
        text.push_str(" (ro)");
    }
    if let Some(usage) = mount.usage_label() {
        text.push_str(&format!(" · {usage}"));
    }
    text
}

fn page_name(page: u16) -> &'static str {
    match page {
        0x0001 => "generic",
        0x0002 => "simulation",
        0x0005 => "game",
        0x0006 => "device",
        0x0007 => "keyboard",
        0x0008 => "led",
        0x0009 => "button",
        0x000A => "ordinal",
        0x000B => "telephony",
        0x000C => "consumer",
        0x000D => "digitizer",
        0x000E => "haptics",
        0x0012 => "oscilloscope",
        0x0014 => "barcode",
        0x0020 => "sensors",
        0x0084 => "power",
        0xFF00..=0xFFFF => "vendor",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_by_display_width() {
        assert_eq!(pad_right("abc", 5), "abc  ");
        assert_eq!(pad_right("中文", 6), "中文  ");
        assert_eq!(pad_right("abcdef", 3), "abcdef");
    }
}
