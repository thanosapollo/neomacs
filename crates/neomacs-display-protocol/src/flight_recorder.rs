//! Always-on, payload-free CPU wall-phase history. No GPU/photon claims.
//! Writers never wait, allocate, wake workers or perform IO. Old entries are
//! filtered on reads; capacity can truncate the five-minute window under load.
use serde::Serialize;
use std::{
    io::{self, Write},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
pub const CAPACITY: usize = 131072;
pub const RETENTION_NS: u64 = 300_000_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    CommandStart,
    CommandEnd,
    RedisplayStart,
    RedisplayEnd,
    LayoutStart,
    LayoutEnd,
    LayoutSealed,
    RenderHandoff,
    Prepared,
    RenderStart,
    Submit,
    CpuPresentCall,
    Superseded,
    Discarded,
}
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Entry {
    pub phase: Phase,
    pub ns: u64,
    pub command: u64,
    pub input_stream: u64,
    pub event_seq: u64,
    pub frame: u64,
    pub revision: u64,
}
const EMPTY: Entry = Entry {
    phase: Phase::CommandStart,
    ns: 0,
    command: 0,
    input_stream: 0,
    event_seq: 0,
    frame: 0,
    revision: 0,
};
struct Ring<const N: usize> {
    entries: [Entry; N],
    next: usize,
    len: usize,
    overwritten: u64,
}
impl<const N: usize> Ring<N> {
    const fn new() -> Self {
        Self {
            entries: [EMPTY; N],
            next: 0,
            len: 0,
            overwritten: 0,
        }
    }
    fn push(&mut self, entry: Entry) {
        self.entries[self.next] = entry;
        self.next = (self.next + 1) % N;
        if self.len == N {
            self.overwritten += 1;
        } else {
            self.len += 1;
        }
    }
    fn recent(&self, now: u64) -> Vec<Entry> {
        (0..self.len)
            .map(|i| self.entries[(self.next + N - self.len + i) % N])
            .filter(|entry| now.saturating_sub(entry.ns) <= RETENTION_NS)
            .collect()
    }
}
static RING: Mutex<Ring<CAPACITY>> = Mutex::new(Ring::new());
static DROPPED: AtomicU64 = AtomicU64::new(0);
static CLOCK_ERRORS: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "linux")]
fn now() -> Option<u64> {
    crate::present_trace::clock_ns(libc::CLOCK_MONOTONIC)
}
#[cfg(not(target_os = "linux"))]
fn now() -> Option<u64> {
    // No blocking once initialization, even on the first concurrent call.
    static ORIGIN: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let mut origin = ORIGIN.try_lock().ok()?;
    let origin = origin.get_or_insert_with(std::time::Instant::now);
    Some(origin.elapsed().as_nanos().min(u64::MAX as u128) as u64)
}
/// Sample Linux CLOCK_MONOTONIC (else process-relative); zero identities mean unavailable.
pub fn record(phase: Phase, command: u64, event_seq: u64, frame: u64, revision: u64) {
    record_input(phase, command, 0, event_seq, frame, revision);
}
/// Existing input identity only; never infer an acknowledgement from this.
pub fn record_input(
    phase: Phase,
    command: u64,
    input_stream: u64,
    event_seq: u64,
    frame: u64,
    revision: u64,
) {
    let Some(ns) = now() else {
        CLOCK_ERRORS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if let Ok(mut ring) = RING.try_lock() {
        ring.push(Entry {
            phase,
            ns,
            command,
            input_stream,
            event_seq,
            frame,
            revision,
        });
    } else {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}
#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub dropped_contention: u64,
    pub dropped_clock: u64,
    pub overwritten: u64,
    pub retention_ns: u64,
    pub capacity: usize,
    pub busy: bool,
    pub now_ns: u64,
}
/// Read-only snapshot: contention yields an explicitly busy empty snapshot.
pub fn recent() -> Snapshot {
    let sampled = now();
    let now_ns = sampled.unwrap_or(0);
    let guard = RING.try_lock().ok();
    Snapshot {
        entries: guard.as_ref().map(|r| r.recent(now_ns)).unwrap_or_default(),
        dropped_contention: DROPPED.load(Ordering::Relaxed),
        dropped_clock: CLOCK_ERRORS.load(Ordering::Relaxed),
        overwritten: guard.as_ref().map(|r| r.overwritten).unwrap_or(0),
        retention_ns: RETENTION_NS,
        capacity: CAPACITY,
        busy: guard.is_none() || sampled.is_none(),
        now_ns,
    }
}
/// Only explicit consumers write files. Creation is exclusive (never truncate).
pub fn dump(path: &Path) -> io::Result<()> {
    let snapshot = recent();
    if snapshot.busy {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "flight recorder busy",
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, &snapshot)?;
    file.write_all(b"\n")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flight_recorder_bounded_chronological() {
        const N: usize = 7;
        let mut ring = Ring::<N>::new();
        for ns in 0..N as u64 + 2 {
            ring.push(Entry { ns, ..EMPTY });
        }
        assert_eq!(ring.len, N);
        assert_eq!(ring.overwritten, 2);
        let entries = ring.recent(N as u64 + 2);
        assert_eq!(entries.first().unwrap().ns, 2);
        assert_eq!(entries.last().unwrap().ns, N as u64 + 1);
    }
    #[test]
    fn flight_recorder_expiry_boundary() {
        let mut ring = Ring::<7>::new();
        ring.push(Entry { ns: 10, ..EMPTY });
        assert_eq!(ring.recent(10 + RETENTION_NS).len(), 1);
        assert!(ring.recent(11 + RETENTION_NS).is_empty());
        assert_eq!(ring.len, 1);
    }
    #[test]
    fn flight_recorder_contention_nonblocking() {
        let _guard = RING.lock().unwrap();
        let before = DROPPED.load(Ordering::Relaxed);
        record(Phase::CpuPresentCall, 0, 0, 1, 2);
        assert!(DROPPED.load(Ordering::Relaxed) > before);
        assert!(recent().busy);
    }
}
