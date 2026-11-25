//! The MIMO Sine benchmark's run program: one case of the Open Benchmark Set (a structure
//! of the MIMO Random suite, version 0.2, with its level rule; one built-in MIMO Sine law;
//! one sweep, stepped sine or dwell) run through the shipped MIMO Sine engine on the
//! simulator, and one slim run folder for the suite's scorer.
//!
//! ```text
//! cargo build --release -p sds-control --example msnb_run
//! msnb_run <case.json> --root <problem-root> --out <run-dir> [--ideal-frf] [--record]
//! ```
//!
//! The case (`msnb-case/1`), its reference (`msnb-reference/1`) and the run folder
//! (`msnb-run/1`) are described in `FORMAT_RUN.md` beside this file, the case also by
//! `msnb-case-1.schema.json`.
//!
//! The run is a user's run on the simulator: `MimoSineService` arms the station, runs its
//! own identification at its defaults, builds the law, and runs the loop with its own
//! guards, ladder and records. Nothing of the engine is re-implemented here. What the
//! program adds:
//!
//! - the structure, built by the open-loop player's own loader (`common/obs_structure.rs`)
//!   from the keys `to_runner_structure` of the benchmark set writes, with the structure's
//!   own step, and only the outputs the run uses (the controls and limit channels);
//! - a tap on the station (`Edge`) that converts the station's samples from m/s^2 to g
//!   (the one place samples are converted; see `units`), and that, for the run's record,
//!   feeds a second, drive-only simulator the same calls so the exact drive the structure
//!   was played is known, steps a copy of the in-band modes on it to read the level rule's
//!   frequencies, and with `--record` keeps every sample;
//! - with `--ideal-frf`, a wrapper on the law's factory that hands the law the structure's
//!   small-signal frequency response in place of the identification.
//!
//! The one setting that differs from a user's simulator run: every law call is held to
//! `hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET` in place of the setup's budget, as the
//! law check's deterministic harness does, so the same case gives the same bytes on a busy
//! machine. The call times are measured and written to `timing.json`.
//!
//! Exit status: 0 completed; 1 ended otherwise (folder written); 2 the case was refused
//! (nothing written); 3 the engine refused the setup or the law, or the identification's
//! gate held the run (folder written).
#![recursion_limit = "512"]

#[allow(dead_code)]
#[path = "../common/obs_structure.rs"]
mod obs_structure;

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{
    Discretization, JointedMode, JointedPlant, JointedStep, ModalPlant, Mode, ModeResponse,
    PlantModel, SimBackend, SimConfig, Stepper,
};
use hertzforge_mimo_sine::law::{LawDescriptor, LawIdentification, LawManifest, LawSetup};
use hertzforge_mimo_sine::records::{Event, EventKind, Outcome, TickRecord};
use hertzforge_mimo_sine::service::{sweep_warning, EngineOptions, MimoSineService, ServiceState};
use hertzforge_mimo_sine::{
    ControlChannel, DriveChannel, GuardSettings, IdentificationSettings, LawChoice, LawFault,
    LevelMode, LimitChannel, MimoSineLaw, MimoSineLawFactory, MimoSineSetup, PhasePoint, Schedule,
    StepPlan, TestPlan,
};
use hertzforge_module_abi::StationClass;
use hertzforge_sdm::{MatrixStack, C64};
use hertzforge_sine::profile::{ProfileRow, SineProfile};
use hertzforge_sine::scheduler::{
    BidirSweepConfig, DwellFor, DwellPlan, LadderConfig, RungFor, StepSineConfig, StepSpacing,
    SweepDirection, SweepTarget,
};
use hertzforge_sine::SweepRate;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const USAGE: &str = "usage: msnb_run <case.json> --root <problem-root> --out <run-dir> [--ideal-frf] [--record] [--no-true-modes]";

/// The code this program was built from: `HF_BUILD_COMMIT` at build time.
const COMMIT: &str = match option_env!("HF_BUILD_COMMIT") {
    Some(c) => c,
    None => "unstated build",
};

/// The control service's own identification seed (`mimo_sine::run::SYSID_SEED`).
const SERVICE_SYSID_SEED: u64 = 0x5EED_0003;

/// Events of one kind kept in the run file before they are only counted.
const EVENTS_KEPT_PER_KIND: usize = 200;

/// Points per octave of a sweep's history.
const BINS_PER_OCTAVE: f64 = 48.0;

/// The one place values change units.
mod units {
    /// Standard gravity, m/s^2 per g.
    pub const G0: f64 = 9.80665;
    /// A case value (m/s^2) as the engine takes it (g).
    pub fn to_engine(m_s2: f64) -> f64 {
        m_s2 / G0
    }
    /// An engine value (g) as the run file gives it (m/s^2).
    pub fn from_engine(g: f64) -> f64 {
        g * G0
    }
    /// A station sample (m/s^2) as the engine reads it (g).
    pub fn sample_to_engine(m_s2: f32) -> f32 {
        (m_s2 as f64 / G0) as f32
    }
}

fn refuse(msg: impl std::fmt::Display) -> ! {
    eprintln!("msnb_run: {msg}");
    std::process::exit(2);
}

// ---------------------------------------------------------------------------------------
// Strict JSON reading.
// ---------------------------------------------------------------------------------------

type Obj = Map<String, Value>;

fn obj<'a>(v: &'a Value, what: &str) -> Result<&'a Obj, String> {
    v.as_object().ok_or_else(|| format!("{what} must be an object"))
}
fn only(o: &Obj, allowed: &[&str], what: &str) -> Result<(), String> {
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(format!("{what}: unknown key \"{k}\""));
        }
    }
    Ok(())
}
fn present<'a>(o: &'a Obj, k: &str) -> Option<&'a Value> {
    match o.get(k) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v),
    }
}
fn opt_f64(o: &Obj, k: &str, what: &str) -> Result<Option<f64>, String> {
    match present(o, k) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite())
            .map(Some)
            .ok_or_else(|| format!("{what}.{k} must be a finite number")),
    }
}
fn req_f64(o: &Obj, k: &str, what: &str) -> Result<f64, String> {
    opt_f64(o, k, what)?.ok_or_else(|| format!("{what}.{k} is required"))
}
fn pos(x: f64, name: &str) -> Result<f64, String> {
    if x > 0.0 {
        Ok(x)
    } else {
        Err(format!("{name} must be above 0"))
    }
}
fn opt_u64(o: &Obj, k: &str, what: &str) -> Result<Option<u64>, String> {
    match present(o, k) {
        None => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{what}.{k} must be a whole number of at least 0")),
    }
}
fn opt_str<'a>(o: &'a Obj, k: &str, what: &str) -> Result<Option<&'a str>, String> {
    match present(o, k) {
        None => Ok(None),
        Some(v) => v
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("{what}.{k} must be a string")),
    }
}
fn f64_list(v: &Value, what: &str) -> Result<Vec<f64>, String> {
    v.as_array()
        .ok_or_else(|| format!("{what} must be a list"))?
        .iter()
        .map(|x| {
            x.as_f64()
                .filter(|x| x.is_finite())
                .ok_or_else(|| format!("{what} holds a value that is not a finite number"))
        })
        .collect()
}
fn str_list(v: &Value, what: &str) -> Result<Vec<String>, String> {
    v.as_array()
        .ok_or_else(|| format!("{what} must be a list"))?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{what} holds a value that is not a string"))
        })
        .collect()
}
fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}
/// A path of the case, relative to the root: no absolute path, no parent step.
fn relpath(s: &str, what: &str) -> Result<PathBuf, String> {
    let p = Path::new(s);
    if s.is_empty()
        || p.is_absolute()
        || s.starts_with('~')
        || p.components().any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "{what} must be a path relative to the root, with no \"..\" (got \"{s}\")"
        ));
    }
    Ok(p.to_path_buf())
}

