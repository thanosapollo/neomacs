use crate::emacs_core::format_eval_result;

#[test]
fn discard_buffer_matches_gnu_parameter_cleanup() {
    let mut eval = crate::test_utils::runtime_startup_context();
    let buffer = eval.buffers.create_buffer("*scratch*");
    eval.buffers.set_current(buffer);
    eval.frames.create_frame("F1", 800, 600, buffer);
    let results = eval.eval_str_each(include_str!("discard_buffer.el"));
    for result in &results {
        assert!(result.is_ok(), "{}", format_eval_result(result));
    }
    assert_eq!(format_eval_result(results.last().unwrap()), "OK 63");
}
