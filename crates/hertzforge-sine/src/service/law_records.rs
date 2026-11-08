//! Every control-law call, recorded.
//!
//! The control tick pushes one [`LawRecord`] per call into a bounded ring the
//! step owns (fixed records in a buffer sized when the step is built, taken
//! with `try_lock` so a reader never holds the tick up); the service drains it
//! between batches and, while a capture runs, appends the records to
//! `law_ticks.bin` in the run's folder, with `law_ticks.json` beside it saying
//! what the bytes are, and `law.json` naming the law. Nothing on the control
//! thread touches a file.
//!
//! `law_ticks.bin` is a sequence of [`LawRecord::BYTES`]-byte little-endian
//! records, fields in [`LAW_TICK_FIELDS`] order.

use crate::law::{Hold, LawFault, LawValues, MAX_LAW_VALUES};
use std::sync::Mutex;

/// The format name written into `law_ticks.json`.
pub const LAW_TICKS_FORMAT: &str = "hertzforge-sine-law-ticks 1";

/// Records the ring holds before it drops the newest (and counts it). Drained
/// every batch, which is a few control ticks: this is many seconds of runway.
pub const LAW_RECORD_CAPACITY: usize = 4096;

/// Why a call was not used, as recorded: 0 used, then the miss reasons.
pub fn miss_code(fault: Option<&LawFault>) -> u8 {
    match fault {
        None => 0,
        Some(LawFault::Failed(_)) => 1,
        Some(LawFault::Crashed(_)) => 2,
        Some(LawFault::OverBudget { .. }) => 3,
        Some(LawFault::OverMemory) => 4,
        Some(LawFault::BadOutput(r)) => match r {
            crate::law::SineOutputRule::NonFinite => 5,
            crate::law::SineOutputRule::Negative => 6,
            crate::law::SineOutputRule::UndeclaredValue => 7,
            crate::law::SineOutputRule::TooManyValues => 8,
        },
    }
}

/// The names of the miss codes, by code.
pub const MISS_NAMES: &[&str] = &[
    "used",
    "error",
    "panicked",
    "over_budget",
    "over_memory",
    "non_finite",
    "negative",
    "undeclared_value",
    "too_many_values",
];

/// One call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LawRecord {
    /// Ticks since the law was engaged (0 at engage).
    pub tick: u64,
    /// The tone's frequency, Hz.
    pub freq_hz: f64,
    /// The target at this frequency and level.
    pub target: f64,
    /// The measured composite.
    pub measured: f64,
    /// The drive in force before the call, drive units peak.
    pub drive_in_force: f64,
    /// The prediction at this frequency, dB; NaN where not trusted.
    pub prediction_db: f64,
    /// Why the core held the drive (0 free, 1 notch, 2 probe).
    pub hold: u8,
    /// The law's drive; NaN when the call gave none.
    pub output: f64,
    /// 0 when the output was used, else the miss reason ([`MISS_NAMES`]).
    pub miss: u8,
    /// Whether a core cap bound the output.
    pub capped: bool,
    /// The drive the core took from the call, before the notch and the drive
    /// ceiling.
    pub applied: f64,
    /// How long the call took, microseconds (0 when untimed).
    pub call_us: f32,
    /// The law's values on the call, by declared index (NaN where unset).
    pub values: [f64; MAX_LAW_VALUES],
}

/// The fields of a record in `law_ticks.bin`, in order, with their types.
pub const LAW_TICK_FIELDS: &[(&str, &str)] = &[
    ("tick", "u64"),
    ("freq_hz", "f64"),
    ("target", "f64"),
    ("measured", "f64"),
    ("drive_in_force", "f64"),
    ("prediction_db", "f64"),
    ("hold", "u8"),
    ("miss", "u8"),
    ("capped", "u8"),
    ("pad", "u8"),
    ("call_us", "f32"),
    ("output", "f64"),
    ("applied", "f64"),
    ("values", "f64x8"),
];

