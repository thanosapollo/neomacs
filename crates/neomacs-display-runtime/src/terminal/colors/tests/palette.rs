use super::*;
use neomacs_display_protocol::neo_term_palette::NeoTermPalette;
use rio_vt::config::colors::ColorRgb;

fn palette() -> NeoTermPalette {
    // Deliberately distinct fg/bg slots detect channel confusion and fixed ANSI.
    NeoTermPalette::from_rgb_bytes(std::array::from_fn(|index| index as u8), false)
}

#[test]
fn canonical_sixteen_slots_are_channel_specific_including_indexed() {
    let palette = palette();
    for index in 0..16 {
        let color = AnsiColor::Indexed(index);
        assert_eq!(
            palette_color(&color, &palette, false, false),
            palette.ansi_foreground[index as usize]
        );
        assert_eq!(
            palette_color(&color, &palette, true, false),
            palette.ansi_background[index as usize]
        );
    }
    assert_eq!(
        palette_color(&AnsiColor::Named(NamedColor::Red), &palette, false, false),
        palette.ansi_foreground[1]
    );
    assert_eq!(
        palette_color(
            &AnsiColor::Named(NamedColor::LightRed),
            &palette,
            true,
            false
        ),
        palette.ansi_background[9]
    );
    assert_ne!(
        ansi_to_color(
            &AnsiColor::Named(NamedColor::Red),
            &palette.foreground,
            &palette.background
        ),
        palette.ansi_foreground[1]
    );
}

#[test]
fn bold_does_not_implicitly_brighten_but_obeys_gnu_option() {
    let mut palette = palette();
    let red = AnsiColor::Named(NamedColor::Red);
    let blue = AnsiColor::Indexed(4);
    assert_eq!(
        palette_colors(&red, &blue, &palette, true, false),
        (palette.ansi_foreground[1], palette.ansi_background[4])
    );
    palette.bold_is_bright = true;
    assert_eq!(
        palette_colors(&red, &blue, &palette, true, false),
        (palette.ansi_foreground[9], palette.ansi_background[12])
    );
    assert_eq!(
        palette_color(
            &AnsiColor::Named(NamedColor::LightRed),
            &palette,
            false,
            true
        ),
        palette.ansi_foreground[9]
    );
}

#[test]
fn explicit_extended_indices_and_rgb_remain_application_owned() {
    let mut palette = palette();
    palette.bold_is_bright = true;
    for index in 16..=255 {
        let color = AnsiColor::Indexed(index);
        let explicit = ansi_to_color(&color, &Color::WHITE, &Color::BLACK).srgb_to_linear();
        assert_eq!(palette_color(&color, &palette, false, true), explicit);
        assert_eq!(palette_color(&color, &palette, true, true), explicit);
    }
    let rgb = AnsiColor::Spec(ColorRgb {
        r: 17,
        g: 121,
        b: 239,
    });
    assert_eq!(
        palette_color(&rgb, &palette, false, true),
        Color::from_u8(17, 121, 239, 255).srgb_to_linear()
    );
}

#[test]
fn inverse_resolves_channels_before_exactly_one_swap() {
    let palette = palette();
    let fg = AnsiColor::Named(NamedColor::Red);
    let bg = AnsiColor::Indexed(4);
    assert_eq!(
        palette_colors(&fg, &bg, &palette, false, true),
        (palette.ansi_background[4], palette.ansi_foreground[1])
    );
    assert_eq!(
        palette_colors(
            &AnsiColor::Named(NamedColor::Foreground),
            &AnsiColor::Named(NamedColor::Background),
            &palette,
            false,
            true
        ),
        (palette.background, palette.foreground)
    );
}

#[test]
fn default_reset_and_cached_cells_follow_new_frame_palette() {
    let dark = NeoTermPalette::fallback(Color::WHITE, Color::BLACK);
    let light = NeoTermPalette::fallback(Color::BLACK, Color::WHITE);
    let fg = AnsiColor::Named(NamedColor::Foreground);
    let bg = AnsiColor::Named(NamedColor::Background);
    assert_eq!(
        palette_colors(&fg, &bg, &dark, false, false),
        (Color::WHITE, Color::BLACK)
    );
    assert_eq!(
        palette_colors(&fg, &bg, &light, false, false),
        (Color::BLACK, Color::WHITE)
    );
    assert_eq!(
        palette_color(
            &AnsiColor::Named(NamedColor::Cursor),
            &palette(),
            false,
            false
        ),
        palette().cursor
    );
}
