use neomacs_display_protocol::flight_recorder::{self, Phase};
#[test]
fn records_cpu_phase_without_payload() {
    flight_recorder::record(Phase::CpuPresentCall, 0, 0, 12345, 67890);
    let s = flight_recorder::recent();
    assert!(!s.busy);
    assert!(
        s.entries
            .iter()
            .any(|e| e.frame == 12345 && e.revision == 67890 && e.phase == Phase::CpuPresentCall)
    );
    assert!(s.entries.len() <= s.capacity);
}
