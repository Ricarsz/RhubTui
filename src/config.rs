//! `~/.config/rhubtui/config.toml`，缺失时全部使用默认值。

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// 设备扫描间隔（毫秒），最小 200。
    pub scan_interval_ms: u64,
    /// 配色：ocean | forest | sunset。
    pub theme: String,
    /// 是否启用鼠标（滚轮滚动、点击选择）。
    pub mouse: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scan_interval_ms: 1000,
            theme: "ocean".to_string(),
            mouse: true,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        let base = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"));
        base.join("rhubtui").join("config.toml")
    }

    /// 写入默认配置；已存在时只有 `force` 才覆盖。
    pub fn init(force: bool) -> Result<PathBuf> {
        let path = Self::path();
        if path.exists() && !force {
            bail!("配置已存在：{}（加 --force 覆盖）", path.display());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("创建目录 {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(&Self::default()).context("序列化默认配置失败")?;
        fs::write(&path, text).with_context(|| format!("写入 {}", path.display()))?;
        Ok(path)
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("读取配置 {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("解析配置 {}", path.display()))
    }

    pub fn scan_interval(&self) -> Duration {
        Duration::from_millis(self.scan_interval_ms.max(200))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config = Config::default();
        assert_eq!(config.scan_interval(), Duration::from_millis(1000));
        assert_eq!(config.theme, "ocean");
        assert!(config.mouse);
    }

    #[test]
    fn clamps_to_safe_minimums() {
        let config: Config = toml::from_str("scan_interval_ms = 0").unwrap();
        assert_eq!(config.scan_interval(), Duration::from_millis(200));
    }
}
