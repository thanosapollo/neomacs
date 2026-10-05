use super::*;

#[test]
fn palette_boundary_validates_before_admission_and_can_retire_explicit_frame() {
    let host = RecordingTerminalDisplayHost::default();
    let mut eval = Context::new();
    eval.set_display_host(Box::new(host.clone()));
    for palette in [
        "[]",
        "nil",
        "(make-vector 105 0)",
        "(make-vector 107 0)",
        "(let ((v (make-vector 106 0))) (aset v 105 nil) (aset v 0 256) v)",
        "(let ((v (make-vector 106 0))) (aset v 105 nil) (aset v 0 -1) v)",
        "(let ((v (make-vector 106 0))) (aset v 105 nil) (aset v 0 0.5) v)",
        "(make-vector 106 0)",
    ] {
        // Invalid frame refuses even a valid nil retirement; bad vector fields
        // are exercised on the selected live frame.
        let frame = if palette == "nil" {
            "'not-a-frame"
        } else {
            "(selected-frame)"
        };
        assert_eq!(eval.eval_str(&format!("(condition-case nil (progn (neomacs-terminal-set-palette {frame} {palette}) 'unexpected) (error 'refused))")).unwrap(), Value::symbol("refused"));
    }
    assert!(host.events.lock().unwrap().is_empty());
    eval.eval_str("(let ((palette (make-vector 106 17))) (aset palette 105 t) (neomacs-terminal-set-palette (selected-frame) palette))").unwrap();
    eval.eval_str("(neomacs-terminal-set-palette (selected-frame) nil)")
        .unwrap();
    let events = host.events.lock().unwrap();
    assert_eq!(events.len(), 2);
    let TerminalHostEvent::Palette {
        frame,
        palette: Some(palette),
    } = &events[0]
    else {
        panic!("expected palette")
    };
    assert!(palette.bold_is_bright);
    assert_eq!(
        palette.foreground,
        neomacs_display_protocol::Color::from_pixel(0x00111111)
    );
    assert_eq!(
        events[1],
        TerminalHostEvent::Palette {
            frame: *frame,
            palette: None
        }
    );
}