// ---------------------------------------------------------------------------------------
// The case.
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum TestSpec {
    Sweep {
        band: (f64, f64),
        rate: f64,
        down: bool,
    },
    Stepped {
        band: (f64, f64),
        per_octave: f64,
        step_s: f64,
    },
    Dwell {
        freq_hz: f64,
        dwell_s: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PhaseMode {
    Specified,
    AmplitudeOnly,
    Auto,
}

#[derive(Debug, Clone, PartialEq)]
struct ReferenceTable {
    controls: Vec<String>,
    freqs: Vec<f64>,
    /// `(lines, controls)`, m/s^2 peak.
    amp: Vec<Vec<f64>>,
    /// `(lines, controls)`, degrees.
    phase: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, PartialEq)]
struct LimitSpec {
    output: String,
    freqs: Vec<f64>,
    amp: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
struct Case {
    case_id: String,
    structure_file: PathBuf,
    structure_sha256: Option<String>,
    controls: Option<Vec<String>>,
    fs_hz: f64,
    tick_s: f64,
    law_id: String,
    law_params: Obj,
    test: TestSpec,
    reference_file: Option<PathBuf>,
    reference_sha256: Option<String>,
    reference_inline: Option<Value>,
    phase: PhaseMode,
    scale: f64,
    levels_db: Vec<f64>,
    rung: RungFor,
    ceiling: Vec<f64>,
    abort: Vec<f64>,
    /// The amplifier's gain: structure input (V) per engine volt. Applied at the
    /// station's edge only; the engine, its law and its ceilings work in engine volts.
    gain: f64,
    noise_rms_m_s2: f64,
    noise_seed: u64,
    ident_seed: u64,
    limits: Vec<LimitSpec>,
    widened: bool,
    max_test_s: Option<f64>,
}

fn volts(v: Option<&Value>, default: f64, what: &str) -> Result<Vec<f64>, String> {
    match v {
        None | Some(Value::Null) => Ok(vec![default]),
        Some(Value::Array(_)) => {
            let l = f64_list(v.unwrap(), what)?;
            if l.is_empty() {
                return Err(format!("{what} is an empty list"));
            }
            Ok(l)
        }
        Some(x) => x
            .as_f64()
            .filter(|x| x.is_finite())
            .map(|x| vec![x])
            .ok_or_else(|| format!("{what} must be a number or a list of numbers")),
    }
}

fn hex64(s: Option<&str>, what: &str) -> Result<Option<String>, String> {
    match s {
        None => Ok(None),
        Some(h) if h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()) => {
            Ok(Some(h.to_ascii_lowercase()))
        }
        Some(_) => Err(format!("{what} must be 64 hexadecimal digits")),
    }
}

/// The control service's default time on each level but the last: one sweep for a sweep,
/// 10 s otherwise (`mimo_sine::config`).
fn default_rung(t: &TestSpec) -> RungFor {
    match t {
        TestSpec::Sweep { .. } => RungFor::Sweeps(1),
        _ => RungFor::Seconds(sds_control::mimo_sine::config::DEFAULT_PER_LEVEL_S),
    }
}

fn parse_case(v: &Value) -> Result<Case, String> {
    let c = obj(v, "the case")?;
    only(
        c,
        &[
            "schema",
            "case_id",
            "structure",
            "controls",
            "fs_hz",
            "tick_s",
            "law",
            "test",
            "reference",
            "full_level_scale",
            "ladder",
            "drives",
            "noise",
            "identification_seed",
            "limits",
            "protective",
            "max_test_s",
        ],
        "the case",
    )?;
    if c.get("schema").and_then(Value::as_str) != Some("msnb-case/1") {
        return Err("schema must be \"msnb-case/1\"".into());
    }
    let case_id = opt_str(c, "case_id", "the case")?
        .filter(|s| !s.is_empty())
        .ok_or("case_id is required")?
        .to_owned();
    let st = obj(c.get("structure").ok_or("structure is required")?, "structure")?;
    only(st, &["file", "sha256"], "structure")?;
    let structure_file = relpath(
        opt_str(st, "file", "structure")?.ok_or("structure.file is required")?,
        "structure.file",
    )?;
    let structure_sha256 = hex64(opt_str(st, "sha256", "structure")?, "structure.sha256")?;
    let controls = match present(c, "controls") {
        None => None,
        Some(v) => {
            let l = str_list(v, "controls")?;
            if l.is_empty() {
                return Err("controls is an empty list".into());
            }
            Some(l)
        }
    };
    let fs_hz = opt_f64(c, "fs_hz", "the case")?.unwrap_or(8192.0);
    let tick_s = opt_f64(c, "tick_s", "the case")?
        .unwrap_or(hertzforge_mimo_sine::limits::DEFAULT_TICK_S);
    let law = obj(c.get("law").ok_or("law is required")?, "law")?;
    only(law, &["id", "params"], "law")?;
    let law_id = opt_str(law, "id", "law")?.ok_or("law.id is required")?.to_owned();
    let law_params = match present(law, "params") {
        None => Map::new(),
        Some(p) => obj(p, "law.params")?.clone(),
    };
    let t = obj(c.get("test").ok_or("test is required")?, "test")?;
    let band = |t: &Obj| -> Result<(f64, f64), String> {
        let b = f64_list(t.get("band_hz").ok_or("test.band_hz is required")?, "test.band_hz")?;
        if b.len() != 2 || !(b[0] > 0.0 && b[1] > b[0]) {
            return Err("test.band_hz must be [lo, hi] with 0 < lo < hi".into());
        }
        Ok((b[0], b[1]))
    };
    let test = match opt_str(t, "type", "test")? {
        Some("sweep") => {
            only(t, &["type", "band_hz", "rate_oct_per_min", "direction"], "test")?;
            let down = match opt_str(t, "direction", "test")? {
                None | Some("up") => false,
                Some("down") => true,
                Some(o) => return Err(format!("test.direction \"{o}\" is not up or down")),
            };
            TestSpec::Sweep {
                band: band(t)?,
                rate: pos(req_f64(t, "rate_oct_per_min", "test")?, "test.rate_oct_per_min")?,
                down,
            }
        }
        Some("stepped") => {
            only(t, &["type", "band_hz", "steps_per_octave", "step_s"], "test")?;
            TestSpec::Stepped {
                band: band(t)?,
                per_octave: pos(
                    req_f64(t, "steps_per_octave", "test")?,
                    "test.steps_per_octave",
                )?,
                step_s: pos(req_f64(t, "step_s", "test")?, "test.step_s")?,
            }
        }
        Some("dwell") => {
            only(t, &["type", "freq_hz", "dwell_s"], "test")?;
            TestSpec::Dwell {
                freq_hz: pos(req_f64(t, "freq_hz", "test")?, "test.freq_hz")?,
                dwell_s: pos(req_f64(t, "dwell_s", "test")?, "test.dwell_s")?,
            }
        }
        _ => return Err("test.type must be \"sweep\", \"stepped\" or \"dwell\"".into()),
    };
    let r = obj(c.get("reference").ok_or("reference is required")?, "reference")?;
    only(r, &["file", "sha256", "inline", "phase"], "reference")?;
    let reference_file = match opt_str(r, "file", "reference")? {
        Some(f) => Some(relpath(f, "reference.file")?),
        None => None,
    };
    let reference_inline = present(r, "inline").cloned();
    if reference_file.is_some() == reference_inline.is_some() {
        return Err("reference gives one of \"file\" and \"inline\"".into());
    }
    let reference_sha256 = hex64(opt_str(r, "sha256", "reference")?, "reference.sha256")?;
    let phase = match opt_str(r, "phase", "reference")? {
        None | Some("auto") => PhaseMode::Auto,
        Some("specified") => PhaseMode::Specified,
        Some("amplitude_only") => PhaseMode::AmplitudeOnly,
        Some(o) => return Err(format!("reference.phase \"{o}\" is not known")),
    };
    let scale = pos(
        opt_f64(c, "full_level_scale", "the case")?.unwrap_or(1.0),
        "full_level_scale",
    )?;
    let (levels_db, rung) = match present(c, "ladder") {
        None => (vec![0.0], default_rung(&test)),
        Some(l) => {
            let l = obj(l, "ladder")?;
            only(l, &["levels_db", "rung"], "ladder")?;
            let levels = match present(l, "levels_db") {
                None => vec![0.0],
                Some(v) => f64_list(v, "ladder.levels_db")?,
            };
            let rung = match present(l, "rung") {
                None => default_rung(&test),
                Some(r) => {
                    let r = obj(r, "ladder.rung")?;
                    match (present(r, "seconds"), present(r, "sweeps")) {
                        (Some(_), None) => {
                            only(r, &["seconds"], "ladder.rung")?;
                            RungFor::Seconds(pos(
                                req_f64(r, "seconds", "ladder.rung")?,
                                "ladder.rung.seconds",
                            )?)
                        }
                        (None, Some(_)) => {
                            only(r, &["sweeps"], "ladder.rung")?;
                            let n = opt_u64(r, "sweeps", "ladder.rung")?.unwrap_or(0);
                            if n == 0 || n > u32::MAX as u64 {
                                return Err("ladder.rung.sweeps must be at least 1".into());
                            }
                            RungFor::Sweeps(n as u32)
                        }
                        _ => return Err("ladder.rung gives one of \"seconds\" and \"sweeps\"".into()),
                    }
                }
            };
            (levels, rung)
        }
    };
    if levels_db.is_empty() {
        return Err("ladder.levels_db is an empty list".into());
    }
    let (ceiling, abort, gain) = match present(c, "drives") {
        None => (vec![9.0], vec![10.0], 1.0),
        Some(d) => {
            let d = obj(d, "drives")?;
            only(d, &["ceiling_vpk", "abort_vpk", "gain"], "drives")?;
            let gain = match opt_f64(d, "gain", "drives")? {
                None => 1.0,
                Some(g) => pos(g, "drives.gain")?,
            };
            (
                volts(d.get("ceiling_vpk"), 9.0, "drives.ceiling_vpk")?,
                volts(d.get("abort_vpk"), 10.0, "drives.abort_vpk")?,
                gain,
            )
        }
    };
    let (noise_rms_m_s2, noise_seed) = match present(c, "noise") {
        None => (0.0, 1),
        Some(n) => {
            let n = obj(n, "noise")?;
            only(n, &["rms_m_s2", "seed"], "noise")?;
            let rms = opt_f64(n, "rms_m_s2", "noise")?.unwrap_or(0.0);
            if rms < 0.0 {
                return Err("noise.rms_m_s2 must be at least 0".into());
            }
            (rms, opt_u64(n, "seed", "noise")?.unwrap_or(1))
        }
    };
    let ident_seed = opt_u64(c, "identification_seed", "the case")?.unwrap_or(SERVICE_SYSID_SEED);
    let mut limits = Vec::new();
    if let Some(l) = present(c, "limits") {
        for (k, e) in l
            .as_array()
            .ok_or("limits must be a list")?
            .iter()
            .enumerate()
        {
            let what = format!("limits[{k}]");
            let e = obj(e, &what)?;
            only(e, &["output", "bound"], &what)?;
            let output = opt_str(e, "output", &what)?
                .ok_or_else(|| format!("{what}.output is required"))?
                .to_owned();
            let b = obj(
                e.get("bound").ok_or_else(|| format!("{what}.bound is required"))?,
                &format!("{what}.bound"),
            )?;
            only(b, &["frequency_hz", "amplitude_m_s2_pk"], &format!("{what}.bound"))?;
            let freqs = f64_list(
                b.get("frequency_hz")
                    .ok_or_else(|| format!("{what}.bound.frequency_hz is required"))?,
                &format!("{what}.bound.frequency_hz"),
            )?;
            let amp = f64_list(
                b.get("amplitude_m_s2_pk")
                    .ok_or_else(|| format!("{what}.bound.amplitude_m_s2_pk is required"))?,
                &format!("{what}.bound.amplitude_m_s2_pk"),
            )?;
            if freqs.len() < 2 || freqs.len() != amp.len() {
                return Err(format!(
                    "{what}.bound needs at least two frequencies and one amplitude for each"
                ));
            }
            if freqs.windows(2).any(|w| !(w[1] > w[0])) || freqs[0] <= 0.0 {
                return Err(format!("{what}.bound.frequency_hz must be above 0 and ascending"));
            }
            if amp.iter().any(|a| *a <= 0.0) {
                return Err(format!("{what}.bound.amplitude_m_s2_pk must be above 0"));
            }
            limits.push(LimitSpec { output, freqs, amp });
        }
    }
    let widened = match opt_str(c, "protective", "the case")? {
        None | Some("default") => false,
        Some("widened") => true,
        Some(o) => return Err(format!("protective \"{o}\" is not default or widened")),
    };
    let max_test_s = match opt_f64(c, "max_test_s", "the case")? {
        Some(x) => Some(pos(x, "max_test_s")?),
        None => None,
    };
    Ok(Case {
        case_id,
        structure_file,
        structure_sha256,
        controls,
        fs_hz,
        tick_s,
        law_id,
        law_params,
        test,
        reference_file,
        reference_sha256,
        reference_inline,
        phase,
        scale,
        levels_db,
        rung,
        ceiling,
        abort,
        gain,
        noise_rms_m_s2,
        noise_seed,
        ident_seed,
        limits,
        widened,
        max_test_s,
    })
}

fn parse_reference(v: &Value) -> Result<ReferenceTable, String> {
    let r = obj(v, "the reference")?;
    only(
        r,
        &["schema", "controls", "frequency_hz", "amplitude_m_s2_pk", "phase_deg"],
        "the reference",
    )?;
    if r.get("schema").and_then(Value::as_str) != Some("msnb-reference/1") {
        return Err("the reference's schema must be \"msnb-reference/1\"".into());
    }
    let controls = str_list(r.get("controls").ok_or("reference controls is required")?, "reference controls")?;
    let freqs = f64_list(
        r.get("frequency_hz").ok_or("reference frequency_hz is required")?,
        "reference frequency_hz",
    )?;
    if freqs.len() < 2 || freqs[0] <= 0.0 || freqs.windows(2).any(|w| !(w[1] > w[0])) {
        return Err("reference frequency_hz needs at least two frequencies, above 0 and ascending".into());
    }
    let rows = |k: &str| -> Result<Vec<Vec<f64>>, String> {
        let l = r
            .get(k)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("reference {k} must be a list of lists"))?;
        if l.len() != freqs.len() {
            return Err(format!("reference {k} has {} rows for {} frequencies", l.len(), freqs.len()));
        }
        l.iter()
            .map(|row| {
                let row = f64_list(row, &format!("reference {k}"))?;
                if row.len() != controls.len() {
                    return Err(format!("reference {k}: a row has {} values for {} controls", row.len(), controls.len()));
                }
                Ok(row)
            })
            .collect()
    };
    let amp = rows("amplitude_m_s2_pk")?;
    let phase = rows("phase_deg")?;
    if amp.iter().flatten().any(|a| *a <= 0.0) {
        return Err("reference amplitude_m_s2_pk must be above 0 everywhere".into());
    }
    if phase.iter().flatten().any(|p| !(*p > -180.0 && *p <= 180.0)) {
        return Err("reference phase_deg must be above -180 and at most 180".into());
    }
    Ok(ReferenceTable {
        controls,
        freqs,
        amp,
        phase,
    })
}

