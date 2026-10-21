//! 通过 udisks2 挂载/卸载/弹出；root 下回退到 `mount(8)`。
//!
//! udisks2 由 D-Bus 激活，不需要提前 enable；普通 U 盘在活动会话里免密。
//! 没有 udisks2 或权限不足时，错误信息会给出下一步。

use std::fs;
use std::process::Command;

use anyhow::{Context, Result, bail};

fn is_root() -> bool {
    // SAFETY: geteuid 没有前置条件。
    unsafe { libc::geteuid() == 0 }
}

fn run(command: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(command)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .output()
        .with_context(|| format!("无法执行 {command}，请确认已安装"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    bail!(
        "{command} {} 失败：{detail}{}",
        args.join(" "),
        hint(&detail)
    )
}

fn hint(error: &str) -> &'static str {
    let lower = error.to_lowercase();
    if lower.contains("no such file") || lower.contains("not found") {
        "\n提示：未安装 udisks2（pacman -S udisks2）"
    } else if lower.contains("serviceunknown") || lower.contains("not been activated") {
        "\n提示：udisks2 未运行，可 sudo systemctl start udisks2（D-Bus 通常会自动激活）"
    } else if lower.contains("not authorized") {
        "\n提示：需要 polkit 授权，且要在活动本地会话中运行"
    } else {
        ""
    }
}

/// 挂载 `dev`，返回挂载点。
pub fn mount(dev: &str, label: &str) -> Result<String> {
    match run("udisksctl", &["mount", "--no-user-interaction", "-b", dev]) {
        Ok(output) => {
            let point = output
                .rsplit_once(" at ")
                .map(|(_, path)| path.trim().trim_end_matches('.').to_string())
                .unwrap_or_else(|| output.trim().to_string());
            return Ok(point);
        }
        Err(error) if !is_root() => return Err(error),
        Err(_) => {}
    }

    let dir = format!("/run/media/{}", sanitize(label));
    fs::create_dir_all(&dir).with_context(|| format!("创建挂载点 {dir} 失败"))?;
    run("mount", &[dev, &dir])?;
    Ok(dir)
}

pub fn unmount(dev: &str) -> Result<()> {
    match run(
        "udisksctl",
        &["unmount", "--no-user-interaction", "-b", dev],
    ) {
        Ok(_) => return Ok(()),
        Err(error) if !is_root() => return Err(error),
        Err(_) => {}
    }
    run("umount", &[dev]).map(|_| ())
}

/// 卸载后断电，方便安全拔出。
pub fn power_off(dev: &str) -> Result<()> {
    match run(
        "udisksctl",
        &["power-off", "--no-user-interaction", "-b", dev],
    ) {
        Ok(_) => Ok(()),
        Err(error) if !is_root() => Err(error),
        Err(_) => bail!("需要 root 或可用的 udisks2"),
    }
}

fn sanitize(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "usb".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitizes_mount_labels() {
        assert_eq!(sanitize("My USB"), "My_USB");
        assert_eq!(sanitize("  "), "usb");
        assert_eq!(sanitize("数据盘"), "数据盘");
    }
}
