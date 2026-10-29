//! CLI 子命令。无子命令时由 `main` 启动 TUI。

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use clap::{Parser, Subcommand};
use serde_json::json;
use unicode_width::UnicodeWidthStr;

use crate::battery;
use crate::config::Config;
use crate::hidraw;
use crate::model::{Category, HidInfo, Peripheral};
use crate::mount;
use crate::ops;
use crate::scan::{self, Snapshot};

#[derive(Parser)]
#[command(
    name = "rhubtui",
    version,
    about = "2.4G 键鼠电量 + USB 外设管理",
    long_about = "读取 MCHOSE 2.4G 接收器与标准 HID 通道的电量，列出 USB 外设与拓扑，\
                  并通过 udisks2 挂载/卸载/弹出存储设备。无子命令时启动 TUI。"
)]
pub struct Cli {
    /// 以 JSON 输出
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// 打印一次设备快照
    List,
    /// 读取键鼠电量
    Battery {
        /// 设备选择：端口（7-1.4）、vid:pid（5253:1020）或 /dev/hidrawN
        device: Option<String>,
    },
    /// 挂载存储设备（默认第一个未挂载分区）
    Mount { device: Option<String> },
    /// 卸载存储设备（默认第一个已挂载分区）
    Umount { device: Option<String> },
    /// 安全弹出（默认第一个未挂载磁盘）
    Eject { device: Option<String> },
    /// 抓取 HID 原始报文
    Probe {
        /// HID 节点，例如 /dev/hidraw2
        node: String,
        /// 抓取时长（秒）
        #[arg(long, default_value_t = 10)]
        seconds: u64,
        /// 按 0xFF 异或解码 payload
        #[arg(long)]
        decode: bool,
    },
    /// 检查依赖、udev 规则与 hidraw 权限
    Doctor,
    /// 显示配置文件路径与当前值
    Config {
        /// 写入默认配置文件
        #[arg(long)]
        init: bool,
        /// 覆盖已有配置文件（配合 --init）
        #[arg(long)]
        force: bool,
    },
}

pub fn run(cmd: Cmd, json: bool) -> Result<()> {
    match cmd {
        Cmd::List => list(json),
        Cmd::Battery { device } => read_batteries(device.as_deref(), json),
        Cmd::Mount { device } => mount_device(device.as_deref(), json),
        Cmd::Umount { device } => umount_device(device.as_deref(), json),
        Cmd::Eject { device } => eject_device(device.as_deref(), json),
        Cmd::Probe {
            node,
            seconds,
            decode,
        } => probe(&node, seconds, decode),
        Cmd::Doctor => doctor(json),
        Cmd::Config { init, force } => show_config(init, force, json),
    }
}

// ─── list ───────────────────────────────────────────────────────────────────

fn list(json: bool) -> Result<()> {
    let snapshot = scan::scan();
    if json {
        let peripherals: Vec<_> = snapshot.peripherals.iter().map(peripheral_json).collect();
        println!("{}", serde_json::to_string_pretty(&peripherals)?);
        return Ok(());
    }
    println!("{}", render_table(&list_rows(&snapshot)));
    Ok(())
}

fn list_rows(snapshot: &Snapshot) -> Vec<Vec<String>> {
    let mut rows = vec![vec![
        "PORT".to_string(),
        "LINK".to_string(),
        "ID".to_string(),
        "TYPE".to_string(),
        "PLACE".to_string(),
        "NAME".to_string(),
        "EXTRA".to_string(),
    ]];
    for peripheral in &snapshot.peripherals {
        rows.push(vec![
            peripheral.usb.sysfs.clone(),
            peripheral.usb.link_label(),
            peripheral.usb.id(),
            category_tag(peripheral.category).to_string(),
            place_tag(peripheral),
            peripheral.name(),
            extra_summary(peripheral),
        ]);
    }
    rows
}