/// One control's phase points in the engine's form: the engine interpolates linearly in
/// phase against log frequency with no unwrapping, so a segment whose ends differ by more
/// than 180 degrees is split where its short way crosses +/-180 degrees. Returns the
/// points and how many splits were made.
fn phase_points(freqs: &[f64], deg: &[f64]) -> Result<(Vec<PhasePoint>, usize), String> {
    const GAP: f64 = 1e-9;
    let mut out = Vec::with_capacity(freqs.len());
    let mut splits = 0;
    for k in 0..freqs.len() {
        if k > 0 {
            let (fa, fb, a, b) = (freqs[k - 1], freqs[k], deg[k - 1], deg[k]);
            if (b - a).abs() > 180.0 {
                let up = b < a; // the short way from a goes up through +180 to b + 360
                let bu = if up { b + 360.0 } else { b - 360.0 };
                let cross = if up { 180.0 } else { -180.0 };
                let t = (cross - a) / (bu - a);
                let lo = fa * (1.0 + 2.0 * GAP);
                let hi = fb / (1.0 + 2.0 * GAP);
                if !(hi > lo) {
                    return Err(format!(
                        "two reference frequencies ({fa} and {fb} Hz) are too close to split a phase wrap between"
                    ));
                }
                let fc = (fa.ln() + t * (fb.ln() - fa.ln())).exp().clamp(lo, hi);
                let below = -180.0 + GAP;
                let (before, after) = if up { (180.0, below) } else { (below, 180.0) };
                out.push(PhasePoint {
                    freq_hz: fc,
                    phase_deg: before,
                });
                out.push(PhasePoint {
                    freq_hz: fc * (1.0 + GAP),
                    phase_deg: after,
                });
                splits += 1;
            }
        }
        out.push(PhasePoint {
            freq_hz: freqs[k],
            phase_deg: deg[k],
        });
    }
    Ok((out, splits))
}

/// A profile of acceleration rows (g peak) with symmetric bands.
fn profile(name: &str, freqs: &[f64], g_pk: &[f64], alarm_db: f64, abort_db: f64) -> SineProfile {
    let mut p = SineProfile::new(name);
    p.global_alarm_db = (alarm_db, alarm_db);
    p.global_abort_db = (abort_db, abort_db);
    for (&f, &a) in freqs.iter().zip(g_pk) {
        p.push(ProfileRow::accel(f, a));
    }
    p
}

// ---------------------------------------------------------------------------------------
// The structure.
// ---------------------------------------------------------------------------------------

/// What the run takes from `structure.json`.
struct Structure {
    /// `jointed` (the level-rule tiers) or `modal` (the linear tier).
    kind: String,
    raw: Value,
    /// Output names, all of them, and their roles.
    names: Vec<String>,
    roles: Vec<String>,
    drive_names: Vec<String>,
    n_modes: usize,
}

fn load_structure(bytes: &[u8]) -> Result<Structure, String> {
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("structure.json: {e}"))?;
    let o = obj(&raw, "structure.json")?;
    let kind = match o.get("kind").and_then(Value::as_str) {
        Some(k @ ("jointed" | "modal")) => k.to_owned(),
        _ => return Err("structure.json: the kind is not jointed or modal".into()),
    };
    let outs = o
        .get("outputs")
        .and_then(Value::as_array)
        .ok_or("structure.json: no outputs")?;
    let names = outs
        .iter()
        .map(|x| x.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or("structure.json: an output has no name")?;
    let roles = outs
        .iter()
        .map(|x| x.get("role").and_then(Value::as_str).unwrap_or("").to_owned())
        .collect();
    let drive_names = o
        .get("drives")
        .and_then(Value::as_array)
        .ok_or("structure.json: no drives")?
        .iter()
        .map(|x| x.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or("structure.json: a drive has no name")?;
    let n_modes = o
        .get("modes")
        .and_then(Value::as_array)
        .ok_or("structure.json: no modes")?
        .len();
    Ok(Structure {
        kind,
        raw,
        names,
        roles,
        drive_names,
        n_modes,
    })
}

/// The structure as the simulator runs it, in the player's keys (case schema
/// `mm-x4-case/2`), with only the outputs `rows` (in that order): the benchmark set's
/// `to_runner_structure`, with the structure's own step (`simulator.step`) carried.
fn runner_structure(st: &Structure, rows: &[usize]) -> Result<Value, String> {
    let o = st.raw.as_object().expect("checked at load");
    let sim = o
        .get("simulator")
        .and_then(Value::as_object)
        .ok_or("structure.json: no simulator block")?;
    let pick = |v: &Value, what: &str| -> Result<Vec<Value>, String> {
        let a = v.as_array().ok_or_else(|| format!("{what} is not a list"))?;
        rows.iter()
            .map(|&i| a.get(i).cloned().ok_or_else(|| format!("{what} is too short")))
            .collect()
    };
    let n_d = o.get("n_drives").and_then(Value::as_u64).ok_or("structure.json: no n_drives")? as usize;
    if st.kind == "modal" {
        // The linear tier: `to_runner_structure`'s modal object (mm-x4-case/1).
        if present(sim, "softening").is_some() {
            return Err("structure.json: a modal structure with softening is not run here".into());
        }
        let mut modes = Vec::new();
        for (k, m) in o["modes"].as_array().expect("checked at load").iter().enumerate() {
            let zeta = m
                .get("zeta")
                .and_then(Value::as_f64)
                .filter(|z| *z > 0.0)
                .ok_or_else(|| format!("mode {k} has no damping ratio above 0"))?;
            modes.push(json!({
                "f_hz": m.get("f_hz").cloned().ok_or_else(|| format!("mode {k} has no f_hz"))?,
                "q": 1.0 / (2.0 * zeta),
                "drive_shape": m.get("drive_shape").cloned().ok_or_else(|| format!("mode {k} has no drive_shape"))?,
                "output_shape": Value::Array(pick(
                    m.get("output_shape").ok_or_else(|| format!("mode {k} has no output_shape"))?,
                    &format!("mode {k} output_shape"),
                )?),
                "response": "acceleration",
            }));
        }
        let residual = match sim.get("residual").and_then(Value::as_array) {
            Some(r) if !r.is_empty() => {
                let mut out = Vec::new();
                for &i in rows {
                    for j in 0..n_d {
                        out.push(r.get(i * n_d + j).cloned().ok_or("simulator.residual is too short")?);
                    }
                }
                out
            }
            _ => Vec::new(),
        };
        return Ok(json!({
            "kind": "modal", "n_outputs": rows.len(), "n_drives": n_d,
            "feedthrough": sim.get("feedthrough").cloned().unwrap_or(json!(0.0)),
            "residual": residual,
            "transport_delay_samples": sim.get("transport_delay_samples").cloned().unwrap_or(json!(0)),
            "modes": modes,
            "discretization": sim.get("discretization").cloned().unwrap_or(Value::Null),
        }));
    }
    let mut modes = Vec::new();
    for (k, m) in o["modes"].as_array().expect("checked at load").iter().enumerate() {
        let m = obj(m, &format!("mode {k}"))?;
        let mut mode = Map::new();
        for key in ["f_hz", "zeta", "drive_shape", "reference_amplitude"] {
            mode.insert(
                key.into(),
                m.get(key).cloned().ok_or_else(|| format!("mode {k} has no {key}"))?,
            );
        }
        mode.insert(
            "output_shape".into(),
            Value::Array(pick(
                m.get("output_shape").ok_or_else(|| format!("mode {k} has no output_shape"))?,
                &format!("mode {k} output_shape"),
            )?),
        );
        if let Some(l) = present(m, "level") {
            mode.insert("level".into(), l.clone());
        }
        modes.push(Value::Object(mode));
    }
    let law = o.get("level_law").and_then(Value::as_object);
    let mut joint = Map::new();
    if let Some(l) = law {
        for k in ["delta", "damping_gain", "freq_exponent", "damping_exponent", "decay_cycles"] {
            if let Some(v) = l.get(k) {
                joint.insert(k.into(), v.clone());
            }
        }
    }
    // The residual is `n_outputs x n_drives` row-major, or empty.
    let residual = match sim.get("residual").and_then(Value::as_array) {
        Some(r) if !r.is_empty() => {
            let mut out = Vec::new();
            for &i in rows {
                for j in 0..n_d {
                    out.push(r.get(i * n_d + j).cloned().ok_or("simulator.residual is too short")?);
                }
            }
            out
        }
        _ => Vec::new(),
    };
    let output_delays = match sim.get("output_delays_s").and_then(Value::as_array) {
        Some(d) if !d.is_empty() => pick(&Value::Array(d.clone()), "simulator.output_delays_s")?,
        _ => Vec::new(),
    };
    let mut s = json!({
        "kind": "jointed",
        "n_outputs": rows.len(),
        "n_drives": n_d,
        "feedthrough": sim.get("feedthrough").cloned().unwrap_or(json!(0.0)),
        "residual": residual,
        "transport_delay_samples": sim.get("transport_delay_samples").cloned().unwrap_or(json!(0)),
        "modes": modes,
        "joint": if joint.is_empty() { Value::Null } else { Value::Object(joint) },
        "output_delays_s": output_delays,
    });
    if let Some(step) = present(sim, "step") {
        s["step"] = step.clone();
    }
    Ok(s)
}

/// The linear tier's structure as the simulator runs it: the research runner's own reading
/// of a modal object (`x4_runner`'s structure builder), for the keys `runner_structure` writes.
/// Measurement noise, when the case asks for it, is on every channel the run reads.
fn modal_plant_of(rs: &Value) -> Result<ModalPlant, String> {
    let n_o = rs["n_outputs"].as_u64().ok_or("n_outputs")? as usize;
    let n_d = rs["n_drives"].as_u64().ok_or("n_drives")? as usize;
    let vec_of = |v: &Value, what: &str| -> Result<Vec<f64>, String> { f64_list(v, what) };
    let mut modes = Vec::new();
    for (k, m) in rs["modes"].as_array().ok_or("modes")?.iter().enumerate() {
        let ds = vec_of(&m["drive_shape"], &format!("mode {k} drive_shape"))?;
        let os = vec_of(&m["output_shape"], &format!("mode {k} output_shape"))?;
        if ds.len() != n_d || os.len() != n_o {
            return Err(format!("mode {k}: the shapes are not {n_d} drives and {n_o} outputs"));
        }
        let mut md = Mode::new(
            m["f_hz"].as_f64().ok_or_else(|| format!("mode {k} f_hz"))?,
            m["q"].as_f64().ok_or_else(|| format!("mode {k} q"))?,
            ds,
            os,
        );
        md.response = ModeResponse::Acceleration;
        modes.push(md);
    }
    let mut mp = ModalPlant::new(n_o, n_d, modes);
    mp.feedthrough = rs["feedthrough"].as_f64().unwrap_or(0.0);
    mp.residual = vec_of(&rs["residual"], "residual")?;
    mp.transport_delay_samples = rs["transport_delay_samples"].as_u64().unwrap_or(0) as usize;
    mp.discretization = match rs.get("discretization").filter(|d| !d.is_null()) {
        None => Discretization::Biquad,
        Some(d) => match d.get("kind").and_then(Value::as_str) {
            Some("biquad") => Discretization::Biquad,
            Some("fitted") => Discretization::Fitted {
                taps: d.get("taps").and_then(Value::as_u64).unwrap_or(9) as usize,
                delay: d.get("delay").and_then(Value::as_u64).unwrap_or(4) as usize,
                band_hz: d
                    .get("band_hz")
                    .and_then(Value::as_f64)
                    .ok_or("a fitted discretization needs band_hz")?,
            },
            _ => return Err("the discretization is not biquad or fitted".into()),
        },
    };
    Ok(mp)
}

// ---------------------------------------------------------------------------------------
// The station's tap.
// ---------------------------------------------------------------------------------------

/// What the tap keeps for the run's record, from the engaged start on.
#[derive(Default)]
struct TapRecord {
    /// Configures seen (the identification's, then the engage's).
    configures: u32,
    /// Starts seen since the last configure.
    starts: u32,
    /// Frames read since the last start.
    frames: u64,
    /// `(frames read so far, each followed mode's envelope)` after each read.
    envelopes: Vec<(u64, Vec<f64>)>,
    /// With `--record`: every sample read (m/s^2), and every drive frame played.
    inputs: Vec<f64>,
    drive: Vec<f64>,
}

/// The station handed to the engine: the simulator, its samples converted to g, and the
/// record's taps.
struct Edge {
    main: SimBackend,
    /// A simulator whose outputs are its drives: fed the same calls, it gives the drive
    /// the structure was played, sample for sample (its FIFO, hold and stop ramp included).
    probe: SimBackend,
    probe_table: ChannelTable,
    /// The followed modes alone, stepped on that drive.
    follow: Option<JointedPlant>,
    stepper: Option<Stepper>,
    fs_hz: f64,
    n_drives: usize,
    record: bool,
    shared: Arc<Mutex<TapRecord>>,
    pbuf: Vec<f32>,
    frame: Vec<f64>,
    one: Vec<f64>,
    /// The amplifier's gain (`drives.gain`): every drive sample the engine writes is
    /// multiplied by it before the structure (and the drive tap) play it.
    gain: f64,
    gbuf: Vec<f32>,
}

impl Edge {
    /// The engaged run is the one after the second configure (the identification parks
    /// the station and the engage configures it again), from its start.
    fn engaged(r: &TapRecord) -> bool {
        r.configures >= 2 && r.starts >= 1
    }
}

impl Backend for Edge {
    fn caps(&self) -> Caps {
        self.main.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.main.open()?;
        self.probe.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.main.close()?;
        self.probe.close()
    }
    fn take_warnings(&mut self) -> Vec<String> {
        self.main.take_warnings()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        let a = self.main.configure(table, acq)?;
        self.probe.configure(&self.probe_table, acq)?;
        self.fs_hz = a.actual_rate_hz;
        let mut r = self.shared.lock().expect("tap");
        r.configures += 1;
        r.starts = 0;
        r.frames = 0;
        r.envelopes.clear();
        r.inputs.clear();
        r.drive.clear();
        self.stepper = None;
        Ok(a)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.main.arm()?;
        self.probe.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.main.start()?;
        self.probe.start()?;
        let mut r = self.shared.lock().expect("tap");
        r.starts += 1;
        if r.configures >= 2 && r.starts == 1 {
            self.stepper = match &self.follow {
                Some(p) => Some(
                    p.stepper(self.fs_hz)
                        .map_err(|e| HalError::Config(format!("the followed modes: {e}")))?,
                ),
                None => None,
            };
        }
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.main.stop()?;
        self.probe.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let got = self.main.read_input(out)?;
        let InputRead::Frames(n) = got else {
            return Ok(got);
        };
        let nd = self.n_drives;
        self.pbuf.resize(n * nd, 0.0);
        match self.probe.read_input(&mut self.pbuf[..n * nd])? {
            InputRead::Frames(m) if m == n => {}
            other => {
                return Err(HalError::Config(format!(
                    "the drive tap read {other:?} for {n} frames"
                )))
            }
        }
        let n_ch = out.len() / n.max(1);
        let mut r = self.shared.lock().expect("tap");
        if Edge::engaged(&r) {
            for f in 0..n {
                for (d, &v) in self.frame.iter_mut().zip(&self.pbuf[f * nd..(f + 1) * nd]) {
                    *d = v as f64;
                }
                if let Some(s) = self.stepper.as_mut() {
                    s.step(&self.frame, &mut self.one);
                }
                if self.record {
                    r.drive.extend_from_slice(&self.frame);
                }
            }
            if self.record {
                r.inputs
                    .extend(out[..n * n_ch].iter().map(|&v| v as f64));
            }
            r.frames += n as u64;
            if let Some(s) = self.stepper.as_ref() {
                let at = r.frames;
                r.envelopes.push((at, s.envelope()));
            }
        }
        drop(r);
        for v in out[..n * n_ch].iter_mut() {
            *v = units::sample_to_engine(*v);
        }
        Ok(got)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        // The amplifier: engine volts to the structure's input. A gain of 1 passes the
        // samples unchanged, so such a run's bytes are a build without the gain's.
        let drive: &[f32] = if self.gain == 1.0 {
            drive
        } else {
            self.gbuf.clear();
            self.gbuf.extend(drive.iter().map(|&v| (v as f64 * self.gain) as f32));
            &self.gbuf
        };
        let a = self.main.write_drive(drive)?;
        let b = self.probe.write_drive(drive)?;
        if a != b {
            return Err(HalError::Config(format!(
                "the drive tap took {b} frames where the station took {a}"
            )));
        }
        Ok(a)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.main.output_space()
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        self.main.input_overloads()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.main.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.main.emergency_stop()?;
        self.probe.emergency_stop()
    }
}

// ---------------------------------------------------------------------------------------
// The law's factory: what the law is given, and the ideal frequency response.
// ---------------------------------------------------------------------------------------

/// The law's factory, keeping the identification each build was given (the run file
/// records it), and with `--ideal-frf` replacing it, when the cell holds one, by the
/// given responses (controls by drives, limits by drives, per line).
struct LawTap {
    inner: Arc<dyn MimoSineLawFactory>,
    ideal: Arc<Mutex<Option<(MatrixStack, MatrixStack)>>>,
    /// The identification of the first build, and how many builds there were.
    given: Arc<Mutex<(Option<LawIdentification>, u32)>>,
}

impl MimoSineLawFactory for LawTap {
    fn build(&self, setup: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        let mut s = setup.clone();
        if let Some((r, l)) = self.ideal.lock().expect("ideal").clone() {
            let id = &mut s.identification;
            assert_eq!(r.dims(), id.response.dims(), "the ideal response's size");
            assert_eq!(l.dims(), id.limit_response.dims(), "the ideal limit response's size");
            id.response = r;
            id.limit_response = l;
            id.coherence.iter_mut().for_each(|c| *c = 1.0);
            id.limit_coherence.iter_mut().for_each(|c| *c = 1.0);
            id.trusted.iter_mut().for_each(|t| *t = true);
        }
        let mut g = self.given.lock().expect("given");
        if g.0.is_none() {
            g.0 = Some(s.identification.clone());
        }
        g.1 += 1;
        drop(g);
        self.inner.build(&s)
    }
    fn describe(&self) -> LawDescriptor {
        self.inner.describe()
    }
    fn manifest(&self) -> LawManifest {
        self.inner.manifest()
    }
}

// ---------------------------------------------------------------------------------------
// Numbers in the run file.
// ---------------------------------------------------------------------------------------

/// A number rounded to 9 significant digits, or null when it is not finite.
fn n9(x: f64) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    if x == 0.0 {
        return json!(0.0);
    }
    let r: f64 = format!("{x:.8e}").parse().expect("a formatted number");
    json!(r)
}
fn n9s(xs: impl IntoIterator<Item = f64>) -> Value {
    Value::Array(xs.into_iter().map(n9).collect())
}