impl LawRecord {
    /// Bytes per record in `law_ticks.bin`.
    pub const BYTES: usize = 8 * 6 + 4 + 4 + 8 * 2 + 8 * MAX_LAW_VALUES;

    pub(crate) fn hold_code(h: Hold) -> u8 {
        match h {
            Hold::Free => 0,
            Hold::Notch => 1,
            Hold::Probe => 2,
        }
    }

    pub(crate) fn values_of(v: &LawValues) -> [f64; MAX_LAW_VALUES] {
        let mut out = [f64::NAN; MAX_LAW_VALUES];
        for (i, x) in v.iter() {
            out[i] = x;
        }
        out
    }

    /// Append the record's bytes.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.tick.to_le_bytes());
        for v in [
            self.freq_hz,
            self.target,
            self.measured,
            self.drive_in_force,
            self.prediction_db,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&[self.hold, self.miss, self.capped as u8, 0]);
        out.extend_from_slice(&self.call_us.to_le_bytes());
        out.extend_from_slice(&self.output.to_le_bytes());
        out.extend_from_slice(&self.applied.to_le_bytes());
        for v in self.values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

/// The bounded ring of calls.
pub struct LawRecordRing {
    inner: Mutex<RingInner>,
}

struct RingInner {
    buf: Vec<LawRecord>,
    head: usize,
    len: usize,
    dropped: u64,
}

impl LawRecordRing {
    pub(crate) fn new(capacity: usize) -> Self {
        let blank = LawRecord {
            tick: 0,
            freq_hz: 0.0,
            target: 0.0,
            measured: 0.0,
            drive_in_force: 0.0,
            prediction_db: 0.0,
            hold: 0,
            output: 0.0,
            miss: 0,
            capped: false,
            applied: 0.0,
            call_us: 0.0,
            values: [0.0; MAX_LAW_VALUES],
        };
        LawRecordRing {
            inner: Mutex::new(RingInner {
                buf: vec![blank; capacity.max(1)],
                head: 0,
                len: 0,
                dropped: 0,
            }),
        }
    }

    /// Push from the control tick: never waits, never allocates. A busy or a
    /// full ring drops the record and counts it.
    pub(crate) fn push(&self, r: LawRecord) {
        let Ok(mut g) = self.inner.try_lock() else {
            return;
        };
        let cap = g.buf.len();
        if g.len == cap {
            g.dropped += 1;
            return;
        }
        let at = (g.head + g.len) % cap;
        g.buf[at] = r;
        g.len += 1;
    }

    /// Take every record, oldest first (between batches).
    pub fn drain_into(&self, out: &mut Vec<LawRecord>) {
        if let Ok(mut g) = self.inner.lock() {
            let cap = g.buf.len();
            for i in 0..g.len {
                out.push(g.buf[(g.head + i) % cap]);
            }
            g.head = (g.head + g.len) % cap;
            g.len = 0;
        }
    }

    /// Records dropped because the ring was full or busy.
    pub fn dropped(&self) -> u64 {
        self.inner.lock().map(|g| g.dropped).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_is_its_stated_size_and_the_ring_counts_what_it_drops() {
        let ring = LawRecordRing::new(2);
        let mut r = LawRecord {
            tick: 1,
            freq_hz: 100.0,
            target: 1.0,
            measured: 0.9,
            drive_in_force: 0.1,
            prediction_db: f64::NAN,
            hold: 0,
            output: 0.11,
            miss: 0,
            capped: false,
            applied: 0.11,
            call_us: 3.0,
            values: [f64::NAN; MAX_LAW_VALUES],
        };
        let mut bytes = Vec::new();
        r.write_to(&mut bytes);
        assert_eq!(bytes.len(), LawRecord::BYTES);
        ring.push(r);
        r.tick = 2;
        ring.push(r);
        r.tick = 3;
        ring.push(r);
        assert_eq!(ring.dropped(), 1);
        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out.iter().map(|r| r.tick).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(MISS_NAMES.len(), 9);
        assert_eq!(
            MISS_NAMES[miss_code(Some(&LawFault::OverMemory)) as usize],
            "over_memory"
        );
    }
}
