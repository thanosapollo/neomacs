use super::*;
use neomacs_display_protocol::flight_recorder::{self, Phase};

#[test]
fn flight_recorder_layout_producer_records_sealed_revision_and_aborts() {
    let (mut eval, frame_id, _, _) = incr_editing_frame("flight recorder\n", 800, 600);
    let mut engine = LayoutEngine::new();
    let before = flight_recorder::recent().entries.last().map_or(0, |e| e.ns);
    let state = match engine.redisplay_frame_attempt(&mut eval, frame_id) {
        FrameLayoutAttempt::Prepared(state) => state,
        FrameLayoutAttempt::Aborted => panic!("fixture must prepare a presentation"),
    };
    let snapshot = flight_recorder::recent();
    let entries: Vec<_> = snapshot
        .entries
        .iter()
        .filter(|e| e.frame == frame_id.0 && e.ns > before)
        .collect();
    assert!(
        entries
            .iter()
            .any(|e| e.phase == Phase::LayoutStart && e.revision == 0)
    );
    assert!(
        entries
            .iter()
            .any(|e| e.phase == Phase::LayoutEnd && e.revision == state.presentation().get())
    );
    assert!(
        entries
            .iter()
            .any(|e| e.phase == Phase::LayoutSealed && e.revision == state.presentation().get())
    );
    assert!(entries.iter().all(|e| e.event_seq == 0));

    let absent = neovm_core::window::FrameId(u64::MAX - 123);
    assert!(matches!(
        engine.redisplay_frame_attempt(&mut eval, absent),
        FrameLayoutAttempt::Aborted
    ));
    let snapshot = flight_recorder::recent();
    let phases: Vec<_> = snapshot
        .entries
        .iter()
        .filter(|e| e.frame == absent.0)
        .map(|e| (e.phase, e.revision))
        .collect();
    assert_eq!(
        phases,
        vec![
            (Phase::LayoutStart, 0),
            (Phase::LayoutEnd, 0),
            (Phase::Discarded, 0)
        ]
    );
}
