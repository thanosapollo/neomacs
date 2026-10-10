//! Paired fixed-loop CPU seam benchmark, not a GUI/scoreboard benchmark.
use neomacs_display_protocol::flight_recorder::{self, Phase};
use std::{hint::black_box, time::Instant};
fn main() {
    let n = 1_000_000u64;
    for round in 0..15 {
        let modes = if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        };
        for enabled in modes {
            let start = Instant::now();
            for id in 0..n {
                black_box(id);
                if enabled {
                    flight_recorder::record(Phase::CommandStart, id, 0, 0, 0);
                    flight_recorder::record(Phase::CommandEnd, id, 0, 0, 0);
                }
            }
            println!(
                "round={round} enabled={enabled} commands={n} ns={}",
                start.elapsed().as_nanos()
            );
        }
    }
    let snapshot = flight_recorder::recent();
    println!(
        "entries={} overwritten={} dropped={} capacity={} bytes_per_entry={}",
        snapshot.entries.len(),
        snapshot.overwritten,
        snapshot.dropped_contention,
        snapshot.capacity,
        std::mem::size_of::<flight_recorder::Entry>()
    );
}
