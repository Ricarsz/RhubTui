//! Minimal HID report descriptor parser.
//!
//! RhubTui 只展示设备身份与拓扑，因此这里只解码用途页（usage page），
//! 用来区分键盘、鼠标和厂商自定义接口。

use std::collections::BTreeSet;

#[derive(Debug, Clone, Default)]
pub struct ReportDescriptor {
    pub usage_pages: BTreeSet<u16>,
}

impl ReportDescriptor {
    pub fn parse(bytes: &[u8]) -> Self {
        let mut out = Self::default();
        let mut i = 0;
        while i < bytes.len() {
            let prefix = bytes[i];
            i += 1;
            if prefix == 0xFE {
                // Long item: skip its data.
                if i + 2 > bytes.len() {
                    break;
                }
                let size = bytes[i] as usize;
                i += 2 + size;
                continue;
            }
            let size = match prefix & 0x03 {
                3 => 4,
                other => other as usize,
            };
            if i + size > bytes.len() {
                break;
            }
            let mut value: u32 = 0;
            for (shift, byte) in bytes[i..i + size].iter().enumerate() {
                value |= (*byte as u32) << (8 * shift);
            }
            i += size;

            let kind = (prefix >> 2) & 0x03;
            let tag = prefix >> 4;
            // Global item, Usage Page.
            if kind == 1 && tag == 0x0 {
                let page = if size == 4 {
                    (value >> 16) as u16
                } else {
                    value as u16
                };
                out.usage_pages.insert(page);
            }
        }
        out
    }

    #[cfg(test)]
    pub fn has_page(&self, page: u16) -> bool {
        self.usage_pages.contains(&page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_usage_pages() {
        let mut d = Vec::new();
        d.extend_from_slice(&[0x06, 0x01, 0x00]); // Usage Page (Generic Desktop)
        d.extend_from_slice(&[0x09, 0x02]); // Usage (Mouse)
        d.extend_from_slice(&[0x06, 0x01, 0xFF]); // Usage Page (Vendor 0xFF01)
        let rd = ReportDescriptor::parse(&d);
        assert!(rd.has_page(0x0001));
        assert!(rd.has_page(0xFF01));
        assert!(!rd.has_page(0x0006));
    }

    #[test]
    fn truncated_descriptor_does_not_panic() {
        let _ = ReportDescriptor::parse(&[0x06, 0x01, 0xFF, 0x95]);
        let _ = ReportDescriptor::parse(&[0xFE, 0x02]);
    }
}
