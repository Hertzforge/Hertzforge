//! Replaying a recorded run's law (acceptance A11).
//!
//! A run with records on keeps every law call: the inputs the law declared, packed,
//! with the loop index and what the law returned (`records::LAW_CALLS`), and the law
//! itself (`records::LAW_RECORD`, with the law file copied beside it when the law is
//! one). [`replay`] builds the recorded law again, built in or through the law host,
//! feeds it the recorded inputs in the recorded order (a seed builds a fresh law, as
//! the run did) and compares each output with the recorded one, line by line.
//!
//! Nothing here drives anything or touches a station: it is computation on recorded
//! numbers.

use crate::records::{self, param_from_json, RecordedCall};
use hertzforge_law_host::Param;
use hertzforge_random_laws::{BuiltinMimo, LawBudgets, LawCatalog, LawChoice, MimoLawFactory};
use hertzforge_sdm::law::{DiagnosticData, MimoControlLaw, MimoLoopOutputs};
use hertzforge_sdm::HermitianStack;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The relative agreement A11 asks of a replay.
pub const REPLAY_TOLERANCE: f64 = 1e-12;

/// The time each replayed call may take (generous: a replay is not a run).
const REPLAY_BUDGET: Duration = Duration::from_secs(600);

/// How to replay.
#[derive(Debug, Clone, Default)]
pub struct ReplayOptions {
    /// A scratch folder for the law catalog a law file is loaded through (`None`:
    /// a folder under the system's temporary folder, removed after).
    pub scratch_root: Option<PathBuf>,
    /// Parameter values to use in place of the recorded ones (a check that the
    /// replay notices a changed law).
    pub params_override: Option<Vec<Param>>,
}

/// What a replay found.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayReport {
    /// Calls recorded.
    pub calls: usize,
    /// Calls whose outputs were compared (both returned a drive).
    pub compared: usize,
    /// Calls that missed in the run and missed again in the replay.
    pub missed_alike: usize,
    /// The largest relative difference of any line of any drive, or of any
    /// diagnostic value.
    pub worst_relative: f64,
    /// Every call that did not reproduce, in words.
    pub mismatches: Vec<String>,
    /// How the law ran: `built-in <id>` or `law file <sha256>`.
    pub law: String,
}

impl ReplayReport {
    /// Every call reproduced within [`REPLAY_TOLERANCE`].
    pub fn reproduced(&self) -> bool {
        self.mismatches.is_empty() && self.worst_relative <= REPLAY_TOLERANCE
    }
}

/// The relative Frobenius difference of one line, 0 when both are zero.
fn line_rel(a: &HermitianStack, b: &HermitianStack, line: usize) -> f64 {
    let (mut num, mut na, mut nb) = (0.0, 0.0, 0.0);
    for (x, y) in a.line(line).iter().zip(b.line(line)) {
        num += (x - y).norm_sqr();
        na += x.norm_sqr();
        nb += y.norm_sqr();
    }
    let den = f64::max(na, nb);
    if den == 0.0 {
        0.0
    } else {
        (num / den).sqrt()
    }
}

fn values(d: &DiagnosticData) -> Vec<f64> {
    match d {
        DiagnosticData::Scalar(x) => vec![*x],
        DiagnosticData::PerLine(v)
        | DiagnosticData::PerControlLine(v)
        | DiagnosticData::PerDriveLine(v) => v.clone(),
    }
}

/// The largest relative difference between two outputs, or why they differ in
/// shape.
fn compare(a: &MimoLoopOutputs, b: &MimoLoopOutputs) -> Result<f64, String> {
    if a.drive.lines() != b.drive.lines() || a.drive.n() != b.drive.n() {
        return Err(format!(
            "the drive is {} by {} on {} lines, recorded {} by {} on {}",
            b.drive.n(),
            b.drive.n(),
            b.drive.lines(),
            a.drive.n(),
            a.drive.n(),
            a.drive.lines()
        ));
    }
    let mut worst = 0.0f64;
    for line in 0..a.drive.lines() {
        worst = worst.max(line_rel(&a.drive, &b.drive, line));
    }
    if a.diagnostics.len() != b.diagnostics.len() {
        return Err("the law reported a different set of diagnostics".into());
    }
    for (x, y) in a.diagnostics.iter().zip(&b.diagnostics) {
        if x.key != y.key {
            return Err(format!("diagnostic `{}` came back as `{}`", x.key, y.key));
        }
        let (vx, vy) = (values(&x.data), values(&y.data));
        if vx.len() != vy.len() {
            return Err(format!("diagnostic `{}` changed length", x.key));
        }
        for (p, q) in vx.iter().zip(&vy) {
            if p.is_nan() && q.is_nan() {
                continue;
            }
            let den = p.abs().max(q.abs());
            if den > 0.0 {
                worst = worst.max((p - q).abs() / den);
            }
        }
    }
    Ok(worst)
}

/// Builds the recorded law: a built-in by id, or the run's law file through a law
/// catalog in a scratch folder.
struct LawSource {
    record: records::LawRecord,
    run_dir: PathBuf,
    catalog: Option<LawCatalog>,
    scratch: Option<PathBuf>,
    remove_scratch: bool,
}

