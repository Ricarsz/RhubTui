//! Minimal HID report descriptor parser.
//!
//! Only the parts needed for this tool are decoded: usage pages, report IDs and
//! the byte length of every input/output/feature report, plus whether a feature
//! report carries the standard "Battery Strength" usage (page 0x06, usage 0x20).

use std::collections::{BTreeMap, BTreeSet};

pub const PAGE_BATTERY: u16 = 0x0006;

/// "Battery Strength" inside the Generic Device Controls page.
pub const USAGE_BATTERY_STRENGTH: u32 = 0x20;

#[derive(Debug, Clone, Default)]
pub struct ReportInfo {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub feature_bytes: usize,
    /// Usage pages seen while this report's fields were declared.
    pub pages: BTreeSet<u16>,
    /// A feature report that contains the Battery Strength usage.
    pub battery_strength: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ReportDescriptor {
    pub usage_pages: BTreeSet<u16>,
    pub reports: BTreeMap<u8, ReportInfo>,
}

#[derive(Clone, Copy, Default)]
struct Globals {
    usage_page: u16,
    report_size: u32,
    report_count: u32,
    report_id: u8,
}

impl ReportDescriptor {
    pub fn parse(data: &[u8]) -> Self {
        let mut out = Self::default();
        let mut g = Globals::default();
        let mut stack: Vec<Globals> = Vec::new();
        // Resolved (usage page, usage id) pairs of the current main item.
        let mut local_usages: Vec<(u16, u32)> = Vec::new();

        let mut i = 0usize;
        while i < data.len() {
            let prefix = data[i];

            // Long item: 0xFE, size byte, tag byte, data.
            if prefix == 0xFE {
                if i + 2 >= data.len() {
                    break;
                }
                let size = data[i + 1] as usize;
                i += 3 + size;
                continue;
            }

            let size = match prefix & 0x03 {
                3 => 4,
                n => n as usize,
            };
            let kind = (prefix >> 2) & 0x03;
            let tag = (prefix >> 4) & 0x0F;

            if i + 1 + size > data.len() {
                break;
            }
            let mut value: u32 = 0;
            for k in 0..size {
                value |= (data[i + 1 + k] as u32) << (8 * k);
            }
            i += 1 + size;

            match kind {
                // Main items.
                0 => match tag {
                    0x8 | 0x9 | 0xB => {
                        let bits = g.report_size.saturating_mul(g.report_count);
                        let bytes = (bits as usize).div_ceil(8);
                        let entry = out.reports.entry(g.report_id).or_default();
                        entry.pages.insert(g.usage_page);
                        let battery = local_usages.iter().any(|(page, usage)| {
                            *page == PAGE_BATTERY && *usage == USAGE_BATTERY_STRENGTH
                        });
                        match tag {
                            0x8 => entry.input_bytes += bytes,
                            0x9 => entry.output_bytes += bytes,
                            _ => {
                                entry.feature_bytes += bytes;
                                if battery {
                                    entry.battery_strength = true;
                                }
                            }
                        }
                        local_usages.clear();
                    }
                    // Collection / End Collection.
                    0xA => local_usages.clear(),
                    0xC => local_usages.clear(),
                    _ => local_usages.clear(),
                },
                // Global items.
                1 => match tag {
                    0x0 => {
                        g.usage_page = value as u16;
                        out.usage_pages.insert(g.usage_page);
                    }
                    0x7 => g.report_size = value,
                    0x8 => g.report_id = value as u8,
                    0x9 => g.report_count = value,
                    0xA => stack.push(g),
                    0xB => {
                        if let Some(prev) = stack.pop() {
                            g = prev;
                        }
                    }
                    _ => {}
                },
                // Local items.
                2 => match tag {
                    // Usage. A 4-byte usage carries its own page in the high half.
                    0x0 => {
                        let (page, usage) = if size == 4 {
                            ((value >> 16) as u16, value & 0xFFFF)
                        } else {
                            (g.usage_page, value)
                        };
                        local_usages.push((page, usage));
                    }
                    // Usage Minimum / Maximum: record both ends of the range.
                    0x1 | 0x2 => {
                        let (page, usage) = if size == 4 {
                            ((value >> 16) as u16, value & 0xFFFF)
                        } else {
                            (g.usage_page, value)
                        };
                        local_usages.push((page, usage));
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        // Expand 32-bit "extended" usages: they already carry page + id, so the
        // battery check above compares the high half against 0x0006.
        out
    }

    pub fn report(&self, id: u8) -> Option<&ReportInfo> {
        self.reports.get(&id)
    }

    pub fn has_page(&self, page: u16) -> bool {
        self.usage_pages.contains(&page)
    }

    /// First feature report (id, payload bytes) exposing Battery Strength.
    pub fn battery_feature_report(&self) -> Option<(u8, usize)> {
        self.reports
            .iter()
            .find(|(_, r)| r.battery_strength && r.feature_bytes > 0)
            .map(|(id, r)| (*id, r.feature_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Report 0x11/0x12 (20/64 bytes in+out) plus input-only 0x13 on page 0xFF01.
    fn vendor_descriptor() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&[0x06, 0x01, 0xFF]); // Usage Page (0xFF01)
        d.extend_from_slice(&[0x09, 0x01]); // Usage (0x01)
        d.extend_from_slice(&[0xA1, 0x01]); // Collection (Application)
        for (id, len) in [(0x11u8, 20usize), (0x12, 64), (0x14, 20)] {
            d.extend_from_slice(&[0x85, id]); // Report ID
            d.extend_from_slice(&[0x09, 0x01]); // Usage
            d.extend_from_slice(&[0x15, 0x00, 0x26, 0xFF, 0x00]); // Logical min/max
            d.extend_from_slice(&[0x75, 0x08]); // Report Size 8
            d.extend_from_slice(&[0x95, len as u8]); // Report Count
            d.extend_from_slice(&[0x81, 0x02]); // Input (Data,Var,Abs)
            d.extend_from_slice(&[0x95, len as u8]);
            d.extend_from_slice(&[0x91, 0x02]); // Output
        }
        d.extend_from_slice(&[0x85, 0x13]); // Report ID 0x13
        d.extend_from_slice(&[0x09, 0x01]);
        d.extend_from_slice(&[0x75, 0x08]);
        d.extend_from_slice(&[0x95, 20]);
        d.extend_from_slice(&[0x81, 0x02]); // Input only
        d.extend_from_slice(&[0xC0]);
        d
    }

    #[test]
    fn parses_vendor_reports() {
        let rd = ReportDescriptor::parse(&vendor_descriptor());
        assert!(rd.has_page(0xFF01));
        let cmd = rd.report(0x11).unwrap();
        assert_eq!(cmd.input_bytes, 20);
        assert_eq!(cmd.output_bytes, 20);
        let data = rd.report(0x12).unwrap();
        assert_eq!(data.output_bytes, 64);
        let push = rd.report(0x13).unwrap();
        assert_eq!(push.input_bytes, 20);
        assert_eq!(push.output_bytes, 0);
    }

    #[test]
    fn parses_battery_strength_feature() {
        let mut d = Vec::new();
        d.extend_from_slice(&[0x06, 0x06, 0x00]); // Usage Page (Generic Device Controls)
        d.extend_from_slice(&[0x09, 0x20]); // Usage (Battery Strength)
        d.extend_from_slice(&[0x85, 0x05]); // Report ID 5
        d.extend_from_slice(&[0x15, 0x00, 0x25, 0x64]); // 0..100
        d.extend_from_slice(&[0x75, 0x08, 0x95, 0x01]); // 8 bits x1
        d.extend_from_slice(&[0xB1, 0x02]); // Feature
        let rd = ReportDescriptor::parse(&d);
        assert_eq!(rd.battery_feature_report(), Some((0x05, 1)));
    }

    #[test]
    fn truncated_descriptor_does_not_panic() {
        let _ = ReportDescriptor::parse(&[0x06, 0x01, 0xFF, 0x95]);
    }
}
