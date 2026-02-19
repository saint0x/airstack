pub type Rgb = (u8, u8, u8);

// Ocean/steel/stone palette shared by CLI and TUI.
#[cfg(feature = "tui")]
pub const STONE_900: Rgb = (31, 36, 40);
#[cfg(feature = "tui")]
pub const STONE_800: Rgb = (41, 47, 52);
#[cfg(feature = "tui")]
pub const STONE_700: Rgb = (58, 66, 74);
pub const GRAY_500: Rgb = (149, 161, 172);
#[cfg(feature = "tui")]
pub const STEEL_300: Rgb = (161, 194, 220);
pub const STEEL_200: Rgb = (206, 226, 242);
pub const OCEAN_400: Rgb = (102, 167, 214);
#[cfg(feature = "tui")]
pub const WHITE_100: Rgb = (224, 229, 233);

pub fn ansi_fg(text: impl AsRef<str>, rgb: Rgb) -> String {
    let (r, g, b) = rgb;
    format!("\x1b[38;2;{r};{g};{b}m{}\x1b[0m", text.as_ref())
}

pub fn ansi_bold(text: impl AsRef<str>) -> String {
    format!("\x1b[1m{}\x1b[0m", text.as_ref())
}
