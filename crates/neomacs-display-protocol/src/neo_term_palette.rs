//! Frame-local face colours for the native terminal emulator.
//!
//! Lisp resolves GNU faces (including inheritance and user overrides). The
//! render thread caches this small immutable snapshot, never calling Lisp from
//! the PTY/parser or cell painting path. Opacity is not a palette property.
use crate::types::Color;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NeoTermPalette {
    pub foreground: Color,
    pub background: Color,
    pub cursor: Color,
    pub ansi_foreground: [Color; 16],
    pub ansi_background: [Color; 16],
    pub bold_is_bright: bool,
}

impl NeoTermPalette {
    /// Decode the validated byte-vector published by Lisp: default fg/bg,
    /// cursor, sixteen foreground slots, then sixteen background slots.
    pub fn from_rgb_bytes(rgb: [u8; 105], bold_is_bright: bool) -> Self {
        let color = |index: usize| {
            Color::from_pixel(
                (u32::from(rgb[index * 3]) << 16)
                    | (u32::from(rgb[index * 3 + 1]) << 8)
                    | u32::from(rgb[index * 3 + 2]),
            )
        };
        Self {
            foreground: color(0),
            background: color(1),
            cursor: color(2),
            ansi_foreground: std::array::from_fn(|index| color(index + 3)),
            ansi_background: std::array::from_fn(|index| color(index + 19)),
            bold_is_bright,
        }
    }

    /// Legacy/native callers without Lisp plumbing still inherit the frame's
    /// default face. The public neo-term constructors publish all GNU slots.
    pub fn fallback(foreground: Color, background: Color) -> Self {
        let ansi = std::array::from_fn(|index| {
            let (r, g, b) = crate::xterm_256_rgb(index as u8);
            Color::from_pixel((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b))
        });
        Self {
            foreground,
            background,
            cursor: foreground,
            ansi_foreground: ansi,
            ansi_background: ansi,
            bold_is_bright: false,
        }
    }
}
