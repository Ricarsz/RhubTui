//! 测试夹具：构造一份稳定、不依赖真实硬件的快照。

use crate::model::{
    AudioCard, Category, Controller, Disk, HidInfo, HubKind, Iface, MountPoint, Partition,
    Peripheral, Placement, UsbDev,
};
use crate::scan::Snapshot;

fn usb(sysfs: &str, bus: u8, port: u8, vid: u16, pid: u16, product: &str) -> UsbDev {
    UsbDev {
        sysfs: sysfs.to_string(),
        bus,
        devnum: 14,
        vid,
        pid,
        bcd_device: "0100".into(),
        manufacturer: "Example".into(),
        product: product.into(),
        serial: "0000000000".into(),
        speed: "480".into(),
        class: 0x00,
        power_ma: Some(500),
        maxchild: 0,
        ltm_capable: false,
        authorized: true,
        usb_version: "2.10".into(),
        place: Placement::External,
        place_reason: format!("根端口 {bus}-1"),
        root_bus: bus,
        root_port: Some(1),
        depth: 1,
        chain: vec![format!("{bus}-1")],
        parent: Some(format!("{bus}-1")),
        parent_name: Some("USB2.0 Hub".into()),
        port: Some(port),
        children: Vec::new(),
        hub_kind: None,
        dock: None,
        controller: Some(Controller {
            slot: "00:14.0".into(),
            driver: "xhci_hcd".into(),
            vendor: 0x1022,
            device: 0x15b9,
            buses: vec![7, 8],
        }),
        ifaces: vec![Iface {
            number: 1,
            class: 0x03,
            subclass: 0x01,
            protocol: 0x02,
            endpoints: 1,
            driver: "usbhid".into(),
        }],
    }
}

fn mount(path: &str, total: u64, free: u64) -> MountPoint {
    MountPoint {
        path: path.into(),
        read_only: false,
        total: Some(total),
        free: Some(free),
    }
}

/// 鼠标、U 盘、扩展坞与内置摄像头各一个。
pub fn snapshot() -> Snapshot {
    let mouse = Peripheral {
        usb: UsbDev {
            manufacturer: "Example".into(),
            product: "A5 Pro Max".into(),
            ..usb("7-1.4", 7, 4, 0x5253, 0x1020, "A5 Pro Max")
        },
        hids: vec![HidInfo {
            node: "/dev/hidraw2".into(),
            iface: Some(1),
            pages: vec![0xFF01],
        }],
        audio: None,
        videos: Vec::new(),
        disks: Vec::new(),
        category: Category::Input,
    };

    let partition = Partition {
        name: "sda1".into(),
        path: "/dev/sda1".into(),
        size: 64_000_000_000,
        fstype: "exfat".into(),
        label: "T7".into(),
        uuid: "1234-ABCD".into(),
        mount: Some(mount("/media/user/T7", 64_000_000_000, 32_000_000_000)),
    };
    let storage = Peripheral {
        usb: UsbDev {
            product: "PSSD T7".into(),
            manufacturer: "Example".into(),
            ifaces: vec![
                Iface {
                    number: 0,
                    class: 0x08,
                    subclass: 0x06,
                    protocol: 0x50,
                    endpoints: 2,
                    driver: "uas".into(),
                },
                Iface {
                    number: 1,
                    class: 0x08,
                    subclass: 0x06,
                    protocol: 0x62,
                    endpoints: 2,
                    driver: "uas".into(),
                },
            ],
            ..usb("7-1.3", 7, 3, 0x04e8, 0x61f5, "PSSD T7")
        },
        hids: Vec::new(),
        audio: None,
        videos: Vec::new(),
        disks: vec![Disk {
            name: "sda".into(),
            path: "/dev/sda".into(),
            size: 64_000_000_000,
            model: "Samsung PSSD T7".into(),
            serial: "S6EXAMPLE".into(),
            removable: true,
            transport: "uas".into(),
            rotational: false,
            ptable: "gpt".into(),
            mount: None,
            partitions: vec![partition],
        }],
        category: Category::Storage,
    };

    let dock = Peripheral {
        usb: UsbDev {
            product: "USB4 Dock".into(),
            class: 0x09,
            maxchild: 4,
            hub_kind: Some(HubKind::Dock),
            children: vec!["7-1.3".into(), "7-1.4".into()],
            dock: Some("USB4 Dock".into()),
            ifaces: vec![Iface {
                number: 0,
                class: 0x09,
                ..Iface::default()
            }],
            ..usb("7-1", 7, 1, 0x2109, 0x8888, "USB4 Dock")
        },
        hids: Vec::new(),
        audio: Some(AudioCard {
            index: 2,
            id: "Dock".into(),
            driver: "USB-Audio".into(),
            name: "USB4 Dock Audio".into(),
            pcms: vec!["usbstream".into()],
        }),
        videos: Vec::new(),
        disks: Vec::new(),
        category: Category::Hub,
    };

    let camera = Peripheral {
        usb: UsbDev {
            product: "Integrated Camera".into(),
            place: Placement::Internal,
            place_reason: "主板内置".into(),
            root_port: None,
            depth: 0,
            parent: None,
            parent_name: None,
            port: None,
            ..usb("3-2", 3, 2, 0x13d3, 0x5406, "Integrated Camera")
        },
        hids: Vec::new(),
        audio: None,
        videos: vec!["Integrated Camera".into()],
        disks: Vec::new(),
        category: Category::Other,
    };

    Snapshot {
        peripherals: vec![storage, mouse, dock, camera],
    }
}
