//! `~/.config/rhubtui/config.toml`，缺失时全部使用默认值。

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 设备扫描间隔（毫秒），最小 200。
    pub scan_interval_ms: u64,
    /// 电量自动刷新间隔（秒），最小 10。
    pub battery_refresh_secs: u64,
    /// 配色：ocean | forest | sunset。
    pub theme: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scan_interval_ms: 1000,
            battery_refresh_secs: 120,
            theme: "ocean".to_string(),
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

    pub fn battery_refresh(&self) -> Duration {
        Duration::from_secs(self.battery_refresh_secs.max(10))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config = Config::default();
        assert_eq!(config.scan_interval(), Duration::from_millis(1000));
        assert_eq!(config.battery_refresh(), Duration::from_secs(120));
        assert_eq!(config.theme, "ocean");
    }

    #[test]
    fn clamps_to_safe_minimums() {
        let config: Config =
            toml::from_str("scan_interval_ms = 0\nbattery_refresh_secs = 0").unwrap();
        assert_eq!(config.scan_interval(), Duration::from_millis(200));
        assert_eq!(config.battery_refresh(), Duration::from_secs(10));
    }
}