/// The 1/48-octave bin of `f` counted from `lo`.
fn bin_of(f: f64, lo: f64) -> i64 {
    (BINS_PER_OCTAVE * (f / lo).log2() + 1e-9).floor() as i64
}

/// Which history points a sweep keeps: the first update of each (level, bin), and the
/// last update. Returns, for each kept update, the range of updates it stands for.
fn sweep_points(ticks: &[TickRecord], lo: f64) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut key: Option<(i64, i64)> = None;
    for (i, t) in ticks.iter().enumerate() {
        let k = ((t.level_db * 1e6).round() as i64, bin_of(t.frequency_hz, lo));
        if key != Some(k) {
            starts.push(i);
            key = Some(k);
        }
    }
    let mut out: Vec<(usize, usize)> = starts
        .iter()
        .enumerate()
        .map(|(j, &s)| (s, starts.get(j + 1).copied().unwrap_or(ticks.len())))
        .collect();
    if let (Some(last), Some(n)) = (out.last().copied(), ticks.len().checked_sub(1)) {
        if last.0 != n {
            let l = out.len() - 1;
            out[l].1 = n;
            out.push((n, n + 1));
        }
    }
    out
}

/// How the measured identification compares with the small-signal response handed to the
/// law with `--ideal-frf` (both controls by drives, g per volt): the size and the angle of
/// the measured over the ideal, element by element, on the trusted lines whose element is
/// at least 1/1000 of the line's largest; and the straight line that best fits the angle of
/// the summed products against frequency (a delay left between the two, samples, and a
/// constant angle, degrees).
fn compare_responses(
    meas: &MatrixStack,
    ideal: &MatrixStack,
    freqs: &[f64],
    trusted: &[bool],
    fs: f64,
) -> Value {
    let per = meas.data().len() / freqs.len().max(1);
    let (mut dbs, mut degs) = (Vec::new(), Vec::new());
    let mut fit: Vec<(f64, f64)> = Vec::new();
    for (k, &f) in freqs.iter().enumerate() {
        if !trusted.get(k).copied().unwrap_or(false) {
            continue;
        }
        let m = &meas.data()[k * per..(k + 1) * per];
        let v = &ideal.data()[k * per..(k + 1) * per];
        let big = m.iter().map(|x| x.norm()).fold(0.0, f64::max);
        let mut sum = C64::new(0.0, 0.0);
        for (a, b) in m.iter().zip(v) {
            if a.norm() > 1e-3 * big && b.norm() > 0.0 {
                dbs.push((20.0 * (a.norm() / b.norm()).log10()).abs());
                degs.push((a / b).arg().to_degrees().abs());
                sum += a * b.conj() / (a.norm() * b.norm());
            }
        }
        if sum.norm() > 0.0 {
            fit.push((f, sum.arg()));
        }
    }
    // Unwrap along frequency, then a least-squares line.
    let mut prev = 0.0;
    let mut off = 0.0;
    let mut pts = Vec::with_capacity(fit.len());
    for (i, &(f, a)) in fit.iter().enumerate() {
        if i > 0 {
            let d = a + off - prev;
            off -= std::f64::consts::TAU * (d / std::f64::consts::TAU).round();
        }
        prev = a + off;
        pts.push((f, prev));
    }
    let (delay, angle) = if pts.len() >= 2 {
        let n = pts.len() as f64;
        let mf = pts.iter().map(|p| p.0).sum::<f64>() / n;
        let ma = pts.iter().map(|p| p.1).sum::<f64>() / n;
        let sxy: f64 = pts.iter().map(|p| (p.0 - mf) * (p.1 - ma)).sum();
        let sxx: f64 = pts.iter().map(|p| (p.0 - mf) * (p.0 - mf)).sum();
        let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
        let c = ma - slope * mf;
        let wrapped = (c + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
        (slope * fs / std::f64::consts::TAU, wrapped.to_degrees())
    } else {
        (f64::NAN, f64::NAN)
    };
    let med = |v: &mut Vec<f64>| -> f64 {
        if v.is_empty() {
            return f64::NAN;
        }
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    json!({
        "elements": dbs.len(),
        "median_db": n9(med(&mut dbs)), "largest_db": n9(dbs.last().copied().unwrap_or(f64::NAN)),
        "median_deg": n9(med(&mut degs)), "largest_deg": n9(degs.last().copied().unwrap_or(f64::NAN)),
        "fitted_delay_samples": n9(delay), "fitted_angle_deg": n9(angle),
    })
}

/// The file holding the identification the law was given.
const LAW_ID_FILE: &str = "law_identification.f8";

/// The columns of [`LAW_ID_FILE`], one row a line.
fn law_id_columns(nc: usize, nl: usize, nd: usize) -> Value {
    json!([
        {"name": "frequency_hz", "count": 1},
        {"name": "response_re", "count": nc * nd, "order": "control-major (control, drive)", "unit": "m/s^2 pk per V pk"},
        {"name": "response_im", "count": nc * nd},
        {"name": "limit_response_re", "count": nl * nd, "order": "limit-major (limit, drive)", "unit": "m/s^2 pk per V pk"},
        {"name": "limit_response_im", "count": nl * nd},
        {"name": "coherence", "count": nc},
        {"name": "limit_coherence", "count": nl},
        {"name": "trusted", "count": 1, "unit": "1 or 0"},
    ])
}

/// Writes the identification the law was given (`LawSetup::identification`, converted
/// from g to m/s^2) as little-endian float64, one row a line; returns the lines and
/// the bytes.
fn write_law_identification(
    dir: &Path,
    id: &LawIdentification,
    nc: usize,
    nl: usize,
    nd: usize,
) -> (usize, Vec<u8>) {
    let lines = id.frequency_hz.len();
    let mut b = Vec::with_capacity(lines * (2 + 2 * (nc + nl) * nd + nc + nl) * 8);
    let mut put = |x: f64| b.extend_from_slice(&x.to_le_bytes());
    for k in 0..lines {
        put(id.frequency_hz[k]);
        let r = &id.response.data()[k * nc * nd..(k + 1) * nc * nd];
        let l = &id.limit_response.data()[k * nl * nd..(k + 1) * nl * nd];
        r.iter().for_each(|v| put(units::from_engine(v.re)));
        r.iter().for_each(|v| put(units::from_engine(v.im)));
        l.iter().for_each(|v| put(units::from_engine(v.re)));
        l.iter().for_each(|v| put(units::from_engine(v.im)));
        (0..nc).for_each(|c| put(id.coherence[c * lines + k]));
        (0..nl).for_each(|c| put(id.limit_coherence[c * lines + k]));
        put(if id.trusted[k] { 1.0 } else { 0.0 });
    }
    std::fs::create_dir_all(dir).unwrap_or_else(|e| refuse(format!("{}: {e}", dir.display())));
    std::fs::write(dir.join(LAW_ID_FILE), &b)
        .unwrap_or_else(|e| refuse(format!("{LAW_ID_FILE}: {e}")));
    (lines, b)
}

// ---------------------------------------------------------------------------------------
// The run.
// ---------------------------------------------------------------------------------------

struct Args {
    case: PathBuf,
    root: PathBuf,
    out: PathBuf,
    ideal_frf: bool,
    record: bool,
    true_modes: bool,
}

fn args() -> Args {
    let mut it = std::env::args().skip(1);
    let case = it.next().unwrap_or_else(|| refuse(USAGE));
    let (mut root, mut out, mut ideal_frf, mut record, mut true_modes) =
        (None, None, false, false, true);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--root" => root = it.next(),
            "--out" => out = it.next(),
            "--ideal-frf" => ideal_frf = true,
            "--record" => record = true,
            "--no-true-modes" => true_modes = false,
            _ => refuse(format!("unknown argument {a}\n{USAGE}")),
        }
    }
    Args {
        case: case.into(),
        root: root.unwrap_or_else(|| refuse(USAGE)).into(),
        out: out.unwrap_or_else(|| refuse(USAGE)).into(),
        ideal_frf,
        record,
        true_modes,
    }
}

