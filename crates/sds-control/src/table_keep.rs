//! The table the control service keeps across restarts (schema/PROTOCOL.md,
//! "Restored Tables").
//!
//! After every table write the service hands the table to one keeper thread, which
//! writes `channel_table.json` in the data folder, beside the runs, atomically (a
//! temporary file renamed over the old one). No run thread and no loop does any of
//! this work, and a write that fails is a problem the service reports, never a
//! reason to refuse a table or stop a run.
//!
//! At start the service reads the file back through the same row parser and
//! validation as a table a client sends. A file that cannot be read, is damaged
//! (its rows do not match the SHA-256 written with them) or fails the rule set is
//! not used, and the service says why; it never repairs or guesses. A file that is
//! used is restored, not applied: no run arms on it until the operator applies it.

use crate::random::record::table_json;
use crate::table::{self, StationContext};
use hertzforge_channel_table::ChannelTable;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

/// The kept table's file name in the data folder.
pub const FILE: &str = "channel_table.json";
/// What the file says it is.
pub const FORMAT: &str = "hertzforge.kept_channel_table";
/// The file's version.
pub const VERSION: u64 = 1;

/// The kept table's path under `data_root`.
pub fn path(data_root: &Path) -> PathBuf {
    data_root.join(FILE)
}

/// A table read back from the file.
#[derive(Debug, Clone)]
pub struct Kept {
    pub revision: u64,
    /// When the file was written, unix seconds.
    pub saved_unix: f64,
    pub table: ChannelTable,
}

/// The file's bytes for `table` at `revision`, written at `saved_unix`: the rows in
/// the protocol's own spelling with their SHA-256, as a run's `table.json` holds them.
pub fn contents(table: &ChannelTable, revision: u64, saved_unix: f64) -> Vec<u8> {
    let v = json!({
        "format": FORMAT,
        "version": VERSION,
        "saved_unix": saved_unix,
        "table": table_json(table, revision),
    });
    let mut bytes = serde_json::to_vec_pretty(&v).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

/// Read the kept table: `Ok(None)` when there is none, `Err` with the reason when
/// there is one that cannot be used.
pub fn read(data_root: &Path, ctx: &StationContext<'_>) -> Result<Option<Kept>, String> {
    let p = path(data_root);
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{} could not be read: {e}", p.display())),
    };
    let refuse = |why: String| format!("{} was not used: {why}", p.display());
    let v: Value = serde_json::from_slice(&bytes)
        .map_err(|e| refuse(format!("it is not readable JSON ({e})")))?;
    if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(refuse(format!(
            "it is not a kept channel table (\"format\" is not \"{FORMAT}\")"
        )));
    }
    if v.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return Err(refuse(format!(
            "it is version {}, and this service reads version {VERSION}",
            v.get("version")
                .map_or("absent".to_owned(), Value::to_string)
        )));
    }
    let saved_unix = v
        .get("saved_unix")
        .and_then(Value::as_f64)
        .filter(|t| t.is_finite())
        .ok_or_else(|| refuse("it does not say when it was written".into()))?;
    let t = v
        .get("table")
        .filter(|t| t.is_object())
        .ok_or_else(|| refuse("it holds no table".into()))?;
    let revision = t
        .get("revision")
        .and_then(Value::as_u64)
        .filter(|r| *r >= 1)
        .ok_or_else(|| refuse("its table has no revision".into()))?;
    let sha = t
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| refuse("its table has no SHA-256".into()))?;
    let rows = t
        .get("rows")
        .ok_or_else(|| refuse("its table has no rows".into()))?;
    let table =
        table::parse_rows(rows).map_err(|e| refuse(format!("a row is refused: {}", e.message)))?;
    let again = table_json(&table, revision);
    if again["sha256"].as_str() != Some(sha) {
        return Err(refuse(
            "it is damaged: its rows do not match the SHA-256 written with them".into(),
        ));
    }
    let report = table::validate(&table, ctx);
    if let Some(first) = report.errors.first() {
        let n = report.errors.len();
        return Err(refuse(format!(
            "its table has {n} error{} by the rule set (first: {}: {})",
            if n == 1 { "" } else { "s" },
            first.reason,
            first.message
        )));
    }
    Ok(Some(Kept {
        revision,
        saved_unix,
        table,
    }))
}

struct Job {
    table: ChannelTable,
    revision: u64,
    saved_unix: f64,
}

/// The keeper: one thread that writes the kept table, and the last problem with it.
pub struct Keeper {
    data_root: PathBuf,
    tx: Mutex<Option<Sender<Job>>>,
    problem: Arc<Mutex<Option<String>>>,
}

