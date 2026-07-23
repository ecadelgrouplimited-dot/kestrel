//! The Kestrel palette, sampled from the logo.
//!
//! One source of truth so the desktop app and the CLI can't drift apart. The
//! values were measured from `docs/kestrel_brand_identity/icon.png` rather than
//! eyeballed: the falcon's dominant gold, the deep bronze on the shaded side of
//! its gradient, and the near-black the mark sits on.

/// An RGB colour.
pub type Rgb = (u8, u8, u8);

/// The falcon's dominant gold — the primary accent everywhere.
pub const GOLD: Rgb = (0xDC, 0x8D, 0x1F);
/// The lit edge of the gradient, for hover and emphasis.
pub const GOLD_BRIGHT: Rgb = (0xF2, 0xB0, 0x4A);
/// The shaded side of the gradient, for pressed states and rules.
pub const BRONZE: Rgb = (0x79, 0x39, 0x04);
/// The near-black the mark sits on.
pub const INK: Rgb = (0x0A, 0x0A, 0x0B);
/// A panel raised slightly off the ink.
pub const INK_RAISED: Rgb = (0x15, 0x15, 0x17);

/// Success green and failure red, warmed slightly so they sit beside the gold.
pub const GREEN: Rgb = (0x5A, 0xBE, 0x6E);
pub const RED: Rgb = (0xDC, 0x64, 0x64);
pub const AMBER: Rgb = (0xDC, 0x96, 0x50);

/// The tagline, for banners and about screens.
pub const TAGLINE: &str = "Autonomous. Efficient. Built for real work.";

/// The nearest xterm-256 index, for terminals that can't do truecolour.
pub const GOLD_256: u8 = 172;

/// Format an RGB as a truecolour ANSI SGR parameter, e.g. `38;2;220;141;31`.
pub fn ansi_fg(colour: Rgb) -> String {
    format!("38;2;{};{};{}", colour.0, colour.1, colour.2)
}

/// Mix `colour` toward `other` by `t` (0.0 = colour, 1.0 = other).
pub fn mix(colour: Rgb, other: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    (
        lerp(colour.0, other.0),
        lerp(colour.1, other.1),
        lerp(colour.2, other.2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gold_matches_the_logo_sample() {
        // Measured from the icon: the dominant gold cluster is DA8B1D–E09323.
        assert_eq!(GOLD, (0xDC, 0x8D, 0x1F));
        // The gradient runs bright → deep, so luminance must be ordered.
        let lum = |c: Rgb| c.0 as u32 + c.1 as u32 + c.2 as u32;
        assert!(lum(GOLD_BRIGHT) > lum(GOLD));
        assert!(lum(GOLD) > lum(BRONZE));
        // The ground is near-black but never pure, matching the mark's backdrop.
        assert!(lum(INK) > 0 && lum(INK) < 60);
        assert!(lum(INK_RAISED) > lum(INK));
    }

    #[test]
    fn ansi_and_mixing() {
        assert_eq!(ansi_fg(GOLD), "38;2;220;141;31");
        assert_eq!(mix(GOLD, GOLD, 0.5), GOLD);
        assert_eq!(mix((0, 0, 0), (100, 200, 50), 1.0), (100, 200, 50));
        assert_eq!(mix((0, 0, 0), (100, 200, 50), 0.5), (50, 100, 25));
        // Out-of-range factors are clamped rather than overshooting.
        assert_eq!(mix((0, 0, 0), (10, 10, 10), 5.0), (10, 10, 10));
    }
}
