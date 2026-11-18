//! The benchmark study's run program: run one built-in multiple-output Random law,
//! closed loop, through the real MIMO Random run service on the simulator, on a
//! structure given as data, and write the run folder a scorer needs. It is the program
//! the Hertzforge Journal's benchmark study of the six built-in laws used on the Open
//! Benchmark Set.
//!
//! ```text
//! cargo run --release -p sds-control --example x4_runner -- <case.json> --out <folder> [--overwrite]
//! ```
//!
//! It reads one case file (JSON) and writes one run folder. The case file holds
//! `schema` (`mm-x4-case/1` or `mm-x4-case/2`), `case_id`, `grid` (`fs_hz`, `nfft`,
//! `band_hz`), the structure (`structure`, or `structure_file` relative to the case
//! file), the `outputs` with their roles, the reference (`reference`, or
//! `reference_file`), and `run` (the law and its parameters, the service settings, the
//! level schedule, the loops recorded and the simulator's seed). The run folder holds
//! `manifest.json` (the case, the outcome, and every file written with its type, shape
//! and meaning), `loops.csv`, `events.txt`, the drive and response matrices as played
//! over the record window, the structure's small-signal frequency response, and an echo
//! of the case.
//!
//! To run it on a problem of the Open Benchmark Set: build the case's structure object
//! from the problem with `to_runner_structure` in the set's `code/reference_model.py`,
//! take the grid from `problem.json`, the reference from the problem's target files and
//! the run settings from `limits.json` (`runner.run`, and one of `runner.passes`), run
//! this program, then score the run folder against the problem with the set's
//! `code/scorer.py`.
//!
//! The service is built and walked the way `sds_control::mimo::law_check` does it
//! (`build`, `engage` and the loop of `run_plant` are private there, so their bodies are
//! repeated here over the public API). No law, estimator, cap or guard is re-implemented.
//! The only thing added is a tap on the backend (`Tee`) that copies the samples the
//! service reads and writes into a Welch estimate, so outputs the control never sees are
//! recorded as the structure played them.
//!
//! Case schema `mm-x4-case/2` adds one thing to version 1: a jointed structure's mode
//! may carry a `level` object, its own level rule (`drop`, `drop_exponent`, `rise`,
//! `rise_exponent`, `rise_cap`, `damping_gain`, `damping_exponent`, `damping_cap`, all
//! required), so one structure can mix softening and hardening modes. A mode without one
//! follows the structure's `joint` as in version 1. A version 1 case is read exactly as
//! before (a `level` object in it is refused). Version 2 also takes an optional `step` on
//! a jointed structure: `{"kind": "triangle_hold"}` (the default, as version 1) or
//! `{"kind": "equalized", "band_hz": 2000.0, "half_taps": 2}`, which matches the
//! continuous modal model at small signal and delays every path `half_taps` samples.
//!
//! Research options (2026-11-18, level-indexed model study), each absent by default so a
//! case without them runs as before:
//! - `run.identification.frf_file`: a raw little-endian complex128 file, band lines by
//!   controls by drives (the layout and units of `frf_identified.c16`). The noise floor
//!   and the identification run as always; then, at the gate, the identification's
//!   frequency response is replaced by the file's on the band lines
//!   (`MimoRandomService::replace_identification_frf`, which repeats the setup and size
//!   checks of `load_identification` and judges the gate again), so the gate and the
//!   law's first solve use it. The manifest's `frf_loaded` names the file (its folder
//!   and name only); `frf_identified.c16` stays the measured one and `frf_loaded.c16` is
//!   the one adopted.
//! - every run writes `frf_final.c16`: the frequency response the service hands the law
//!   when the run ends (`frf_law`), controls by drives as `frf_identified.c16` (this
//!   runner sets no transformation; were one set, the file would be
//!   `frf_final_law_space.c16` and say so).
//! - `run.stop_after_loops`: after that many control loops (loops.csv's `loop`), the run
//!   ends with an operator stop (`Stopped`), recorded in the manifest's
//!   `outcome.stopped_after_loops`; exit status 0 when nothing tripped.
//!
//! Exit status: 0 the run reached the end of its full-level loops (or, with `stop_after`
//! set to `seed`, made its first solve, or stopped cleanly at `stop_after_loops`); 1 it
//! tripped or stopped early (the folder is written); 2 the case was refused (no folder is
//! made).
#![recursion_limit = "512"]

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::{cola, fft, window};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_hal_sim::{
    Discretization, JointLaw, JointedMode, JointedPlant, JointedStep, LevelRule, ModalPlant,
    ModalSoftening, Mode, ModeResponse, PlantModel, SimConfig,
};
use hertzforge_mimo_random::{
    DriveSpec, FrfMode, KeptLawCall, MimoConfig, MimoError, MimoRandomService, MimoState,
    ReferenceSpec,
};
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::config::{LevelSchedule, RunMode};
use hertzforge_random::LoopConfig;
use hertzforge_random_laws::{builtin_mimo_manifest, BuiltinMimo, LawChoice};
use hertzforge_sdm::law::Tolerances;
use hertzforge_sdm::{linalg, HermitianStack, MatrixStack, C64};
use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const USAGE: &str = "usage: x4_runner <case.json> --out <folder> [--overwrite]\nexit status: 0 the run reached the end of its full-level loops (or, with stop_after seed, made its first solve); 1 it tripped or stopped early (the folder is written); 2 the case was refused (no folder is made)";
/// The code this program was built on. A build may set `HF_BUILD_COMMIT` to
/// `git rev-parse HEAD` at compile time, and `HF_BUILD_PARTS` to the commits it was
/// made from; without them the manifest says the build is unstated.
const COMMIT: &str = match option_env!("HF_BUILD_COMMIT") {
    Some(c) => c,
    None => "unstated build",
};
const PARTS: &str = match option_env!("HF_BUILD_PARTS") {
    Some(c) => c,
    None => "",
};

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("x4_runner: {msg}");
    std::process::exit(2);
}

// ---------------------------------------------------------------------------------------
// JSON helpers (strict: unknown keys are refused).
// ---------------------------------------------------------------------------------------

type Obj = Map<String, Value>;

fn obj<'a>(v: &'a Value, what: &str) -> &'a Obj {
    v.as_object()
        .unwrap_or_else(|| die(format!("{what} must be an object")))
}
fn only(o: &Obj, allowed: &[&str], what: &str) {
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            die(format!("{what}: unknown key \"{k}\""));
        }
    }
}
fn present<'a>(o: &'a Obj, k: &str) -> Option<&'a Value> {
    match o.get(k) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v),
    }
}
fn getf(o: &Obj, k: &str) -> Option<f64> {
    present(o, k).map(|v| {
        v.as_f64()
            .unwrap_or_else(|| die(format!("\"{k}\" must be a number")))
    })
}
fn needf(o: &Obj, k: &str, what: &str) -> f64 {
    getf(o, k).unwrap_or_else(|| die(format!("{what} needs \"{k}\"")))
}
fn getu(o: &Obj, k: &str) -> Option<u64> {
    present(o, k).map(|v| {
        v.as_u64()
            .unwrap_or_else(|| die(format!("\"{k}\" must be a whole number, zero or more")))
    })
}
fn needu(o: &Obj, k: &str, what: &str) -> usize {
    getu(o, k).unwrap_or_else(|| die(format!("{what} needs \"{k}\""))) as usize
}
fn gets<'a>(o: &'a Obj, k: &str) -> Option<&'a str> {
    present(o, k).map(|v| {
        v.as_str()
            .unwrap_or_else(|| die(format!("\"{k}\" must be text")))
    })
}
fn getb(o: &Obj, k: &str) -> Option<bool> {
    present(o, k).map(|v| {
        v.as_bool()
            .unwrap_or_else(|| die(format!("\"{k}\" must be true or false")))
    })
}
fn vecf(v: Option<&Value>, what: &str) -> Vec<f64> {
    match v {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => v
            .as_array()
            .unwrap_or_else(|| die(format!("{what} must be an array")))
            .iter()
            .map(|x| {
                x.as_f64()
                    .unwrap_or_else(|| die(format!("{what} holds a value that is not a number")))
            })
            .collect(),
    }
}
fn load_json(path: &Path) -> Value {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{}: {e}", path.display())));
    serde_json::from_str(&text).unwrap_or_else(|e| die(format!("{}: {e}", path.display())))
}

// ---------------------------------------------------------------------------------------
// The grid (as law_check's private Grid).
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Grid {
    fs_hz: f64,
    nfft: usize,
    band_hz: (f64, f64),
}

impl Grid {
    fn lines(&self) -> usize {
        self.nfft / 2 + 1
    }
    fn df(&self) -> f64 {
        self.fs_hz / self.nfft as f64
    }
    /// `(lo, hi)`, `hi` exclusive.
    fn band(&self) -> (usize, usize) {
        let df = self.df();
        (
            (self.band_hz.0 / df).round() as usize,
            (self.band_hz.1 / df).round() as usize + 1,
        )
    }
    fn loop_config(&self) -> LoopConfig {
        let mut c = LoopConfig::demo_default();
        c.fs_hz = self.fs_hz;
        c.nfft = self.nfft;
        c.band_lo_hz = self.band_hz.0;
        c.band_hi_hz = self.band_hz.1;
        c.block_frames = 256;
        c
    }
}

// ---------------------------------------------------------------------------------------
// The structure from data.
// ---------------------------------------------------------------------------------------

fn shapes(m: &Obj, n_d: usize, n_o: usize, i: usize) -> (Vec<f64>, Vec<f64>) {
    let ds = vecf(m.get("drive_shape"), "drive_shape");
    let os = vecf(m.get("output_shape"), "output_shape");
    if ds.len() != n_d || os.len() != n_o {
        die(format!(
            "mode {i}: drive_shape has {} values (want {n_d}) and output_shape {} (want {n_o})",
            ds.len(),
            os.len()
        ));
    }
    (ds, os)
}

/// A mode's own level rule (case schema version 2): every field is required.
fn level_rule(v: &Value, i: usize) -> LevelRule {
    let what = format!("mode {i}'s level");
    let l = obj(v, &what);
    only(
        l,
        &[
            "drop",
            "drop_exponent",
            "rise",
            "rise_exponent",
            "rise_cap",
            "damping_gain",
            "damping_exponent",
            "damping_cap",
        ],
        &what,
    );
    if present(l, "damping_cap").is_none() {
        die(format!(
            "{what} needs \"damping_cap\" (the amplitude ratio above which its damping stops \
             growing, required in a mode's own rule since 2026-11-14)"
        ));
    }
    LevelRule {
        drop: needf(l, "drop", &what),
        drop_exponent: needf(l, "drop_exponent", &what),
        rise: needf(l, "rise", &what),
        rise_exponent: needf(l, "rise_exponent", &what),
        rise_cap: needf(l, "rise_cap", &what),
        damping_gain: needf(l, "damping_gain", &what),
        damping_exponent: needf(l, "damping_exponent", &what),
        damping_cap: needf(l, "damping_cap", &what),
    }
}