impl Keeper {
    /// A keeper for `data_root`, starting with `problem` (why the file was not read
    /// at start, if it was not). The thread starts with the first write.
    pub fn new(data_root: PathBuf, problem: Option<String>) -> Keeper {
        Keeper {
            data_root,
            tx: Mutex::new(None),
            problem: Arc::new(Mutex::new(problem)),
        }
    }

    /// The last problem: why the kept table was not used at start, or why the last
    /// write failed. Cleared by a write that succeeds.
    pub fn problem(&self) -> Option<String> {
        self.problem.lock().ok().and_then(|p| p.clone())
    }

    /// Hand the table to the keeper thread. Never waits on the disk.
    pub fn keep(&self, table: &ChannelTable, revision: u64, saved_unix: f64) {
        let job = Job {
            table: table.clone(),
            revision,
            saved_unix,
        };
        let Ok(mut tx) = self.tx.lock() else {
            return;
        };
        if tx.is_none() {
            *tx = self.spawn();
        }
        let failed = match tx.as_ref() {
            Some(t) => t.send(job).is_err(),
            None => true,
        };
        if failed {
            *tx = None;
            self.say(format!(
                "The channel table could not be kept in {}: the keeper thread is not running.",
                path(&self.data_root).display()
            ));
        }
    }

    fn say(&self, words: String) {
        eprintln!("sds-control: {words}");
        if let Ok(mut p) = self.problem.lock() {
            *p = Some(words);
        }
    }

    fn spawn(&self) -> Option<Sender<Job>> {
        let (tx, rx) = channel::<Job>();
        let root = self.data_root.clone();
        let problem = self.problem.clone();
        std::thread::Builder::new()
            .name("sds-control-table-keeper".into())
            .spawn(move || {
                while let Ok(mut job) = rx.recv() {
                    // Only the newest table matters.
                    while let Ok(newer) = rx.try_recv() {
                        job = newer;
                    }
                    let p = path(&root);
                    let r = std::fs::create_dir_all(&root).and_then(|()| {
                        hertzforge_capture::write_atomic(
                            &p,
                            &contents(&job.table, job.revision, job.saved_unix),
                        )
                    });
                    let words = r.err().map(|e| {
                        format!(
                            "The channel table (revision {}) could not be kept in {}: {e}. It is \
                             in use as it is, and will not be restored at the next start.",
                            job.revision,
                            p.display()
                        )
                    });
                    if let Some(w) = &words {
                        eprintln!("sds-control: {w}");
                    }
                    if let Ok(mut slot) = problem.lock() {
                        *slot = words;
                    }
                }
            })
            .ok()
            .map(|_| tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::Selection;
    use crate::table::Table;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sds-control-keep-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sim() -> Selection {
        Selection::Sim
    }

    #[test]
    fn kept_table_file_reads_back_as_written() {
        let d = scratch("back");
        let t = Table::new();
        std::fs::write(path(&d), contents(&t.table, 7, 1_794_240_000.5)).unwrap();
        let sel = sim();
        let ctx = StationContext {
            selection: &sel,
            scan: &[],
        };
        let k = read(&d, &ctx).unwrap().unwrap();
        assert_eq!(k.revision, 7);
        assert_eq!(k.saved_unix, 1_794_240_000.5);
        assert_eq!(table_json(&k.table, 7), table_json(&t.table, 7));
    }

    #[test]
    fn kept_table_file_damaged_or_unreadable_is_refused_with_its_reason() {
        let d = scratch("damaged");
        let t = Table::new();
        let sel = sim();
        let ctx = StationContext {
            selection: &sel,
            scan: &[],
        };
        assert!(read(&d, &ctx).unwrap().is_none(), "no file is no table");
        // A sensitivity changed after the hash was written.
        let mut v: Value = serde_json::from_slice(&contents(&t.table, 3, 1.0)).unwrap();
        v["table"]["rows"][0]["sensitivity_mv_per_eu"] = json!(10.0);
        std::fs::write(path(&d), serde_json::to_vec(&v).unwrap()).unwrap();
        let e = read(&d, &ctx).unwrap_err();
        assert!(e.contains("damaged"), "{e}");
        std::fs::write(path(&d), b"{ not json").unwrap();
        assert!(read(&d, &ctx).unwrap_err().contains("not readable JSON"));
        // A table the rule set refuses: two rows with one channel number.
        let mut two = t.table.clone();
        two.channels[1].index = two.channels[0].index;
        std::fs::write(path(&d), contents(&two, 3, 1.0)).unwrap();
        let e = read(&d, &ctx).unwrap_err();
        assert!(e.contains("refused") || e.contains("rule set"), "{e}");
    }
}
