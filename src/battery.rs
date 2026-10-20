//! 电量后端分发：按扫描阶段识别出的 HID 通道选择协议。

use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::hidraw;
use crate::keyboard;
use crate::mchose::{self, Status};
use crate::model::HidInfo;

/// 读取一个设备的电量；阻塞调用，不要放在 UI 线程。
pub fn read(hid: &HidInfo) -> Result<Status> {
    if hid.mchose {
        let mut mouse = mchose::Mouse::open(&hid.node, hid.cmd_out, hid.data_out, hid.in_len)
            .with_context(|| format!("打开 {} 失败", hid.node))?;
        return match mouse.battery(Duration::from_secs(3)) {
            Ok(Some(status)) => Ok(status),
            Ok(None) => bail!("设备休眠或无响应，移动鼠标后重试"),
            Err(error) => Err(error).with_context(|| format!("读取 {} 失败", hid.node)),
        };
    }

    if hid.keyboard {
        return keyboard::read_battery(&hid.node);
    }

    if let Some((id, len)) = hid.battery_feature {
        let file =
            hidraw::open(&hid.node, false).with_context(|| format!("打开 {} 失败", hid.node))?;
        let payload = hidraw::get_feature(&file, id, len)
            .with_context(|| format!("读取 {} 失败", hid.node))?;
        let pct = payload
            .iter()
            .find(|byte| **byte <= 100)
            .copied()
            .or_else(|| payload.first().copied())
            .context("空的 feature report")?;
        return Ok(Status {
            battery: pct,
            charging: false,
            polling_hz: None,
            name: String::new(),
            sleeping: false,
        });
    }

    bail!("没有可用的电量协议")
}

/// 该接口是否有电量后端。
pub fn supported(hid: &HidInfo) -> bool {
    hid.mchose || hid.keyboard || hid.battery_feature.is_some()
}
