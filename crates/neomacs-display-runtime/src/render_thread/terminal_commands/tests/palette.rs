use super::*;
use neomacs_display_protocol::neo_term_palette::NeoTermPalette;
use neomacs_display_protocol::types::DisplayFrameId;

#[test]
fn palette_changes_schedule_idle_reprojection_without_terminal_or_grid_work() {
    let mut app = make_test_app();
    let frame = DisplayFrameId::new(17);
    let palette = NeoTermPalette::fallback(Color::WHITE, Color::BLACK);
    let clear = |app: &mut RenderApp| {
        app.frame_windows
            .primary_window_mut()
            .unwrap()
            .render
            .begin_presentable_render();
    };
    let dirty = |app: &RenderApp| {
        app.frame_windows
            .primary_window()
            .unwrap()
            .render
            .compositor
            .dirty
    };
    clear(&mut app);
    app.handle_terminal(TerminalCommand::TerminalSetPalette {
        frame,
        palette: Some(palette),
    });
    assert!(dirty(&app));
    assert_eq!(app.terminal_manager.palettes.get(&frame), Some(&palette));
    assert!(app.terminal_manager.terminals.is_empty());
    clear(&mut app);
    app.handle_terminal(TerminalCommand::TerminalSetPalette {
        frame,
        palette: Some(palette),
    });
    assert!(!dirty(&app), "identical cached palette is a no-op");
    app.handle_terminal(TerminalCommand::TerminalSetPalette {
        frame,
        palette: None,
    });
    assert!(dirty(&app));
    assert!(!app.terminal_manager.palettes.contains_key(&frame));
}