/// 对齐成纯文本表格；返回不含结尾换行的多行字符串。
fn render_table(rows: &[Vec<String>]) -> String {
    let columns = rows.first().map_or(0, Vec::len);
    let mut widths = vec![0usize; columns];
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(UnicodeWidthStr::width(cell.as_str()));
        }
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let mut line = String::new();
            for (index, cell) in row.iter().enumerate() {
                if index + 1 == row.len() {
                    line.push_str(cell);
                } else {
                    line.push_str(&pad(cell, widths[index] + 2));
                }
            }
            line.trim_end().to_string()
        })
        .collect();
    lines.join("\n")
}

fn pad(text: &str, width: usize) -> String {
    let mut text = text.to_string();
    let used = UnicodeWidthStr::width(text.as_str());
    if used < width {
        text.push_str(&" ".repeat(width - used));
    }
    text
}

fn place_tag(peripheral: &Peripheral) -> String {
    let mut tag = peripheral.usb.place.label().to_string();
    if peripheral.usb.on_dock() {
        tag.push_str("/坞上");
    }
    tag
}

fn extra_summary(peripheral: &Peripheral) -> String {
    let mut extra = Vec::new();
    let supported: Vec<&HidInfo> = peripheral
        .hids
        .iter()
        .filter(|hid| hid.has_battery())
        .collect();
    for hid in &supported {
        extra.push(format!("{} {}", hid.node, hid.protocol_name()));
    }
    if peripheral.hids.len() > supported.len() {
        extra.push(format!("共 {} 个 HID 接口", peripheral.hids.len()));
    }
    for disk in &peripheral.disks {
        for partition in &disk.partitions {
            let state = match &partition.mount {
                Some(mount) => format!("mounted {}", mount.path),
                None => "unmounted".to_string(),
            };
            extra.push(format!("{} {}", partition.name, state));
        }
        if let Some(mount) = &disk.mount {
            extra.push(format!("{} mounted {}", disk.name, mount.path));
        }
        if disk.partitions.is_empty() {
            extra.push(format!("{} 无介质", disk.name));
        }
    }
    if let Some(audio) = &peripheral.audio {
        extra.push(format!("card {} {}", audio.index, audio.id));
    }
    for net in &peripheral.nets {
        extra.push(net.label());
    }
    if !peripheral.videos.is_empty() {
        extra.push(peripheral.videos.join(" "));
    }
    if !peripheral.ttys.is_empty() {
        extra.push(peripheral.ttys.join(" "));
    }
    if !peripheral.printers.is_empty() {
        extra.push(peripheral.printers.join(" "));
    }
    if peripheral.usb.is_hub() {
        extra.push(format!(
            "{} · {} 口 · {} 台",
            peripheral
                .usb
                .hub_kind
                .unwrap_or(crate::model::HubKind::Hub)
                .label(),
            peripheral.usb.maxchild,
            peripheral.usb.children.len()
        ));
    }
    if extra.is_empty() {
        "-".to_string()
    } else {
        extra.join(" · ")
    }
}

fn peripheral_json(peripheral: &Peripheral) -> serde_json::Value {
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
            crate::model::HubKind::Hub => "hub",
            crate::model::HubKind::Dock => "dock",
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
            "protocol": hid.protocol_name(),
            "battery_supported": hid.has_battery(),
            "reports": {
                "cmd": hid.cmd_out,
                "data": hid.data_out,
                "in": hid.in_len,
            },
        })).collect::<Vec<_>>(),
        "disks": disks,
        "audio": peripheral.audio.as_ref().map(|audio| json!({
            "index": audio.index,
            "id": audio.id,
            "driver": audio.driver,
            "name": audio.name,
            "pcms": audio.pcms,
        })),
        "nets": peripheral.nets.iter().map(|net| json!({
            "name": net.name,
            "mac": net.mac,
            "speed_mbps": net.speed_mbps,
            "state": net.state,
            "driver": net.driver,
        })).collect::<Vec<_>>(),
        "videos": peripheral.videos,
        "ttys": peripheral.ttys,
        "printers": peripheral.printers,
    })
}

