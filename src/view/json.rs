//! CLI JSON：与 TUI 共用同一份设备语义，只做机器可读的投影。

use serde_json::json;

use crate::model::{Category, HubKind, MountPoint, Peripheral};

pub fn peripheral(peripheral: &Peripheral) -> serde_json::Value {
    let disks: Vec<_> = peripheral
        .disks
        .iter()
        .map(|disk| {
            json!({
                "name": disk.name,
                "path": disk.path,
                "size": disk.size,
                "model": disk.model,
                "serial": disk.serial,
                "removable": disk.removable,
                "transport": disk.transport,
                "rotational": disk.rotational,
                "ptable": disk.ptable,
                "mount": disk.mount.as_ref().map(mount_json),
                "partitions": disk.partitions.iter().map(|part| json!({
                    "name": part.name,
                    "path": part.path,
                    "size": part.size,
                    "fstype": part.fstype,
                    "label": part.label,
                    "uuid": part.uuid,
                    "mount": part.mount.as_ref().map(mount_json),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    json!({
        "sysfs": peripheral.usb.sysfs,
        "bus": peripheral.usb.bus,
        "devnum": peripheral.usb.devnum,
        "port": peripheral.usb.port,
        "root_port": peripheral.usb.root_port,
        "depth": peripheral.usb.depth,
        "chain": peripheral.usb.chain,
        "place": if peripheral.usb.place.is_internal() { "internal" } else { "external" },
        "place_reason": peripheral.usb.place_reason,
        "attachment": peripheral.usb.attachment(),
        "link": peripheral.usb.link_label(),
        "dock": peripheral.usb.dock,
        "hub_kind": peripheral.usb.hub_kind.map(|kind| match kind {
            HubKind::Hub => "hub",
            HubKind::Dock => "dock",
        }),
        "controller": peripheral.usb.controller.as_ref().map(|controller| json!({
            "slot": controller.slot,
            "driver": controller.driver,
            "vendor": format!("{:04x}", controller.vendor),
            "device": format!("{:04x}", controller.device),
            "buses": controller.buses,
        })),
        "id": peripheral.usb.id(),
        "vid": format!("{:04x}", peripheral.usb.vid),
        "pid": format!("{:04x}", peripheral.usb.pid),
        "name": peripheral.name(),
        "category": category_tag(peripheral.category),
        "speed": peripheral.usb.speed,
        "usb_version": peripheral.usb.usb_version,
        "bcd_device": peripheral.usb.bcd_device,
        "manufacturer": peripheral.usb.manufacturer,
        "product": peripheral.usb.product,
        "serial": peripheral.usb.serial,
        "power_ma": peripheral.usb.power_ma,
        "authorized": peripheral.usb.authorized,
        "ltm_capable": peripheral.usb.ltm_capable,
        "parent": peripheral.usb.parent,
        "parent_name": peripheral.usb.parent_name,
        "children": peripheral.usb.children,
        "interfaces": peripheral.usb.ifaces.iter().map(|iface| json!({
            "number": iface.number,
            "class": iface.class,
            "subclass": iface.subclass,
            "protocol": iface.protocol,
            "function": iface.function(),
            "driver": iface.driver,
            "endpoints": iface.endpoints,
        })).collect::<Vec<_>>(),
        "hids": peripheral.hids.iter().map(|hid| json!({
            "node": hid.node,
            "iface": hid.iface,
            "pages": hid.pages.iter().map(|p| format!("0x{p:04X}")).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "disks": disks,
        "audio": peripheral.audio.as_ref().map(|audio| json!({
            "index": audio.index,
            "id": audio.id,
            "driver": audio.driver,
            "name": audio.name,
            "pcms": audio.pcms,
        })),
        "videos": peripheral.videos,
    })
}

fn mount_json(mount: &MountPoint) -> serde_json::Value {
    json!({
        "path": mount.path,
        "read_only": mount.read_only,
        "total": mount.total,
        "free": mount.free,
        "used_percent": mount.used_pct(),
    })
}

pub(crate) fn category_tag(category: Category) -> &'static str {
    match category {
        Category::Storage => "storage",
        Category::Input => "input",
        Category::Audio => "audio",
        Category::Hub => "hub",
        Category::Other => "other",
    }
}