fn build_plant(p: &Obj, out: &Path, version: u32) -> PlantModel {
    match gets(p, "kind").unwrap_or("") {
        "builtin" => {
            only(p, &["kind", "name"], "structure");
            let name = gets(p, "name").unwrap_or_else(|| die("a builtin structure needs \"name\""));
            StandardPlant::from_name(name)
                .unwrap_or_else(|| die(format!("no standard structure is named {name}")))
                .build()
                .model
        }
        "modal" => {
            only(
                p,
                &[
                    "kind",
                    "n_outputs",
                    "n_drives",
                    "modes",
                    "feedthrough",
                    "residual",
                    "transport_delay_samples",
                    "discretization",
                    "softening",
                    "noisy_outputs",
                ],
                "structure",
            );
            let n_o = needu(p, "n_outputs", "structure");
            let n_d = needu(p, "n_drives", "structure");
            let modes: Vec<Mode> = p
                .get("modes")
                .and_then(Value::as_array)
                .unwrap_or_else(|| die("the structure needs \"modes\""))
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let m = obj(m, "a mode");
                    only(
                        m,
                        &["f_hz", "q", "drive_shape", "output_shape", "response"],
                        "mode",
                    );
                    let (ds, os) = shapes(m, n_d, n_o, i);
                    let mut md = Mode::new(needf(m, "f_hz", "mode"), needf(m, "q", "mode"), ds, os);
                    md.response = match gets(m, "response").unwrap_or("bandpass") {
                        "bandpass" => ModeResponse::Bandpass,
                        "acceleration" => ModeResponse::Acceleration,
                        other => die(format!("mode {i}: response \"{other}\" is not known")),
                    };
                    md
                })
                .collect();
            let mut mp = ModalPlant::new(n_o, n_d, modes);
            mp.feedthrough = getf(p, "feedthrough").unwrap_or(0.0);
            mp.residual = vecf(p.get("residual"), "residual");
            mp.transport_delay_samples = getu(p, "transport_delay_samples").unwrap_or(0) as usize;
            mp.discretization = match present(p, "discretization") {
                None => Discretization::Biquad,
                Some(d) => {
                    let d = obj(d, "discretization");
                    only(d, &["kind", "band_hz", "taps", "delay"], "discretization");
                    match gets(d, "kind").unwrap_or("") {
                        "biquad" => Discretization::Biquad,
                        "fitted" => Discretization::Fitted {
                            taps: getu(d, "taps").unwrap_or(9) as usize,
                            delay: getu(d, "delay").unwrap_or(4) as usize,
                            band_hz: needf(d, "band_hz", "a fitted discretization"),
                        },
                        other => die(format!("discretization \"{other}\" is not known")),
                    }
                }
            };
            mp.softening = present(p, "softening").map(|s| {
                let s = obj(s, "softening");
                only(
                    s,
                    &[
                        "modes",
                        "reference_rms",
                        "fraction",
                        "smoothing",
                        "read_samples",
                    ],
                    "softening",
                );
                ModalSoftening {
                    modes: vecf(s.get("modes"), "softening.modes")
                        .into_iter()
                        .map(|x| x as usize)
                        .collect(),
                    reference_rms: vecf(s.get("reference_rms"), "softening.reference_rms"),
                    fraction: needf(s, "fraction", "softening"),
                    smoothing: getf(s, "smoothing").unwrap_or(0.2),
                    read_samples: getu(s, "read_samples").unwrap_or(4096) as usize,
                    log: Some(out.join("softening_log.csv")),
                }
            });
            mp.noisy_inputs = getu(p, "noisy_outputs").map(|n| n as usize);
            PlantModel::Modal(mp)
        }
        "jointed" => {
            only(
                p,
                &[
                    "kind",
                    "n_outputs",
                    "n_drives",
                    "modes",
                    "joint",
                    "feedthrough",
                    "residual",
                    "transport_delay_samples",
                    "output_delays_s",
                    "step",
                ],
                "structure",
            );
            if version < 2 && p.contains_key("step") {
                die("a jointed structure's \"step\" needs case schema mm-x4-case/2");
            }
            let n_o = needu(p, "n_outputs", "structure");
            let n_d = needu(p, "n_drives", "structure");
            let modes: Vec<JointedMode> = p
                .get("modes")
                .and_then(Value::as_array)
                .unwrap_or_else(|| die("the structure needs \"modes\""))
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let m = obj(m, "a mode");
                    let mut keys = vec![
                        "f_hz",
                        "zeta",
                        "drive_shape",
                        "output_shape",
                        "reference_amplitude",
                    ];
                    if version >= 2 {
                        keys.push("level");
                    }
                    only(m, &keys, "mode");
                    let (ds, os) = shapes(m, n_d, n_o, i);
                    JointedMode {
                        f_hz: needf(m, "f_hz", "mode"),
                        zeta: needf(m, "zeta", "mode"),
                        input_shape: ds,
                        output_shape: os,
                        reference_amplitude: needf(m, "reference_amplitude", "mode"),
                        level: present(m, "level").map(|v| level_rule(v, i)),
                    }
                })
                .collect();
            let mut jp = JointedPlant::new(n_o, n_d, modes);
            jp.joint = present(p, "joint").map(|j| {
                let j = obj(j, "joint");
                only(
                    j,
                    &[
                        "delta",
                        "damping_gain",
                        "freq_exponent",
                        "damping_exponent",
                        "decay_cycles",
                    ],
                    "joint",
                );
                let mut law = JointLaw::new(
                    needf(j, "delta", "joint"),
                    needf(j, "damping_gain", "joint"),
                );
                if let Some(v) = getf(j, "freq_exponent") {
                    law.freq_exponent = v;
                }
                if let Some(v) = getf(j, "damping_exponent") {
                    law.damping_exponent = v;
                }
                if let Some(v) = getf(j, "decay_cycles") {
                    law.decay_cycles = v;
                }
                law
            });
            jp.feedthrough = getf(p, "feedthrough").unwrap_or(0.0);
            jp.residual = vecf(p.get("residual"), "residual");
            jp.transport_delay_samples = getu(p, "transport_delay_samples").unwrap_or(0) as usize;
            let delays = vecf(p.get("output_delays_s"), "output_delays_s");
            if !delays.is_empty() {
                jp.output_delays_s = delays;
            }
            if let Some(st) = present(p, "step") {
                let st = obj(st, "step");
                only(st, &["kind", "band_hz", "half_taps"], "step");
                jp.step = match gets(st, "kind").unwrap_or("") {
                    "triangle_hold" => JointedStep::TriangleHold,
                    "equalized" => JointedStep::Equalized {
                        band_hz: needf(st, "band_hz", "step"),
                        half_taps: getu(st, "half_taps").unwrap_or(2) as usize,
                    },
                    other => die(format!(
                        "step kind \"{other}\" is not known (triangle_hold, equalized)"
                    )),
                };
            }
            PlantModel::Jointed(jp)
        }
        other => die(format!(
            "structure kind \"{other}\" is not known (modal, jointed, builtin)"
        )),
    }
}

// ---------------------------------------------------------------------------------------
// The check's own reference (law_check.rs reference_drive and well_driven, repeated).
// ---------------------------------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn sym(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        2.0 * ((self.0 >> 11) as f64 / (1u64 << 53) as f64) - 1.0
    }
}

fn reference_drive(n_d: usize, grid: &Grid, vrms: f64, coherent: f64) -> HermitianStack {
    let (lo, hi) = grid.band();
    let s = vrms * vrms / ((hi - lo) as f64 * grid.df());
    let mut rng = Rng(0xD21E ^ 0x9E37_79B9_7F4A_7C15);
    let a: Vec<C64> = (0..n_d * n_d)
        .map(|_| C64::new(rng.sym(), rng.sym()))
        .collect();
    let mut g = HermitianStack::zeros(grid.lines(), n_d);
    for line in lo..hi {
        for i in 0..n_d {
            for j in i..n_d {
                let mut acc = C64::new(0.0, 0.0);
                for k in 0..n_d {
                    acc += a[i * n_d + k] * a[j * n_d + k].conj();
                }
                let mut v = acc * (coherent / n_d as f64);
                if i == j {
                    v += C64::new(1.0 - coherent, 0.0);
                }
                g.set(line, i, j, v * s);
            }
        }
    }
    g
}

fn well_driven(
    h: &MatrixStack,
    x: HermitianStack,
    grid: &Grid,
    keep_ratio: f64,
    weak_weight: f64,
) -> Result<HermitianStack, String> {
    let svd = linalg::svd(h).map_err(|e| e.to_string())?;
    let (lines, n_d, k) = (h.lines(), h.cols(), svd.k());
    let (lo, hi) = grid.band();
    let mut p = MatrixStack::zeros(lines, n_d, n_d);
    for line in lo..hi {
        let s = &svd.s[line * k..(line + 1) * k];
        let w: Vec<f64> = (0..k)
            .map(|m| {
                if s[m] >= keep_ratio * s[0] {
                    1.0
                } else {
                    weak_weight.sqrt()
                }
            })
            .collect();
        for r in 0..n_d {
            for c in 0..n_d {
                let mut acc = C64::new(0.0, 0.0);
                for (m, wm) in w.iter().enumerate() {
                    acc += svd.v.get(line, r, m) * svd.v.get(line, c, m).conj() * *wm;
                }
                p.set(line, r, c, acc);
            }
        }
    }
    let mut y = linalg::congruence(&p, &x).map_err(|e| e.to_string())?;
    let before: f64 = linalg::trace(&x).iter().sum();
    let after: f64 = linalg::trace(&y).iter().sum();
    if after > 0.0 {
        y.scale_lines(&vec![before / after; lines])
            .map_err(|e| e.to_string())?;
    }
    Ok(y)
}

fn tolerances(
    r: &HermitianStack,
    band: (usize, usize),
    alarm_db: Option<f64>,
    abort_db: Option<f64>,
) -> Tolerances {
    let n = r.n();
    let mut t = Tolerances::none(r.lines(), n);
    for line in band.0..band.1 {
        for i in 0..n {
            let p = r.get(line, i, i).re;
            let k = line * n + i;
            if let Some(a) = alarm_db {
                t.alarm_upper[k] = p * 10f64.powf(a / 10.0);
                t.alarm_lower[k] = p * 10f64.powf(-a / 10.0);
            }
            if let Some(a) = abort_db {
                t.abort_upper[k] = p * 10f64.powf(a / 10.0);
                t.abort_lower[k] = p * 10f64.powf(-a / 10.0);
            }
        }
    }
    t
}

/// Rows `rows` of `h`.
fn select_rows(h: &MatrixStack, rows: &[usize]) -> MatrixStack {
    let (lines, _, n_d) = h.dims();
    let mut out = MatrixStack::zeros(lines, rows.len(), n_d);
    for l in 0..lines {
        for (r, &src) in rows.iter().enumerate() {
            for c in 0..n_d {
                out.set(l, r, c, h.get(l, src, c));
            }
        }
    }
    out
}