fn mount_json(mount: &crate::model::MountPoint) -> serde_json::Value {
    json!({
        "path": mount.path,
        "read_only": mount.read_only,
        "total": mount.total,
        "free": mount.free,
        "used_percent": mount.used_pct(),
    })
}

fn category_tag(category: Category) -> &'static str {
    match category {
        Category::Storage => "storage",
        Category::Input => "input",
        Category::Audio => "audio",
        Category::Network => "network",
        Category::Hub => "hub",
        Category::Other => "other",
    }
}

// ─── battery ────────────────────────────────────────────────────────────────

fn read_batteries(selector: Option<&str>, json: bool) -> Result<()> {
    let snapshot = scan::scan();
    let targets: Vec<(&Peripheral, &HidInfo)> = snapshot
        .peripherals
        .iter()
        .flat_map(|peripheral| {
            peripheral
                .hids
                .iter()
                .filter(|hid| battery::supported(hid))
                .map(move |hid| (peripheral, hid))
        })
        .filter(|(peripheral, hid)| match selector {
            Some(selector) => matches_hid(peripheral, hid, selector),
            None => true,
        })
        .collect();

    if targets.is_empty() {
        bail!("没有找到可读电量的设备（MCHOSE 接收器或标准 HID 电量通道）");
    }

    // 并行读取：休眠鼠标最坏要等 6 秒，不能让它拖住键盘。
    let results: Vec<Result<crate::mchose::Status>> = std::thread::scope(|scope| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(_, hid)| {
                let hid = (*hid).clone();
                scope.spawn(move || battery::read(&hid))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err(anyhow!("电量读取线程崩溃")))
            })
            .collect()
    });

    let mut readings = Vec::new();
    for ((peripheral, hid), result) in targets.into_iter().zip(results) {
        match result {
            Ok(status) => readings.push((peripheral, hid, status)),
            Err(error) => eprintln!("rhubtui: {} ({})：{error:#}", peripheral.name(), hid.node),
        }
    }

    if json {
        let values: Vec<_> = readings
            .iter()
            .map(|(peripheral, hid, status)| {
                json!({
                    "device": peripheral.name(),
                    "sysfs": peripheral.usb.sysfs,
                    "node": hid.node,
                    "iface": hid.iface,
                    "protocol": hid.protocol_name(),
                    "percent": status.battery,
                    "charging": status.charging,
                    "polling_hz": status.polling_hz,
                    "dpi_level": status.dpi,
                    "wireless": status.wireless,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        for (peripheral, hid, status) in &readings {
            let mut extra = Vec::new();
            if status.charging {
                extra.push("charging".to_string());
            }
            if let Some(wireless) = status.wireless {
                extra.push(if wireless { "2.4G" } else { "USB" }.to_string());
            }
            if let Some(dpi) = status.dpi {
                extra.push(format!("DPI{dpi}"));
            }
            if let Some(hz) = status.polling_hz {
                extra.push(format!("{hz}Hz"));
            }
            println!(
                "{:<14} {:>3}%  {:<18} {}",
                hid.node,
                status.battery,
                extra.join(" "),
                crate::model::truncate_cols(&peripheral.name(), 40)
            );
        }
    }

    if readings.is_empty() {
        bail!("所有设备都读取失败");
    }
    Ok(())
}

fn matches_hid(peripheral: &Peripheral, hid: &HidInfo, selector: &str) -> bool {
    let selector = selector.trim().to_lowercase();
    if selector == peripheral.usb.sysfs
        || selector == peripheral.usb.id()
        || normalize_id(&selector) == peripheral.usb.id()
    {
        return true;
    }
    let node = hid.node.trim_start_matches("/dev/").to_lowercase();
    let selector = selector.trim_start_matches("/dev/");
    selector == node || selector == hid.node.to_lowercase()
}

fn normalize_id(selector: &str) -> String {
    let Some((vid, pid)) = selector.split_once(':') else {
        return selector.to_string();
    };
    format!("{:04x}:{:04x}", parse_hex(vid), parse_hex(pid))
}

fn parse_hex(text: &str) -> u16 {
    u16::from_str_radix(text.trim().trim_start_matches("0x"), 16).unwrap_or(0)
}

// ─── mount / umount / eject ─────────────────────────────────────────────────

fn mount_device(selector: Option<&str>, json: bool) -> Result<()> {
    let snapshot = scan::scan();
    let (device, label) = match selector {
        Some(selector) => {
            let target = ops::mount_target_by(&snapshot, selector)?;
            (target.device, target.label)
        }
        None => {
            let target = ops::first_mount_target(&snapshot)?;
            (target.device, target.label)
        }
    };
    let point = mount::mount(&device, &label)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": true,
                "device": device,
                "mountpoint": point,
            }))?
        );
    } else {
        println!("已挂载 {device} → {point}");
    }
    Ok(())
}

