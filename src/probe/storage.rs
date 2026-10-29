//! 块设备、挂载点与文件系统元数据。

use std::collections::HashMap;
use std::fs;
use std::process::Command;


use super::*;
use std::ffi::CString;
use std::sync::Mutex;
use std::time::{Duration, Instant};


pub(crate) fn scan_disks(
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

pub(crate) fn read_mounts() -> HashMap<(u32, u32), MountPoint> {
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
pub(crate) struct FsEntry {
    fstype: String,
    label: String,
    uuid: String,
    pttype: String,
}

type FsMeta = HashMap<String, FsEntry>;
static FS_CACHE: Mutex<Option<(Instant, FsMeta)>> = Mutex::new(None);

pub(crate) fn lsblk_meta() -> FsMeta {
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
    fn parses_majmin() {
        assert_eq!(parse_majmin("8:17"), Some((8, 17)));
        assert_eq!(parse_majmin("bad"), None);
    }
}