/// The settings a case runs under.
struct Protective {
    alarm_db: f64,
    abort_db: f64,
    abort_ticks: u32,
    limit_alarm_db: f64,
    limit_abort_db: f64,
    divergence_s: f64,
    drive_abort: Option<f64>,
}

fn protective(widened: bool) -> Protective {
    let g = GuardSettings::default();
    if widened {
        Protective {
            alarm_db: 1.5,
            abort_db: 20.0,
            abort_ticks: 100,
            limit_alarm_db: 1.5,
            limit_abort_db: 20.0,
            divergence_s: 60.0,
            drive_abort: Some(hertzforge_mimo_sine::limits::MAX_DRIVE_VPK),
        }
    } else {
        Protective {
            alarm_db: hertzforge_sine::profile::DEFAULT_ALARM_DB.0,
            abort_db: hertzforge_sine::profile::DEFAULT_ABORT_DB.0,
            abort_ticks: g.abort_ticks,
            limit_alarm_db: hertzforge_sine::profile::DEFAULT_ALARM_DB.0,
            limit_abort_db: hertzforge_sine::profile::DEFAULT_ABORT_DB.0,
            divergence_s: g.divergence_s,
            drive_abort: None,
        }
    }
}

/// The test's own length, s (without the settle windows of a stepped sine).
/// The lowest and highest frequency the test visits, Hz.
fn test_span(t: &TestSpec) -> (f64, f64) {
    match *t {
        TestSpec::Sweep { band, .. } | TestSpec::Stepped { band, .. } => band,
        TestSpec::Dwell { freq_hz, .. } => (freq_hz, freq_hz),
    }
}

/// Whether a grid covers `span` (its first point at or under the low end, its last at
/// or over the high end).
fn covers(grid: &[f64], span: (f64, f64), what: &str) -> Result<(), String> {
    match (grid.first(), grid.last()) {
        (Some(&a), Some(&b)) if a <= span.0 && b >= span.1 => Ok(()),
        _ => Err(format!(
            "{what} runs from {} to {} Hz and does not cover the test's {} to {} Hz",
            grid.first().copied().unwrap_or(f64::NAN),
            grid.last().copied().unwrap_or(f64::NAN),
            span.0,
            span.1
        )),
    }
}

fn test_length_s(t: &TestSpec) -> f64 {
    match *t {
        TestSpec::Sweep { band, rate, .. } => 60.0 * (band.1 / band.0).log2() / rate,
        TestSpec::Stepped {
            band,
            per_octave,
            step_s,
        } => ((band.1 / band.0).log2() * per_octave + 2.0) * (step_s + 1.0),
        TestSpec::Dwell { dwell_s, .. } => dwell_s,
    }
}

