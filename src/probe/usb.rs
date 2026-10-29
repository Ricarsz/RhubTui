//! USB 拓扑、控制器与位置判定。

use std::collections::{BTreeSet, HashMap};
use std::fs;

use super::*;

pub(crate) fn scan_usb() -> Vec<UsbDev> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