/// The same structure seen only at outputs `rows` (for the analytic FRF of the outputs that
/// are not bookkeeping monitors; hundreds of stress monitors would make it very large).
/// `None` for a structure kind whose rows cannot be cut here.
fn plant_rows(p: &PlantModel, rows: &[usize]) -> Option<PlantModel> {
    let cut = |v: &Vec<f64>| -> Vec<f64> { rows.iter().map(|&r| v[r]).collect() };
    match p {
        PlantModel::Modal(m) => {
            let mut c = m.clone();
            let n_d = c.n_drives;
            for mode in &mut c.modes {
                mode.output_shape = cut(&mode.output_shape);
            }
            if !c.residual.is_empty() {
                c.residual = rows
                    .iter()
                    .flat_map(|&r| m.residual[r * n_d..(r + 1) * n_d].to_vec())
                    .collect();
            }
            c.n_inputs = rows.len();
            c.noisy_inputs = None;
            c.softening = None;
            c.record = None;
            Some(PlantModel::Modal(c))
        }
        PlantModel::Jointed(j) => {
            let mut c = j.clone();
            let n_d = c.n_drives;
            for mode in &mut c.modes {
                mode.output_shape = cut(&mode.output_shape);
            }
            if !c.residual.is_empty() {
                c.residual = rows
                    .iter()
                    .flat_map(|&r| j.residual[r * n_d..(r + 1) * n_d].to_vec())
                    .collect();
            }
            if !c.output_delays_s.is_empty() {
                c.output_delays_s = cut(&c.output_delays_s);
            }
            c.n_inputs = rows.len();
            Some(PlantModel::Jointed(c))
        }
        _ => None,
    }
}

fn read_c16(path: &Path, want: usize) -> Vec<C64> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| die(format!("{}: {e}", path.display())));
    if bytes.len() != want * 16 {
        die(format!(
            "{} holds {} bytes; {} complex128 values ({} bytes) were expected",
            path.display(),
            bytes.len(),
            want,
            want * 16
        ));
    }
    bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|c| {
            C64::new(
                f64::from_le_bytes(c[..8].try_into().unwrap()),
                f64::from_le_bytes(c[8..].try_into().unwrap()),
            )
        })
        .collect()
}

fn build_reference(r: &Obj, base: &Path, grid: &Grid, h_controls: &MatrixStack) -> HermitianStack {
    let (lo, hi) = grid.band();
    let n_c = h_controls.rows();
    match gets(r, "kind").unwrap_or("") {
        "from_drive" => {
            only(
                r,
                &[
                    "kind",
                    "drive_vrms",
                    "coherent_share",
                    "keep_ratio",
                    "weak_weight",
                ],
                "reference",
            );
            let x = reference_drive(
                h_controls.cols(),
                grid,
                getf(r, "drive_vrms").unwrap_or(0.5),
                getf(r, "coherent_share").unwrap_or(0.4),
            );
            let x = well_driven(
                h_controls,
                x,
                grid,
                getf(r, "keep_ratio").unwrap_or(0.5),
                getf(r, "weak_weight").unwrap_or(0.1),
            )
            .unwrap_or_else(|e| die(e));
            linalg::response_from_drive(h_controls, &x).unwrap_or_else(|e| die(e))
        }
        "sdm" => {
            only(
                r,
                &["kind", "n", "lo_line", "n_lines", "file", "re", "im"],
                "reference",
            );
            let n = needu(r, "n", "reference");
            let lo_line = needu(r, "lo_line", "reference");
            let n_lines = needu(r, "n_lines", "reference");
            if n != n_c {
                die(format!(
                    "the reference is {n} by {n}; there are {n_c} controls"
                ));
            }
            if lo_line != lo || n_lines != hi - lo {
                die(format!(
                    "the reference covers lines {lo_line} to {} ({n_lines} lines); the grid's band is lines {lo} to {} ({} lines)",
                    lo_line + n_lines.max(1) - 1,
                    hi - 1,
                    hi - lo
                ));
            }
            let want = n_lines * n * n;
            let data: Vec<C64> = match gets(r, "file") {
                Some(f) => read_c16(&base.join(f), want),
                None => {
                    let re = vecf(r.get("re"), "reference.re");
                    let im = vecf(r.get("im"), "reference.im");
                    if re.len() != want || im.len() != want {
                        die(format!(
                            "an inline reference carries {want} real and {want} imaginary values; {} and {} were given",
                            re.len(),
                            im.len()
                        ));
                    }
                    re.iter().zip(&im).map(|(a, b)| C64::new(*a, *b)).collect()
                }
            };
            let mut g = HermitianStack::zeros(grid.lines(), n);
            for l in 0..n_lines {
                for i in 0..n {
                    let d = data[(l * n + i) * n + i];
                    if d.re.is_nan() || d.re < 0.0 || d.im.abs() > 1e-9 * d.re.abs().max(1e-300) {
                        die(format!(
                            "reference line {} control {i}: the diagonal is {d}, not real and non-negative",
                            lo + l
                        ));
                    }
                    for j in i..n {
                        g.set(lo + l, i, j, data[(l * n + i) * n + j]);
                    }
                }
            }
            g
        }
        other => die(format!(
            "reference kind \"{other}\" is not known (sdm, from_drive)"
        )),
    }
}

// ---------------------------------------------------------------------------------------
// The tap: a Welch estimate of the samples the backend is handed and hands back.
// ---------------------------------------------------------------------------------------

struct Welch {
    n: usize,
    nfft: usize,
    step: usize,
    lo: usize,
    nl: usize,
    win: Vec<f64>,
    s2: f64,
    conj: bool,
    /// `true`: the whole matrix is accumulated (`g`). `false`: autospectra only (`p`)
    /// and the one cross term between channels 0 and 1 (`x01`).
    full: bool,
    buf: Vec<Vec<f32>>,
    spec: Vec<C64>,
    /// `nl x n x n`, upper triangle accumulated (only when `full`).
    g: Vec<C64>,
    /// `nl x n`, autospectra accumulated (always).
    p: Vec<f64>,
    /// `nl`, the cross term of channels 0 and 1 accumulated (always, when `n > 1`).
    x01: Vec<C64>,
    /// Band moments of a block of outputs (autospectra mode only).
    mom: Option<Moments>,
    frames: usize,
}

/// Displacement band moments of a block of acceleration outputs, real symmetric:
/// `M0_ij = sum over band lines of Re(G_ij) df / (2 pi f)^4` and `M2_ij` the same with
/// `f^2` inside the sum. With `G` the played spectral matrix of the block, any linear
/// combination `z = t . a / (2 pi f)^2` of its outputs has `m0 = t M0 t^T` and
/// `m2 = t M2 t^T` exactly, so a stress built from modal accelerations needs only these
/// two matrices, not the full matrix on every line.
struct Moments {
    first: usize,
    count: usize,
    /// `sqrt(df / (2 pi f)^4)` per band line.
    w: Vec<f64>,
    /// `f` per band line.
    f: Vec<f64>,
    /// `count x 2 nl`: each output's weighted spectrum, real parts then imaginary parts.
    cols: Vec<f64>,
    /// The same times `f`.
    cols2: Vec<f64>,
    /// `count x count`, upper triangle accumulated.
    m0: Vec<f64>,
    m2: Vec<f64>,
}

fn dot8(a: &[f64], b: &[f64]) -> f64 {
    let mut acc = [0.0f64; 8];
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for k in 0..8 {
            acc[k] += x[k] * y[k];
        }
    }
    let mut s: f64 = acc.iter().sum();
    for (x, y) in ra.iter().zip(rb) {
        s += x * y;
    }
    s
}

impl Moments {
    fn new(first: usize, count: usize, lo: usize, nl: usize, df: f64) -> Moments {
        let f: Vec<f64> = (0..nl).map(|l| (lo + l) as f64 * df).collect();
        let w = f
            .iter()
            .map(|f| (df / (std::f64::consts::TAU * f.max(df)).powi(4)).sqrt())
            .collect();
        Moments {
            first,
            count,
            w,
            f,
            cols: vec![0.0; count * 2 * nl],
            cols2: vec![0.0; count * 2 * nl],
            m0: vec![0.0; count * count],
            m2: vec![0.0; count * count],
        }
    }
    #[inline]
    fn put(&mut self, c: usize, l: usize, re: f64, im: f64) {
        if c < self.first || c >= self.first + self.count {
            return;
        }
        let nl = self.w.len();
        let base = (c - self.first) * 2 * nl;
        let (w, f) = (self.w[l], self.f[l]);
        self.cols[base + l] = re * w;
        self.cols[base + nl + l] = im * w;
        self.cols2[base + l] = re * w * f;
        self.cols2[base + nl + l] = im * w * f;
    }
    fn accumulate(&mut self) {
        let n = self.count;
        let len = 2 * self.w.len();
        for i in 0..n {
            let (ai, bi) = (
                &self.cols[i * len..(i + 1) * len],
                &self.cols2[i * len..(i + 1) * len],
            );
            for j in i..n {
                self.m0[i * n + j] += dot8(ai, &self.cols[j * len..(j + 1) * len]);
                self.m2[i * n + j] += dot8(bi, &self.cols2[j * len..(j + 1) * len]);
            }
        }
    }
    /// `[M0, M2]`, each `count x count`, both triangles.
    fn finish(&self, scale: f64) -> Vec<f64> {
        let n = self.count;
        let mut out = vec![0.0; 2 * n * n];
        for (k, m) in [&self.m0, &self.m2].into_iter().enumerate() {
            for i in 0..n {
                for j in i..n {
                    let v = m[i * n + j] * scale;
                    out[k * n * n + i * n + j] = v;
                    out[k * n * n + j * n + i] = v;
                }
            }
        }
        out
    }
}

