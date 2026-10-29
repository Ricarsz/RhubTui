//! 与终端无关的展示模型：TUI 负责上色与排版，CLI/JSON 复用同一份语义。

use crate::model::{Category, Disk, HubKind, Peripheral, human_size};
use crate::scan::Snapshot;

pub mod help;
pub mod json;
pub mod table;

/// 语义色调，由具体主题映射成颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Good,
    Warn,
    Dim,
    Faint,
}

#[derive(Debug, Clone)]
pub struct Badge {
    pub text: String,
    pub tone: Tone,
}

#[derive(Debug, Clone)]
pub enum RowText {
    /// 分类分组标题。
    Section(String),
    Device {
        name: String,
        /// 内置 / 坞上等位置标记。
        flags: Vec<(String, Tone)>,
        badge: Option<Badge>,
    },
}

#[derive(Debug, Clone)]
pub struct Row {
    pub text: RowText,
    pub device: Option<usize>,
}

impl Row {
    pub fn section(title: impl Into<String>) -> Self {
        Self {
            text: RowText::Section(title.into()),
            device: None,
        }
    }

    pub fn device(index: usize, peripheral: &Peripheral) -> Self {
        let mut flags = Vec::new();
        if peripheral.usb.place.is_internal() {
            flags.push(("内置".to_string(), Tone::Faint));
        } else if peripheral.usb.on_dock() {
            flags.push(("坞上".to_string(), Tone::Dim));
        }
        Self {
            device: Some(index),
            text: RowText::Device {
                name: peripheral.name(),
                flags,
                badge: badge(peripheral),
            },
        }
    }
}

/// 列表徽标：存储状态、Hub 端口或功能摘要。
pub fn badge(peripheral: &Peripheral) -> Option<Badge> {
    if !peripheral.disks.is_empty() {
        let partitions: usize = peripheral.disks.iter().map(|d| d.partitions.len()).sum();
        if peripheral.disks.iter().any(Disk::mounted) {
            return Some(Badge {
                text: "已挂载".into(),
                tone: Tone::Good,
            });
        }
        if partitions > 0 {
            return Some(Badge {
                text: "未挂载".into(),
                tone: Tone::Warn,
            });
        }
        return Some(Badge {
            text: "无介质".into(),
            tone: Tone::Faint,
        });
    }

    if peripheral.usb.is_hub() {
        let text = match peripheral.usb.hub_kind.unwrap_or(HubKind::Hub) {
            HubKind::Dock => format!("扩展坞 · {} 口", peripheral.usb.maxchild),
            HubKind::Hub => {
                let mut text = format!("{} 口", peripheral.usb.maxchild);
                if !peripheral.usb.children.is_empty() {
                    text.push_str(&format!(" · {} 台", peripheral.usb.children.len()));
                }
                text
            }
        };
        return Some(Badge {
            text,
            tone: Tone::Faint,
        });
    }

    let functions = peripheral.functions();
    if !functions.is_empty() && peripheral.category != Category::Other {
        return Some(Badge {
            text: functions.join(" · "),
            tone: Tone::Faint,
        });
    }
    None
}

// ─── 详情 ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub tone: Tone,
    pub bold: bool,
}

impl Span {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            bold: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    pub fn kv(label: &str, value: impl Into<String>) -> Self {
        Self::kv_tone(label, value, Tone::Normal)
    }

    pub fn kv_tone(label: &str, value: impl Into<String>, tone: Tone) -> Self {
        Self {
            spans: vec![
                Span::new(format!("{} ", pad_right(label, 8)), Tone::Dim),
                Span::new(value, tone),
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub enum Details {
    Empty,
    Device {
        title: String,
        tags: Vec<String>,
        lines: Vec<Line>,
    },
}

/// 详情只保留人类真正关心的几行：存储、音频/摄像头、扩展坞与位置。
pub fn details(snapshot: &Snapshot, index: Option<usize>) -> Details {
    let Some(peripheral) = index.and_then(|index| snapshot.peripherals.get(index)) else {
        return Details::Empty;
    };
    let usb = &peripheral.usb;

    let mut tags = vec![peripheral.kind_label().to_string()];
    tags.push(usb.id());
    tags.push(usb.place.label().to_string());
    if usb.on_dock() {
        tags.push("经扩展坞".into());
    }

    let mut lines = Vec::new();

    for disk in &peripheral.disks {
        let mount = disk
            .mount
            .as_ref()
            .or_else(|| disk.partitions.iter().find_map(|part| part.mount.as_ref()));
        let mut value = format!("{} · {}", disk.name, human_size(disk.size));
        match mount {
            Some(mount) => {
                value.push_str(&format!(" · {}", mount.path));
                if let Some(pct) = mount.used_pct() {
                    value.push_str(&format!(" ({pct}%)"));
                }
            }
            None => value.push_str(" · 未挂载"),
        }
        lines.push(Line::kv("存储", value));
    }

    if let Some(audio) = &peripheral.audio {
        lines.push(Line::kv("音频", audio.name.clone()));
    }
    if !peripheral.videos.is_empty() {
        lines.push(Line::kv("摄像头", peripheral.videos.join(" · ")));
    }
    if usb.is_hub() {
        lines.push(Line::kv(
            "扩展坞",
            format!(
                "{} · {} 台下挂",
                usb.hub_kind.unwrap_or(HubKind::Hub).label(),
                usb.children.len()
            ),
        ));
    }
    lines.push(Line::kv(
        "位置",
        format!("{} · {}", usb.place.label(), usb.friendly_attachment()),
    ));
    lines.truncate(6);

    Details::Device {
        title: peripheral.name(),
        tags,
        lines,
    }
}

fn pad_right(text: &str, width: usize) -> String {
    let mut out = text.to_string();
    let len = unicode_width::UnicodeWidthStr::width(text);
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_by_display_width() {
        assert_eq!(pad_right("abc", 5), "abc  ");
        assert_eq!(pad_right("中文", 6), "中文  ");
        assert_eq!(pad_right("abcdef", 3), "abcdef");
    }
}
