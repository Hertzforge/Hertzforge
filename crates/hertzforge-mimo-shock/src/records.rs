//! The run's records: every pulse's drives and every input row, every law call
//! with its inputs and its answer, and the replay of the law calls (on MIMO
//! Random's pattern): the recorded law is built again from the recorded setup
//! and fed the recorded inputs in order, and each answer is compared with the
//! recorded one.
//!
//! A record is written to a folder ([`RunRecord::write_dir`]) as `setup.json`
//! (the law's setup and descriptor), `law_calls.jsonl` (one call a line) and
//! `pulses.jsonl` (one pulse a line). Numbers are written in their shortest
//! round-trip form, so a record read back ([`RunRecord::read_dir`]) is the
//! record written, bit for bit.

use crate::law::MimoShockLawFactory;
use crate::pulse::{PulseInputs, PulseOutputs};
use crate::setup::Setup;
use hertzforge_shock_common::law_guard::{LawCall, LawDescriptor, LawFault};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

/// The relative agreement a replay asks of every drive.
pub const REPLAY_TOLERANCE: f64 = 1e-12;

/// One law call as recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawCallEntry {
    /// The seed or an iteration.
    pub call: LawCall,
    /// The seed's level, dB (the seed takes no inputs).
    pub seed_level_db: Option<f64>,
    /// What an iteration was given.
    pub inputs: Option<PulseInputs>,
    /// What the law answered, when it answered.
    pub outputs: Option<PulseOutputs>,
    /// Why the answer was not used, when it was not.
    pub miss: Option<LawFault>,
    /// How long the call took, microseconds.
    pub call_us: u64,
}

/// One fired pulse as recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseEntry {
    /// From 0.
    pub index: u64,
    /// The level played, dB.
    pub level_db: f64,
    /// The drives as played, channel after channel, volts.
    pub drives: Vec<f64>,
    /// Quiet frames before the record in every input row.
    pub quiet_frames: usize,
    /// The table index of each input row.
    pub channels: Vec<u32>,
    /// Every input row, quiet lead-in and lead-out included.
    pub rows: Vec<Vec<f64>>,
}

/// A run's records.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RunRecord {
    /// The law's setup (after identification).
    pub setup: Option<Setup>,
    /// The law as its factory described it.
    pub law: Option<LawDescriptor>,
    pub law_calls: Vec<LawCallEntry>,
    pub pulses: Vec<PulseEntry>,
    /// Every step the overshoot guard halved.
    #[serde(default)]
    pub step_notes: Vec<crate::ladder::StepNote>,
}

#[derive(Serialize, Deserialize)]
struct Head {
    setup: Option<Setup>,
    law: Option<LawDescriptor>,
    #[serde(default)]
    step_notes: Vec<crate::ladder::StepNote>,
}

fn write_lines<T: Serialize>(path: &Path, items: &[T]) -> Result<(), String> {
    let mut f = std::io::BufWriter::new(fs::File::create(path).map_err(|e| e.to_string())?);
    for it in items {
        serde_json::to_writer(&mut f, it).map_err(|e| e.to_string())?;
        f.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    f.flush().map_err(|e| e.to_string())
}

fn read_lines<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>, String> {
    let f = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    BufReader::new(f)
        .lines()
        .map(|l| {
            let l = l.map_err(|e| e.to_string())?;
            serde_json::from_str(&l).map_err(|e| e.to_string())
        })
        .collect()
}

impl RunRecord {
    /// Write the record into `dir` (created when missing).
    pub fn write_dir(&self, dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let head = Head {
            setup: self.setup.clone(),
            law: self.law.clone(),
            step_notes: self.step_notes.clone(),
        };
        let s = serde_json::to_string(&head).map_err(|e| e.to_string())?;
        fs::write(dir.join("setup.json"), s).map_err(|e| e.to_string())?;
        write_lines(&dir.join("law_calls.jsonl"), &self.law_calls)?;
        write_lines(&dir.join("pulses.jsonl"), &self.pulses)
    }

    /// Read a record written by [`Self::write_dir`].
    pub fn read_dir(dir: &Path) -> Result<Self, String> {
        let s = fs::read_to_string(dir.join("setup.json")).map_err(|e| e.to_string())?;
        let head: Head = serde_json::from_str(&s).map_err(|e| e.to_string())?;
        Ok(RunRecord {
            setup: head.setup,
            law: head.law,
            step_notes: head.step_notes,
            law_calls: read_lines(&dir.join("law_calls.jsonl"))?,
            pulses: read_lines(&dir.join("pulses.jsonl"))?,
        })
    }
}

/// What a replay found.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayReport {
    /// Calls recorded.
    pub calls: usize,
    /// Calls whose drives were compared (both answered).
    pub compared: usize,
    /// The largest difference of any drive sample, relative to that call's
    /// largest recorded drive sample.
    pub worst_relative: f64,
    /// Every call that did not reproduce, in words.
    pub mismatches: Vec<String>,
}

impl ReplayReport {
    /// Every call reproduced within [`REPLAY_TOLERANCE`].
    pub fn reproduced(&self) -> bool {
        self.mismatches.is_empty() && self.worst_relative <= REPLAY_TOLERANCE
    }
}

/// Replay the law calls of `record` through a law built by `factory` from the
/// recorded setup. Nothing is driven: it is computation on recorded numbers.
pub fn replay(
    record: &RunRecord,
    factory: &dyn MimoShockLawFactory,
) -> Result<ReplayReport, String> {
    let setup = record.setup.as_ref().ok_or("the record holds no setup")?;
    let mut law = None;
    let mut rep = ReplayReport {
        calls: record.law_calls.len(),
        compared: 0,
        worst_relative: 0.0,
        mismatches: Vec::new(),
    };
    for (i, call) in record.law_calls.iter().enumerate() {
        let answer = match call.call {
            LawCall::Seed => {
                let mut l = factory.build(setup).map_err(|e| e.to_string())?;
                let level = call.seed_level_db.ok_or("a seed with no level")?;
                let a = l.seed(level);
                law = Some(l);
                a
            }
            LawCall::Iterate => {
                let l = law.as_mut().ok_or("an iteration before any seed")?;
                let inputs = call.inputs.as_ref().ok_or("an iteration with no inputs")?;
                l.iterate(inputs)
            }
        };
        match (&call.outputs, answer) {
            (Some(want), Ok(got)) => {
                rep.compared += 1;
                if want.drives.len() != got.drives.len() {
                    rep.mismatches
                        .push(format!("call {i}: the drive lengths differ"));
                    continue;
                }
                let scale = want.drives.iter().fold(0.0f64, |a, v| a.max(v.abs()));
                let diff = want
                    .drives
                    .iter()
                    .zip(&got.drives)
                    .fold(0.0f64, |a, (x, y)| a.max((x - y).abs()));
                let rel = if scale > 0.0 { diff / scale } else { diff };
                rep.worst_relative = rep.worst_relative.max(rel);
                if rel > REPLAY_TOLERANCE {
                    rep.mismatches
                        .push(format!("call {i}: drives differ by {rel:e} of their peak"));
                }
            }
            (None, Err(_)) => {}
            (Some(_), Err(e)) => rep.mismatches.push(format!(
                "call {i}: answered in the run, failed in the replay: {e}"
            )),
            (None, Ok(_)) => rep.mismatches.push(format!(
                "call {i}: failed in the run, answered in the replay"
            )),
        }
    }
    Ok(rep)
}
