//! CLI 子命令。无子命令时由 `main` 启动 TUI。

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use serde_json::json;

use crate::battery;
use crate::config::Config;
use crate::hidraw;
use crate::model::{Category, Disk, Peripheral};
use crate::mount;
use crate::scan::{self, Snapshot};

#[derive(Parser)]
#[command(
    name = "rhubtui",
    version,
    about = "2.4G 键鼠电量 + USB 外设管理",
    long_about = "读取 MCHOSE 2.4G 接收器与标准 HID 通道的电量，列出 USB 外设，\
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
    Config,
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
        Cmd::Config => show_config(json),
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

    println!(
        "{:<8} {:<10} {:<8} {:<32} EXTRA",
        "PORT", "ID", "TYPE", "NAME"
    );
    for peripheral in &snapshot.peripherals {
        let mut extra = Vec::new();
        if let Some(hid) = &peripheral.hid {
            extra.push(format!("{} {}", hid.node, pages(hid)));
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
                extra.push(format!("{} empty", disk.name));
            }
        }
        println!(
            "{:<8} {:<10} {:<8} {:<32} {}",
            peripheral.usb.sysfs,
            peripheral.usb.id(),
            category_tag(peripheral.category),
            crate::model::truncate(&peripheral.name(), 32),
            extra.join(" · ")
        );
    }
    Ok(())
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
                "mount": disk.mount.as_ref().map(|m| json!({
                    "path": m.path,
                    "read_only": m.read_only,
                })),
                "partitions": disk.partitions.iter().map(|part| json!({
                    "name": part.name,
                    "path": part.path,
                    "size": part.size,
                    "fstype": part.fstype,
                    "label": part.label,
                    "mount": part.mount.as_ref().map(|m| json!({
                        "path": m.path,
                        "read_only": m.read_only,
                    })),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    json!({
        "port": peripheral.usb.sysfs,
        "id": peripheral.usb.id(),
        "vid": peripheral.usb.vid,
        "pid": peripheral.usb.pid,
        "name": peripheral.name(),
        "category": category_tag(peripheral.category),
        "speed": peripheral.usb.speed,
        "serial": peripheral.usb.serial,
        "power_ma": peripheral.usb.power_ma,
        "hid": peripheral.hid.as_ref().map(|hid| json!({
            "node": hid.node,
            "pages": hid.pages.iter().map(|p| format!("0x{p:04X}")).collect::<Vec<_>>(),
            "protocol": if hid.mchose { "mchose-mouse" } else if hid.keyboard { "mchose-keyboard" } else if hid.battery_feature.is_some() { "hid-battery" } else { "none" },
        })),
        "disks": disks,
    })
}

fn pages(hid: &crate::model::HidInfo) -> String {
    let pages: Vec<String> = hid.pages.iter().map(|p| format!("0x{p:04X}")).collect();
    if pages.is_empty() {
        "-".to_string()
    } else {
        pages.join(",")
    }
}

fn category_tag(category: Category) -> &'static str {
    match category {
        Category::Storage => "storage",
        Category::Input => "input",
        Category::Audio => "audio",
        Category::Hub => "hub",
        Category::Other => "other",
    }
}

// ─── battery ────────────────────────────────────────────────────────────────

fn read_batteries(selector: Option<&str>, json: bool) -> Result<()> {
    let snapshot = scan::scan();
    let targets: Vec<&Peripheral> = snapshot
        .peripherals
        .iter()
        .filter(|peripheral| peripheral.hid.as_ref().is_some_and(battery::supported))
        .filter(|peripheral| match selector {
            Some(selector) => matches_hid(peripheral, selector),
            None => true,
        })
        .collect();

    if targets.is_empty() {
        bail!("没有找到可读电量的设备（MCHOSE 接收器或标准 HID 电量通道）");
    }

    let mut readings = Vec::new();
    let mut errors = Vec::new();
    for peripheral in targets {
        let hid = peripheral.hid.as_ref().unwrap();
        match battery::read(hid) {
            Ok(status) => readings.push((peripheral, status)),
            Err(error) => errors.push(format!("{}：{error:#}", peripheral.name())),
        }
    }

    if json {
        let values: Vec<_> = readings
            .iter()
            .map(|(peripheral, status)| {
                json!({
                    "device": peripheral.name(),
                    "port": peripheral.usb.sysfs,
                    "node": peripheral.hid.as_ref().map(|h| h.node.clone()),
                    "percent": status.battery,
                    "charging": status.charging,
                    "polling_hz": status.polling_hz,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        for (peripheral, status) in &readings {
            let mut extra = Vec::new();
            if status.charging {
                extra.push("charging".to_string());
            }
            if let Some(hz) = status.polling_hz {
                extra.push(format!("{hz}Hz"));
            }
            let node = peripheral
                .hid
                .as_ref()
                .map(|h| h.node.as_str())
                .unwrap_or("-");
            println!(
                "{:<14} {:>3}%  {:<20} {}",
                node,
                status.battery,
                extra.join(" "),
                crate::model::truncate(&peripheral.name(), 34)
            );
        }
    }

    for error in &errors {
        eprintln!("rhubtui: {error}");
    }
    if readings.is_empty() {
        bail!("所有设备都读取失败");
    }
    Ok(())
}

fn matches_hid(peripheral: &Peripheral, selector: &str) -> bool {
    if peripheral.usb.sysfs == selector || peripheral.usb.id() == selector {
        return true;
    }
    peripheral
        .hid
        .as_ref()
        .is_some_and(|hid| hid.node == selector || hid.node.trim_start_matches("/dev/") == selector)
}

// ─── mount / umount / eject ─────────────────────────────────────────────────

fn mount_device(selector: Option<&str>, json: bool) -> Result<()> {
    let snapshot = scan::scan();
    let (device, label) = match selector {
        Some(selector) => find_mount_target(&snapshot, selector)?,
        None => find_first_mount_target(&snapshot)?,
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
        Some(selector) => find_mounted(&snapshot, selector)?,
        None => find_first_mounted(&snapshot)?,
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
        Some(selector) => find_disk(&snapshot, selector)?,
        None => find_first_idle_disk(&snapshot)?,
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

fn find_mount_target(snapshot: &Snapshot, selector: &str) -> Result<(String, String)> {
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            for partition in &disk.partitions {
                if matches_device(selector, &partition.path, &partition.name) {
                    if partition.mount.is_some() {
                        bail!("{} 已经挂载", partition.path);
                    }
                    let label = if partition.label.is_empty() {
                        partition.name.clone()
                    } else {
                        partition.label.clone()
                    };
                    return Ok((partition.path.clone(), label));
                }
            }
            if matches_device(selector, &disk.path, &disk.name) {
                if disk.partitions.is_empty() {
                    let label = if disk.model.is_empty() {
                        disk.name.clone()
                    } else {
                        disk.model.clone()
                    };
                    return Ok((disk.path.clone(), label));
                }
                bail!("{} 有分区，请指定分区（如 {}{}）", disk.path, disk.name, 1);
            }
        }
    }
    bail!("没有找到设备 {selector}")
}

fn find_first_mount_target(snapshot: &Snapshot) -> Result<(String, String)> {
    let mut whole_disk = None;
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            for partition in &disk.partitions {
                if partition.mount.is_none() {
                    let label = if partition.label.is_empty() {
                        partition.name.clone()
                    } else {
                        partition.label.clone()
                    };
                    return Ok((partition.path.clone(), label));
                }
            }
            if disk.partitions.is_empty() && disk.mount.is_none() && disk.size > 0 {
                let label = if disk.model.is_empty() {
                    disk.name.clone()
                } else {
                    disk.model.clone()
                };
                whole_disk.get_or_insert((disk.path.clone(), label));
            }
        }
    }
    whole_disk.ok_or_else(|| anyhow::anyhow!("没有可挂载的存储设备"))
}

fn find_mounted(snapshot: &Snapshot, selector: &str) -> Result<String> {
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            for partition in &disk.partitions {
                if matches_device(selector, &partition.path, &partition.name) {
                    if partition.mount.is_some() {
                        return Ok(partition.path.clone());
                    }
                    bail!("{} 没有挂载", partition.path);
                }
            }
            if matches_device(selector, &disk.path, &disk.name) {
                if disk.mount.is_some() {
                    return Ok(disk.path.clone());
                }
                bail!("{} 没有挂载", disk.path);
            }
        }
    }
    bail!("没有找到设备 {selector}")
}

fn find_first_mounted(snapshot: &Snapshot) -> Result<String> {
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            for partition in &disk.partitions {
                if partition.mount.is_some() {
                    return Ok(partition.path.clone());
                }
            }
            if disk.mount.is_some() {
                return Ok(disk.path.clone());
            }
        }
    }
    bail!("没有已挂载的存储设备")
}

