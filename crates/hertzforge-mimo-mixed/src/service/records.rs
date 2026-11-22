//! The run's law-call record (A7), modelled on single-output Mixed Mode's:
//! one fixed-size record per law call, the floor law's per loop and each
//! tone's per tick, in `law_ticks.bin` with `law.json` describing the
//! layout. The tone laws' records are pushed by the step into a
//! preallocated buffer and the floor law's by the loop side; both are turned
//! into bytes between batches and handed to a writer thread, which does all
//! the file work (none on the loop's thread).

use std::path::Path;

/// The file the records are written to, in the run's folder.
pub const LAW_TICKS_FILE: &str = "law_ticks.bin";
/// The file that describes them.
pub const LAW_JSON_FILE: &str = "law.json";
/// The records' format name, in `law.json`.
pub const LAW_TICKS_FORMAT: &str = "hertzforge.mimo_mixed.law_ticks.v1";

/// Which law a record is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordRole {
    /// The floor law (one call per loop).
    Floor = 0,
    /// A tone's law instance (one call per control tick).
    Tone = 1,
}

/// One law call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LawRecord {
    /// Station time, seconds since the step was built.
    pub t_s: f64,
    /// The drive in force before the call: the floor's in-band drive rms
    /// summed over the drives (volts, full level) or the norm of the tone's
    /// drive vector (volts peak, at the level).
    pub drive_in: f64,
    /// The drive the core took from the call, same unit.
    pub drive_out: f64,
    /// The call's time, microseconds.
    pub took_us: f32,
    /// Which law.
    pub role: RecordRole,
    /// The tone's index among the tones (0 for the floor).
    pub tone: u8,
    /// 0 for a usable call, else 1.
    pub miss: u8,
    /// Bit 0: a core cap bound. Bit 1: the call was the engage (or the floor
    /// law's seed).
    pub flags: u8,
}

/// The fields of a record in file order, with their types, for `law.json`.
pub const LAW_TICK_FIELDS: [(&str, &str); 8] = [
    ("t_s", "f64"),
    ("drive_in", "f64"),
    ("drive_out", "f64"),
    ("took_us", "f32"),
    ("role", "u8"),
    ("tone", "u8"),
    ("miss", "u8"),
    ("flags", "u8"),
];

impl LawRecord {
    /// Bytes per record.
    pub const BYTES: usize = 32;

    /// Append the record, little-endian, in [`LAW_TICK_FIELDS`] order.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.t_s.to_le_bytes());
        out.extend_from_slice(&self.drive_in.to_le_bytes());
        out.extend_from_slice(&self.drive_out.to_le_bytes());
        out.extend_from_slice(&self.took_us.to_le_bytes());
        out.extend_from_slice(&[self.role as u8, self.tone, self.miss, self.flags]);
    }

    /// Read one record back from its bytes.
    pub fn read_from(b: &[u8]) -> Option<Self> {
        if b.len() < Self::BYTES {
            return None;
        }
        let f = |a: usize| f64::from_le_bytes(b[a..a + 8].try_into().unwrap_or([0; 8]));
        Some(LawRecord {
            t_s: f(0),
            drive_in: f(8),
            drive_out: f(16),
            took_us: f32::from_le_bytes(b[24..28].try_into().unwrap_or([0; 4])),
            role: if b[28] == 0 {
                RecordRole::Floor
            } else {
                RecordRole::Tone
            },
            tone: b[29],
            miss: b[30],
            flags: b[31],
        })
    }
}

/// `law.json`: the format, the layout and the laws by role.
pub fn law_json(floor_law: &str, tone_law: &str, tones: usize) -> serde_json::Value {
    serde_json::json!({
        "format": LAW_TICKS_FORMAT,
        "record_bytes": LawRecord::BYTES,
        "fields": LAW_TICK_FIELDS
            .iter()
            .map(|(n, t)| serde_json::json!({"name": n, "type": t}))
            .collect::<Vec<_>>(),
        "roles": {"floor": floor_law, "tones": tone_law},
        "tones": tones,
    })
}

