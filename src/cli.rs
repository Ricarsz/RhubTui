//! CLI 子命令。无子命令时由 `main` 启动 TUI。

use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use serde_json::json;

use crate::config::Config;
use crate::mount;
use crate::ops;
use crate::scan;
use crate::view;

#[derive(Parser)]
#[command(
    name = "rhubtui",
    version,
    about = "USB 设备查看与移动盘挂载",
    long_about = "列出 USB 外设、拓扑与接口信息，并通过 udisks2 挂载、卸载、\
                  安全弹出 U 盘与移动硬盘。无子命令时启动 TUI。"
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
    /// 挂载存储设备（默认第一个未挂载分区）
    Mount { device: Option<String> },
    /// 卸载存储设备（默认第一个已挂载分区）
    Umount { device: Option<String> },
    /// 安全弹出（默认第一个未挂载磁盘）
    Eject { device: Option<String> },
    /// 检查依赖与服务状态
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
        Cmd::Mount { device } => mount_device(device.as_deref(), json),
        Cmd::Umount { device } => umount_device(device.as_deref(), json),
        Cmd::Eject { device } => eject_device(device.as_deref(), json),
        Cmd::Doctor => doctor(json),
        Cmd::Config { init, force } => show_config(init, force, json),
    }
}

// ─── list ───────────────────────────────────────────────────────────────────

fn list(json: bool) -> Result<()> {
    let snapshot = scan::scan();
    if json {
        let peripherals: Vec<_> = snapshot
            .peripherals
            .iter()
            .map(view::json::peripheral)
            .collect();
        println!("{}", serde_json::to_string_pretty(&peripherals)?);
        return Ok(());
    }
    println!("{}", view::table::render(&view::table::rows(&snapshot)));
    Ok(())
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

    let snapshot = scan::scan();
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
                    "theme": config.theme,
                    "mouse": config.mouse,
                },
                "checks": values,
            }))?
        );
    } else {
        println!("依赖与服务");
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
        println!("有依赖未就绪：设备枚举仍可用，挂载/弹出可能失败。");
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
                "theme": config.theme,
                "mouse": config.mouse,
                "themes": crate::theme::Palette::NAMES,
            }))?
        );
    } else {
        println!("配置文件 {}", Config::path().display());
        println!("  scan_interval_ms     {}", config.scan_interval_ms);
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
        let text = view::table::render(&view::table::rows(&snapshot));
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
        assert!(mouse.contains("/dev/hidraw2"), "{mouse}");
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
        let storage = view::json::peripheral(&snapshot.peripherals[0]);
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

        let mouse = view::json::peripheral(&snapshot.peripherals[1]);
        assert_eq!(mouse["category"], json!("input"));
        assert_eq!(mouse["name"], json!("A5 Pro Max"));
        assert_eq!(mouse["hids"][0]["node"], json!("/dev/hidraw2"));
        assert_eq!(mouse["hids"][0]["pages"][0], json!("0xFF01"));

        let camera = view::json::peripheral(&snapshot.peripherals[3]);
        assert_eq!(camera["place"], json!("internal"));
        assert_eq!(camera["videos"][0], json!("Integrated Camera"));
    }
}
