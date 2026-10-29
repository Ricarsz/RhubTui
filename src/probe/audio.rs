//! ALSA 声卡。

use std::collections::HashMap;
use std::fs;
use std::path::Path;


use super::*;


pub(crate) fn scan_audio() -> HashMap<String, AudioCard> {
    let text = fs::read_to_string("/proc/asound/cards").unwrap_or_default();
    let mut out = HashMap::new();
    for mut card in parse_cards(&text) {
        let link = format!("/sys/class/sound/card{}/device", card.index);
        if let Ok(real) = fs::canonicalize(link)
            && let Some(usb) = usb_of(&real)
        {
            card.pcms = read_pcms(card.index);
            out.insert(usb, card);
        }
    }
    out
}

fn read_audio(path: &Path, key: &str) -> String {
    read_str(path.join(key))
}

fn read_pcms(index: u32) -> Vec<String> {
    let dir = format!("/proc/asound/card{index}");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("pcm") {
            continue;
        }
        let info = entry.path().join("info");
        let id = read_audio(&info, "id");
        if id.is_empty() {
            continue;
        }
        out.push(format!("{name} · {id}"));
    }
    out.sort();
    out
}

fn parse_cards(text: &str) -> Vec<AudioCard> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let Some((index_part, rest)) = trimmed.split_once(" [") else {
            continue;
        };
        let Ok(index) = index_part.trim().parse::<u32>() else {
            continue;
        };
        let Some((id, rest)) = rest.split_once(']') else {
            continue;
        };
        let rest = rest.trim_start_matches(':').trim();
        let (driver, name) = match rest.split_once(" - ") {
            Some((driver, name)) => (driver.trim().to_string(), name.trim().to_string()),
            None => (rest.to_string(), String::new()),
        };
        out.push(AudioCard {
            index,
            id: id.trim().to_string(),
            driver,
            name,
            pcms: Vec::new(),
        });
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alsa_cards() {
        let text = "\
 0 [PCH            ]: HDA-Intel - HDA Intel PCH
                      HDA Intel PCH at 0x50400000 irq 74
 2 [Device         ]: USB-Audio - USB Audio Device
                      Generic USB Audio at usb-0000:00:14.0-1, full speed
";
        let cards = parse_cards(text);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[1].index, 2);
        assert_eq!(cards[1].id, "Device");
        assert_eq!(cards[1].driver, "USB-Audio");
        assert_eq!(cards[1].name, "USB Audio Device");
    }
}
