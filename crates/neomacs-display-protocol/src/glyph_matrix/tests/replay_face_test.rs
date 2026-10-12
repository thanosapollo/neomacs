//! A retained row must publish every face its glyphs or decorations can draw.

use super::*;
use crate::face::Face;

#[test]
fn shifted_fringe_proof_requires_both_exact_producer_slots() {
    let mut row = GlyphRow::new(GlyphRowRole::Text);
    let left = bitmap(1, 21);
    let right = bitmap(1, 22);
    row.left_fringe_bitmap = Some(left);
    assert!(!row.fringes_are_row_local());
    row.left_fringe_producer = Some(RowFringeProducer::SourceDisplay(left));
    assert!(row.fringes_are_row_local());
    row.right_fringe_bitmap = Some(right);
    assert!(!row.fringes_are_row_local());
    row.right_fringe_producer = Some(RowFringeProducer::RowIndicator(right));
    assert!(row.fringes_are_row_local());
    let mut shifted = row.clone();
    shifted.pixel_y -= 20.0;
    assert!(shifted.fringes_are_row_local());
    shifted.left_fringe_bitmap = Some(bitmap(1, 99));
    assert!(
        !shifted.fringes_are_row_local(),
        "same bitmap with different face is not certified"
    );
    shifted.left_fringe_bitmap = Some(bitmap(2, 21));
    assert!(
        !shifted.fringes_are_row_local(),
        "same face with different bitmap is not certified"
    );
}

fn bitmap(index: u16, face: u32) -> FringeBitmapInfo {
    FringeBitmapInfo {
        bitmap_index: index,
        face_id: FaceId::new(face),
    }
}

#[test]
fn bitmap_only_row_retains_all_three_decoration_faces() {
    let mut row = GlyphRow::new(GlyphRowRole::Text);
    assert_eq!(row.referenced_face_ids().count(), 0);
    row.left_fringe_bitmap = Some(bitmap(1, 21));
    row.right_fringe_bitmap = Some(bitmap(2, 22));
    row.overlay_arrow_bitmap = Some(bitmap(3, 23));
    assert_eq!(row.total_glyphs(), 0);

    let source_faces: FrameFaceMap = [21, 22, 23]
        .into_iter()
        .map(|id| {
            let face = Face::new(FaceId::new(id));
            (face.id, face)
        })
        .collect();
    let replay_faces: FrameFaceMap = row
        .referenced_face_ids()
        .map(|id| (id, source_faces[&id].clone()))
        .collect();
    assert_eq!(replay_faces.len(), 3);
    for id in [21, 22, 23] {
        assert!(
            replay_faces.contains_key(&FaceId::new(id)),
            "a bitmap face must survive replay without a glyph publishing it"
        );
    }
}

#[test]
fn bitmap_dependencies_keep_all_glyph_and_hover_faces() {
    let mut row = GlyphRow::new(GlyphRowRole::Text);
    for (area, id) in GlyphArea::ALL.into_iter().zip([11, 12, 13]) {
        row.glyphs[area.index()].push(Glyph::char('x', FaceId::new(id), 0));
    }
    row.intern_pointer_appearance(GlyphPointerAppearance {
        source: GlyphPointerSourceIdentity {
            kind: GlyphPointerSourceKind::Buffer,
            source_id: 7,
            range_start: 0,
            range_end: 1,
            property_owner: 0,
            occurrence: GlyphPointerOccurrenceIdentity::Source,
        },
        face_id: FaceId::new(14),
    })
    .expect("one hover appearance fits the row-local table");
    row.left_fringe_bitmap = Some(bitmap(1, 21));
    row.right_fringe_bitmap = Some(bitmap(2, 22));
    row.overlay_arrow_bitmap = Some(bitmap(3, 23));

    assert_eq!(
        row.referenced_face_ids().collect::<Vec<_>>(),
        [11, 12, 13, 14, 21, 22, 23]
            .into_iter()
            .map(FaceId::new)
            .collect::<Vec<_>>()
    );
}
