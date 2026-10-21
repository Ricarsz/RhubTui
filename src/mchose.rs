//! MCHOSE 2.4G HID protocol (vendor usage page 0xFF01).
//!
//! There is no battery command in this protocol. The device pushes a status
//! event (report 0x13, payload 0xE2) that carries battery, charge state,
//! connection mode, DPI level, polling rate and the device name. Sending
//! `0x0B 0xAA` (reload profile from EEPROM) makes the firmware emit that
//! event, which is how the vendor web driver gets the battery on every connect.
//!
//! Transport notes (they matter):
//! * every payload byte is XORed with 0xFF; the report ID is not,
//! * commands go out as output reports (`write`), responses arrive on the
//!   interrupt IN endpoint (`read`) — feature reports return a kernel-cached
//!   echo on Linux and must not be used for this device.

use std::fs::File;
use std::io;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::hidraw;

pub const REPORT_CMD: u8 = 0x11;
pub const REPORT_DATA: u8 = 0x12;
pub const REPORT_INPUT: u8 = 0x13;

const XOR_KEY: u8 = 0xFF;
const CMD_RELOAD: u8 = 0x0B;
const RELOAD_SUB: u8 = 0xAA;
const PUSH_STATUS: u8 = 0xE2;

const POLLING_HZ: [u32; 6] = [125, 500, 1000, 2000, 4000, 8000];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub battery: u8,
    pub charging: bool,
    pub polling_hz: Option<u32>,
    pub name: String,
    /// `0xE2` 状态包里的连接方式：`true` 为 2.4G，`false` 为 USB 有线。
    pub wireless: Option<bool>,
    /// `0xE2` 状态包里的当前 DPI 档位（1..=6）。
    pub dpi: Option<u8>,
    /// Device is in sleep mode: the 0xE2 it sent is a wake indicator with an
    /// empty name, so `battery` is meaningless.
    pub sleeping: bool,
}

pub struct Mouse {
    file: File,
    cmd_len: usize,
    data_len: usize,
    in_len: usize,
}

/// Build a wire packet: report ID followed by the XORed, zero-padded payload.
pub fn build_packet(report_id: u8, payload: &[u8], payload_len: usize) -> Vec<u8> {
    let mut body = vec![0u8; payload_len];
    let n = payload.len().min(payload_len);
    body[..n].copy_from_slice(&payload[..n]);
    for b in body.iter_mut() {
        *b ^= XOR_KEY;
    }

    let mut packet = Vec::with_capacity(payload_len + 1);
    packet.push(report_id);
    packet.extend_from_slice(&body);
    packet
}

/// Decode an interrupt-IN payload (report ID already stripped).
pub fn decode(mut payload: Vec<u8>) -> Vec<u8> {
    for b in payload.iter_mut() {
        *b ^= XOR_KEY;
    }
    payload
}

pub fn parse_status(payload: &[u8]) -> Option<Status> {
    if payload.len() < 17 || payload[0] != PUSH_STATUS {
        return None;
    }

    let charging = payload[3] != 0;
    let battery = payload[4];
    let polling = payload[6];
    // Web driver: `e2 01 01 ...` 的 [5] 是当前 DPI 档位；[1] 为 00 表示 USB。
    let wireless = Some(payload[1] != 0);
    let dpi = (1..=6).contains(&payload[5]).then_some(payload[5]);
    let name: String = payload[9..17]
        .iter()
        .take_while(|b| **b != 0)
        .filter(|b| b.is_ascii_graphic() || **b == b' ')
        .map(|b| *b as char)
        .collect::<String>()
        .trim()
        .to_string();

    // Sleep/wake packet: empty name and 125 Hz. Battery byte is garbage.
    let sleeping = name.is_empty() && polling == 0;
    if !sleeping && !(1..=100).contains(&battery) {
        return None;
    }

    Some(Status {
        battery,
        charging,
        polling_hz: POLLING_HZ.get(polling as usize).copied(),
        name,
        wireless,
        dpi,
        sleeping,
    })
}

impl Mouse {
    /// `cmd_len`/`data_len` are output report payload sizes, `in_len` the push
    /// report size — all taken from the HID report descriptor.
    pub fn open(node: &str, cmd_len: usize, data_len: usize, in_len: usize) -> io::Result<Self> {
        Ok(Self {
            file: hidraw::open(node, true)?,
            cmd_len: cmd_len.max(2),
            data_len: data_len.max(2),
            in_len: in_len.max(17),
        })
    }

