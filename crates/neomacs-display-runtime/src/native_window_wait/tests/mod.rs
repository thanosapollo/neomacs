use super::*;
#[test]
fn active_revocation_terminalizes_only_its_connection() {
    let waits = Arc::new(NativeWindowWaits::default());
    let live = Arc::new(AtomicBool::new(true));
    waits.register(
        1,
        live.clone(),
        Instant::now() + std::time::Duration::from_secs(15),
    );
    let result = waits.run(1, || {
        live.store(false, Ordering::Release);
        assert!(waits.interrupt_required());
        Ok(42)
    });
    assert!(result.is_err());
    assert!(waits.terminal());
    let fresh = Arc::new(AtomicBool::new(true));
    waits.register(
        2,
        fresh,
        Instant::now() + std::time::Duration::from_secs(15),
    );
    assert!(waits.run(2, || Ok(42)).is_err());
}
#[test]
fn completed_constructor_loses_interrupt_authority() {
    let waits = NativeWindowWaits::default();
    let live = Arc::new(AtomicBool::new(true));
    waits.register(
        1,
        live.clone(),
        Instant::now() + std::time::Duration::from_secs(15),
    );
    assert_eq!(waits.run(1, || Ok(42)), Ok(42));
    live.store(false, Ordering::Release);
    assert!(!waits.interrupt_required());
    assert!(!waits.terminal());
}
