//! 配色：与 RszTui 同一套思路，语义色 + 低饱和背景。

use ratatui::style::Color;

#[derive(Clone, Copy)]
pub struct Palette {
    pub accent: Color,
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub good: Color,
    pub warn: Color,
    pub bad: Color,
    pub border: Color,
    pub surface: Color,
    pub selected: Color,
}

const OCEAN: Palette = Palette {
    accent: Color::Rgb(90, 200, 220),
    text: Color::Rgb(214, 221, 230),
    dim: Color::Rgb(108, 117, 130),
    faint: Color::Rgb(78, 88, 102),
    good: Color::Rgb(120, 210, 140),
    warn: Color::Rgb(240, 190, 90),
    bad: Color::Rgb(255, 110, 90),
    border: Color::Rgb(72, 82, 96),
    surface: Color::Rgb(40, 47, 58),
    selected: Color::Rgb(29, 46, 56),
};

const FOREST: Palette = Palette {
    accent: Color::Rgb(122, 210, 168),
    text: Color::Rgb(218, 229, 222),
    dim: Color::Rgb(108, 126, 116),
    faint: Color::Rgb(78, 96, 86),
    good: Color::Rgb(130, 215, 150),
    warn: Color::Rgb(235, 190, 100),
    bad: Color::Rgb(250, 110, 90),
    border: Color::Rgb(70, 90, 80),
    surface: Color::Rgb(38, 50, 44),
    selected: Color::Rgb(27, 48, 39),
};

const SUNSET: Palette = Palette {
    accent: Color::Rgb(250, 166, 102),
    text: Color::Rgb(236, 226, 214),
    dim: Color::Rgb(130, 116, 104),
    faint: Color::Rgb(96, 82, 70),
    good: Color::Rgb(150, 205, 125),
    warn: Color::Rgb(245, 185, 85),
    bad: Color::Rgb(255, 100, 85),
    border: Color::Rgb(94, 76, 66),
    surface: Color::Rgb(52, 42, 36),
    selected: Color::Rgb(59, 42, 31),
};

impl Palette {
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "forest" => FOREST,
            "sunset" => SUNSET,
            _ => OCEAN,
        }
    }

    pub const NAMES: [&'static str; 3] = ["ocean", "forest", "sunset"];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_theme_falls_back_to_ocean() {
        let palette = Palette::from_name("nope");
        assert_eq!(palette.accent, OCEAN.accent);
    }

    #[test]
    fn theme_names_are_stable() {
        assert_eq!(Palette::from_name("forest").accent, FOREST.accent);
        assert_eq!(Palette::from_name("SUNSET").accent, SUNSET.accent);
    }
}