    fn send(&mut self, report_id: u8, payload: &[u8]) -> io::Result<()> {
        let len = if report_id == REPORT_DATA {
            self.data_len
        } else {
            self.cmd_len
        };
        let packet = build_packet(report_id, payload, len);
        hidraw::write_packet(&self.file, &packet)?;
        // The receiver needs a moment before it accepts the next command.
        sleep(Duration::from_millis(30));
        Ok(())
    }

    fn read_input(&mut self) -> io::Result<Option<(u8, Vec<u8>)>> {
        let mut buf = vec![0u8; self.in_len + 1];
        let Some(n) = hidraw::read_packet(&self.file, &mut buf)? else {
            return Ok(None);
        };
        if n == 0 {
            return Ok(None);
        }
        let report_id = buf[0];
        Ok(Some((report_id, decode(buf[1..n].to_vec()))))
    }

    /// Discard queued interrupts for `duration`; returns the last status seen.
    fn drain(&mut self, duration: Duration) -> Option<Status> {
        let deadline = Instant::now() + duration;
        let mut last = None;
        while Instant::now() < deadline {
            match self.read_input() {
                Ok(Some((REPORT_INPUT, payload))) => {
                    if let Some(st) = parse_status(&payload)
                        && !st.sleeping
                    {
                        last = Some(st);
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => sleep(Duration::from_millis(10)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        last
    }

    /// Ask the firmware for a status event and wait for the battery byte.
    pub fn battery(&mut self, timeout: Duration) -> io::Result<Option<Status>> {
        // Two attempts: a sleeping mouse may drop the first reload.
        for attempt in 0..2 {
            if attempt > 0 {
                sleep(Duration::from_millis(250));
            }

            let _ = self.drain(Duration::from_millis(200));
            self.send(REPORT_CMD, &[CMD_RELOAD, RELOAD_SUB])?;

            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                match self.read_input()? {
                    Some((REPORT_INPUT, payload)) => {
                        if let Some(st) = parse_status(&payload) {
                            if st.sleeping {
                                continue;
                            }
                            return Ok(Some(st));
                        }
                    }
                    Some(_) => {}
                    None => sleep(Duration::from_millis(5)),
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e2_packet(battery: u8, charging: bool, polling: u8, name: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; 20];
        p[0] = 0xE2;
        p[1] = 0x01;
        p[2] = 0x01;
        p[3] = u8::from(charging);
        p[4] = battery;
        p[5] = 0x02;
        p[6] = polling;
        p[7] = 0x06;
        p[8] = 0x28;
        p[9..9 + name.len()].copy_from_slice(name);
        p
    }

    #[test]
    fn packet_is_report_id_plus_xored_payload() {
        let p = build_packet(REPORT_CMD, &[0x0B, 0xAA], 20);
        assert_eq!(p.len(), 21);
        assert_eq!(p[0], 0x11);
        assert_eq!(p[1], 0x0B ^ 0xFF);
        assert_eq!(p[2], 0xAA ^ 0xFF);
        assert!(p[3..].iter().all(|b| *b == 0xFF));
    }

    #[test]
    fn decodes_status_event() {
        let st = parse_status(&e2_packet(67, false, 2, b"A7 Pro\0\0")).unwrap();
        assert_eq!(st.battery, 67);
        assert!(!st.charging);
        assert_eq!(st.polling_hz, Some(1000));
        assert_eq!(st.name, "A7 Pro");
        assert_eq!(st.wireless, Some(true));
        assert_eq!(st.dpi, Some(2));
        assert!(!st.sleeping);
    }

    #[test]
    fn reports_wired_connection_mode() {
        let mut p = e2_packet(50, false, 2, b"A7 Pro\0\0");
        p[1] = 0x00;
        let st = parse_status(&p).unwrap();
        assert_eq!(st.wireless, Some(false));
    }

    #[test]
    fn ignores_out_of_range_battery() {
        assert!(parse_status(&e2_packet(0, false, 2, b"A7 Pro\0\0")).is_none());
        assert!(parse_status(&e2_packet(101, false, 2, b"A7 Pro\0\0")).is_none());
    }

    #[test]
    fn detects_sleep_packet() {
        let st = parse_status(&e2_packet(0, false, 0, b"\0\0\0\0\0\0\0\0")).unwrap();
        assert!(st.sleeping);
    }

    #[test]
    fn round_trips_through_xor() {
        let original = e2_packet(42, true, 5, b"A7 Pro\0\0");
        let wire = build_packet(REPORT_INPUT, &original, 20);
        let decoded = decode(wire[1..].to_vec());
        assert_eq!(decoded, original);
        let st = parse_status(&decoded).unwrap();
        assert!(st.charging);
        assert_eq!(st.polling_hz, Some(8000));
    }
}