impl LawSource {
    fn factory(
        &mut self,
        id: &str,
        params: Vec<Param>,
        controls: usize,
    ) -> Result<Arc<dyn MimoLawFactory>, String> {
        let choice = LawChoice::new(id, params);
        if BuiltinMimo::from_id(id).is_some() {
            return crate::law::builtin_factory(&choice, controls).map_err(|e| e.to_string());
        }
        if self.catalog.is_none() {
            let file = self.record.file.as_ref().ok_or_else(|| {
                format!(
                    "the law `{id}` is a law file and the run did not keep a copy of it; \
                     replay needs the law file"
                )
            })?;
            let root = match &self.scratch {
                Some(r) => r.clone(),
                None => {
                    let r = std::env::temp_dir().join(format!(
                        "hertzforge-replay-{}-{}",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos())
                            .unwrap_or(0)
                    ));
                    self.remove_scratch = true;
                    self.scratch = Some(r.clone());
                    r
                }
            };
            let mut cat = LawCatalog::open(&root).map_err(|e| e.to_string())?;
            cat.import(&self.run_dir.join(file))
                .map_err(|e| format!("the run's law file did not load: {e}"))?;
            self.catalog = Some(cat);
        }
        let cat = self.catalog.as_ref().ok_or("no law catalog")?;
        cat.resolve_mimo(&choice, controls)
            .map_err(|e| e.to_string())
    }
}

impl Drop for LawSource {
    fn drop(&mut self) {
        if self.remove_scratch {
            if let Some(r) = &self.scratch {
                let _ = std::fs::remove_dir_all(r);
            }
        }
    }
}

fn call(
    law: &mut (dyn MimoControlLaw + Send),
    c: &RecordedCall,
) -> Result<MimoLoopOutputs, String> {
    let inputs = c.inputs.as_inputs();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if c.seed {
            law.seed(&inputs)
        } else {
            law.iterate(&inputs)
        }
    })) {
        Ok(Ok(o)) => Ok(o),
        Ok(Err(e)) => Err(e.0),
        Err(_) => Err("the law failed while computing".into()),
    }
}

/// Replay the run recorded in `dir`.
pub fn replay(dir: &Path, opts: &ReplayOptions) -> Result<ReplayReport, String> {
    let record = records::read_law_record(dir)?;
    let calls = records::read_law_calls(dir)?;
    if calls.is_empty() {
        return Err("the run recorded no law calls; it ended before the law was seeded".into());
    }
    let mut source = LawSource {
        record: record.clone(),
        run_dir: dir.to_path_buf(),
        catalog: None,
        scratch: opts.scratch_root.clone(),
        remove_scratch: false,
    };
    let mut report = ReplayReport {
        calls: calls.len(),
        compared: 0,
        missed_alike: 0,
        worst_relative: 0.0,
        mismatches: Vec::new(),
        law: if record.builtin {
            format!("built-in {}", record.id)
        } else {
            format!("law file {}", record.sha256)
        },
    };
    let mut law: Option<Box<dyn MimoControlLaw + Send>> = None;
    for c in &calls {
        if c.seed {
            let (id, params) = c
                .law
                .clone()
                .unwrap_or_else(|| (record.id.clone(), record.params.clone()));
            let params: Vec<Param> = match &opts.params_override {
                Some(p) => p.clone(),
                None => params
                    .iter()
                    .map(param_from_json)
                    .collect::<Result<_, _>>()?,
            };
            let setup = c.inputs.setup();
            let f = source.factory(&id, params, setup.controls)?;
            law = Some(
                f.build(&setup, LawBudgets::same(REPLAY_BUDGET))
                    .map_err(|e| format!("the recorded law did not build: {e}"))?,
            );
        }
        let Some(l) = law.as_mut() else {
            return Err(format!(
                "call {} iterates a law that was never seeded in the record",
                c.seq
            ));
        };
        match (&c.outcome, call(l.as_mut(), c)) {
            (Ok(want), Ok(got)) => match compare(want, &got) {
                Ok(rel) => {
                    report.compared += 1;
                    report.worst_relative = report.worst_relative.max(rel);
                    if rel > REPLAY_TOLERANCE {
                        report.mismatches.push(format!(
                            "call {} (loop {}): differs by {rel:.3e} relative",
                            c.seq, c.loop_index
                        ));
                    }
                }
                Err(e) => report
                    .mismatches
                    .push(format!("call {} (loop {}): {e}", c.seq, c.loop_index)),
            },
            (Err(_), Err(_)) => report.missed_alike += 1,
            (Ok(_), Err(e)) => report.mismatches.push(format!(
                "call {} (loop {}): the run's law answered and the replay's did not ({e})",
                c.seq, c.loop_index
            )),
            (Err(w), Ok(_)) => report.mismatches.push(format!(
                "call {} (loop {}): the run's law missed ({w}) and the replay's answered",
                c.seq, c.loop_index
            )),
        }
    }
    Ok(report)
}
