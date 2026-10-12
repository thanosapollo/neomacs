use super::super::*;
use neovm_core::face::FontSlant;

#[test]
fn selected_font_name_projection_preserves_native_style_without_capability_loading() {
    let mut metrics = FontMetricsService::new();
    let selected = metrics
        .select_font_for_char('M', "Monospace", 400, false, 16.0)
        .expect("native monospace selection");
    for width in 0..=10 {
        for slant in [
            FontSlant::Normal,
            FontSlant::ReverseItalic,
            FontSlant::ReverseOblique,
        ] {
            let mut font = selected.clone();
            font.resolved.width = width;
            font.slant = slant;
            font.foundry = Some("Selected Foundry".into());
            let name = core_font_name_from_selection(font.clone());
            let mut capability_calls = 0;
            let opened = core_opened_font_from_selection(font, |_, _| {
                capability_calls += 1;
                None
            });
            assert_eq!(
                name,
                neovm_core::emacs_core::display_host::ResolvedFontName::from_match(
                    &ResolvedFontMatch {
                        font: opened,
                        glyph_code: None
                    }
                ),
            );
            if selected.resolved.identity.file_path.is_some() {
                assert_eq!(
                    capability_calls, 1,
                    "full font-at path still loads capabilities"
                );
            }
        }
    }
}

#[test]
fn selected_font_capability_reuse_preserves_the_complete_opened_font() {
    // Exercise native selection and the complete projection used by font-at,
    // without a window constructor, renderer or event loop.
    let mut metrics = FontMetricsService::new();
    let selected = metrics
        .select_font_for_char('M', "Monospace", 400, false, 16.0)
        .expect("native monospace selection");
    let name_only = core_font_name_from_selection(selected.clone());
    let expected = core_opened_font_from_selection(selected, font_otf_capability_for_file);
    assert_eq!(
        name_only,
        neovm_core::emacs_core::display_host::ResolvedFontName::from_match(&ResolvedFontMatch {
            font: expected.clone(),
            glyph_code: None,
        }),
        "name-only selection preserves all public name properties",
    );
    for _ in 0..30 {
        let selected = metrics
            .select_font_for_char('M', "Monospace", 400, false, 16.0)
            .expect("same native selection");
        let cached = core_opened_font_from_selection(selected, |file, face_index| {
            metrics
                .otf_capability_for_file(file, face_index)
                .map(core_font_otf_capability)
        });
        assert_eq!(
            cached, expected,
            "names, metrics, identity and both OTF tables stay unchanged"
        );
    }
    metrics.clear_caches();
    let selected = metrics
        .select_font_for_char('M', "Monospace", 400, false, 16.0)
        .expect("selection after native invalidation");
    let reopened = core_opened_font_from_selection(selected, |file, face_index| {
        metrics
            .otf_capability_for_file(file, face_index)
            .map(core_font_otf_capability)
    });
    assert_eq!(reopened, expected);
}
