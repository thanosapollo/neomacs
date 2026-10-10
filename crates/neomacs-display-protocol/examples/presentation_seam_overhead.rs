use neomacs_display_protocol::{
    PresentationId,
    present_trace::{self, Stage},
};
use std::{hint::black_box, time::Instant};
fn main() {
    present_trace::init();
    let n = 1_000_000u64;
    for round in 0..15 {
        let start = Instant::now();
        for id in 0..n {
            present_trace::record(
                Stage::RenderStart,
                black_box(1),
                PresentationId::new(black_box(id)),
            );
            present_trace::record(
                Stage::Present,
                black_box(1),
                PresentationId::new(black_box(id)),
            );
        }
        println!(
            "round={round} commands={n} ns={}",
            start.elapsed().as_nanos()
        );
    }
}