/// Append `bytes` to the run folder's records, writing `law.json` the first
/// time.
pub fn append(dir: &Path, json: &serde_json::Value, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let j = dir.join(LAW_JSON_FILE);
    if !j.exists() {
        std::fs::write(&j, serde_json::to_vec_pretty(json).unwrap_or_default())?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LAW_TICKS_FILE))?;
    f.write_all(bytes)
}

/// The record's writer thread: the loop side hands it whole records with a
/// send that never waits, and it does all the file work. A write that fails
/// is handed back as words for the run's events; the run goes on.
pub(crate) struct RecordWriter {
    tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    errors: std::sync::mpsc::Receiver<String>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl RecordWriter {
    /// Start the writer for the run folder `dir`, `law.json` holding `json`.
    pub(crate) fn start(dir: std::path::PathBuf, json: serde_json::Value) -> Self {
        Self::start_on(dir, json, |f| {
            std::thread::Builder::new()
                .name("mimo-mixed-law-record".into())
                .spawn(f)
        })
    }

    /// [`Self::start`] with the thread started by `spawn`. A writer that
    /// could not start says so among the writer's failures (the run's
    /// events), and no records are written.
    pub(crate) fn start_on<S>(dir: std::path::PathBuf, json: serde_json::Value, spawn: S) -> Self
    where
        S: FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<std::thread::JoinHandle<()>>,
    {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let (etx, errors) = std::sync::mpsc::channel::<String>();
        let said = etx.clone();
        let run: Box<dyn FnOnce() + Send> = Box::new(move || {
            let mut failed = false;
            for bytes in rx {
                if failed {
                    continue;
                }
                if let Err(e) = append(&dir, &json, &bytes) {
                    failed = true;
                    let _ = etx.send(format!("law record: {e}; no more records are written"));
                }
            }
        });
        match spawn(run) {
            Ok(h) => RecordWriter {
                tx: Some(tx),
                errors,
                handle: Some(h),
            },
            Err(e) => {
                let _ = said.send(format!(
                    "law record: the writer could not start ({e}); no records are written"
                ));
                RecordWriter {
                    tx: None,
                    errors,
                    handle: None,
                }
            }
        }
    }

    /// Hand `bytes` to the writer (never waits).
    pub(crate) fn send(&self, bytes: Vec<u8>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(bytes);
        }
    }

    /// The writer's failures since the last call.
    pub(crate) fn failures(&self) -> Vec<String> {
        self.errors.try_iter().collect()
    }

    /// Let the writer finish what it holds and stop.
    pub(crate) fn close(&mut self) {
        self.tx = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for RecordWriter {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_mixed_law_record_is_its_stated_size_and_reads_back() {
        let r = LawRecord {
            t_s: 1.5,
            drive_in: 0.25,
            drive_out: 0.5,
            took_us: 12.0,
            role: RecordRole::Tone,
            tone: 3,
            miss: 0,
            flags: 1,
        };
        let mut b = Vec::new();
        r.write_to(&mut b);
        assert_eq!(b.len(), LawRecord::BYTES);
        let sizes: usize = LAW_TICK_FIELDS
            .iter()
            .map(|(_, t)| match *t {
                "f64" => 8,
                "f32" => 4,
                _ => 1,
            })
            .sum();
        assert_eq!(sizes, LawRecord::BYTES);
        assert_eq!(LawRecord::read_from(&b), Some(r));
    }

    /// Requirement: a record writer whose thread could not start says so
    /// among its failures (the run's events) and takes records without
    /// effect; it used to lose them with no word.
    #[test]
    fn mimo_mixed_a_record_writer_that_cannot_start_says_so() {
        let dir = std::env::temp_dir().join("mimo-mixed-no-writer-never-made");
        let w = RecordWriter::start_on(dir.clone(), serde_json::json!({}), |_| {
            Err(std::io::Error::other("no thread"))
        });
        w.send(vec![0u8; LawRecord::BYTES]);
        let f = w.failures();
        println!("failures: {f:?}");
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("could not start") && f[0].contains("no thread"));
        assert!(!dir.exists());
    }
}
