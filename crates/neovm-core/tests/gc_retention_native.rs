//! Native integration evidence, distinct from historical GC unit-test runs.

#[test]
fn native_original_killed_window_retention() {
    neovm_core::gc_retention_harness::a_window_configuration_keeps_a_killed_window_buffer();
}

#[test]
fn native_observational_killed_window_retention() {
    neovm_core::gc_retention_harness::killed_window_retention_observational_discriminator();
}