fn umount_device(selector: Option<&str>, json: bool) -> Result<()> {
    let snapshot = scan::scan();
    let device = match selector {
        Some(selector) => ops::mounted_by(&snapshot, selector)?,
        None => ops::first_mounted(&snapshot)?,
    };
    mount::unmount(&device)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "ok": true, "device": device }))?
        );
    } else {
        println!("已卸载 {device}");
    }
    Ok(())
}

fn eject_device(selector: Option<&str>, json: bool) -> Result<()> {
    let snapshot = scan::scan();
    let disk = match selector {
        Some(selector) => ops::disk_by(&snapshot, selector)?,
        None => ops::first_idle_disk(&snapshot)?,
    };
    if disk.mounted() {
        bail!("{} 还有挂载点，请先卸载", disk.path);
    }
    mount::power_off(&disk.path)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "ok": true, "device": disk.path }))?
        );
    } else {
        println!("已安全弹出 {}", disk.path);
    }
    Ok(())
}

// ─── probe ──────────────────────────────────────────────────────────────────

fn probe(node: &str, seconds: u64, decode: bool) -> Result<()> {
    let file = hidraw::open(node, false)?;
    println!("监听 {node}，{seconds} 秒（Ctrl+C 结束）");

    let start = Instant::now();
    let deadline = start + Duration::from_secs(seconds);
    let mut buffer = vec![0u8; 256];
    while Instant::now() < deadline {
        match hidraw::read_packet(&file, &mut buffer) {
            Ok(Some(n)) if n > 0 => {
                let raw = &buffer[..n];
                let mut line =
                    format!("{:>8} ms  id=0x{:02X}", start.elapsed().as_millis(), raw[0]);
                for byte in raw {
                    line.push_str(&format!(" {byte:02X}"));
                }
                if decode {
                    line.push_str("  |");
                    for byte in &raw[1..] {
                        line.push_str(&format!(" {:02X}", byte ^ 0xFF));
                    }
                }
                println!("{line}");
            }
            Ok(_) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

// ─── doctor ─────────────────────────────────────────────────────────────────

struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
    hint: Option<&'static str>,
    required: bool,
}

fn doctor(json: bool) -> Result<()> {
    let mut checks = Vec::new();

    checks.push(tool_check("lsblk", true));
    checks.push(tool_check("udisksctl", true));

    let (service_ok, service_detail) = udisks_service();
    checks.push(Check {
        name: "udisks2 服务",
        ok: service_ok,
        detail: service_detail,
        hint: Some("sudo systemctl enable --now udisks2"),
        required: false,
    });

    let rule_paths = [
        "/usr/lib/udev/rules.d/70-rhubtui.rules",
        "/etc/udev/rules.d/70-rhubtui.rules",
    ];
    let rule = rule_paths.iter().find(|path| Path::new(path).exists());
    checks.push(Check {
        name: "udev 规则",
        ok: rule.is_some(),
        detail: rule
            .map(|path| path.to_string())
            .unwrap_or_else(|| "未安装".to_string()),
        hint: Some("sudo install -m 0644 udev/70-rhubtui.rules /etc/udev/rules.d/"),
        required: false,
    });

    let snapshot = scan::scan();
    let hid_targets: Vec<&HidInfo> = snapshot
        .peripherals
        .iter()
        .flat_map(|peripheral| peripheral.hids.iter())
        .filter(|hid| battery::supported(hid))
        .collect();
    if hid_targets.is_empty() {
        checks.push(Check {
            name: "hidraw 权限",
            ok: true,
            detail: "未连接 MCHOSE/HID 电量设备".to_string(),
            hint: None,
            required: false,
        });
    } else {
        for hid in hid_targets {
            let writable = hidraw::open(&hid.node, true).is_ok();
            checks.push(Check {
                name: "hidraw 权限",
                ok: writable,
                detail: format!(
                    "{}（{} · {}）",
                    hid.node,
                    hid.protocol_name(),
                    hid.iface
                        .map(|iface| format!("iface {iface}"))
                        .unwrap_or_else(|| "iface ?".into())
                ),
                hint: Some("安装 udev 规则后重新插拔接收器"),
                required: false,
            });
        }
    }

    let config = Config::load()?;
    checks.push(Check {
        name: "配置文件",
        ok: true,
        detail: if Config::path().exists() {
            Config::path().display().to_string()
        } else {
            format!("使用默认值（{}）", Config::path().display())
        },
        hint: Some("rhubtui config --init"),
        required: false,
    });

    checks.push(Check {
        name: "设备枚举",
        ok: true,
        detail: format!(
            "{} 个 USB 设备 · {} 个内置 · {} 个扩展坞",
            snapshot.peripherals.len(),
            snapshot
                .peripherals
                .iter()
                .filter(|p| p.usb.place.is_internal())
                .count(),
            snapshot
                .peripherals
                .iter()
                .filter(|p| p.usb.hub_kind == Some(crate::model::HubKind::Dock))
                .count()
        ),
        hint: None,
        required: false,
    });

    if json {
        let values: Vec<_> = checks
            .iter()
            .map(|check| {
                json!({
                    "name": check.name,
                    "ok": check.ok,
                    "detail": check.detail,
                    "hint": check.hint,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "config": {
                    "scan_interval_ms": config.scan_interval_ms,
                    "battery_refresh_secs": config.battery_refresh_secs,
                    "theme": config.theme,
                    "mouse": config.mouse,
                },
                "checks": values,
            }))?
        );
    } else {
        println!("依赖与权限");
        for check in &checks {
            println!(
                "{} {:<12} {}",
                if check.ok { "✅" } else { "❌" },
                check.name,
                check.detail
            );
            if !check.ok
                && let Some(hint) = check.hint
            {
                println!("   {hint}");
            }
        }
    }

    if checks.iter().any(|check| check.required && !check.ok) {
        bail!("依赖检查未通过");
    }
    if checks.iter().any(|check| !check.ok) {
        println!();
        println!("有权限/规则未就绪：设备枚举仍可用，电量/挂载会失败。");
    }
    Ok(())
}

/// udisks2 由 D-Bus 按需激活，`inactive` 不代表不可用。
fn udisks_service() -> (bool, String) {
    let active = Command::new("systemctl")
        .args(["is-active", "udisks2"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if active {
        return (true, "active".to_string());
    }
    let dbus = Path::new("/usr/share/dbus-1/system-services/org.freedesktop.UDisks2.service");
    if dbus.exists() {
        return (true, "未运行 · D-Bus 按需激活".to_string());
    }
    (false, "未启用且缺少 D-Bus 服务文件".to_string())
}

fn tool_check(name: &'static str, required: bool) -> Check {
    match find_in_path(name) {
        Some(path) => Check {
            name,
            ok: true,
            detail: path.display().to_string(),
            hint: None,
            required,
        },
        None => Check {
            name,
            ok: false,
            detail: "未安装".to_string(),
            hint: Some("安装 util-linux / udisks2"),
            required,
        },
    }
}

fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

// ─── config ─────────────────────────────────────────────────────────────────

fn show_config(init: bool, force: bool, json: bool) -> Result<()> {
    if init {
        let path = Config::init(force)?;
        if !json {
            println!("已写入默认配置 {}", path.display());
        }
    }
    let config = Config::load()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": Config::path(),
                "scan_interval_ms": config.scan_interval_ms,
                "battery_refresh_secs": config.battery_refresh_secs,
                "theme": config.theme,
                "mouse": config.mouse,
                "themes": crate::theme::Palette::NAMES,
            }))?
        );
    } else {
        println!("配置文件 {}", Config::path().display());
        println!("  scan_interval_ms     {}", config.scan_interval_ms);
        println!("  battery_refresh_secs {}", config.battery_refresh_secs);
        println!(
            "  theme                {}（可选 {}）",
            config.theme,
            crate::theme::Palette::NAMES.join(" / ")
        );
        println!("  mouse                {}", config.mouse);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn table_lists_every_device_with_place_and_extra() {
        let snapshot = crate::testing::snapshot();
        let text = render_table(&list_rows(&snapshot));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), snapshot.peripherals.len() + 1);
        assert!(lines[0].starts_with("PORT"));
        let storage = lines.iter().find(|line| line.contains("PSSD T7")).unwrap();
        assert!(storage.contains("storage"), "{storage}");
        assert!(storage.contains("sda1 mounted /media/user/T7"), "{storage}");
        let mouse = lines
            .iter()
            .find(|line| line.contains("A5 Pro Max"))
            .unwrap();
        assert!(mouse.contains("5253:1020"), "{mouse}");
        assert!(mouse.contains("/dev/hidraw2 MCHOSE 鼠标"), "{mouse}");
        let dock = lines
            .iter()
            .find(|line| line.contains("USB4 Dock"))
            .unwrap();
        assert!(dock.contains("扩展坞"), "{dock}");
        assert!(dock.contains("/坞上"), "{dock}");
        let camera = lines
            .iter()
            .find(|line| line.contains("Integrated Camera"))
            .unwrap();
        assert!(camera.contains("内置"), "{camera}");
    }

    #[test]
    fn json_shape_is_stable() {
        let snapshot = crate::testing::snapshot();
        let storage = peripheral_json(&snapshot.peripherals[0]);
        assert_eq!(storage["sysfs"], json!("7-1.3"));
        assert_eq!(storage["id"], json!("04e8:61f5"));
        assert_eq!(storage["category"], json!("storage"));
        assert_eq!(storage["place"], json!("external"));
        assert_eq!(storage["disks"][0]["partitions"][0]["label"], json!("T7"));
        assert_eq!(
            storage["disks"][0]["partitions"][0]["mount"]["path"],
            json!("/media/user/T7")
        );
        assert_eq!(storage["interfaces"][0]["driver"], json!("uas"));

        let mouse = peripheral_json(&snapshot.peripherals[1]);
        assert_eq!(mouse["category"], json!("input"));
        assert_eq!(mouse["name"], json!("A5 Pro Max"));
        assert_eq!(mouse["hids"][0]["node"], json!("/dev/hidraw2"));
        assert_eq!(mouse["hids"][0]["protocol"], json!("MCHOSE 鼠标"));

        let camera = peripheral_json(&snapshot.peripherals[3]);
        assert_eq!(camera["place"], json!("internal"));
        assert_eq!(camera["videos"][0], json!("Integrated Camera"));
    }
}
