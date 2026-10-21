//! Read the peripheral state out of sysfs/procfs.
//!
//! Everything here is unprivileged: USB topology and HID report descriptors live
//! in sysfs (world readable), mounts in `/proc/self/mountinfo`, filesystem
//! metadata is taken from `lsblk`, ALSA cards from `/proc/asound/cards`.

use std::collections::{BTreeSet, HashMap};
use std::ffi::CString;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::desc::ReportDescriptor;
use crate::model::*;

pub struct Snapshot {
    pub peripherals: Vec<Peripheral>,
}

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

fn read_str(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn read_hex(path: impl AsRef<Path>) -> u16 {
    let s = read_str(path);
    u16::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0)
}

fn read_u64(path: impl AsRef<Path>) -> u64 {
    read_str(path).parse().unwrap_or(0)
}

/// Last path component of a sysfs `driver` symlink.
fn driver_of(dir: &Path) -> String {
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
fn usb_of(path: &Path) -> Option<String> {
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
fn iface_of(path: &Path) -> Option<u8> {
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

// ─── USB ────────────────────────────────────────────────────────────────────

fn scan_usb() -> Vec<UsbDev> {
    let mut out = Vec::new();
    let Ok(dir) = fs::read_dir("/sys/bus/usb/devices") else {
        return out;
    };

    for entry in dir.flatten() {
        let sysfs = entry.file_name().to_string_lossy().to_string();
        // Device names are "bus-port[.port]"; root hubs ("usb1") are skipped.
        if !sysfs.contains('-') || sysfs.contains(':') {
            continue;
        }
        let base = entry.path();
        let mut dev = UsbDev {
            sysfs: sysfs.clone(),
            vid: read_hex(base.join("idVendor")),
            pid: read_hex(base.join("idProduct")),
            bcd_device: read_str(base.join("bcdDevice")),
            manufacturer: read_str(base.join("manufacturer")),
            product: read_str(base.join("product")),
            serial: read_str(base.join("serial")),
            speed: read_str(base.join("speed")),
            class: read_hex(base.join("bDeviceClass")) as u8,
            subclass: read_hex(base.join("bDeviceSubClass")) as u8,
            protocol: read_hex(base.join("bDeviceProtocol")) as u8,
            num_interfaces: read_u64(base.join("bNumInterfaces")) as u8,
            num_configs: read_u64(base.join("bNumConfigurations")) as u8,
            maxchild: read_u64(base.join("maxchild")) as u8,
            ltm_capable: read_str(base.join("ltm_capable")) == "yes",
            authorized: read_str(base.join("authorized")) != "0",
            usb_version: read_str(base.join("version")),
            root_bus: sysfs
                .split('-')
                .next()
                .and_then(|bus| bus.parse().ok())
                .unwrap_or(0),
            ..Default::default()
        };

        for (key, value) in parse_uevent(&read_str(base.join("uevent"))) {
            match key.as_str() {
                "BUSNUM" => dev.bus = value.parse().unwrap_or(0),
                "DEVNUM" => dev.devnum = value.parse().unwrap_or(0),
                _ => {}
            }
        }

        if let Some(power) = read_str(base.join("bMaxPower")).strip_suffix("mA") {
            dev.power_ma = power.trim().parse().ok();
        }

        if let Ok(children) = fs::read_dir(&base) {
            for child in children.flatten() {
                let name = child.file_name().to_string_lossy().to_string();
                if !name.starts_with(&format!("{sysfs}:")) {
                    continue;
                }
                let iface_dir = child.path();
                dev.ifaces.push(Iface {
                    number: read_hex(iface_dir.join("bInterfaceNumber")) as u8,
                    class: read_hex(iface_dir.join("bInterfaceClass")) as u8,
                    subclass: read_hex(iface_dir.join("bInterfaceSubClass")) as u8,
                    protocol: read_hex(iface_dir.join("bInterfaceProtocol")) as u8,
                    endpoints: read_hex(iface_dir.join("bNumEndpoints")) as u8,
                    driver: driver_of(&iface_dir),
                });
            }
        }
        dev.ifaces.sort_by_key(|iface| iface.number);
        out.push(dev);
    }

    // 控制器：同一 PCI 功能通常带 USB2 + USB3 两条根总线。
    let controllers = controller_map();

    // 拓扑：父设备、端口号、层级、链路、内置/外接。
    struct Meta {
        parent: Option<String>,
        port: Option<u8>,
        root_port: Option<u8>,
        depth: u8,
        chain: Vec<String>,
        place: Placement,
        reason: String,
    }
    let mut meta = Vec::with_capacity(out.len());
    for device in &out {
        let top = top_level(&device.sysfs);
        let (place, reason) = placement_of(top, device.root_bus);
        let parent = parent_of(&device.sysfs);
        let mut chain = Vec::new();
        let mut cursor = parent.clone();
        while let Some(name) = cursor {
            chain.push(name.clone());
            cursor = parent_of(&name);
        }
        chain.reverse();
        meta.push(Meta {
            parent,
            port: port_of(&device.sysfs),
            root_port: port_of(top),
            depth: device.sysfs.matches('.').count() as u8,
            chain,
            place,
            reason,
        });
    }
    for (device, meta) in out.iter_mut().zip(meta) {
        device.parent = meta.parent;
        device.port = meta.port;
        device.root_port = meta.root_port;
        device.depth = meta.depth;
        device.chain = meta.chain;
        device.place = meta.place;
        device.place_reason = meta.reason;
        device.controller = controllers.get(&device.root_bus).cloned();
    }

    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for device in &out {
        if let Some(parent) = &device.parent {
            children
                .entry(parent.clone())
                .or_default()
                .push(device.sysfs.clone());
        }
    }
    let names: HashMap<String, String> = out
        .iter()
        .map(|device| (device.sysfs.clone(), device.name()))
        .collect();

    // Hub / 扩展坞判定需要「这棵子树里有哪些功能接口」。
    let mut function_classes: HashMap<String, BTreeSet<u8>> = HashMap::new();
    for hub in out.iter().filter(|device| device.is_hub()) {
        let mut classes = BTreeSet::new();
        for device in &out {
            if device.sysfs == hub.sysfs {
                continue;
            }
            let behind = device.chain.iter().any(|ancestor| ancestor == &hub.sysfs);
            if behind {
                for iface in &device.ifaces {
                    classes.insert(iface.class);
                }
            }
        }
        function_classes.insert(hub.sysfs.clone(), classes);
    }

    let mut hub_kinds: HashMap<String, HubKind> = HashMap::new();
    for device in out.iter().filter(|device| device.is_hub()) {
        let classes = function_classes
            .get(&device.sysfs)
            .cloned()
            .unwrap_or_default();
        let name = format!("{} {}", device.manufacturer, device.product);
        hub_kinds.insert(
            device.sysfs.clone(),
            classify_hub(&name, device.maxchild, &classes),
        );
    }

    for device in &mut out {
        device.children = children.remove(&device.sysfs).unwrap_or_default();
        device.parent_name = device.parent.as_ref().and_then(|p| names.get(p).cloned());

        if device.is_hub() {
            device.hub_kind = hub_kinds.get(&device.sysfs).copied();
            // 自身是扩展坞时 dock 留空；否则记录上游扩展坞。
            for ancestor in device.chain.iter().rev() {
                if hub_kinds.get(ancestor) == Some(&HubKind::Dock) {
                    device.dock = names.get(ancestor).cloned();
                    break;
                }
            }
            if device.depth > 0
                && let Some(reason) = device.chain.last().and_then(|parent| names.get(parent))
            {
                device.place_reason = match device.place {
                    Placement::Internal => format!("接在内置 Hub「{reason}」上"),
                    Placement::External => format!("接在外接 Hub「{reason}」上"),
                };
            }
            continue;
        }

        // 最近一层 Hub 的形态 + 所属扩展坞名。
        for ancestor in device.chain.iter().rev() {
            if let Some(kind) = hub_kinds.get(ancestor) {
                device.hub_kind = Some(*kind);
                if *kind == HubKind::Dock {
                    device.dock = names.get(ancestor).cloned();
                }
                break;
            }
        }
        if device.depth > 0
            && let Some(reason) = device.chain.last().and_then(|parent| names.get(parent))
        {
            device.place_reason = match device.place {
                Placement::Internal => format!("接在内置 Hub「{reason}」上"),
                Placement::External => format!("接在外接 Hub「{reason}」上"),
            };
        }
    }

    out
}

/// 控制器映射：根总线号 → PCI 控制器。
fn controller_map() -> HashMap<u8, Controller> {
    let mut map: HashMap<u8, Controller> = HashMap::new();
    let Ok(dir) = fs::read_dir("/sys/bus/usb/devices") else {
        return map;
    };
    let mut per_slot: HashMap<String, Controller> = HashMap::new();
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(bus) = name
            .strip_prefix("usb")
            .and_then(|rest| rest.parse::<u8>().ok())
        else {
            continue;
        };
        let Ok(real) = fs::canonicalize(entry.path()) else {
            continue;
        };
        let Some(parent) = real.parent() else {
            continue;
        };
        let Some(slot) = parent.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let controller = per_slot.entry(slot.clone()).or_insert_with(|| Controller {
            slot,
            driver: driver_of(parent),
            vendor: read_hex(parent.join("vendor")),
            device: read_hex(parent.join("device")),
            buses: Vec::new(),
        });
        controller.buses.push(bus);
    }
    for controller in per_slot.values_mut() {
        controller.buses.sort_unstable();
        for bus in &controller.buses {
            map.insert(*bus, controller.clone());
        }
    }
    map
}

fn top_level(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

fn parent_of(name: &str) -> Option<String> {
    name.rsplit_once('.').map(|(parent, _)| parent.to_string())
}

fn port_of(name: &str) -> Option<u8> {
    name.rsplit(['-', '.']).next()?.parse().ok()
}

/// 内置判定：顶层设备标记为 fixed，或它所在的根端口不是热插拔口。
/// 返回判定结果和可展示的依据。
fn placement_of(top: &str, bus: u8) -> (Placement, String) {
    let removable = read_str(format!("/sys/bus/usb/devices/{top}/removable"));
    let connect_type = port_of(top)
        .and_then(|port| read_connect_type(bus, port))
        .unwrap_or_default();
    placement_from(&removable, &connect_type)
}

fn read_connect_type(bus: u8, port: u8) -> Option<String> {
    let candidates = [
        format!("/sys/bus/usb/devices/usb{bus}/{bus}-0:1.0/usb{bus}-port{port}/connect_type"),
        format!("/sys/bus/usb/devices/usb{bus}/{bus}-port{port}/connect_type"),
        format!("/sys/bus/usb/devices/usb{bus}/usb{bus}-port{port}/connect_type"),
    ];
    candidates
        .iter()
        .find_map(|path| fs::read_to_string(path).ok())
        .map(|value| value.trim().to_string())
}

fn placement_from(removable: &str, connect_type: &str) -> (Placement, String) {
    if removable == "fixed" {
        return (Placement::Internal, "根端口标记为固定焊接".into());
    }
    match connect_type {
        "hardwired" | "not used" => (
            Placement::Internal,
            format!("根端口连接类型为 {connect_type}"),
        ),
        "hotplug" | "hot-plug" => (Placement::External, "根端口可热插拔".into()),
        "" => (Placement::External, "无固定端口标记".into()),
        other => (Placement::External, format!("根端口连接类型为 {other}")),
    }
}

/// Hub 还是扩展坞：名称线索，或下挂功能足够丰富（音频 + 视频/网口…）。
fn classify_hub(name: &str, ports: u8, classes: &BTreeSet<u8>) -> HubKind {
    let lower = name.to_lowercase();
    if ["dock", "docking", "扩展坞", "thunderbolt", "dock station"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return HubKind::Dock;
    }
    let dock_functions = [0x01u8, 0x02, 0x06, 0x07, 0x0A, 0x0E, 0x11]
        .iter()
        .filter(|class| classes.contains(class))
        .count();
    if dock_functions >= 2 || (dock_functions >= 1 && ports >= 6) {
        HubKind::Dock
    } else {
        HubKind::Hub
    }
}

/// `uevent` is `KEY=VALUE` lines; only BUSNUM/DEVNUM are used here.
fn parse_uevent(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// ─── HID ────────────────────────────────────────────────────────────────────

fn scan_hidraw() -> HashMap<String, Vec<HidInfo>> {
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

// ─── block devices ──────────────────────────────────────────────────────────

fn scan_disks(
    mounts: &HashMap<(u32, u32), MountPoint>,
    fsmeta: &FsMeta,
) -> Vec<(Option<String>, Disk)> {
    let mut out = Vec::new();
    let Ok(dir) = fs::read_dir("/sys/block") else {
        return out;
    };

    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let base = entry.path();
        let removable = read_str(base.join("removable")) == "1";
        let usb = fs::canonicalize(&base).ok().and_then(|p| usb_of(&p));
        if usb.is_none() {
            continue;
        }
        if !removable && !name.starts_with("sd") {
            continue;
        }

        let mut mount = parse_majmin(&read_str(base.join("dev")))
            .and_then(|key| mounts.get(&key))
            .cloned();
        if let Some(mount) = mount.as_mut() {
            fill_usage(mount);
        }

        let mut partitions = Vec::new();
        if let Ok(children) = fs::read_dir(&base) {
            for child in children.flatten() {
                let pname = child.file_name().to_string_lossy().to_string();
                if pname == name || !pname.starts_with(&name) {
                    continue;
                }
                let ppath = child.path();
                if !ppath.join("partition").exists() {
                    continue;
                }
                let mut mount = parse_majmin(&read_str(ppath.join("dev")))
                    .and_then(|key| mounts.get(&key))
                    .cloned();
                if let Some(mount) = mount.as_mut() {
                    fill_usage(mount);
                }
                let entry = fsmeta.get(&pname);
                let fstype = entry.map(|e| e.fstype.clone()).unwrap_or_default();
                let label = entry.map(|e| e.label.clone()).unwrap_or_default();
                let uuid = entry.map(|e| e.uuid.clone()).unwrap_or_default();
                partitions.push(Partition {
                    name: pname.clone(),
                    path: format!("/dev/{pname}"),
                    size: read_u64(ppath.join("size")) * 512,
                    fstype,
                    label,
                    uuid,
                    mount,
                });
            }
        }
        partitions.sort_by(|a, b| a.name.cmp(&b.name));

        let meta = fsmeta.get(&name);
        out.push((
            usb,
            Disk {
                name: name.clone(),
                path: format!("/dev/{name}"),
                size: read_u64(base.join("size")) * 512,
                model: read_str(base.join("device/model")),
                serial: read_str(base.join("device/serial")),
                removable,
                transport: driver_of(&base.join("device")),
                rotational: read_str(base.join("queue/rotational")) == "1",
                ptable: meta.map(|e| e.pttype.clone()).unwrap_or_default(),
                mount,
                partitions,
            },
        ));
    }

    out
}

/// 通过 `statvfs` 补全挂载点的使用量；失败时保持 `None`。
fn fill_usage(mount: &mut MountPoint) {
    let Ok(path) = CString::new(mount.path.as_str()) else {
        return;
    };
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` 是以 NUL 结尾的有效字符串，`stat` 是有效的可写结构体。
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return;
    }
    let block = stat.f_frsize.max(1);
    mount.total = Some((stat.f_blocks as u64).saturating_mul(block));
    mount.free = Some((stat.f_bavail as u64).saturating_mul(block));
}

fn parse_majmin(text: &str) -> Option<(u32, u32)> {
    let (maj, min) = text.split_once(':')?;
    Some((maj.trim().parse().ok()?, min.trim().parse().ok()?))
}

fn read_mounts() -> HashMap<(u32, u32), MountPoint> {
    let mut out = HashMap::new();
    let text = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    for line in text.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        if fields.len() < 6 {
            continue;
        }
        let Some(key) = parse_majmin(fields[2]) else {
            continue;
        };
        let path = unescape_octal(fields[4]);
        let read_only = fields[5].split(',').any(|option| option == "ro");
        out.entry(key).or_insert(MountPoint {
            path,
            read_only,
            total: None,
            free: None,
        });
    }
    out
}

fn unescape_octal(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4].iter().all(u8::is_ascii_digit)
        {
            let value =
                (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            out.push(value);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Filesystem metadata is stable for seconds; scanning runs every second, so
/// avoid spawning `lsblk` on every tick.
#[derive(Debug, Clone, Default)]
struct FsEntry {
    fstype: String,
    label: String,
    uuid: String,
    pttype: String,
}

type FsMeta = HashMap<String, FsEntry>;
static FS_CACHE: Mutex<Option<(Instant, FsMeta)>> = Mutex::new(None);

fn lsblk_meta() -> FsMeta {
    if let Ok(guard) = FS_CACHE.lock()
        && let Some((at, map)) = guard.as_ref()
        && at.elapsed() < Duration::from_secs(5)
    {
        return map.clone();
    }

    let map = query_lsblk();
    if let Ok(mut guard) = FS_CACHE.lock() {
        *guard = Some((Instant::now(), map.clone()));
    }
    map
}

/// `lsblk -P` gives `KEY="value"` pairs; used for filesystem and partition metadata.
fn query_lsblk() -> FsMeta {
    let mut out = HashMap::new();
    let Ok(output) = Command::new("lsblk")
        .args(["-P", "-o", "NAME,FSTYPE,LABEL,UUID,PTTYPE"])
        .output()
    else {
        return out;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let pairs = parse_pairs(line);
        let get = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let name = get("NAME");
        if name.is_empty() {
            continue;
        }
        out.insert(
            name,
            FsEntry {
                fstype: get("FSTYPE"),
                label: get("LABEL"),
                uuid: get("UUID"),
                pttype: get("PTTYPE"),
            },
        );
    }
    out
}

fn parse_pairs(line: &str) -> Vec<(String, String)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let key = String::from_utf8_lossy(&bytes[key_start..i]).into_owned();
        i += 1;
        if i >= bytes.len() || bytes[i] != b'"' {
            break;
        }
        i += 1;
        let mut value = Vec::new();
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\'
                && i + 3 < bytes.len()
                && bytes[i + 1] == b'x'
                && let Ok(byte) = u8::from_str_radix(&line[i + 2..i + 4], 16)
            {
                value.push(byte);
                i += 4;
                continue;
            }
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 1;
            }
            value.push(bytes[i]);
            i += 1;
        }
        i += 1;
        out.push((key, String::from_utf8_lossy(&value).into_owned()));
    }
    out
}

// ─── audio ──────────────────────────────────────────────────────────────────

fn scan_audio() -> HashMap<String, AudioCard> {
    let text = fs::read_to_string("/proc/asound/cards").unwrap_or_default();
    let mut out = HashMap::new();
    for mut card in parse_cards(&text) {
        let link = format!("/sys/class/sound/card{}/device", card.index);
        if let Ok(real) = fs::canonicalize(link)
            && let Some(usb) = usb_of(&real)
        {
            card.pcms = read_pcms(card.index);
            out.insert(usb, card);
        }
    }
    out
}

fn read_audio(path: &Path, key: &str) -> String {
    read_str(path.join(key))
}

fn read_pcms(index: u32) -> Vec<String> {
    let dir = format!("/proc/asound/card{index}");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("pcm") {
            continue;
        }
        let info = entry.path().join("info");
        let id = read_audio(&info, "id");
        if id.is_empty() {
            continue;
        }
        out.push(format!("{name} · {id}"));
    }
    out.sort();
    out
}

fn parse_cards(text: &str) -> Vec<AudioCard> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let Some((index_part, rest)) = trimmed.split_once(" [") else {
            continue;
        };
        let Ok(index) = index_part.trim().parse::<u32>() else {
            continue;
        };
        let Some((id, rest)) = rest.split_once(']') else {
            continue;
        };
        let rest = rest.trim_start_matches(':').trim();
        let (driver, name) = match rest.split_once(" - ") {
            Some((driver, name)) => (driver.trim().to_string(), name.trim().to_string()),
            None => (rest.to_string(), String::new()),
        };
        out.push(AudioCard {
            index,
            id: id.trim().to_string(),
            driver,
            name,
            pcms: Vec::new(),
        });
    }
    out
}

// ─── network / video / tty / printer ────────────────────────────────────────

fn scan_net() -> HashMap<String, Vec<NetIf>> {
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

fn scan_video() -> HashMap<String, Vec<String>> {
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

fn scan_tty() -> HashMap<String, Vec<String>> {
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

fn scan_printers() -> HashMap<String, Vec<String>> {
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

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lsblk_pairs_with_escapes() {
        let pairs = parse_pairs(r#"NAME="sdb1" FSTYPE="exfat" LABEL="U\x20disk""#);
        assert_eq!(pairs[0], ("NAME".into(), "sdb1".into()));
        assert_eq!(pairs[2], ("LABEL".into(), "U disk".into()));
    }

    #[test]
    fn unescapes_mount_points() {
        assert_eq!(
            unescape_octal(r"/run/media/user/My\040Disk"),
            "/run/media/user/My Disk"
        );
        assert_eq!(unescape_octal("/run/media/正常"), "/run/media/正常");
    }

    #[test]
    fn parses_alsa_cards() {
        let text = "\
 0 [PCH            ]: HDA-Intel - HDA Intel PCH
                      HDA Intel PCH at 0x50400000 irq 74
 2 [Device         ]: USB-Audio - USB Audio Device
                      Generic USB Audio at usb-0000:00:14.0-1, full speed
";
        let cards = parse_cards(text);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[1].index, 2);
        assert_eq!(cards[1].id, "Device");
        assert_eq!(cards[1].driver, "USB-Audio");
        assert_eq!(cards[1].name, "USB Audio Device");
    }

    #[test]
    fn locates_usb_device_behind_a_hub() {
        let path = Path::new("/sys/devices/usb7/7-1/7-1.4/7-1.4:1.2/0003:5253:1020.000E");
        assert_eq!(usb_of(path).as_deref(), Some("7-1.4"));
        assert_eq!(iface_of(path), Some(2));
    }

    #[test]
    fn locates_usb_device_from_block_path() {
        let path =
            Path::new("/sys/devices/usb8/8-1/8-1.1/8-1.1:1.0/host8/target8:0:0/8:0:0:0/block/sda");
        assert_eq!(usb_of(path).as_deref(), Some("8-1.1"));
    }

    #[test]
    fn ignores_non_usb_paths() {
        assert_eq!(usb_of(Path::new("/sys/devices/platform/foo/bar")), None);
    }

    #[test]
    fn splits_usb_topology() {
        assert_eq!(top_level("7-1.4"), "7-1");
        assert_eq!(top_level("1-4"), "1-4");
        assert_eq!(parent_of("7-1.4"), Some("7-1".into()));
        assert_eq!(parent_of("7-1"), None);
        assert_eq!(port_of("7-1.4"), Some(4));
        assert_eq!(port_of("1-4"), Some(4));
        assert_eq!(port_of("7-1"), Some(1));
    }

    #[test]
    fn classifies_internal_devices() {
        assert_eq!(placement_from("fixed", "").0, Placement::Internal);
        assert_eq!(
            placement_from("removable", "not used").0,
            Placement::Internal
        );
        assert_eq!(
            placement_from("unknown", "hardwired").0,
            Placement::Internal
        );
        assert_eq!(
            placement_from("removable", "hotplug").0,
            Placement::External
        );
        assert_eq!(placement_from("unknown", "").0, Placement::External);
    }

    #[test]
    fn parses_majmin() {
        assert_eq!(parse_majmin("8:17"), Some((8, 17)));
        assert_eq!(parse_majmin("bad"), None);
    }

    #[test]
    fn detects_docks_by_name_and_functions() {
        let none = BTreeSet::new();
        assert_eq!(classify_hub("USB2.1 Hub", 4, &none), HubKind::Hub);
        assert_eq!(classify_hub("Thunderbolt Dock", 4, &none), HubKind::Dock);
        let mut classes = BTreeSet::new();
        classes.insert(0x01);
        classes.insert(0x0A);
        assert_eq!(classify_hub("USB Billboard", 8, &classes), HubKind::Dock);
        let mut single = BTreeSet::new();
        single.insert(0x08);
        assert_eq!(classify_hub("Card Reader Hub", 4, &single), HubKind::Hub);
    }

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
