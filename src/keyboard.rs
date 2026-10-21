//! MCHOSE 2.4G 键盘协议（usage page `0xFF31`）。
//!
//! 键盘没有单独的电量查询命令：`0x05`（getFuncConfig）返回的 64 字节配置块里，
//! 第 32 字节低 7 位是电量，最高位是充电标志。帧格式为
//! `flag, command, 0, checksum, size, offset_lo, offset_hi, 0, data...`，
//! 校验和是 `size + offset + data` 的字节和。

use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::hidraw;
use crate::mchose::Status;

/// `getFuncConfig(0)`：偏移 0 的 56 字节，校验和 0x38。
fn request() -> [u8; 64] {
    let mut frame = [0u8; 64];
    frame[0] = 0x55; // 请求
    frame[1] = 0x05; // 命令
    frame[3] = 0x38; // size + offset + reserved 的字节和
    frame[4] = 0x38; // 56 字节
    frame
}

pub fn parse(data: &[u8]) -> Option<Status> {
    if data.len() < 64 || data[0] != 0xAA || data[1] != 0x05 {
        return None;
    }
    let raw = data[40];
    let battery = raw & 0x7F;
    if !(1..=100).contains(&battery) {
        return None;
    }
    Some(Status {
        battery,
        charging: raw & 0x80 != 0,
        polling_hz: None,
        name: String::new(),
        wireless: None,
        dpi: None,
        sleeping: false,
    })
}

pub fn read_battery(node: &str) -> Result<Status> {
    let file = hidraw::open(node, true).with_context(|| format!("打开 {node} 失败"))?;
    let mut buffer = [0u8; 64];

    for attempt in 0..2 {
        if attempt > 0 {
            sleep(Duration::from_millis(150));
        }
        // 丢掉上一次残留的应答，避免把旧帧当成这次的结果。
        while let Ok(Some(_)) = hidraw::read_packet(&file, &mut buffer) {}

        hidraw::write_packet(&file, &request()).with_context(|| format!("写入 {node} 失败"))?;

        let deadline = Instant::now() + Duration::from_millis(800);
        while Instant::now() < deadline {
            match hidraw::read_packet(&file, &mut buffer) {
                Ok(Some(n)) => {
                    if let Some(status) = parse(&buffer[..n]) {
                        return Ok(status);
                    }
                }
                Ok(None) => sleep(Duration::from_millis(5)),
                Err(error) => return Err(error).with_context(|| format!("读取 {node} 失败")),
            }
        }
    }

    bail!("键盘无响应（可能休眠）")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(battery_byte: u8) -> Vec<u8> {
        let mut frame = vec![0u8; 64];
        frame[0] = 0xAA;
        frame[1] = 0x05;
        frame[4] = 0x38;
        frame[40] = battery_byte;
        frame
    }

    #[test]
    fn decodes_battery_and_charge_bit() {
        let status = parse(&response(0x60)).unwrap();
        assert_eq!(status.battery, 96);
        assert!(!status.charging);

        let status = parse(&response(0x80 | 55)).unwrap();
        assert_eq!(status.battery, 55);
        assert!(status.charging);
    }

    #[test]
    fn rejects_other_frames() {
        assert!(parse(&response(0)).is_none());
        assert!(parse(&[0u8; 10]).is_none());
        let mut frame = response(50);
        frame[1] = 0x03;
        assert!(parse(&frame).is_none());
    }
}