impl Welch {
    fn new(n: usize, nfft: usize, lo: usize, nl: usize, full: bool) -> Welch {
        let win = window::hann(nfft);
        let s2 = win.iter().map(|w| w * w).sum();
        Welch {
            n,
            nfft,
            step: nfft / 2,
            lo,
            nl,
            win,
            s2,
            conj: cola::forward_exponent_is_positive(),
            full,
            buf: vec![Vec::new(); n],
            spec: vec![C64::new(0.0, 0.0); if full { n * nl } else { 2 * nl }],
            g: vec![C64::new(0.0, 0.0); if full { nl * n * n } else { 0 }],
            p: vec![0.0; nl * n],
            x01: vec![C64::new(0.0, 0.0); nl],
            mom: None,
            frames: 0,
        }
    }
    fn clear_pending(&mut self) {
        for b in &mut self.buf {
            b.clear();
        }
    }
    fn push(&mut self, data: &[f32], frames: usize) {
        let n = self.n;
        for f in 0..frames {
            for c in 0..n {
                self.buf[c].push(data[f * n + c]);
            }
        }
        while self.buf[0].len() >= self.nfft {
            if self.full {
                self.frame();
            } else {
                self.frame_auto();
            }
            let step = self.step;
            for b in &mut self.buf {
                b.drain(..step);
            }
        }
    }
    /// One channel's windowed segment (mean removed) into `seg` as the real part.
    fn load(&self, c: usize, seg: &mut [(f64, f64)], imag: bool) {
        let s = &self.buf[c][..self.nfft];
        let mean = s.iter().map(|v| *v as f64).sum::<f64>() / self.nfft as f64;
        for i in 0..self.nfft {
            let v = (s[i] as f64 - mean) * self.win[i];
            if imag {
                seg[i].1 = v;
            } else {
                seg[i] = (v, 0.0);
            }
        }
    }
    fn frame(&mut self) {
        let (n, nfft, nl, lo) = (self.n, self.nfft, self.nl, self.lo);
        let mut seg = vec![(0.0f64, 0.0f64); nfft];
        for c in 0..n {
            self.load(c, &mut seg, false);
            fft::fft_in_place(&mut seg);
            for l in 0..nl {
                let (re, im) = seg[lo + l];
                self.spec[l * n + c] = if self.conj {
                    C64::new(re, -im)
                } else {
                    C64::new(re, im)
                };
            }
        }
        for l in 0..nl {
            let x = &self.spec[l * n..(l + 1) * n];
            let g = &mut self.g[l * n * n..(l + 1) * n * n];
            for i in 0..n {
                let xi = x[i];
                let row = &mut g[i * n..(i + 1) * n];
                for j in i..n {
                    row[j] += xi * x[j].conj();
                }
                self.p[l * n + i] += xi.norm_sqr();
            }
            if n > 1 {
                self.x01[l] += x[0] * x[1].conj();
            }
        }
        self.frames += 1;
    }
    /// Autospectra only. Channels 0 and 1 are transformed one at a time (their cross
    /// term is kept, for the sign check); the rest go two to a transform, one as the
    /// real part and one as the imaginary part: `X_a = (Z_k + conj(Z_-k)) / 2`,
    /// `X_b = (Z_k - conj(Z_-k)) / 2i`.
    fn frame_auto(&mut self) {
        let (n, nfft, nl, lo) = (self.n, self.nfft, self.nl, self.lo);
        let mut seg = vec![(0.0f64, 0.0f64); nfft];
        for c in 0..n.min(2) {
            self.load(c, &mut seg, false);
            fft::fft_in_place(&mut seg);
            for l in 0..nl {
                let (re, im) = seg[lo + l];
                let x = if self.conj {
                    C64::new(re, -im)
                } else {
                    C64::new(re, im)
                };
                self.spec[c * nl + l] = x;
                self.p[l * n + c] += x.norm_sqr();
                if let Some(m) = &mut self.mom {
                    m.put(c, l, re, im);
                }
            }
        }
        if n > 1 {
            for l in 0..nl {
                self.x01[l] += self.spec[l] * self.spec[nl + l].conj();
            }
        }
        let mut c = 2;
        while c < n {
            let pair = c + 1 < n;
            self.load(c, &mut seg, false);
            if pair {
                self.load(c + 1, &mut seg, true);
            }
            fft::fft_in_place(&mut seg);
            for l in 0..nl {
                let k = lo + l;
                let (zr, zi) = seg[k];
                if pair {
                    let (mr, mi) = seg[(nfft - k) % nfft];
                    // X_a = (Z_k + conj(Z_-k)) / 2, X_b = (Z_k - conj(Z_-k)) / 2i.
                    let (ar, ai) = (0.5 * (zr + mr), 0.5 * (zi - mi));
                    let (br, bi) = (0.5 * (zi + mi), 0.5 * (mr - zr));
                    self.p[l * n + c] += ar * ar + ai * ai;
                    self.p[l * n + c + 1] += br * br + bi * bi;
                    if let Some(m) = &mut self.mom {
                        m.put(c, l, ar, ai);
                        m.put(c + 1, l, br, bi);
                    }
                } else {
                    self.p[l * n + c] += zr * zr + zi * zi;
                    if let Some(m) = &mut self.mom {
                        m.put(c, l, zr, zi);
                    }
                }
            }
            c += 2;
        }
        if let Some(m) = &mut self.mom {
            m.accumulate();
        }
        self.frames += 1;
    }
    fn scale(&self, fs_hz: f64) -> f64 {
        2.0 / (fs_hz * self.s2 * self.frames.max(1) as f64)
    }
    /// The one-sided spectral density matrix, full (both triangles), `nl x n x n`.
    fn finish(&self, fs_hz: f64) -> Vec<C64> {
        let n = self.n;
        let sc = self.scale(fs_hz);
        let mut out = vec![C64::new(0.0, 0.0); self.g.len()];
        for l in 0..self.nl {
            for i in 0..n {
                for j in i..n {
                    let v = self.g[(l * n + i) * n + j] * sc;
                    out[(l * n + i) * n + j] = v;
                    out[(l * n + j) * n + i] = v.conj();
                }
            }
        }
        out
    }
    /// The one-sided autospectra, `nl x n`.
    fn finish_auto(&self, fs_hz: f64) -> Vec<f64> {
        let sc = self.scale(fs_hz);
        self.p.iter().map(|v| v * sc).collect()
    }
    /// The cross term of channels 0 and 1, `nl` (same scale).
    fn finish_x01(&self, fs_hz: f64) -> Vec<C64> {
        let sc = self.scale(fs_hz);
        self.x01.iter().map(|v| *v * sc).collect()
    }
}

struct Recorder {
    on: bool,
    y: Welch,
    d: Welch,
    total_in: u64,
    total_dr: u64,
    on_at_in: u64,
    on_at_dr: u64,
}

impl Recorder {
    fn switch_on(&mut self) {
        if !self.on {
            self.y.clear_pending();
            self.d.clear_pending();
            self.on_at_in = self.total_in;
            self.on_at_dr = self.total_dr;
            self.on = true;
        }
    }
}

struct Tee {
    inner: Box<dyn Backend>,
    rec: Arc<Mutex<Recorder>>,
    n_in: usize,
    n_d: usize,
}