fn find_disk<'a>(snapshot: &'a Snapshot, selector: &str) -> Result<&'a Disk> {
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            if matches_device(selector, &disk.path, &disk.name) {
                return Ok(disk);
            }
            for partition in &disk.partitions {
                if matches_device(selector, &partition.path, &partition.name) {
                    return Ok(disk);
                }
            }
        }
    }
    bail!("没有找到磁盘 {selector}")
}

fn find_first_idle_disk(snapshot: &Snapshot) -> Result<&Disk> {
    for peripheral in &snapshot.peripherals {
        for disk in &peripheral.disks {
            if !disk.mounted() && disk.size > 0 {
                return Ok(disk);
            }
        }
    }
    bail!("没有可弹出的磁盘")
}

fn matches_device(selector: &str, path: &str, name: &str) -> bool {
    let selector = selector.trim();
    selector == path || selector == name || selector.trim_start_matches("/dev/") == name
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
    let hid_targets: Vec<_> = snapshot
        .peripherals
        .iter()
        .filter_map(|peripheral| peripheral.hid.as_ref())
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
                detail: format!("{}（{}）", hid.node, protocol_name(hid)),
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

fn protocol_name(hid: &crate::model::HidInfo) -> &'static str {
    if hid.mchose {
        "mchose mouse"
    } else if hid.keyboard {
        "mchose keyboard"
    } else if hid.battery_feature.is_some() {
        "hid battery"
    } else {
        "unknown"
    }
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

fn show_config(json: bool) -> Result<()> {
    let config = Config::load()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": Config::path(),
                "scan_interval_ms": config.scan_interval_ms,
                "battery_refresh_secs": config.battery_refresh_secs,
                "theme": config.theme,
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
    }
    Ok(())
}