fn main() {
    let t_start = Instant::now();
    let a = args();
    let case_bytes = std::fs::read(&a.case)
        .unwrap_or_else(|e| refuse(format!("{}: {e}", a.case.display())));
    let case_v: Value = serde_json::from_slice(&case_bytes)
        .unwrap_or_else(|e| refuse(format!("{}: {e}", a.case.display())));
    let case = parse_case(&case_v).unwrap_or_else(|e| refuse(e));
    let case_sha = sha256_hex(&case_bytes);

    // The run folder: made new, never overwritten.
    if a.out.exists()
        && std::fs::read_dir(&a.out)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        refuse(format!("{} exists and is not empty", a.out.display()));
    }

    // The structure.
    let st_bytes = std::fs::read(a.root.join(&case.structure_file)).unwrap_or_else(|e| {
        refuse(format!("{}: {e}", case.structure_file.display()))
    });
    let st_sha = sha256_hex(&st_bytes);
    if let Some(want) = &case.structure_sha256 {
        if *want != st_sha {
            refuse(format!(
                "{}: SHA-256 {st_sha}, the case says {want}",
                case.structure_file.display()
            ));
        }
    }
    let st = load_structure(&st_bytes).unwrap_or_else(|e| refuse(e));
    let find = |name: &str| -> usize {
        st.names
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| refuse(format!("no output \"{name}\" in the structure")))
    };
    let control_rows: Vec<usize> = match &case.controls {
        Some(l) => l.iter().map(|n| find(n)).collect(),
        None => (0..st.names.len())
            .filter(|&i| st.roles[i] == "control")
            .collect(),
    };
    if control_rows.is_empty() {
        refuse("the structure has no control output");
    }
    let limit_rows: Vec<usize> = case.limits.iter().map(|l| find(&l.output)).collect();
    let mut seen = control_rows.clone();
    seen.extend(&limit_rows);
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    if sorted.len() != seen.len() {
        refuse("an output is named twice among the controls and limit channels");
    }
    let rows = seen;
    let (nc, nl, nd) = (control_rows.len(), limit_rows.len(), st.drive_names.len());
    let control_names: Vec<String> = control_rows.iter().map(|&i| st.names[i].clone()).collect();
    let limit_names: Vec<String> = limit_rows.iter().map(|&i| st.names[i].clone()).collect();
    let rs = runner_structure(&st, &rows).unwrap_or_else(|e| refuse(e));
    let model = if st.kind == "jointed" {
        PlantModel::Jointed(obs_structure::plant_of(rs.as_object().expect("an object")))
    } else {
        PlantModel::Modal(modal_plant_of(&rs).unwrap_or_else(|e| refuse(e)))
    };
    if let Err(e) = model.validate(case.fs_hz) {
        refuse(format!("the structure at {} Hz: {e}", case.fs_hz));
    }
    // The level rule's modes, for the record of their frequencies.
    let empty = JointedPlant::new(1, 1, Vec::new());
    let plant = match &model {
        PlantModel::Jointed(j) => j,
        _ => &empty,
    };

    // The reference.
    let (ref_v, ref_sha) = match (&case.reference_file, &case.reference_inline) {
        (Some(f), _) => {
            let b = std::fs::read(a.root.join(f))
                .unwrap_or_else(|e| refuse(format!("{}: {e}", f.display())));
            let sha = sha256_hex(&b);
            if let Some(want) = &case.reference_sha256 {
                if *want != sha {
                    refuse(format!("{}: SHA-256 {sha}, the case says {want}", f.display()));
                }
            }
            (
                serde_json::from_slice::<Value>(&b)
                    .unwrap_or_else(|e| refuse(format!("{}: {e}", f.display()))),
                Some(sha),
            )
        }
        (None, Some(v)) => (v.clone(), None),
        (None, None) => unreachable!("checked in the case"),
    };
    let reference = parse_reference(&ref_v).unwrap_or_else(|e| refuse(e));
    if reference.controls != control_names {
        refuse(format!(
            "the reference's controls {:?} are not the case's {:?}",
            reference.controls, control_names
        ));
    }
    // The engine's profiles give no value outside their grid: the reference and every
    // bound must cover the frequencies the test visits.
    let span = test_span(&case.test);
    if let Err(e) = covers(&reference.freqs, span, "the reference") {
        refuse(e);
    }
    for l in &case.limits {
        if let Err(e) = covers(&l.freqs, span, &format!("the bound of {}", l.output)) {
            refuse(e);
        }
    }

    // The law.
    let Some(decl) = hertzforge_mimo_sine_laws::builtin_decl(&case.law_id) else {
        refuse(format!("\"{}\" is not a built-in MIMO Sine law", case.law_id));
    };
    let specs = hertzforge_mimo_sine_laws::facts::host_manifest(decl).params;
    for k in case.law_params.keys() {
        if !specs.iter().any(|s| &s.key == k) {
            refuse(format!("law.params: \"{k}\" is not a parameter of {}", case.law_id));
        }
    }
    let params = sds_control::mimo::adapter::params_of(&specs, &case.law_params);
    if params.len() != case.law_params.len() {
        refuse("law.params: a value is not of its parameter's kind");
    }
    let inner = hertzforge_mimo_sine_laws::resolve_builtin_with(&case.law_id, &params)
        .unwrap_or_else(|e| refuse(format!("law: {e}")));
    let manifest = inner.manifest();
    let descriptor = inner.describe();
    let phase_specified = match case.phase {
        PhaseMode::Specified => true,
        PhaseMode::AmplitudeOnly => false,
        PhaseMode::Auto => !manifest.amplitude_only,
    };
    let ideal_cell: Arc<Mutex<Option<(MatrixStack, MatrixStack)>>> = Arc::new(Mutex::new(None));
    let given: Arc<Mutex<(Option<LawIdentification>, u32)>> = Arc::new(Mutex::new((None, 0)));
    let factory: Arc<dyn MimoSineLawFactory> = Arc::new(LawTap {
        inner: inner.clone(),
        ideal: ideal_cell.clone(),
        given: given.clone(),
    });

    // The setup, as a user's screen would give it.
    let prot = protective(case.widened);
    let per_drive = |v: &[f64], what: &str| -> Vec<f64> {
        match v.len() {
            1 => vec![v[0]; nd],
            n if n == nd => v.to_vec(),
            n => refuse(format!("{what} has {n} values for {nd} drives")),
        }
    };
    let ceiling = per_drive(&case.ceiling, "drives.ceiling_vpk");
    let abort = match prot.drive_abort {
        Some(v) => vec![v; nd],
        None => per_drive(&case.abort, "drives.abort_vpk"),
    };
    let mut phase_splits = 0;
    let mut controls = Vec::with_capacity(nc);
    for k in 0..nc {
        let amp: Vec<f64> = reference
            .amp
            .iter()
            .map(|r| units::to_engine(r[k] * case.scale))
            .collect();
        let phase = if phase_specified {
            let deg: Vec<f64> = reference.phase.iter().map(|r| r[k]).collect();
            let (p, s) = phase_points(&reference.freqs, &deg).unwrap_or_else(|e| refuse(e));
            phase_splits += s;
            p
        } else {
            Vec::new()
        };
        controls.push(ControlChannel {
            channel: 1 + k,
            profile: profile(&control_names[k], &reference.freqs, &amp, prot.alarm_db, prot.abort_db),
            phase,
        });
    }
    let limits: Vec<LimitChannel> = case
        .limits
        .iter()
        .enumerate()
        .map(|(j, l)| {
            let g: Vec<f64> = l.amp.iter().map(|&x| units::to_engine(x)).collect();
            LimitChannel {
                channel: 1 + nc + j,
                profile: profile(&l.output, &l.freqs, &g, prot.limit_alarm_db, prot.limit_abort_db),
            }
        })
        .collect();
    let test = match case.test {
        TestSpec::Sweep { band, rate, down } => TestPlan::Sweep(BidirSweepConfig {
            band,
            rate: SweepRate::LogOctPerMin(rate),
            start_hz: if down { band.1 } else { band.0 },
            initial_direction: if down {
                SweepDirection::Down
            } else {
                SweepDirection::Up
            },
            target: SweepTarget::Sweeps(1),
        }),
        TestSpec::Stepped {
            band,
            per_octave,
            step_s,
        } => TestPlan::Stepped(StepPlan::Grid(StepSineConfig {
            band,
            steps_per_octave: per_octave,
            spacing: StepSpacing::Log,
            dwell: Some(DwellFor::Seconds(step_s)),
        })),
        TestSpec::Dwell { freq_hz, dwell_s } => TestPlan::Dwell(DwellPlan {
            freq_hz,
            for_: DwellFor::Seconds(dwell_s),
        }),
    };
    let guards = GuardSettings {
        abort_ticks: prot.abort_ticks,
        divergence_s: prot.divergence_s,
        ..GuardSettings::default()
    };
    let top_db = *case.levels_db.last().expect("checked");
    let setup = MimoSineSetup {
        controls,
        drives: (0..nd)
            .map(|d| DriveChannel {
                channel: 1 + nc + nl + d,
                ceiling_vpk: ceiling[d],
                abort_vpk: abort[d],
            })
            .collect(),
        limits,
        phase_specified,
        test: test.clone(),
        schedule: Schedule {
            mode: LevelMode::Auto,
            ladder: LadderConfig {
                sysid_level_db: IdentificationSettings::default().level_db,
                start_db: case.levels_db[0],
                table_db: case.levels_db.clone(),
                per_rung: case.rung,
                full_for: RungFor::Seconds(1e9),
            },
        },
        fs_hz: case.fs_hz,
        tick_s: case.tick_s,
        tracking: Default::default(),
        identification: IdentificationSettings::default(),
        law: LawChoice {
            id: case.law_id.clone(),
            params: case.law_params.clone(),
        },
        requests: Default::default(),
        guards: guards.clone(),
    };
    let max_test_s = case.max_test_s.unwrap_or_else(|| {
        let rungs = (case.levels_db.len() - 1) as f64
            * match case.rung {
                RungFor::Seconds(s) => s,
                RungFor::Sweeps(n) => n as f64 * test_length_s(&case.test),
            };
        1.5 * (test_length_s(&case.test) + rungs) + 60.0
    });

    // The channel table: controls, limit channels, drives.
    let mut table = ChannelTable::new();
    for k in 0..nc + nl + nd {
        let mut c = Channel::new(k as u32 + 1);
        c.role = if k < nc {
            ChannelRole::Control
        } else if k < nc + nl {
            ChannelRole::Monitor
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        c.description = if k < nc {
            control_names[k].clone()
        } else if k < nc + nl {
            limit_names[k - nc].clone()
        } else {
            st.drive_names[k - nc - nl].clone()
        };
        table.channels.push(c);
    }
    let mut probe_table = ChannelTable::new();
    for k in 0..2 * nd {
        let mut c = Channel::new(k as u32 + 1);
        c.role = if k < nd {
            ChannelRole::Monitor
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        probe_table.channels.push(c);
    }

    // The station.
    let level_dependent = matches!(model, PlantModel::Jointed(_)) && plant.is_level_dependent();
    let (glo, ghi) = hertzforge_mimo_sine::ident::grid_band(test.band_hz(), case.fs_hz);
    let followed: Vec<usize> = if level_dependent && a.true_modes {
        (0..plant.modes.len())
            .filter(|&k| plant.modes[k].f_hz >= glo && plant.modes[k].f_hz <= ghi)
            .collect()
    } else {
        Vec::new()
    };
    let follow = (a.true_modes && !followed.is_empty()).then(|| JointedPlant {
        n_inputs: 1,
        modes: followed
            .iter()
            .map(|&k| JointedMode {
                output_shape: vec![0.0],
                ..plant.modes[k].clone()
            })
            .collect(),
        residual: Vec::new(),
        feedthrough: 0.0,
        output_delays_s: Vec::new(),
        ..plant.clone()
    });
    let follow_plant = follow.clone();
    let mut probe_plant = JointedPlant::new(nd, nd, Vec::new());
    probe_plant.residual = (0..nd * nd)
        .map(|i| if i / nd == i % nd { 1.0 } else { 0.0 })
        .collect();
    probe_plant.step = JointedStep::TriangleHold;
    let shared = Arc::new(Mutex::new(TapRecord::default()));
    let edge = Edge {
        main: SimConfig::new()
            .sample_rate_hz(case.fs_hz)
            .plant(model.clone())
            .noise_level(case.noise_rms_m_s2)
            .seed(case.noise_seed)
            .shared_clock(true)
            .build(),
        probe: SimConfig::new()
            .sample_rate_hz(case.fs_hz)
            .plant(PlantModel::Jointed(probe_plant))
            .noise_level(0.0)
            .seed(1)
            .shared_clock(true)
            .build(),
        probe_table,
        follow,
        stepper: None,
        fs_hz: case.fs_hz,
        n_drives: nd,
        record: a.record,
        shared: shared.clone(),
        pbuf: Vec::new(),
        gain: case.gain,
        gbuf: Vec::new(),
        frame: vec![0.0; nd],
        one: vec![0.0; 1],
    };

    // Everything the run file says whatever the outcome.
    let runner_sha = std::env::current_exe()
        .and_then(std::fs::read)
        .map(|b| sha256_hex(&b))
        .unwrap_or_else(|e| refuse(format!("the program's own file: {e}")));
    let test_json = match case.test {
        TestSpec::Sweep { band, rate, down } => json!({
            "type": "sweep", "band_hz": [band.0, band.1], "rate_oct_per_min": rate,
            "direction": if down { "down" } else { "up" },
            "start_hz": if down { band.1 } else { band.0 }, "sweeps": 1}),
        TestSpec::Stepped { band, per_octave, step_s } => json!({
            "type": "stepped", "band_hz": [band.0, band.1], "steps_per_octave": per_octave,
            "spacing": "log", "step_s": step_s}),
        TestSpec::Dwell { freq_hz, dwell_s } => json!({
            "type": "dwell", "freq_hz": freq_hz, "dwell_s": dwell_s}),
    };
    let st_obj = st.raw.as_object().expect("an object");
    let mut run = json!({
        "schema": "msnb-run/1",
        "case_id": case.case_id,
        "case_sha256": case_sha,
        "runner_sha256": runner_sha,
        "code_commit": COMMIT,
        "producer": {"program": "msnb_run", "engine": "hertzforge-mimo-sine", "simulator": "hertzforge-hal-sim"},
        "options": {"ideal_frf": a.ideal_frf, "record": a.record, "true_modes": a.true_modes},
        "structure": {
            "file": case.structure_file.to_string_lossy(),
            "sha256": st_sha,
            "structure_id": st_obj.get("structure_id").cloned().unwrap_or(Value::Null),
            "variant": st_obj.get("variant").cloned().unwrap_or(Value::Null),
            "seed": st_obj.get("seed").cloned().unwrap_or(Value::Null),
            "controls": control_names, "limits": limit_names, "drives": st.drive_names,
            "n_modes": st.n_modes, "fs_hz": case.fs_hz,
        },
        "law": {
            "id": case.law_id, "name": descriptor.name, "version": descriptor.version,
            "params": Value::Object(case.law_params.clone()),
            "amplitude_only": manifest.amplitude_only, "phase_specified": phase_specified,
        },
        "test": test_json,
        "reference": {
            "source": if case.reference_file.is_some() { "file" } else { "inline" },
            "file": case.reference_file.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "sha256": ref_sha, "lines": reference.freqs.len(),
            "full_level_scale": case.scale, "phase_splits": phase_splits,
        },
        "settings": {
            "protective": if case.widened { "widened" } else { "default" },
            "alarm_db": prot.alarm_db, "abort_db": prot.abort_db, "abort_ticks": prot.abort_ticks,
            "limit_alarm_db": prot.limit_alarm_db, "limit_abort_db": prot.limit_abort_db,
            "divergence_s": guards.divergence_s, "max_step_db": guards.max_step_db,
            "max_rise_db_per_s": guards.max_rise_db_per_s, "rotation_cap_deg": guards.rotation_cap_deg,
            "law_budget_fraction": guards.law_budget, "law_misses": guards.law_misses,
            "law_call_deadline_s": hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET.as_secs_f64(),
            "ceiling_vpk": ceiling, "abort_vpk": abort, "drive_gain": case.gain,
            "ladder_db": case.levels_db,
            "rung": match case.rung { RungFor::Seconds(s) => json!({"seconds": s}), RungFor::Sweeps(n) => json!({"sweeps": n}) },
            "noise_rms_m_s2": case.noise_rms_m_s2, "noise_seed": case.noise_seed,
            "identification_seed": case.ident_seed, "max_test_s": max_test_s,
            "tracking": "engine default", "identification": "engine default",
        },
        "identification": Value::Null,
        "outcome": Value::Null,
    });
    let out_dir = a.out.clone();
    let write = |run: &Value, timing: &Value, code: i32| -> ! {
        std::fs::create_dir_all(&out_dir)
            .unwrap_or_else(|e| refuse(format!("{}: {e}", out_dir.display())));
        let mut s = serde_json::to_string(run).expect("the run file");
        s.push('\n');
        std::fs::write(out_dir.join("run.json"), s)
            .unwrap_or_else(|e| refuse(format!("run.json: {e}")));
        let mut t = serde_json::to_string_pretty(timing).expect("the timing file");
        t.push('\n');
        std::fs::write(out_dir.join("timing.json"), t)
            .unwrap_or_else(|e| refuse(format!("timing.json: {e}")));
        std::process::exit(code);
    };
    let ended_early = |run: &mut Value, class: &str, message: String, t0: Instant| -> ! {
        run["outcome"] = json!({"class": class, "engine_outcome": Value::Null, "guard": Value::Null,
            "channel": Value::Null, "frequency_hz": Value::Null, "time_s": Value::Null,
            "level_db": Value::Null, "message": message});
        eprintln!("msnb_run: {class}: {message}");
        write(run, &json!({"total_s": t0.elapsed().as_secs_f64()}), 3)
    };

    // The run: arm, identify, engage, loop.
    let mut opts = EngineOptions::new(StationClass::HertzforgeSim);
    opts.seed = case.ident_seed;
    opts.law_budget = Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET);
    let mut svc = match MimoSineService::new(setup, table, Box::new(edge), factory, opts) {
        Ok(s) => s,
        Err(e) => ended_early(&mut run, "refused", e.to_string(), t_start),
    };
    if let Err(e) = svc.arm() {
        ended_early(&mut run, "refused", e.to_string(), t_start);
    }
    let t_ident = Instant::now();
    let rep = match svc.identify() {
        Ok(r) => r.clone(),
        Err(e) => ended_early(&mut run, "refused", e.to_string(), t_start),
    };
    let identify_s = t_ident.elapsed().as_secs_f64();
    let fs = svc.sample_rate_hz().unwrap_or(case.fs_hz);
    let band = test.band_hz();
    let fit_band = match case.test {
        TestSpec::Dwell { freq_hz, .. } => (freq_hz / 1.25, freq_hz * 1.25),
        _ => band,
    };
    let resonances: Vec<Value> = if let TestPlan::Sweep(_) = &test {
        rep.sweep_rate_warning
            .iter()
            .map(|r| json!({"f_hz": n9(r.f_hz), "q": n9(r.q), "time_constants": n9(r.time_constants), "flagged": r.flagged}))
            .collect()
    } else {
        sweep_warning(&rep.identification, fit_band, SweepRate::LogOctPerMin(1.0))
            .1
            .iter()
            .map(|r| json!({"f_hz": n9(r.f_hz), "q": n9(r.q)}))
            .collect()
    };
    let mut ident_json = json!({
        "source": if a.ideal_frf { "ideal_frf" } else { "engine" },
        "level_db": rep.level_db.map(n9), "lag_frames": rep.lag_frames, "frames_ahead": rep.frames_ahead,
        "record_len": rep.record_len, "realizations": rep.realizations,
        "line_spacing_hz": n9(rep.line_spacing_hz), "duration_s": n9(rep.duration_s),
        "band_coherence": n9s(rep.band_coherence.iter().copied()), "gate_passed": rep.gate_passed,
        "resonances": resonances, "ideal_vs_measured": Value::Null,
    });
    {
        let id = &rep.identification;
        let h = model.analytic_frf_at(&id.frequency_hz, fs);
        // The identification is the response with the measured lag taken out, counted
        // from the engine's own drive sample: the structure's small-signal response turned
        // by the lag less the frames ahead and the service's simulator block. The block
        // term was found by fitting the measured against the response (a delay of -256.0
        // samples left on a linear and a realistic structure, 6x6); `ideal_vs_measured`
        // fits it again on every run.
        let shift = rep.lag_frames as f64
            - rep.frames_ahead as f64
            - hertzforge_mimo_sine::service::SIM_BLOCK_FRAMES as f64;
        let lines = id.frequency_hz.len();
        let mut r = MatrixStack::zeros(lines, nc, nd);
        let mut l = MatrixStack::zeros(lines, nl, nd);
        for (k, &f) in id.frequency_hz.iter().enumerate() {
            let w = std::f64::consts::TAU * f / fs * shift;
            let rot = C64::new(w.cos(), w.sin());
            let line = &h.data()[k * (nc + nl) * nd..(k + 1) * (nc + nl) * nd];
            for i in 0..nc + nl {
                for j in 0..nd {
                    // Per engine volt: the structure's response times the amplifier's gain.
                    let v = line[i * nd + j] * rot * case.gain / units::G0;
                    if i < nc {
                        r.data_mut()[k * nc * nd + i * nd + j] = v;
                    } else {
                        l.data_mut()[k * nl * nd + (i - nc) * nd + j] = v;
                    }
                }
            }
        }
        let mut cmp = compare_responses(&id.response, &r, &id.frequency_hz, &id.trusted, fs);
        cmp["delay_turn_samples"] = json!(shift);
        ident_json["ideal_vs_measured"] = cmp;
        if a.ideal_frf {
            *ideal_cell.lock().expect("ideal") = Some((r, l));
        }
    }
    run["identification"] = ident_json;
    match svc.state() {
        ServiceState::CoherenceHeld => ended_early(
            &mut run,
            "held_by_identification",
            "the identification's coherence was under the gate".into(),
            t_start,
        ),
        ServiceState::AwaitingReview => {
            if let Err(e) = svc.proceed_after_review() {
                ended_early(&mut run, "refused", e.to_string(), t_start);
            }
        }
        _ => {}
    }
    let t_engage = Instant::now();
    if let Err(e) = svc.engage() {
        let class = if matches!(svc.state(), ServiceState::Ended(Outcome::LawFault)) {
            "law_fault"
        } else {
            "refused"
        };
        ended_early(&mut run, class, e.to_string(), t_start);
    }
    let engage_s = t_engage.elapsed().as_secs_f64();
    let tick_s_actual = svc.law_setup().map(|s| s.tick_s).unwrap_or(case.tick_s);
    let (rt, re) = svc.take_records().expect("records");
    let t_run = Instant::now();
    let mut ticks: Vec<TickRecord> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    let block = hertzforge_mimo_sine::service::SIM_BLOCK_FRAMES as f64;
    let rounds = (max_test_s * fs / block / 16.0) as u64 + 64;
    let mut station_error = None;
    let mut alive = true;
    for _ in 0..rounds {
        match svc.run_batches(16) {
            Ok(x) => alive = x,
            Err(e) => {
                station_error = Some(e.to_string());
                alive = false;
            }
        }
        ticks.extend(rt.try_iter());
        events.extend(re.try_iter());
        if !alive {
            break;
        }
    }
    let mut runner_stopped = false;
    if alive {
        runner_stopped = true;
        svc.request_stop();
        for _ in 0..4096 {
            match svc.run_batches(16) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    station_error = Some(e.to_string());
                    break;
                }
            }
            ticks.extend(rt.try_iter());
            events.extend(re.try_iter());
        }
    }
    ticks.extend(rt.try_iter());
    events.extend(re.try_iter());
    let run_s = t_run.elapsed().as_secs_f64();
    let t_write = Instant::now();
    let outcome = match svc.state() {
        ServiceState::Ended(o) => Some(o),
        _ => None,
    };
    let core = svc.core().expect("engaged");
    let over_ceiling = core.over_ceiling_count();
    let largest_sample_v = core.largest_sample_v();
    let drive_is_zero = core.drive_is_zero();
    let setup_budget_s = core.setup_law_budget_s();
    let records_dropped = svc.records_dropped();
    let tick_frames = (tick_s_actual * fs).round() as u64;

    // The outcome.
    let ch_name = |kind: &str, i: u8| -> Value {
        let i = i as usize;
        match kind {
            "control" => control_names.get(i).cloned().map(Value::from).unwrap_or(Value::Null),
            "limit" => limit_names.get(i).cloned().map(Value::from).unwrap_or(Value::Null),
            "drive" => st.drive_names.get(i).cloned().map(Value::from).unwrap_or(Value::Null),
            _ => Value::Null,
        }
    };
    let stopping = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::Stopping { .. }))
        .copied();
    let (guard, channel) = match outcome {
        Some(Outcome::ControlAbort { control }) => (Some("control_abort"), ch_name("control", control)),
        Some(Outcome::LimitAbort { limit }) => (Some("limit_abort"), ch_name("limit", limit)),
        Some(Outcome::DriveAbort { drive }) => (Some("drive_abort"), ch_name("drive", drive)),
        Some(Outcome::Kinematic { control }) => (Some("kinematic"), ch_name("control", control)),
        Some(Outcome::Divergence) => (Some("divergence"), Value::Null),
        Some(Outcome::FrequencyOutOfRange) => (Some("frequency_out_of_range"), Value::Null),
        _ => (None, Value::Null),
    };
    let class = if runner_stopped {
        "runner_stopped"
    } else {
        match outcome {
            Some(Outcome::Completed) => "completed",
            Some(Outcome::LawFault) => "law_fault",
            Some(Outcome::Station) | Some(Outcome::RunThreadFailure) | None => "station_fault",
            Some(Outcome::Stopped) => "runner_stopped",
            Some(_) if case.widened => "aborted",
            Some(_) => "stopped_by_protective_default",
        }
    };
    run["outcome"] = json!({
        "class": class,
        "engine_outcome": outcome.map(|o| o.as_str()),
        "guard": guard,
        "channel": channel,
        "frequency_hz": stopping.filter(|_| class != "completed").map(|e| n9(e.frequency_hz)),
        "time_s": stopping.filter(|_| class != "completed").map(|e| n9(e.time_s)),
        "level_db": stopping.filter(|_| class != "completed").map(|e| n9(e.level_db)),
        "message": station_error.clone().or_else(|| svc.station_note().map(str::to_owned)),
    });

    // The timeline.
    let mut kept: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut dropped: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut ev_json = Vec::new();
    for e in &events {
        let kind = e.kind.as_str();
        let n = kept.entry(kind).or_default();
        if matches!(e.kind, EventKind::Alarm { .. } | EventKind::AlarmCleared { .. })
            && *n >= EVENTS_KEPT_PER_KIND
        {
            *dropped.entry(kind).or_default() += 1;
            continue;
        }
        *n += 1;
        let detail = match e.kind {
            EventKind::Held { by_law } => json!({"by_law": by_law}),
            EventKind::ManualFrequency { to_hz } => json!({"to_hz": n9(to_hz)}),
            EventKind::NotchIn { depth } => json!({"depth": n9(depth)}),
            EventKind::Alarm { control, error_db } => json!({"control": ch_name("control", control), "error_db": n9(error_db)}),
            EventKind::AlarmCleared { control } => json!({"control": ch_name("control", control)}),
            EventKind::LawMiss { reason } => json!({"reason": reason}),
            EventKind::OverCeiling { scale } => json!({"scale": n9(scale)}),
            EventKind::Stopping { outcome } | EventKind::Ended { outcome } => json!({"outcome": outcome.as_str()}),
            _ => json!({}),
        };
        ev_json.push(json!({"time_s": n9(e.time_s), "index": e.index, "frequency_hz": n9(e.frequency_hz),
            "level_db": n9(e.level_db), "kind": kind, "detail": detail}));
    }
    run["events"] = Value::Array(ev_json);
    run["events_dropped"] = json!(dropped);

    // The history.
    let at_top = |t: &TickRecord| (t.level_db - top_db).abs() < 1e-6;
    // The law check's rule (`hertzforge_mimo_sine_laws::check`).
    let judged = |t: &TickRecord| !t.settling && t.miss.is_none() && at_top(t);
    let is_sweep = matches!(case.test, TestSpec::Sweep { .. });
    let spans: Vec<(usize, usize)> = if is_sweep {
        sweep_points(&ticks, band.0)
    } else {
        (0..ticks.len()).map(|i| (i, i + 1)).collect()
    };
    let pts: Vec<&TickRecord> = spans.iter().map(|&(s, _)| &ticks[s]).collect();
    let cplx = |f: &dyn Fn(&TickRecord) -> Vec<C64>, conv: bool| -> (Value, Value) {
        let (mut re_, mut im_) = (Vec::new(), Vec::new());
        for t in &pts {
            let v = f(t);
            let s = |x: f64| if conv { units::from_engine(x) } else { x };
            re_.push(n9s(v.iter().map(|c| s(c.re))));
            im_.push(n9s(v.iter().map(|c| s(c.im))));
        }
        (Value::Array(re_), Value::Array(im_))
    };
    let (c_re, c_im) = cplx(&|t| t.control[..nc].to_vec(), true);
    let (r_re, r_im) = cplx(&|t| t.reference[..nc].to_vec(), true);
    let (d_re, d_im) = cplx(&|t| t.drive[..nd].to_vec(), false);
    let mut hist = json!({
        "decimation": if is_sweep { "1/48 octave" } else { "every update" },
        "points": pts.len(),
        "index": pts.iter().map(|t| t.index).collect::<Vec<_>>(),
        "time_s": n9s(pts.iter().map(|t| t.time_s)),
        "frequency_hz": n9s(pts.iter().map(|t| t.frequency_hz)),
        "level_db": n9s(pts.iter().map(|t| t.level_db)),
        "judged": pts.iter().map(|t| judged(t)).collect::<Vec<_>>(),
        "settling": pts.iter().map(|t| t.settling).collect::<Vec<_>>(),
        "miss": pts.iter().map(|t| t.miss).collect::<Vec<_>>(),
        "control_re": c_re, "control_im": c_im,
        "reference_re": r_re, "reference_im": r_im,
        "error_db": pts.iter().map(|t| n9s(t.error_db[..nc].iter().copied())).collect::<Vec<_>>(),
        "drive_re": d_re, "drive_im": d_im,
        "notch_depth": n9s(pts.iter().map(|t| t.notch_depth)),
        "ceiling_scale": n9s(pts.iter().map(|t| t.ceiling_scale)),
        "cap_bound": pts.iter().map(|t| t.cap_bound).collect::<Vec<_>>(),
        "opposition_ratio": n9s(pts.iter().map(|t| t.opposition_ratio)),
    });
    if nl > 0 {
        let (jr_re, jr_im) = cplx(&|t| t.judged_reference[..nc].to_vec(), true);
        let (l_re, l_im) = cplx(&|t| t.limit[..nl].to_vec(), true);
        hist["judged_reference_re"] = jr_re;
        hist["judged_reference_im"] = jr_im;
        hist["limit_re"] = l_re;
        hist["limit_im"] = l_im;
        hist["limit_bound"] = Value::Array(
            pts.iter()
                .map(|t| n9s(t.limit_bound[..nl].iter().map(|&b| units::from_engine(b))))
                .collect(),
        );
    }
    if is_sweep {
        let mut upd = Vec::new();
        let mut jud = Vec::new();
        let mut emin = Vec::new();
        let mut emax = Vec::new();
        for &(s, e) in &spans {
            let span = &ticks[s..e.min(ticks.len())];
            let j: Vec<&TickRecord> = span.iter().filter(|t| judged(t)).collect();
            upd.push(span.len());
            jud.push(j.len());
            if j.is_empty() {
                emin.push(Value::Null);
                emax.push(Value::Null);
            } else {
                emin.push(n9s((0..nc).map(|k| j.iter().map(|t| t.error_db[k]).fold(f64::INFINITY, f64::min))));
                emax.push(n9s((0..nc).map(|k| j.iter().map(|t| t.error_db[k]).fold(f64::NEG_INFINITY, f64::max))));
            }
        }
        hist["bin_updates"] = json!(upd);
        hist["bin_judged"] = json!(jud);
        hist["bin_error_db_min"] = Value::Array(emin);
        hist["bin_error_db_max"] = Value::Array(emax);
    }
    run["history"] = hist;

    // The simulator's mode frequencies at each point.
    let tap = shared.lock().expect("tap");
    let tracked = follow_plant.is_some() && tap.configures == 2 && !tap.envelopes.is_empty();
    let mut true_freqs = Vec::new();
    if let (true, Some(fp)) = (tracked, follow_plant.as_ref()) {
        for t in &pts {
            let at = (t.time_s * fs).round() as u64;
            let k = tap.envelopes.partition_point(|(s, _)| *s <= at);
            let env = &tap.envelopes[k.saturating_sub(1)].1;
            true_freqs.push(n9s(fp.modes_at_envelope(env).iter().map(|m| m.0)));
        }
    }
    run["true_modes"] = json!({
        "tracked": tracked,
        "modes": followed.iter().map(|&k| json!({"index": k, "f0_hz": n9(plant.modes[k].f_hz)})).collect::<Vec<_>>(),
        "frequency_hz": true_freqs,
        "resolution_samples": hertzforge_mimo_sine::service::SIM_BLOCK_FRAMES,
        "configures_seen": tap.configures,
    });
    let largest_drive = ticks
        .iter()
        .flat_map(|t| t.drive[..nd].iter().map(|d| d.norm()).collect::<Vec<_>>())
        .fold(0.0f64, f64::max);
    run["structure"]["tick_s"] = n9(tick_s_actual);
    run["structure"]["tick_frames"] = json!(tick_frames);
    run["summary"] = json!({
        "updates": ticks.len(), "records_dropped": records_dropped,
        "judged_updates": ticks.iter().filter(|t| judged(t)).count(),
        "largest_drive_vpk": n9(largest_drive), "largest_sample_v": n9(largest_sample_v),
        "over_ceiling_samples": over_ceiling, "drive_is_zero": drive_is_zero,
    });

    // The identification the law was given.
    let (law_given, builds) = given.lock().expect("given").clone();
    if let Some(id) = law_given {
        let (lines, b) = write_law_identification(&a.out, &id, nc, nl, nd);
        run["identification"]["law_given"] = json!({
            "file": LAW_ID_FILE, "sha256": sha256_hex(&b), "lines": lines,
            "controls": nc, "limits": nl, "drives": nd, "builds": builds,
            "columns": law_id_columns(nc, nl, nd),
        });
    }

    // The record of the engaged run's samples.
    if a.record {
        let dir = a.out.join("record");
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| refuse(format!("{}: {e}", dir.display())));
        let f8 = |name: &str, v: &[f64]| {
            let mut b = Vec::with_capacity(v.len() * 8);
            for x in v {
                b.extend_from_slice(&x.to_le_bytes());
            }
            std::fs::write(dir.join(name), b).unwrap_or_else(|e| refuse(format!("{name}: {e}")));
        };
        f8("drive.f8", &tap.drive);
        f8("inputs.f8", &tap.inputs);
        let mut s = serde_json::to_string(&rs).expect("the structure");
        s.push('\n');
        std::fs::write(dir.join("structure.json"), s)
            .unwrap_or_else(|e| refuse(format!("record/structure.json: {e}")));
        run["record"] = json!({"drive_frames": tap.drive.len() / nd.max(1),
            "input_frames": tap.inputs.len() / (nc + nl).max(1), "channels": nc + nl, "drives": nd});
    }
    drop(tap);

    // The clock.
    let mut calls: Vec<f64> = ticks.iter().filter(|t| t.law_called).map(|t| t.law_s).collect();
    let over = calls.iter().filter(|&&s| s > setup_budget_s).count();
    calls.sort_by(f64::total_cmp);
    let timing = json!({
        "identify_s": identify_s, "engage_s": engage_s, "run_s": run_s,
        "write_s": t_write.elapsed().as_secs_f64(), "total_s": t_start.elapsed().as_secs_f64(),
        "law_calls": calls.len(), "law_median_s": calls.get(calls.len() / 2).copied(),
        "law_slowest_s": calls.last().copied(), "law_over_setup_budget": over,
        "setup_law_budget_s": setup_budget_s,
        "test_s": ticks.last().map(|t| t.time_s),
        "law_s": pts.iter().map(|t| if t.law_called { json!(t.law_s) } else { Value::Null }).collect::<Vec<_>>(),
    });
    let code = if class == "completed" { 0 } else { 1 };
    eprintln!(
        "msnb_run: {} {} ({} updates, {} points) in {:.1} s",
        case.case_id,
        class,
        ticks.len(),
        pts.len(),
        t_start.elapsed().as_secs_f64()
    );
    write(&run, &timing, code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Value {
        json!({
            "schema": "msnb-case/1", "case_id": "t",
            "structure": {"file": "p/structure.json"},
            "law": {"id": "matrix"},
            "test": {"type": "dwell", "freq_hz": 100.0, "dwell_s": 2.0},
            "reference": {"file": "p/ref.json"}
        })
    }

    #[test]
    fn msnb_a_minimal_case_reads_with_its_defaults() {
        let c = parse_case(&base()).expect("a case");
        assert_eq!(c.fs_hz, 8192.0);
        assert_eq!(c.levels_db, vec![0.0]);
        assert_eq!(c.ident_seed, SERVICE_SYSID_SEED);
        assert!(!c.widened);
        assert_eq!(c.phase, PhaseMode::Auto);
    }

    #[test]
    fn msnb_the_case_refuses_what_it_does_not_know() {
        let mut v = base();
        v["extra"] = json!(1);
        assert!(parse_case(&v).unwrap_err().contains("unknown key"));
        let mut v = base();
        v["structure"]["file"] = json!("/abs/structure.json");
        assert!(parse_case(&v).is_err());
        let mut v = base();
        v["structure"]["file"] = json!("a/../b.json");
        assert!(parse_case(&v).is_err());
        let mut v = base();
        v["test"] = json!({"type": "sweep", "band_hz": [20, 2000], "rate_oct_per_min": 1, "direction": "sideways"});
        assert!(parse_case(&v).is_err());
        let mut v = base();
        v["test"] = json!({"type": "sweep", "band_hz": [2000, 20], "rate_oct_per_min": 1});
        assert!(parse_case(&v).is_err());
        let mut v = base();
        v["reference"] = json!({"file": "a", "inline": {}});
        assert!(parse_case(&v).is_err());
        let mut v = base();
        v["protective"] = json!("loose");
        assert!(parse_case(&v).is_err());
        let mut v = base();
        v["test"] = json!({"type": "sweep", "band_hz": [20, 2000], "rate_oct_per_min": 1, "direction": "down"});
        assert_eq!(
            parse_case(&v).unwrap().test,
            TestSpec::Sweep { band: (20.0, 2000.0), rate: 1.0, down: true }
        );
    }

    #[test]
    fn msnb_a_phase_wrap_is_split_the_short_way() {
        // 170 to -170 degrees: the short way is up through 180.
        let (p, n) = phase_points(&[100.0, 200.0], &[170.0, -170.0]).unwrap();
        assert_eq!(n, 1);
        assert_eq!(p.len(), 4);
        assert_eq!(p[1].phase_deg, 180.0);
        assert!(p[2].phase_deg > -180.0 && p[2].phase_deg < -179.9);
        // The crossing sits halfway in log frequency.
        assert!((p[1].freq_hz - (100.0f64 * 200.0).sqrt()).abs() < 1e-6);
        assert!(p.windows(2).all(|w| w[1].freq_hz > w[0].freq_hz));
        // The other way.
        let (p, n) = phase_points(&[100.0, 200.0], &[-170.0, 170.0]).unwrap();
        assert_eq!(n, 1);
        assert!(p[1].phase_deg < -179.9 && p[2].phase_deg == 180.0);
        // No wrap, no split.
        let (p, n) = phase_points(&[100.0, 200.0, 400.0], &[10.0, 100.0, -60.0]).unwrap();
        assert_eq!((n, p.len()), (0, 3));
    }

    #[test]
    fn msnb_sweep_points_are_one_a_bin_and_the_last() {
        let mut ticks = Vec::new();
        for i in 0..1000 {
            let mut t = TickRecord::empty(1, 1, 0);
            t.index = i;
            t.frequency_hz = 20.0 * 2f64.powf(i as f64 / 200.0); // 5 octaves
            ticks.push(t);
        }
        let p = sweep_points(&ticks, 20.0);
        assert_eq!(p.len(), 240 + 1);
        assert_eq!(p.last().unwrap().0, 999);
        assert_eq!(p.iter().map(|(s, e)| e - s).sum::<usize>(), 1000);
        // Down, the same bins.
        ticks.reverse();
        let q = sweep_points(&ticks, 20.0);
        assert!((q.len() as i64 - 241).abs() <= 1);
    }

    #[test]
    fn msnb_a_grid_must_cover_the_test() {
        assert!(covers(&[20.0, 2000.0], (20.0, 2000.0), "r").is_ok());
        assert!(covers(&[21.0, 2000.0], (20.0, 2000.0), "r").is_err());
        assert!(covers(&[20.0, 1999.0], (20.0, 2000.0), "r").is_err());
        assert!(covers(&[100.0, 200.0], (150.0, 150.0), "r").is_ok());
    }

    #[test]
    fn msnb_numbers_keep_nine_digits() {
        assert_eq!(n9(1.234567891234), json!(1.23456789));
        assert_eq!(n9(f64::NAN), Value::Null);
        assert_eq!(n9(0.0), json!(0.0));
    }
}
