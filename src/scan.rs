//! Read the peripheral state out of sysfs/procfs.
//!
//! Everything here is unprivileged: USB topology and HID report descriptors live
//! in sysfs (world readable), mounts in `/proc/self/mountinfo`, filesystem
//! metadata is taken from `lsblk`, ALSA cards from `/proc/asound/cards`.

use std::collections::HashMap;
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

    let mut peripherals = Vec::new();
    for usb in usbs {
        let hid_info = hid.get(&usb.sysfs).cloned();
        let audio_info = audio.get(&usb.sysfs).cloned();

        let mut dev_disks: Vec<Disk> = disks
            .iter()
            .filter(|(owner, _)| owner.as_deref() == Some(usb.sysfs.as_str()))
            .map(|(_, disk)| disk.clone())
            .collect();
        dev_disks.sort_by(|a, b| a.name.cmp(&b.name));

        let has = |class: u8| usb.class == class || usb.ifaces.iter().any(|i| i.class == class);

        let category = if !dev_disks.is_empty() || has(0x08) {
            Category::Storage
        } else if audio_info.is_some() || has(0x01) {
            Category::Audio
        } else if hid_info.is_some() || has(0x03) {
            Category::Input
        } else if has(0x09) {
            Category::Hub
        } else {
            Category::Other
        };

        peripherals.push(Peripheral {
            usb,
            hid: hid_info,
            audio: audio_info,
            disks: dev_disks,
            category,
        });
    }

    peripherals.sort_by(|a, b| {
        (a.category, a.name().to_lowercase()).cmp(&(b.category, b.name().to_lowercase()))
    });

    Snapshot { peripherals }
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
            manufacturer: read_str(base.join("manufacturer")),
            product: read_str(base.join("product")),
            serial: read_str(base.join("serial")),
            speed: read_str(base.join("speed")),
            class: read_hex(base.join("bDeviceClass")) as u8,
            maxchild: read_u64(base.join("maxchild")) as u8,
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
                let number = read_hex(iface_dir.join("bInterfaceNumber")) as u8;
                let class = read_hex(iface_dir.join("bInterfaceClass")) as u8;
                dev.ifaces.push(Iface {
                    number,
                    class,
                    driver: driver_of(&iface_dir),
                });
            }
        }
        dev.ifaces.sort_by_key(|i| i.number);
        out.push(dev);
    }

    out
}

/// `uevent` is `KEY=VALUE` lines; only BUSNUM/DEVNUM are used here.
fn parse_uevent(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// ─── HID ────────────────────────────────────────────────────────────────────

fn scan_hidraw() -> HashMap<String, HidInfo> {
    let mut out: HashMap<String, HidInfo> = HashMap::new();
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
            pages: desc.usage_pages.iter().copied().collect(),
            mchose,
            cmd_out: desc
                .report(0x11)
                .map(|r| r.output_bytes)
                .unwrap_or(20)
                .max(2),
            data_out: desc
                .report(0x12)
                .map(|r| r.output_bytes)
                .unwrap_or(64)
                .max(2),
            in_len: desc
                .report(0x13)
                .map(|r| r.input_bytes)
                .unwrap_or(20)
                .max(17),
            battery_feature: desc.battery_feature_report(),
            keyboard,
        };

        // A receiver exposes several HID interfaces. Prefer the MCHOSE one,
        // then any vendor page (that is where an unknown protocol would live).
        let score = |info: &HidInfo| {
            if info.mchose {
                3
            } else if info.keyboard {
                2
            } else if info.pages.iter().any(|page| *page >= 0xFF00) {
                1
            } else {
                0
            }
        };
        match out.get(&usb) {
            Some(existing) if score(existing) >= score(&info) => {}
            _ => {
                out.insert(usb, info);
            }
        }
    }
    out
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

        let size = read_u64(base.join("size")) * 512;
        let model = read_str(base.join("device/model"));
        let serial = read_str(base.join("device/serial"));
        let mount = parse_majmin(&read_str(base.join("dev")))
            .and_then(|key| mounts.get(&key))
            .cloned();

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
                let mount = parse_majmin(&read_str(ppath.join("dev")))
                    .and_then(|key| mounts.get(&key).cloned());
                let (fstype, label) = fsmeta.get(&pname).cloned().unwrap_or_default();
                partitions.push(Partition {
                    name: pname.clone(),
                    path: format!("/dev/{pname}"),
                    size: read_u64(ppath.join("size")) * 512,
                    fstype,
                    label,
                    mount,
                });
            }
        }
        partitions.sort_by(|a, b| a.name.cmp(&b.name));

        out.push((
            usb,
            Disk {
                name: name.clone(),
                path: format!("/dev/{name}"),
                size,
                model,
                serial,
                removable,
                mount,
                partitions,
            },
        ));
    }

    out
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
        out.entry(key).or_insert(MountPoint { path, read_only });
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
type FsMeta = HashMap<String, (String, String)>;
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

/// `lsblk -P` gives `KEY="value"` pairs; used for filesystem type and label.
fn query_lsblk() -> FsMeta {
    let mut out = HashMap::new();
    let Ok(output) = Command::new("lsblk")
        .args(["-P", "-o", "NAME,FSTYPE,LABEL"])
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
        out.insert(name, (get("FSTYPE"), get("LABEL")));
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
    for card in parse_cards(&text) {
        let link = format!("/sys/class/sound/card{}/device", card.index);
        if let Ok(real) = fs::canonicalize(link)
            && let Some(usb) = usb_of(&real)
        {
            out.insert(usb, card);
        }
    }
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
        });
    }
    out
}

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
    fn parses_majmin() {
        assert_eq!(parse_majmin("8:17"), Some((8, 17)));
        assert_eq!(parse_majmin("bad"), None);
    }
}
