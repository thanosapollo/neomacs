//! Focused production font-info opening microbenchmark, not a GUI journey.
use neomacs_layout_engine::font::{metrics::FontMetricsService, resolver::FontEntityQuery};
use neomacs_layout_engine::font_backend::FontFamilyName;
use std::{hint::black_box, time::Instant};
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
fn thread_cpu_ns() -> Option<u64> {
    #[repr(C)]
    struct Timespec {
        seconds: i64,
        nanos: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, result: *mut Timespec) -> i32;
    }
    let mut t = Timespec {
        seconds: 0,
        nanos: 0,
    };
    // Linux CLOCK_THREAD_CPUTIME_ID; this is the benchmark thread, not Lisp.
    assert_eq!(unsafe { clock_gettime(3, &mut t) }, 0);
    Some((t.seconds as u64) * 1_000_000_000 + t.nanos as u64)
}
#[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
fn thread_cpu_ns() -> Option<u64> {
    None
}
fn main() {
    let family = std::env::args().nth(1).unwrap_or_else(|| "Iosevka".into());
    let count: u32 = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "1000".into())
        .parse()
        .unwrap();
    let mut service = FontMetricsService::new();
    let query = FontEntityQuery::new(Some(FontFamilyName::new(&family).unwrap()))
        .with_weight(400)
        .with_slant(neovm_core::face::FontSlant::Normal)
        .with_width(neovm_core::face::FontWidth::Normal);
    let first = service
        .open_font_entity(&query, 20)
        .expect("font must open");
    println!(
        "identity={:?} metrics={:?}",
        first.entity.matched.identity, first.metrics
    );
    for sample in 0..7 {
        let cpu = thread_cpu_ns();
        let start = Instant::now();
        for _ in 0..count {
            let opened = service
                .open_font_entity(black_box(&query), black_box(20))
                .expect("font must open");
            assert_eq!(
                opened, first,
                "microbenchmark must preserve exact opened identity and metrics"
            );
            black_box(opened);
        }
        let wall_ns = start.elapsed().as_nanos();
        let cpu_ns = match (cpu, thread_cpu_ns()) {
            (Some(before), Some(after)) => (after - before).to_string(),
            _ => "UNKNOWN".to_owned(),
        };
        println!("sample={sample} count={count} wall_ns={wall_ns} thread_cpu_ns={cpu_ns}");
    }
    service.clear_caches();
    assert_eq!(
        service.open_font_entity(&query, 20).unwrap(),
        first,
        "clear must preserve observable result"
    );
}
