//! Straight-alpha sRGB colours, parsed from and printed as the config's hex strings.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const BLACK: Color = Color::rgb(0, 0, 0);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// Perceived brightness (Rec. 601 luma of the encoded values), 0 = black, 1 = white.
    pub fn luma(self) -> f32 {
        (0.299 * self.r as f32 + 0.587 * self.g as f32 + 0.114 * self.b as f32) / 255.0
    }

    /// Whether dark text reads better than white text on this colour. White stays on
    /// saturated mid-tones (red, blue, green, orange), as on road signs.
    pub fn is_light(self) -> bool {
        self.luma() > 0.68
    }

    /// Black or white: whichever reads on this colour.
    pub fn contrasting(self) -> Color {
        if self.is_light() {
            Color::rgb(0x1c, 0x1c, 0x1e)
        } else {
            Color::WHITE
        }
    }

    pub(crate) fn skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.r, self.g, self.b, self.a)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseColorError(String);

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a #rrggbb or #rrggbbaa colour", self.0)
    }
}

impl std::error::Error for ParseColorError {}

impl FromStr for Color {
    type Err = ParseColorError;

    /// `#rgb`, `#rrggbb` or `#rrggbbaa`, with or without the `#`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseColorError(s.to_string());
        let hex = s.trim().trim_start_matches('#');
        if !hex.is_ascii() {
            return Err(err());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| err());
        match hex.len() {
            3 => {
                let digit = |i: usize| {
                    u8::from_str_radix(&hex[i..i + 1], 16)
                        .map(|d| d * 17)
                        .map_err(|_| err())
                };
                Ok(Color::rgb(digit(0)?, digit(1)?, digit(2)?))
            }
            6 => Ok(Color::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Ok(Color {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: byte(6)?,
            }),
            _ => Err(err()),
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_hex() {
        assert_eq!(
            "#ff3b30".parse::<Color>().unwrap(),
            Color::rgb(0xff, 0x3b, 0x30)
        );
        assert_eq!(
            "0a84ff".parse::<Color>().unwrap(),
            Color::rgb(0x0a, 0x84, 0xff)
        );
        assert_eq!("#fff".parse::<Color>().unwrap(), Color::WHITE);
        assert_eq!(
            "#00000080".parse::<Color>().unwrap(),
            Color::BLACK.with_alpha(0x80)
        );
        assert!("#12345".parse::<Color>().is_err());
        assert!("#ggg".parse::<Color>().is_err());
        assert_eq!(Color::rgb(0xff, 0x3b, 0x30).to_string(), "#ff3b30");
        assert_eq!(Color::BLACK.with_alpha(0x80).to_string(), "#00000080");
    }

    #[test]
    fn contrast_picks_readable_text() {
        assert_eq!(
            "#ffcc00".parse::<Color>().unwrap().contrasting(),
            Color::rgb(0x1c, 0x1c, 0x1e)
        );
        assert_eq!(
            "#ff3b30".parse::<Color>().unwrap().contrasting(),
            Color::WHITE
        );
        assert_eq!(
            "#0a84ff".parse::<Color>().unwrap().contrasting(),
            Color::WHITE
        );
        assert_eq!(Color::WHITE.contrasting(), Color::rgb(0x1c, 0x1c, 0x1e));
    }
}
