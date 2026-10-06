use super::*;
use crate::terminal::content::{RenderCell, RenderCursor, TerminalContent};
use neomacs_display_protocol::neo_term_palette::NeoTermPalette;
use rio_vt::config::colors::{AnsiColor, NamedColor};
use rio_vt::crosswords::style::StyleFlags;

#[test]
fn cached_semantic_grid_reprojects_dark_light_inverse_and_cursor() {
    let mut frame = FrameGlyphBuffer::with_size(80.0, 24.0);
    frame.glyphs.push(FrameGlyph::Terminal {
        terminal_id: 7,
        x: 0.0,
        y: 0.0,
        width: 80.0,
        height: 24.0,
    });
    let content = TerminalContent {
        cells: vec![RenderCell {
            col: 0,
            row: 0,
            c: 'x',
            fg: Color::WHITE,
            bg: Color::BLACK,
            ansi: Some((
                AnsiColor::Named(NamedColor::Foreground),
                AnsiColor::Named(NamedColor::Background),
            )),
            flags: StyleFlags::INVERSE | StyleFlags::BOLD,
        }],
        cols: 1,
        rows: 1,
        cursor: RenderCursor {
            col: 0,
            row: 0,
            visible: true,
        },
        default_fg: Color::WHITE,
        default_bg: Color::BLACK,
    };
    let contents = HashMap::from([(crate::terminal::TerminalId::new(7).unwrap(), content)]);
    for (foreground, background) in [(Color::WHITE, Color::BLACK), (Color::BLACK, Color::WHITE)] {
        let mut palette = NeoTermPalette::fallback(foreground, background);
        palette.cursor = Color::RED;
        let (glyphs, faces) =
            RenderApp::expanded_terminal_glyphs_for_frame(&frame, &contents, &palette);
        assert!(
            matches!(glyphs.first(), Some(FrameGlyph::Stretch { bg, .. }) if *bg == background)
        );
        assert!(glyphs.iter().any(|g| matches!(g, FrameGlyph::Stretch { bg, width, .. } if *bg == foreground && *width == frame.char_width)));
        let face = glyphs
            .iter()
            .find_map(|g| match g {
                FrameGlyph::Char { face_id, .. } => faces.get(face_id),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            face.foreground, background,
            "inverse must swap resolved defaults"
        );
        assert!(face.attributes.contains(FaceAttributes::BOLD));
        assert!(
            glyphs
                .iter()
                .any(|g| matches!(g, FrameGlyph::Border { color, .. } if *color == Color::RED))
        );
    }
    assert_eq!(
        contents.values().next().unwrap().default_bg,
        Color::BLACK,
        "reprojection must not rewrite the cached grid"
    );
}