impl Backend for Tee {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn take_warnings(&mut self) -> Vec<String> {
        self.inner.take_warnings()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let r = self.inner.read_input(out)?;
        if let InputRead::Frames(n) = &r {
            let n = *n;
            let mut rec = self.rec.lock().unwrap();
            rec.total_in += n as u64;
            if rec.on {
                rec.y.push(&out[..n * self.n_in], n);
            }
        }
        Ok(r)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let n = self.inner.write_drive(drive)?;
        let mut rec = self.rec.lock().unwrap();
        rec.total_dr += n as u64;
        if rec.on {
            rec.d.push(&drive[..n * self.n_d], n);
        }
        Ok(n)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        self.inner.input_overloads()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}

// ---------------------------------------------------------------------------------------
// Writing.
// ---------------------------------------------------------------------------------------

fn write_c16(path: &Path, data: &[C64]) {
    let mut bytes = Vec::with_capacity(data.len() * 16);
    for v in data {
        bytes.extend_from_slice(&v.re.to_le_bytes());
        bytes.extend_from_slice(&v.im.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| die(format!("{}: {e}", path.display())));
}
fn write_f8(path: &Path, data: &[f64]) {
    let mut bytes = Vec::with_capacity(data.len() * 8);
    for v in data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| die(format!("{}: {e}", path.display())));
}

/// The band lines of a Hermitian stack, full matrices. A stack on every grid line is
/// cut to the band; one already on the band's lines is taken whole.
fn herm_band(h: &HermitianStack, grid: &Grid) -> Option<Vec<C64>> {
    let (lo, hi) = grid.band();
    let first = if h.lines() == grid.lines() {
        lo
    } else if h.lines() == hi - lo {
        0
    } else {
        return None;
    };
    let n = h.n();
    let mut out = Vec::with_capacity((hi - lo) * n * n);
    for l in 0..hi - lo {
        for i in 0..n {
            for j in 0..n {
                out.push(h.get(first + l, i, j));
            }
        }
    }
    Some(out)
}
fn mat_band(h: &MatrixStack, grid: &Grid) -> Option<Vec<C64>> {
    let (lo, hi) = grid.band();
    let first = if h.lines() == grid.lines() {
        lo
    } else if h.lines() == hi - lo {
        0
    } else {
        return None;
    };
    let mut out = Vec::with_capacity((hi - lo) * h.rows() * h.cols());
    for l in 0..hi - lo {
        out.extend_from_slice(h.line(first + l));
    }
    Some(out)
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[i]
}

/// Walk the run from idle to engaged, overriding a failed gate or prediction
/// (law_check.rs `engage`).
fn engage(
    svc: &mut MimoRandomService,
    gate_passed: &mut Option<bool>,
    frf_load: Option<(&[C64], &str)>,
    band: (usize, usize),
    measured_frf: &mut Option<MatrixStack>,
) -> Result<(), MimoError> {
    const ACK: &str =
        "A benchmark run on the simulator; the gate is the structure's, not the law's.";
    svc.start()?;
    if svc.state() == MimoState::Feasibility {
        svc.override_with(ACK)?;
    }
    if svc.state() == MimoState::NoiseFloor {
        svc.run_noise_floor()?;
    }
    if svc.state() == MimoState::SysId {
        svc.run_identification()?;
    }
    // `identification.frf_file`: the measured identification's frequency response is
    // replaced, band lines only, before the gate and the law's first solve (the
    // service's own checks apply; the lines outside the band keep what was measured).
    if let (MimoState::Gate, Some((data, source))) = (svc.state(), frf_load) {
        if let Some(id) = svc.identification() {
            let measured = id.frf.clone();
            let mut h = measured.clone();
            let (_, rows, cols) = h.dims();
            for l in 0..band.1 - band.0 {
                for r in 0..rows {
                    for c in 0..cols {
                        h.set(band.0 + l, r, c, data[(l * rows + r) * cols + c]);
                    }
                }
            }
            *measured_frf = Some(measured);
            svc.replace_identification_frf(h, source)?;
        }
    }
    if svc.state() == MimoState::Gate {
        let passed = svc.gate().is_some_and(|g| g.passed);
        *gate_passed = Some(passed);
        if !passed {
            svc.override_with(ACK)?;
        }
        svc.proceed()?;
    }
    if svc.state() == MimoState::Predict {
        if let Err(MimoError::Blocked(_)) = svc.proceed() {
            svc.override_with(ACK)?;
            svc.proceed()?;
        }
    }
    Ok(())
}

fn main() {
    let t_start = Instant::now();
    let mut case_path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut overwrite = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = Some(args.next().unwrap_or_else(|| die(USAGE)).into()),
            "--overwrite" => overwrite = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ if case_path.is_none() => case_path = Some(PathBuf::from(&a)),
            _ => die(USAGE),
        }
    }
    let case_path = case_path.unwrap_or_else(|| die(USAGE));
    let out = out.unwrap_or_else(|| die(USAGE));
    if out.join("manifest.json").exists() && !overwrite {
        die(format!(
            "{} already holds a run; give --overwrite to replace it",
            out.display()
        ));
    }
    let base = case_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let case_v = load_json(&case_path);
    let case = obj(&case_v, "the case file");
    only(
        case,
        &[
            "schema",
            "case_id",
            "grid",
            "structure",
            "structure_file",
            "outputs",
            "reference",
            "reference_file",
            "run",
        ],
        "case",
    );
    let version: u32 = match gets(case, "schema") {
        Some("mm-x4-case/1") => 1,
        Some("mm-x4-case/2") => 2,
        _ => die("\"schema\" must be \"mm-x4-case/1\" or \"mm-x4-case/2\""),
    };
    let case_id = gets(case, "case_id").unwrap_or("case").to_owned();

    // Grid.
    let g = obj(
        case.get("grid")
            .unwrap_or_else(|| die("case needs \"grid\"")),
        "grid",
    );
    only(g, &["fs_hz", "nfft", "band_hz"], "grid");
    let band_hz = vecf(g.get("band_hz"), "grid.band_hz");
    if band_hz.len() != 2 {
        die("grid.band_hz is [lo, hi]");
    }
    let grid = Grid {
        fs_hz: needf(g, "fs_hz", "grid"),
        nfft: needu(g, "nfft", "grid"),
        band_hz: (band_hz[0], band_hz[1]),
    };
    if !grid.nfft.is_power_of_two() {
        die("grid.nfft must be a power of two");
    }
    let (lo, hi) = grid.band();
    let nl = hi - lo;

    // The structure.
    let plant_v = match gets(case, "structure_file") {
        Some(f) => load_json(&base.join(f)),
        None => case
            .get("structure")
            .cloned()
            .unwrap_or_else(|| die("case needs \"structure\" or \"structure_file\"")),
    };
    let plant = build_plant(obj(&plant_v, "structure"), &out, version);
    plant
        .validate(grid.fs_hz)
        .unwrap_or_else(|e| die(format!("the structure was refused: {e}")));
    let (n_o, n_d) = plant
        .dims()
        .unwrap_or_else(|| die("the structure has no size"));
    // A jointed structure's level rules as read, each mode's highest reachable frequency and
    // the bound it was checked against.
    let plant_level = match &plant {
        PlantModel::Jointed(jp) => json!({
            "joint": jp.joint.map(|j| json!({"delta": j.delta, "damping_gain": j.damping_gain,
                "freq_exponent": j.freq_exponent, "damping_exponent": j.damping_exponent,
                "decay_cycles": j.decay_cycles})),
            "decay_cycles": jp.joint.map_or(3.0, |j| j.decay_cycles),
            "modes_with_level": jp.modes.iter().filter(|m| m.level.is_some()).count(),
            "levels": jp.modes.iter().map(|m| m.level.map(|l| json!({"drop": l.drop,
                "drop_exponent": l.drop_exponent, "rise": l.rise, "rise_exponent": l.rise_exponent,
                "rise_cap": l.rise_cap, "damping_gain": l.damping_gain,
                "damping_exponent": l.damping_exponent, "damping_cap": l.damping_cap}))).collect::<Vec<_>>(),
            "highest_frequency_hz": jp.highest_frequencies_hz(),
            "step": format!("{:?}", jp.step),
            "step_delay_samples": jp.step_delay_samples(),
            "frequency_bound_hz": JointedPlant::frequency_bound_hz(grid.fs_hz),
        }),
        _ => Value::Null,
    };

    // Outputs and roles.
    let mut names: Vec<String> = Vec::new();
    let mut roles: Vec<String> = Vec::new();
    match present(case, "outputs") {
        None => {
            for i in 0..n_o {
                names.push(format!("o{i:02}"));
                roles.push("control".into());
            }
        }
        Some(v) => {
            let a = v
                .as_array()
                .unwrap_or_else(|| die("\"outputs\" must be an array"));
            if a.len() != n_o {
                die(format!(
                    "\"outputs\" has {} entries; the structure has {n_o} outputs",
                    a.len()
                ));
            }
            for (i, o) in a.iter().enumerate() {
                let o = obj(o, "an output");
                only(o, &["name", "role"], "output");
                names.push(gets(o, "name").map_or_else(|| format!("o{i:02}"), str::to_owned));
                let role = gets(o, "role").unwrap_or("control");
                if !["control", "held_out", "monitor"].contains(&role) {
                    die(format!("output {i}: role \"{role}\" is not known"));
                }
                roles.push(role.to_owned());
            }
        }
    }
    let control_rows: Vec<usize> = (0..n_o).filter(|&i| roles[i] == "control").collect();
    let n_c = control_rows.len();
    if n_c == 0 {
        die("no output has the role \"control\"");
    }

    // The structure's small-signal FRF: every output, or (when the case asks for autospectra
    // only, see `response_record`) every output that is not a bookkeeping monitor.
    let wants_autospectra = case
        .get("run")
        .and_then(|r| r.get("response_record"))
        .and_then(Value::as_str)
        == Some("autospectra");
    let mut analytic_rows: Vec<usize> = (0..n_o).collect();
    let mut plant_seen = None;
    if wants_autospectra {
        let rows: Vec<usize> = (0..n_o).filter(|&i| roles[i] != "monitor").collect();
        if rows.len() < n_o {
            if let Some(p) = plant_rows(&plant, &rows) {
                plant_seen = Some(p);
                analytic_rows = rows;
            }
        }
    }
    let n_a = analytic_rows.len();
    let h_all =
        plant_seen
            .as_ref()
            .unwrap_or(&plant)
            .analytic_frf(grid.lines(), grid.df(), grid.fs_hz);
    let control_in_analytic: Vec<usize> = control_rows
        .iter()
        .map(|c| analytic_rows.iter().position(|r| r == c).unwrap())
        .collect();
    let h_ctl = select_rows(&h_all, &control_in_analytic);

    // Reference.
    let ref_v = match gets(case, "reference_file") {
        Some(f) => load_json(&base.join(f)),
        None => case
            .get("reference")
            .cloned()
            .unwrap_or_else(|| die("case needs \"reference\" or \"reference_file\"")),
    };
    let r_sdm = build_reference(obj(&ref_v, "reference"), &base, &grid, &h_ctl);

    // Run options.
    let empty = Value::Object(Map::new());
    let run = obj(case.get("run").unwrap_or(&empty), "run");
    only(
        run,
        &[
            "law",
            "frf_mode",
            "levels_db",
            "loops_at_full",
            "record_loops",
            "frames_per_loop",
            "memory_loops",
            "window",
            "overlap",
            "identification",
            "noise_sigma",
            "noise_below_db",
            "drive_ceiling_vrms",
            "drive_pretest_max_vrms",
            "alarm_db",
            "abort_db",
            "sim_seed",
            "drive_seed",
            "budget_fraction",
            "keep_loop_matrices",
            "divergence_loops",
            "divergence_floor_db",
            "protections",
            "envelope_abort_lines",
            "overload_fraction",
            "settle_loops",
            "response_record",
            "moments",
            "keep_loop_drive",
            "stop_after",
            "stop_after_loops",
        ],
        "run",
    );
    // "stop_after_loops": the run ends with an operator stop (clean, `Stopped`) once
    // that many control loops have been played, counted as loops.csv's `loop` column,
    // whatever the level. Absent: no such stop.
    let stop_after_loops = getu(run, "stop_after_loops");
    if stop_after_loops == Some(0) {
        die("stop_after_loops must be 1 or more");
    }
    // "stop_after": "seed" ends the run once the law has made its first solve (the seed
    // call, on the identification FRF, before any loop): the identification and that
    // drive are written and nothing is played at test level.
    let seed_only = match gets(run, "stop_after") {
        None => false,
        Some("seed") => true,
        Some(other) => die(format!("stop_after \"{other}\" is not known (seed)")),
    };
    let keep_loop_drive = getb(run, "keep_loop_drive").unwrap_or(false);
    let moments_block = present(run, "moments").map(|m| {
        let m = obj(m, "run.moments");
        only(m, &["first", "count"], "run.moments");
        (
            needu(m, "first", "run.moments"),
            needu(m, "count", "run.moments"),
        )
    });
    let record_matrix = match gets(run, "response_record").unwrap_or("matrix") {
        "matrix" => true,
        "autospectra" => false,
        other => die(format!(
            "response_record \"{other}\" is not known (matrix, autospectra)"
        )),
    };
    if let Some((first, count)) = moments_block {
        if record_matrix {
            die("\"moments\" needs \"response_record\": \"autospectra\"");
        }
        if count == 0 || first + count > n_o {
            die(format!(
                "moments: outputs {first} to {} are not all outputs of the structure ({n_o})",
                first + count
            ));
        }
    }
    let law_o = obj(run.get("law").unwrap_or(&empty), "run.law");
    only(law_o, &["id", "params"], "run.law");
    let law_id = gets(law_o, "id").unwrap_or("pinv_closed").to_owned();
    let builtin = BuiltinMimo::from_id(&law_id)
        .unwrap_or_else(|| die(format!("\"{law_id}\" is not a built-in law")));
    let manifest = builtin_mimo_manifest(builtin);
    let given = law_o
        .get("params")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for k in given.keys() {
        if !manifest.law.params.iter().any(|s| &s.key == k) {
            die(format!(
                "law {law_id} declares no parameter \"{k}\" (see law_manifest.txt)"
            ));
        }
    }
    let params = sds_control::mimo::adapter::params_of(&manifest.law.params, &given);
    let frf_mode = match present(run, "frf_mode") {
        None => FrfMode::Hold,
        Some(v) => {
            let m = obj(v, "run.frf_mode");
            only(m, &["kind", "blend"], "run.frf_mode");
            match gets(m, "kind").unwrap_or("") {
                "hold" => FrfMode::Hold,
                "update" => FrfMode::Update {
                    blend: getf(m, "blend").unwrap_or(0.5),
                },
                other => die(format!("frf_mode \"{other}\" is not known (hold, update)")),
            }
        }
    };
    let levels = match present(run, "levels_db") {
        None => vec![-12.0, -9.0, -6.0, -3.0, 0.0],
        Some(v) => vecf(Some(v), "run.levels_db"),
    };
    let loops_at_full = getu(run, "loops_at_full").unwrap_or(40);
    let record_loops = getu(run, "record_loops").unwrap_or(20).min(loops_at_full);
    let alarm_db = match run.get("alarm_db") {
        None => Some(6.0),
        Some(Value::Null) => None,
        Some(_) => getf(run, "alarm_db"),
    };
    let abort_db = match run.get("abort_db") {
        None => Some(12.0),
        Some(Value::Null) => None,
        Some(_) => getf(run, "abort_db"),
    };
    let keep_loop_matrices = getb(run, "keep_loop_matrices").unwrap_or(false);

    // Noise (law_check.rs noise_of).
    let noise = match getf(run, "noise_below_db") {
        Some(below) => {
            let df = grid.df();
            let smallest = (0..n_c)
                .map(|c| {
                    (lo..hi)
                        .map(|l| r_sdm.get(l, c, c).re * df)
                        .sum::<f64>()
                        .sqrt()
                })
                .fold(f64::INFINITY, f64::min);
            let bw = nl as f64 * df;
            smallest * 10f64.powf(-below / 20.0) * (grid.fs_hz / (2.0 * bw)).sqrt()
        }
        None => getf(run, "noise_sigma").unwrap_or(1e-4),
    };

    // The channel table: every structure output an enabled input, the controls listed.
    let mut table = ChannelTable::new();
    for (i, role) in roles.iter().enumerate() {
        let mut c = Channel::new(1 + i as u32);
        c.role = if role == "control" {
            ChannelRole::Control
        } else {
            ChannelRole::Monitor
        };
        c.units = Units::AccelG;
        // A bookkeeping monitor (a stress or modal monitor, scaled far above the noise)
        // never latches the overload indicator.
        c.range_max = if role == "monitor" { 1.0e30 } else { 1.0e6 };
        table.channels.push(c);
    }
    // Drive channel ids start at 101 as before, or past the last input on a structure with
    // 100 or more outputs.
    let drive_id0 = if n_o < 100 { 101 } else { n_o as u32 + 1 };
    for k in 0..n_d {
        let mut d = Channel::new(drive_id0 + k as u32);
        d.role = ChannelRole::Drive;
        d.units = Units::VoltageV;
        table.channels.push(d);
    }

    // The configuration (law_check.rs build).
    let band = grid.band();
    let reference = ReferenceSpec {
        tolerances: tolerances(&r_sdm, band, alarm_db, abort_db),
        sdm: r_sdm,
        band,
        df_hz: grid.df(),
    };
    let ceiling = getf(run, "drive_ceiling_vrms").unwrap_or(5.0);
    let drives = (0..n_d)
        .map(|k| {
            let mut d = DriveSpec::new(drive_id0 + k as u32, ceiling);
            d.pretest_max_vrms = getf(run, "drive_pretest_max_vrms").unwrap_or(1.0);
            d
        })
        .collect();
    let controls: Vec<u32> = control_rows.iter().map(|&i| 1 + i as u32).collect();
    let mut cfg = MimoConfig::new(controls, drives, reference);
    cfg.law = LawChoice::new(law_id.clone(), params.clone());
    cfg.frf_mode = frf_mode;
    cfg.schedule = LevelSchedule {
        mode: RunMode::Auto,
        start_db: levels.first().copied().unwrap_or(0.0),
        table_db: levels.clone(),
        dwell_s: vec![0.0],
    };
    cfg.records.enabled = false;
    cfg.estimation.frames_per_loop = getu(run, "frames_per_loop").unwrap_or(32) as usize;
    cfg.estimation.memory_loops = getf(run, "memory_loops").unwrap_or(8.0);
    match gets(run, "window").unwrap_or("hann") {
        "hann" => {}
        "rectangular" => {
            cfg.estimation.window = hertzforge_mimo_random::config::EstimationWindow::Rectangular
        }
        other => die(format!("window \"{other}\" is not known")),
    }
    if let Some(v) = getf(run, "overlap") {
        cfg.estimation.overlap = v;
    }
    // "identification.frf_file": a raw little-endian complex128 file, band lines by
    // controls by drives, the layout of frf_identified.c16; read after the setup is known.
    let mut frf_file: Option<PathBuf> = None;
    if let Some(v) = present(run, "identification") {
        let i = obj(v, "run.identification");
        only(
            i,
            &[
                "frames",
                "noise_frames",
                "start_vrms",
                "below_reference_db",
                "frf_file",
            ],
            "run.identification",
        );
        if let Some(f) = gets(i, "frf_file") {
            frf_file = Some(base.join(f));
        }
        if let Some(x) = getu(i, "frames") {
            cfg.identification.frames = x as usize;
        }
        if let Some(x) = getu(i, "noise_frames") {
            cfg.identification.noise_frames = x as usize;
        }
        if let Some(x) = getf(i, "start_vrms") {
            cfg.identification.start_vrms = x;
        }
        if let Some(x) = getf(i, "below_reference_db") {
            cfg.identification.below_reference_db = x;
        }
    }
    if let Some(v) = getf(run, "budget_fraction") {
        cfg.law_guards.budget_fraction = v;
    }
    if let Some(v) = getu(run, "drive_seed") {
        cfg.seed = v;
    }
    // The service's divergence guard (product defaults 5 loops, 3 dB floor).
    // Absent keys keep the product defaults; mimo_study.rs opens them to 16 and 1000.
    // "protections": "default" leaves the product's run-time defaults
    // (config.rs Protections::default); "opened" sets the Protections block mimo_study.rs sets.
    // The single keys below are applied after the preset.
    let preset = gets(run, "protections").unwrap_or("default").to_owned();
    match preset.as_str() {
        "default" => {}
        "opened" => {
            cfg.protections.envelope_abort_lines = usize::MAX;
            cfg.protections.divergence_loops = 16;
            cfg.protections.divergence_floor_db = 1000.0;
            cfg.protections.overload_fraction = 1.0;
        }
        other => die(format!(
            "protections \"{other}\" is not known (default, opened)"
        )),
    }
    if let Some(v) = getu(run, "divergence_loops") {
        cfg.protections.divergence_loops = v as u32;
    }
    if let Some(v) = getf(run, "divergence_floor_db") {
        cfg.protections.divergence_floor_db = v;
    }
    if let Some(v) = getu(run, "envelope_abort_lines") {
        cfg.protections.envelope_abort_lines = v as usize;
    }
    if let Some(v) = getf(run, "overload_fraction") {
        cfg.protections.overload_fraction = v;
    }
    if let Some(v) = getu(run, "settle_loops") {
        cfg.protections.settle_loops = v as u32;
    }
    let protections = cfg.protections;
    let sim_seed = getu(run, "sim_seed").unwrap_or(0x51_4D_00);
    let est = cfg.estimation;
    let ident = cfg.identification;
    let budget_fraction = cfg.law_guards.budget_fraction;

    let mut y_welch = Welch::new(n_o, grid.nfft, lo, nl, record_matrix);
    if let Some((first, count)) = moments_block {
        y_welch.mom = Some(Moments::new(first, count, lo, nl, grid.df()));
    }
    let rec = Arc::new(Mutex::new(Recorder {
        on: false,
        y: y_welch,
        d: Welch::new(n_d, grid.nfft, lo, nl, true),
        total_in: 0,
        total_dr: 0,
        on_at_in: 0,
        on_at_dr: 0,
    }));
    // The loaded frequency response (the runner sets no limit channels, so its rows are
    // the controls).
    let frf_loaded: Option<(Vec<C64>, String)> = frf_file.as_ref().map(|p| {
        let data = read_c16(p, nl * n_c * n_d);
        if let Some(bad) = data
            .iter()
            .position(|v| !(v.re.is_finite() && v.im.is_finite()))
        {
            die(format!("{}: value {bad} is not finite", p.display()));
        }
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (data, name)
    });
    let frf_loaded_from = frf_file.as_ref().map(|p| {
        // The file name and its parent folder (the arm), never a home path.
        let parent = p
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if parent.is_empty() {
            name
        } else {
            format!("{parent}/{name}")
        }
    });
    // The case is accepted: only now is the run folder made (a refused case leaves none).
    std::fs::create_dir_all(&out).unwrap_or_else(|e| die(format!("{}: {e}", out.display())));
    let sim: Box<dyn Backend> = Box::new(
        SimConfig::new()
            .sample_rate_hz(grid.fs_hz)
            .plant(plant.clone())
            .noise_level(noise)
            .shared_clock(true)
            .seed(sim_seed)
            .build(),
    );
    let backend: Box<dyn Backend> = Box::new(Tee {
        inner: sim,
        rec: rec.clone(),
        n_in: n_o,
        n_d,
    });
    let scratch = out.join("_service_scratch");
    let _ = std::fs::create_dir_all(&scratch);
    let mut svc = MimoRandomService::new(
        cfg,
        table,
        grid.loop_config(),
        backend,
        StationClass::HertzforgeSim,
        &scratch,
    )
    .unwrap_or_else(|e| die(format!("the service refused the setup: {e}")));
    svc.keep_for_record(true);
    let loop_s = svc.loop_config().segment_seconds();

    std::fs::write(
        out.join("law_manifest.txt"),
        format!("{:#?}\n", manifest.law),
    )
    .ok();

    // Engage, then tick.
    let mut gate_passed: Option<bool> = None;
    let t_engage = Instant::now();
    let mut csv = String::from(
        "loop,state,level_db,at_full,wall_s,law_ms,estimate_ms,plant_ms,max_error_db,max_worst_line_db",
    );
    for c in 0..n_c {
        csv.push_str(&format!(",error_db_c{c}"));
    }
    for c in 0..n_c {
        csv.push_str(&format!(",worst_line_db_c{c}"));
    }
    for k in 0..n_d {
        csv.push_str(&format!(",drive_vrms_d{k}"));
    }
    csv.push('\n');
    let worst = |v: &[f64]| v.iter().fold(0.0f64, |a, b| a.max(b.abs()));
    let mut last_call: Option<KeptLawCall> = None;
    let mut full_ticks = 0u64;
    let mut first_converged: Option<u64> = None;
    let mut record_first_loop: Option<u64> = None;
    let mut engage_s = 0.0;
    let mut loop_wall: Vec<f64> = Vec::new();
    // The drive in force once the run is engaged and before any loop: the law's seed call
    // (its first solve, from the identification FRF and the reference, no measured
    // response fed back) after the service's output check and drive ceilings
    // (service.rs predict, prediction_from). Physical, full level.
    let mut first_drive: Option<Vec<C64>> = None;
    let mut measured_frf: Option<MatrixStack> = None;
    let mut stopped_after_loops = false;
    let frf_load_src = frf_loaded_from.clone().unwrap_or_default();
    let outcome = engage(
        &mut svc,
        &mut gate_passed,
        frf_loaded
            .as_ref()
            .map(|(d, _)| (d.as_slice(), frf_load_src.as_str())),
        (lo, hi),
        &mut measured_frf,
    )
    .and_then(|()| {
        engage_s = t_engage.elapsed().as_secs_f64();
        first_drive = herm_band(svc.drive_full_level(), &grid);
        if seed_only {
            return Ok(());
        }
        let ladder = levels.len() as u64;
        while svc.state().is_running() {
            if svc.state() == MimoState::FullLevel
                && full_ticks + record_loops >= loops_at_full
                && record_first_loop.is_none()
            {
                rec.lock().unwrap().switch_on();
                record_first_loop = Some(svc.loop_count() + 1);
            }
            let t = Instant::now();
            svc.loop_tick()?;
            let wall = t.elapsed().as_secs_f64();
            loop_wall.push(wall);
            let at_full = svc.state() == MimoState::FullLevel;
            let timing = svc.timing();
            csv.push_str(&format!(
                "{},{:?},{:.3},{},{:.4},{:.3},{:.3},{:.3},{:.4},{:.4}",
                svc.loop_count(),
                svc.state(),
                svc.level_db(),
                at_full as u8,
                wall,
                timing.law_ms,
                timing.estimate_ms,
                timing.plant_ms,
                worst(svc.error_db()),
                worst(svc.worst_error_db()),
            ));
            // Every row has the header's length: a loop with no judged error yet (the
            // first, state Engaged) writes "nan" in the per-control columns.
            let pad = |csv: &mut String, v: &[f64], n: usize, sci: bool| {
                for k in 0..n {
                    match v.get(k) {
                        Some(x) if sci => csv.push_str(&format!(",{x:.6e}")),
                        Some(x) => csv.push_str(&format!(",{x:.4}")),
                        None => csv.push_str(",nan"),
                    }
                }
            };
            pad(&mut csv, svc.error_db(), n_c, false);
            pad(&mut csv, svc.worst_error_db(), n_c, false);
            pad(&mut csv, svc.drive_rms(), n_d, true);
            csv.push('\n');
            let mut kept = svc.take_kept();
            if let Some(c) = kept.law_calls.pop() {
                last_call = Some(c);
            }
            if at_full {
                full_ticks += 1;
                if first_converged.is_none() && full_ticks > 1 && worst(svc.error_db()) <= 1.0 {
                    first_converged = Some(full_ticks);
                }
                if keep_loop_matrices || keep_loop_drive {
                    if let Some(l) = kept.loops.pop() {
                        if let Some(d) = herm_band(&l.drive_full, &grid) {
                            write_c16(&out.join(format!("loop_drive_sdm_{full_ticks:03}.c16")), &d);
                        }
                        if !keep_loop_matrices {
                            // keep_loop_drive: the drive matrix only.
                        } else if let Some(r) =
                            l.response_full.as_ref().and_then(|r| herm_band(r, &grid))
                        {
                            write_c16(
                                &out.join(format!("loop_response_sdm_{full_ticks:03}.c16")),
                                &r,
                            );
                        }
                    }
                }
                if full_ticks >= loops_at_full {
                    break;
                }
            }
            if let Some(n) = stop_after_loops {
                if svc.loop_count() >= n && svc.state().is_running() {
                    svc.stop();
                    stopped_after_loops = true;
                    break;
                }
            }
            if svc.loop_count() > ladder + loops_at_full + 20 {
                break;
            }
        }
        Ok(())
    });
    rec.lock().unwrap().on = false;
    let run_error = outcome.err().map(|e| e.to_string());
    let trip = svc.trip().map(|t| t.kind.words());
    let completed = run_error.is_none() && trip.is_none() && full_ticks >= loops_at_full;
    let clean_stop = stopped_after_loops && run_error.is_none() && trip.is_none();
    // What tripped, when and on which control or drive (the service's own record, whole).
    let trip_record = svc.trip().map(|t| {
        let kind = serde_json::to_value(&t.kind).unwrap_or(Value::Null);
        // The kind is `{"<name>": {fields}}` or a tagged object; find its name and fields.
        let (name, fields) = match &kind {
            Value::Object(o) if o.len() == 1 && o.values().next().is_some_and(Value::is_object) => {
                let (k, v) = o.iter().next().unwrap();
                (k.clone(), v.clone())
            }
            other => (
                format!("{:?}", t.kind)
                    .split(|c: char| !c.is_alphanumeric())
                    .next()
                    .unwrap_or("")
                    .to_owned(),
                other.clone(),
            ),
        };
        let control = fields.get("control").and_then(Value::as_u64);
        let drive = fields.get("drive").and_then(Value::as_u64);
        json!({
            "kind": name,
            "fields": fields,
            "debug": format!("{:?}", t.kind),
            "words": t.kind.words(),
            "control": control,
            "control_name": control.and_then(|c| control_rows.get(c as usize)).map(|&r| names[r].clone()),
            "drive": drive,
            "at_loop": t.at_loop,
            "at_level_db": t.at_level_db,
            "at_full_level": t.at_level_db >= levels.last().copied().unwrap_or(0.0) - 1e-9,
            "full_level_loops_done": full_ticks,
            "full_level_loops_asked": loops_at_full,
            "state_after": format!("{:?}", svc.state()),
        })
    });

    // Arrays.
    let mut arrays = Vec::new();
    let mut add = |file: &str, dtype: &str, shape: Vec<usize>, what: &str| {
        arrays.push(json!({"file": file, "dtype": dtype, "shape": shape, "what": what}));
    };
    let freqs: Vec<f64> = (lo..hi).map(|l| l as f64 * grid.df()).collect();
    write_f8(&out.join("freqs_hz.f8"), &freqs);
    add("freqs_hz.f8", "<f8", vec![nl], "band line frequencies, Hz");
    if let Some(d) = herm_band(svc.reference(), &grid) {
        write_c16(&out.join("reference.c16"), &d);
        add(
            "reference.c16",
            "<c16",
            vec![nl, n_c, n_c],
            "the reference as the service holds it",
        );
    }
    let drive_cmd = herm_band(svc.drive_full_level(), &grid);
    if let Some(d) = &drive_cmd {
        write_c16(&out.join("drive_sdm_commanded.c16"), d);
        add(
            "drive_sdm_commanded.c16",
            "<c16",
            vec![nl, n_d, n_d],
            "the service's drive in force at the end, physical, full level, V^2/Hz",
        );
    }
    if let Some(d) = &first_drive {
        write_c16(&out.join("drive_sdm_first.c16"), d);
        add(
            "drive_sdm_first.c16",
            "<c16",
            vec![nl, n_d, n_d],
            "the drive in force after the law's seed call (its first solve), before any loop: physical, full level, V^2/Hz",
        );
    }
    let resp_svc = svc.response_full_level().and_then(|r| herm_band(r, &grid));
    if let Some(d) = &resp_svc {
        write_c16(&out.join("response_sdm_service.c16"), d);
        add(
            "response_sdm_service.c16",
            "<c16",
            vec![nl, n_c, n_c],
            "the service's control response estimate, last judged loop, full level",
        );
    }
    if let Some(id) = svc.identification() {
        // With identification.frf_file, the FRF the identification measured (before the
        // file's replaced it); frf_loaded.c16 is the one in force from the gate.
        let measured = measured_frf.as_ref().unwrap_or(&id.frf);
        if let Some(d) = mat_band(measured, &grid) {
            write_c16(&out.join("frf_identified.c16"), &d);
            add(
                "frf_identified.c16",
                "<c16",
                vec![nl, measured.rows(), measured.cols()],
                "the identification FRF, controls by drives",
            );
        }
        if measured_frf.is_some() {
            if let Some(d) = mat_band(&id.frf, &grid) {
                write_c16(&out.join("frf_loaded.c16"), &d);
                add(
                    "frf_loaded.c16",
                    "<c16",
                    vec![nl, id.frf.rows(), id.frf.cols()],
                    "the FRF loaded from identification.frf_file and adopted at the gate (it replaced the measured one), controls by drives",
                );
            }
        }
        write_f8(&out.join("coherence_identified.f8"), &id.coherence);
        add(
            "coherence_identified.f8",
            "<f8",
            vec![id.coherence.len()],
            "identification multiple coherence as the service stores it (length lines x controls, line-major, when it equals that product)",
        );
    }
    if let Some(c) = &last_call {
        if let Some(d) = mat_band(&c.frf, &grid) {
            write_c16(&out.join("frf_last.c16"), &d);
            add(
                "frf_last.c16",
                "<c16",
                vec![nl, c.frf.rows(), c.frf.cols()],
                "the FRF handed to the law on its last call",
            );
        }
    }
    // The FRF the law has in force at the end (the service's own, after any update).
    let frf_final_physical = svc.law_space_is_physical();
    if let Some(f) = svc.frf_law() {
        if let Some(d) = mat_band(f, &grid) {
            let (file, what) = if frf_final_physical {
                ("frf_final.c16", "the FRF the service hands the law at the end of the run (its frf_law, after any update), controls by drives, same units as frf_identified.c16")
            } else {
                ("frf_final_law_space.c16", "the FRF the service hands the law at the end of the run, in the law's transformed space (NOT controls by drives)")
            };
            write_c16(&out.join(file), &d);
            add(file, "<c16", vec![nl, f.rows(), f.cols()], what);
        }
    }
    let h_band = mat_band(&h_all, &grid).unwrap_or_default();
    write_c16(&out.join("frf_plant_analytic.c16"), &h_band);
    add(
        "frf_plant_analytic.c16",
        "<c16",
        vec![nl, n_a, n_d],
        "the structure's small-signal analytic FRF, outputs (manifest analytic_rows) by drives",
    );

    // What was played.
    let r = rec.lock().unwrap();
    let pyy = r.y.finish_auto(grid.fs_hz);
    let x01 = r.y.finish_x01(grid.fs_hz);
    let sdd = r.d.finish(grid.fs_hz);
    if record_matrix {
        let syy = r.y.finish(grid.fs_hz);
        write_c16(&out.join("response_sdm_played.c16"), &syy);
        add(
            "response_sdm_played.c16",
            "<c16",
            vec![nl, n_o, n_o],
            "Welch estimate of every structure output as read back, record window",
        );
    }
    write_f8(&out.join("response_psd_played.f8"), &pyy);
    add(
        "response_psd_played.f8",
        "<f8",
        vec![nl, n_o],
        "Welch autospectrum of every structure output as read back, record window (the diagonal of the played response matrix)",
    );
    if let Some(m) = &r.y.mom {
        write_f8(
            &out.join("response_moments_played.f8"),
            &m.finish(r.y.scale(grid.fs_hz)),
        );
        add(
            "response_moments_played.f8",
            "<f8",
            vec![2, m.count, m.count],
            "displacement band moments of the outputs of run.moments: [M0, M2], M0_ij = sum over band lines of Re(G_ij) df / (2 pi f)^4, M2 with f^2 inside the sum; record window",
        );
    }
    write_c16(&out.join("drive_sdm_played.c16"), &sdd);
    add(
        "drive_sdm_played.c16",
        "<c16",
        vec![nl, n_d, n_d],
        "Welch estimate of the drive samples handed to the backend, record window",
    );
    let loops_recorded =
        record_first_loop.map_or(0, |f| (loop_wall.len() as u64 + 1).saturating_sub(f));
    let window_status = if r.d.frames == 0 || r.y.frames == 0 {
        "empty"
    } else if completed {
        "complete"
    } else {
        "partial"
    };
    let window = json!({
        "status": window_status,
        "frames_response": r.y.frames, "frames_drive": r.d.frames,
        "first_loop": record_first_loop, "loops": record_loops,
        "loops_recorded": loops_recorded,
        "input_samples_before_window": r.on_at_in, "drive_samples_before_window": r.on_at_dr,
        "input_samples_total": r.total_in, "drive_samples_total": r.total_dr,
        "response_record": if record_matrix { "matrix" } else { "autospectra" },
        "moments": moments_block.map(|(first, count)| json!({"first": first, "count": count})),
        "estimate": "Hann, nfft points, 50 percent overlap, segment mean removed, one-sided, G_ij = E[X_i conj(X_j)] in the analysis convention",
    });
    std::fs::write(
        out.join("record_window.json"),
        serde_json::to_string_pretty(&window).unwrap(),
    )
    .ok();

    // Consistency: the played drive through the structure's analytic FRF against the played
    // response, on the autospectrum of every output the analytic FRF covers, and the sign
    // of one cross term (outputs 0 and 1, when the FRF covers both).
    let mut ratios: Vec<f64> = Vec::new();
    // The same with the simulator's measurement noise added to the prediction (a law that
    // leaves lines almost undriven, min_drive at its defaults, reads sensor noise there).
    let mut ratios_n: Vec<f64> = Vec::new();
    let noise_psd = 2.0 * noise * noise / grid.fs_hz;
    let noisy_rows = match &plant {
        PlantModel::Modal(m) => m.noisy_inputs.unwrap_or(n_o).min(n_o),
        _ => n_o,
    };
    let mut per_out_sum = vec![0.0f64; n_a];
    let (mut same, mut conj) = (0.0f64, 0.0f64);
    let cross_ok = n_a > 1 && analytic_rows[0] == 0 && analytic_rows[1] == 1;
    if r.y.frames > 0 && r.d.frames > 0 {
        let mut hs = vec![C64::new(0.0, 0.0); n_a * n_d];
        for l in 0..nl {
            let h = &h_band[l * n_a * n_d..(l + 1) * n_a * n_d];
            let s = &sdd[l * n_d * n_d..(l + 1) * n_d * n_d];
            for i in 0..n_a {
                for b in 0..n_d {
                    let mut acc = C64::new(0.0, 0.0);
                    for a in 0..n_d {
                        acc += h[i * n_d + a] * s[a * n_d + b];
                    }
                    hs[i * n_d + b] = acc;
                }
            }
            for i in 0..n_a {
                let mut p = C64::new(0.0, 0.0);
                for b in 0..n_d {
                    p += hs[i * n_d + b] * h[i * n_d + b].conj();
                }
                let meas = pyy[l * n_o + analytic_rows[i]];
                if p.re > 0.0 && meas > 0.0 {
                    let db = 10.0 * (meas / p.re).log10();
                    ratios.push(db);
                    per_out_sum[i] += db;
                    let floor = if analytic_rows[i] < noisy_rows {
                        noise_psd
                    } else {
                        0.0
                    };
                    ratios_n.push((10.0 * (meas / (p.re + floor)).log10()).abs());
                }
            }
            if cross_ok {
                let mut p = C64::new(0.0, 0.0);
                for b in 0..n_d {
                    p += hs[b] * h[n_d + b].conj();
                }
                let meas = x01[l];
                same += (meas * p.conj()).re;
                conj += (meas * p).re;
            }
        }
    }
    let mut abs: Vec<f64> = ratios.iter().map(|v| v.abs()).collect();
    abs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ratios_n.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let per_out_mean: Vec<f64> = per_out_sum.iter().map(|s| s / nl as f64).collect();
    // The service's own control estimate against the played one (a scale check).
    let mut scale: Vec<f64> = Vec::new();
    if let Some(rs) = &resp_svc {
        if r.y.frames > 0 {
            for l in 0..nl {
                for (c, &row) in control_rows.iter().enumerate() {
                    let a = pyy[l * n_o + row];
                    let b = rs[(l * n_c + c) * n_c + c].re;
                    if a > 0.0 && b > 0.0 {
                        scale.push(10.0 * (a / b).log10());
                    }
                }
            }
        }
    }
    scale.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (frames_y, frames_d) = (r.y.frames, r.d.frames);
    drop(r);

    let events: Vec<String> = svc
        .log()
        .events()
        .iter()
        .map(|e| format!("{} {:?}", e.seq, e.kind))
        .collect();
    std::fs::write(out.join("events.txt"), events.join("\n") + "\n").ok();
    std::fs::write(out.join("loops.csv"), &csv).ok();
    let stats = svc.law_stats();
    let final_error = worst(svc.error_db());
    let final_worst_line = worst(svc.worst_error_db());
    let gate = svc.gate().map(|g| {
        json!({"passed": g.passed, "worst_condition": g.worst_condition,
               "low_coherence_controls": g.low_coherence, "ill_conditioned_lines": g.ill_conditioned_lines.len()})
    });
    let ident_j = svc.identification().map(|i| {
        json!({"frames": i.frames, "steps": i.steps, "level_db": i.level_db,
               "drive_vrms": i.drive_vrms, "lag_frames": i.lag_frames})
    });
    if svc.state().is_running() {
        svc.stop();
    }
    drop(svc);
    let _ = std::fs::remove_dir_all(&scratch);

    let mut lw = loop_wall.clone();
    lw.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let manifest_j = json!({
        "schema": "mm-x4-run/1",
        "case_id": case_id,
        "wall_s": t_start.elapsed().as_secs_f64(),
        "code_commit": COMMIT,
        "code_parts": PARTS,
        "grid": {"fs_hz": grid.fs_hz, "nfft": grid.nfft, "df_hz": grid.df(),
                 "lo_line": lo, "n_lines": nl, "band_hz": [grid.band_hz.0, grid.band_hz.1]},
        "case_schema": format!("mm-x4-case/{version}"),
        "plant_kind": plant.kind(),
        "plant_level": plant_level,
        "n_outputs": n_o, "n_drives": n_d, "n_controls": n_c,
        "outputs": names.iter().zip(&roles).map(|(n, r)| json!({"name": n, "role": r})).collect::<Vec<_>>(),
        "control_rows": control_rows,
        "analytic_rows": analytic_rows,
        "response_record": if record_matrix { "matrix" } else { "autospectra" },
        "moments": moments_block.map(|(first, count)| json!({"first": first, "count": count})),
        "law": {"id": law_id, "params_given": given, "params_resolved": format!("{params:?}")},
        "frf_mode": format!("{frf_mode:?}"),
        "frf_loaded": frf_loaded_from.as_ref().map(|f| json!({"loaded": true, "from": f, "adopted_at": "the gate, after the measured identification"})),
        "levels_db": levels,
        "loops_at_full_asked": loops_at_full,
        "estimation": {"frames_per_loop": est.frames_per_loop, "memory_loops": est.memory_loops,
                       "window": format!("{:?}", est.window), "overlap": est.overlap},
        "identification_setup": {"frames": ident.frames, "noise_frames": ident.noise_frames,
                                 "start_vrms": ident.start_vrms, "below_reference_db": ident.below_reference_db},
        "identification": ident_j,
        "noise_sigma": noise, "sim_seed": sim_seed,
        "alarm_db": alarm_db, "abort_db": abort_db,
        "divergence_loops": protections.divergence_loops, "divergence_floor_db": protections.divergence_floor_db,
        "protections": {
            "preset": preset,
            "envelope_abort_lines": protections.envelope_abort_lines as u64,
            "divergence_loops": protections.divergence_loops,
            "divergence_floor_db": protections.divergence_floor_db,
            "overload_fraction": protections.overload_fraction,
            "settle_loops": protections.settle_loops,
        },
        "outcome": {
            "completed": completed,
            "trip": trip,
            "stopped_after_seed": seed_only && run_error.is_none() && trip.is_none(),
            "stopped_after_loops": if stopped_after_loops { json!({"operator_stop": true, "loops": stop_after_loops, "clean": clean_stop}) } else { Value::Null },
            "trip_record": trip_record,
            "record_window_status": window_status,
            "record_frames_drive": frames_d,
            "record_frames_response": frames_y,
            "record_loops_recorded": loops_recorded,
            "record_loops_asked": record_loops,
            "run_error": run_error,
            "gate": gate,
            "gate_overridden": gate_passed == Some(false),
            "loops_total": loop_wall.len(),
            "loops_at_full": full_ticks,
            "first_full_loop_within_1_db": first_converged,
            "final_max_error_db": final_error,
            "final_max_worst_line_db": final_worst_line,
            "law_calls": stats.calls,
            "repaired_lines": stats.repaired_lines,
            "capped_drive_lines": stats.capped_drive_lines,
        },
        "timing": {
            "wall_total_s": t_start.elapsed().as_secs_f64(),
            "wall_engage_s": engage_s,
            "wall_loop_median_s": quantile(&lw, 0.5),
            "wall_loop_max_s": lw.last().copied(),
            "simulated_loop_s": loop_s,
            "law_budget_ms": budget_fraction * loop_s * 1e3,
        },
        "consistency": {
            "what": "10 log10( played response autospectrum / (H_analytic S_dd_played H^H) ), every output and band line",
            "values": ratios.len(),
            "median_db": quantile(&ratios, 0.5),
            "p05_db": quantile(&ratios, 0.05),
            "p95_db": quantile(&ratios, 0.95),
            "p95_abs_db": quantile(&abs, 0.95),
            "max_abs_db": abs.last().copied(),
            "p50_abs_db_noise_in_prediction": quantile(&ratios_n, 0.5),
            "p95_abs_db_noise_in_prediction": quantile(&ratios_n, 0.95),
            "mean_db_per_output": per_out_mean,
            "cross_term_sign": if !cross_ok || (same == 0.0 && conj == 0.0) { "not_checked" } else if same >= conj { "as_analytic" } else { "conjugate_of_analytic" },
        },
        "scale_check": {
            "what": "10 log10( played control autospectrum / service control response estimate ), band lines",
            "median_db": quantile(&scale, 0.5),
            "p05_db": quantile(&scale, 0.05),
            "p95_db": quantile(&scale, 0.95),
        },
        "arrays": arrays,
    });
    let mut f = std::fs::File::create(out.join("manifest.json"))
        .unwrap_or_else(|e| die(format!("manifest.json: {e}")));
    f.write_all(
        serde_json::to_string_pretty(&manifest_j)
            .unwrap()
            .as_bytes(),
    )
    .ok();
    std::fs::write(
        out.join("case_echo.json"),
        serde_json::to_string_pretty(
            &json!({"case": case_v, "structure": plant_v, "reference": ref_v}),
        )
        .unwrap(),
    )
    .ok();
    println!(
        "{case_id}: {} law {law_id}, {} loops ({} at full), final error {:.2} dB, worst line {:.2} dB, consistency median {:.2} dB p95|.| {:.2} dB, wall {:.1} s",
        if completed { "completed" } else { "NOT completed" },
        loop_wall.len(),
        full_ticks,
        final_error,
        final_worst_line,
        quantile(&ratios, 0.5),
        quantile(&abs, 0.95),
        t_start.elapsed().as_secs_f64()
    );
    if let Some(t) = &manifest_j["outcome"]["trip"].as_str() {
        println!(
            "  trip: {t} (loop {}, level {} dB, record window {window_status})",
            manifest_j["outcome"]["trip_record"]["at_loop"],
            manifest_j["outcome"]["trip_record"]["at_level_db"]
        );
    }
    if let Some(e) = &manifest_j["outcome"]["run_error"].as_str() {
        println!("  error: {e}");
    }
    let seed_done = manifest_j["outcome"]["stopped_after_seed"] == json!(true);
    if stopped_after_loops {
        println!(
            "  operator stop after {} loops (stop_after_loops)",
            loop_wall.len()
        );
    }
    std::process::exit(if completed || seed_done || clean_stop {
        0
    } else {
        1
    });
}
