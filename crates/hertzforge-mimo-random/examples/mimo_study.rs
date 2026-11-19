//! Runs one multiple-output Random test on a structure's modal model.
//!
//!     cargo build --release -p hertzforge-mimo-random --example mimo_study
//!     mimo_study setup.json summary.json
//!
//! The setup is a JSON file beside raw little-endian arrays, written by
//! `examples/rc_plate_band_average/run_study.py`. The plant writes every sample to
//! its stream file, and the summary holds what the service reported loop by loop.
//!
//! This is the real run service and the real simulator backend, with the service's
//! protections opened up so a simulated study can run a law as it is.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal_sim::plant::PlantModel;
use hertzforge_hal_sim::{
    Discretization, ModalPlant, ModalSoftening, Mode, ModeResponse, SimConfig,
};
use hertzforge_law_host::{HostConfig, LawHost, LawIdentity, Param, ParamValue};
use hertzforge_mimo_laws::{BandAverageLaw, BuiltinMimo, BuiltinMimoLaw, LawSpec, ParamKindDecl};
use hertzforge_mimo_random::config::{Caps, FrfMode, Gates, Protections, Records};
use hertzforge_mimo_random::{
    DriveSpec, Estimation, Identification, MimoConfig, MimoRandomService, ReferenceSpec,
};
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::config::{LevelSchedule, RunMode};
use hertzforge_random::service::law::LawClock;
use hertzforge_random::LoopConfig;
use hertzforge_random_laws::{ChoiceError, LawCatalog, LawChoice, MimoLawFactory};
use hertzforge_sdm::law::{MimoControlLaw, MimoSetup, Tolerances};
use hertzforge_sdm::{HermitianStack, C64};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Any drive the study's laws ask for is well under this, in lbf at 1 lbf/V.
const CEILING: f64 = 1.0e6;
/// A simulated first solve may take as long as it takes.
const LAW_FILE_DEADLINE: Duration = Duration::from_secs(3600);

/// The clock a simulated run's law calls are judged on. Simulated time stands still
/// while a law computes, so the law's time budget has nothing to judge.
struct StoppedClock;

impl LawClock for StoppedClock {
    fn now_s(&self) -> f64 {
        0.0
    }
}

/// A law file, with a deadline long enough for a first solve.
struct Unhurried(Arc<dyn MimoLawFactory>);

impl MimoLawFactory for Unhurried {
    fn identity(&self) -> LawIdentity {
        self.0.identity()
    }

    fn params(&self) -> &[Param] {
        self.0.params()
    }

    fn build(
        &self,
        setup: &MimoSetup,
        _budget: Duration,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError> {
        self.0.build(setup, LAW_FILE_DEADLINE)
    }
}

fn doubles(path: &Path) -> Vec<f64> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes
        .chunks_exact(8)
        .map(|b| f64::from_le_bytes(b.try_into().expect("eight bytes")))
        .collect()
}

fn indices(v: &Value) -> Vec<usize> {
    v.as_array()
        .expect("a list")
        .iter()
        .map(|i| i.as_u64().expect("a whole number") as usize)
        .collect()
}

/// How far past the band's top each mode is fitted to its continuous response.
const FIT_MARGIN: f64 = 1.1;

/// The plant with its inputs in table order, controls then the drive echoes: the
/// setup's modal state space (`A` block diagonal, one `[[0, 1], [-w^2, -2 z w]]`
/// block per mode, acceleration outputs, as `hertzforge.core.state_space` writes
/// it) as the simulator's modal plant, every mode an acceleration mode fitted to its
/// continuous response to `FIT_MARGIN` times the band's top. What `D` holds beyond
/// the modes' mass lines (the drive echoes) is the residual.
fn plant(
    setup: &Value,
    dir: &Path,
    controls: &[usize],
    echoes: &[usize],
    band_hi: f64,
) -> ModalPlant {
    let p = &setup["plant"];
    let states = p["states"].as_u64().expect("states") as usize;
    let drives = p["inputs"].as_u64().expect("inputs") as usize;
    let outputs = p["outputs"].as_u64().expect("outputs") as usize;
    let raw = doubles(&dir.join(p["file"].as_str().expect("plant.file")));
    let (na, nb, nc) = (states * states, states * drives, outputs * states);
    assert_eq!(
        raw.len(),
        na + nb + nc + outputs * drives,
        "the plant file's length"
    );
    let (a, rest) = raw.split_at(na);
    let (b, rest) = rest.split_at(nb);
    let (c, d) = rest.split_at(nc);
    let rows: Vec<usize> = controls.iter().chain(echoes).copied().collect();
    let mut modes = Vec::with_capacity(states / 2);
    for q in (0..states).step_by(2) {
        let v = q + 1;
        let (w2, cw) = (-a[v * states + q], -a[v * states + v]);
        assert!(
            a[q * states + q] == 0.0 && a[q * states + v] == 1.0 && w2 > 0.0,
            "state {q} is not in modal form"
        );
        assert!(
            (0..drives).all(|j| b[q * drives + j] == 0.0),
            "B is not zero on displacement state {q}"
        );
        let w = w2.sqrt();
        let output_shape: Vec<f64> = rows.iter().map(|&r| -c[r * states + q] / w2).collect();
        for (k, &r) in rows.iter().enumerate() {
            let want = -cw * output_shape[k];
            assert!(
                (c[r * states + v] - want).abs() <= 1e-9 * want.abs().max(1e-300),
                "output {r}'s velocity column is not the acceleration of mode {}",
                q / 2
            );
        }
        modes.push(Mode {
            response: ModeResponse::Acceleration,
            ..Mode::new(
                w / std::f64::consts::TAU,
                w / cw,
                b[v * drives..(v + 1) * drives].to_vec(),
                output_shape,
            )
        });
    }
    let mut residual: Vec<f64> = rows
        .iter()
        .flat_map(|&r| d[r * drives..(r + 1) * drives].iter().copied())
        .collect();
    for mode in &modes {
        for (i, o) in mode.output_shape.iter().enumerate() {
            for (j, u) in mode.input_shape.iter().enumerate() {
                residual[i * drives + j] -= o * u;
            }
        }
    }
    let softening = (!p["softening"].is_null()).then(|| {
        let s = &p["softening"];
        ModalSoftening {
            modes: indices(&s["displacement_states"])
                .into_iter()
                .map(|q| q / 2)
                .collect(),
            reference_rms: s["reference_rms"]
                .as_array()
                .expect("reference_rms")
                .iter()
                .filter_map(Value::as_f64)
                .collect(),
            fraction: s["fraction"].as_f64().expect("fraction"),
            smoothing: s["smoothing"].as_f64().expect("smoothing"),
            read_samples: s["read_samples"].as_u64().expect("read_samples") as usize,
            log: s["log"].as_str().map(PathBuf::from),
        }
    });
    ModalPlant {
        feedthrough: 0.0,
        residual,
        discretization: Discretization::fitted(FIT_MARGIN * band_hi),
        softening,
        noisy_inputs: Some(controls.len()),
        record: p["stream"].as_str().map(PathBuf::from),
        ..ModalPlant::new(rows.len(), drives, modes)
    }
}

fn table(controls: usize, echoes: usize, drives: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 0..controls + echoes {
        let mut c = Channel::new(1 + i as u32);
        c.role = if i < controls {
            ChannelRole::Control
        } else {
            ChannelRole::Monitor
        };
        c.units = if i < controls {
            Units::AccelG
        } else {
            Units::VoltageV
        };
        c.range_max = CEILING;
        t.channels.push(c);
    }
    for k in 0..drives {
        let mut d = Channel::new(1001 + k as u32);
        d.role = ChannelRole::Drive;
        d.units = Units::VoltageV;
        t.channels.push(d);
    }
    t
}

fn reference(
    setup: &Value,
    dir: &Path,
    n: usize,
    lines: usize,
    band: (usize, usize),
    df: f64,
) -> ReferenceSpec {
    let raw = doubles(&dir.join(setup["specification"].as_str().expect("specification")));
    assert_eq!(
        raw.len(),
        2 * lines * n * n,
        "the specification file's length"
    );
    let mut sdm = HermitianStack::zeros(lines, n);
    for line in band.0..band.1 {
        for i in 0..n {
            for j in i..n {
                let k = 2 * ((line * n + i) * n + j);
                sdm.set(line, i, j, C64::new(raw[k], raw[k + 1]));
            }
        }
    }
    ReferenceSpec {
        sdm,
        tolerances: Tolerances::none(lines, n),
        band,
        df_hz: df,
    }
}

/// The study's values as the kinds a law declares.
fn declared_params(spec: &LawSpec, values: &Value) -> Vec<Param> {
    values
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, v)| {
            let decl = spec
                .params
                .iter()
                .find(|p| p.key == key)
                .unwrap_or_else(|| panic!("the law `{}` has no parameter `{key}`", spec.name));
            let value = match decl.kind {
                ParamKindDecl::Number { .. } => ParamValue::Number(v.as_f64().expect("a number")),
                ParamKindDecl::Integer { .. } => {
                    ParamValue::Integer(v.as_i64().expect("a whole number"))
                }
                ParamKindDecl::Toggle(_) => ParamValue::Toggle(v.as_bool().expect("true or false")),
                ParamKindDecl::Choice { .. } => {
                    ParamValue::Choice(v.as_str().expect("a name").into())
                }
            };
            Param::new(key.clone(), value)
        })
        .collect()
}

fn factory(law: &Value, dir: &Path, n_c: usize) -> (LawChoice, Option<Arc<dyn MimoLawFactory>>) {
    match law["kind"].as_str().expect("law.kind") {
        "builtin" => {
            let id = law["id"].as_str().expect("law.id");
            let spec = BuiltinMimo::from_id(id)
                .unwrap_or_else(|| panic!("there is no built-in law `{id}`"))
                .spec();
            (
                LawChoice::new(id, declared_params(&spec, &law["params"])),
                None,
            )
        }
        "file" => {
            let host = LawHost::new(HostConfig {
                memory_limit_bytes: law["memory_mib"].as_u64().unwrap_or(2048) as usize
                    * 1024
                    * 1024,
                ..HostConfig::default()
            })
            .expect("the law host");
            let root = dir.join("law_catalog");
            let mut catalog = LawCatalog::with_host(host, &root).expect("the law catalog");
            let path = dir.join(law["path"].as_str().expect("law.path"));
            let entry = catalog
                .import(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let params = declared_params(&BandAverageLaw::spec(), &law["params"]);
            let choice = LawChoice::new(entry.id.clone(), params);
            let resolved = catalog
                .resolve_mimo(&choice, n_c)
                .unwrap_or_else(|e| panic!("{e}"));
            (choice, Some(Arc::new(Unhurried(resolved))))
        }
        other => panic!("unknown law kind {other}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, setup_path, summary_path] = args.as_slice() else {
        eprintln!("usage: mimo_study setup.json summary.json");
        std::process::exit(2);
    };
    let setup_path = Path::new(setup_path);
    let dir = setup_path.parent().unwrap_or(Path::new("."));
    let setup: Value =
        serde_json::from_str(&std::fs::read_to_string(setup_path).expect("the setup"))
            .expect("JSON");
    let num = |k: &str| {
        setup[k]
            .as_f64()
            .unwrap_or_else(|| panic!("`{k}` must be a number"))
    };
    let int = |k: &str| {
        setup[k]
            .as_u64()
            .unwrap_or_else(|| panic!("`{k}` must be a whole number")) as usize
    };

    let (fs, nfft) = (num("sample_rate"), int("nfft"));
    let (lines, df) = (nfft / 2 + 1, fs / nfft as f64);
    let (lo, hi) = (num("band_lo_hz"), num("band_hi_hz"));
    let band = ((lo / df).round() as usize, (hi / df).round() as usize + 1);
    let control_rows = indices(&setup["control_outputs"]);
    let echo_rows = indices(&setup["drive_outputs"]);
    let (n_c, n_d) = (control_rows.len(), echo_rows.len());
    let model = plant(&setup, dir, &control_rows, &echo_rows, hi);

    let mut lc = LoopConfig::demo_default();
    lc.fs_hz = fs;
    lc.nfft = nfft;
    lc.band_lo_hz = lo;
    lc.band_hi_hz = hi;
    lc.block_frames = 256;

    let drives: Vec<DriveSpec> = (0..n_d)
        .map(|k| {
            let mut d = DriveSpec::new(1001 + k as u32, CEILING);
            d.pretest_max_vrms = num("sysid_rms");
            d
        })
        .collect();
    let controls: Vec<u32> = (1..=n_c as u32).collect();
    let mut cfg = MimoConfig::new(
        controls,
        drives,
        reference(&setup, dir, n_c, lines, band, df),
    );
    let (choice, law_factory) = factory(&setup["law"], dir, n_c);
    cfg.law = choice;
    cfg.frf_mode = if setup["update_frf"].as_bool().unwrap_or(false) {
        FrfMode::Update { blend: 1.0 }
    } else {
        FrfMode::Hold
    };
    cfg.estimation = Estimation {
        frames_per_loop: int("frames_per_loop"),
        memory_loops: num("memory_loops"),
        ..Estimation::default()
    };
    // No level search: identify at the stated level, wherever that sits.
    cfg.identification = Identification {
        start_vrms: num("sysid_rms"),
        below_reference_db: 0.0,
        tolerance_db: 1000.0,
        max_steps: 1,
        frames: int("sysid_frames"),
        noise_frames: int("noise_frames"),
    };
    cfg.gates = Gates {
        coherence_threshold: 1e-9,
        condition_threshold: f64::MAX,
    };
    cfg.caps = Caps {
        up_db: 6.0,
        down_db: 20.0,
    };
    cfg.law_guards.budget_fraction = 1.0;
    let levels: Vec<f64> = setup["levels"]
        .as_array()
        .expect("levels")
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    cfg.schedule = LevelSchedule {
        mode: RunMode::Auto,
        start_db: levels[0],
        // The last rung's time is the full-level hold.
        dwell_s: (0..levels.len())
            .map(|k| {
                if k + 1 == levels.len() {
                    0.0
                } else {
                    num("dwell_s")
                }
            })
            .collect(),
        table_db: levels,
    };
    cfg.level_ramp_s = num("level_ramp_s");
    cfg.shutdown_ramp_s = 0.0;
    cfg.soft_start_db = 0.0;
    cfg.full_level_duration_s = Some(num("full_level_s"));
    cfg.protections = Protections {
        envelope_abort_lines: usize::MAX,
        divergence_loops: 16,
        divergence_floor_db: 1000.0,
        overload_fraction: 1.0,
        settle_loops: 1,
    };
    cfg.records = Records {
        enabled: setup["records"].as_bool().unwrap_or(false),
        snapshot_every_s: 1e9,
        law_inputs: false,
    };
    cfg.seed = setup["seed"].as_u64().expect("seed");

    let backend = SimConfig::new()
        .sample_rate_hz(fs)
        .plant(PlantModel::Modal(model))
        .noise_level(setup["plant"]["noise_rms"].as_f64().expect("noise_rms"))
        .shared_clock(true)
        .seed(cfg.seed)
        .build();
    let mut svc = MimoRandomService::new(
        cfg,
        table(n_c, n_d, n_d),
        lc,
        Box::new(backend),
        StationClass::HertzforgeSim,
        dir.join("records"),
    )
    .unwrap_or_else(|e| panic!("the service: {e}"))
    .with_law_clock(Arc::new(StoppedClock));
    if let Some(f) = law_factory {
        svc = svc.with_law_factory(f);
    }

    let wall = Instant::now();
    let state = svc
        .run_auto(0)
        .unwrap_or_else(|e| panic!("the run did not engage: {e}\n{:?}", svc.log().events()));
    eprintln!(
        "{state:?} after {:.1} s; first solve {:.1} ms",
        wall.elapsed().as_secs_f64(),
        svc.timing().law_ms
    );
    let first_solve_s = svc.timing().law_ms / 1e3;
    let mut loops = Vec::new();
    let mut failure = None;
    while svc.state().is_running() {
        if let Err(e) = svc.loop_tick() {
            failure = Some(e.to_string());
            break;
        }
        let (timing, guards) = (svc.timing(), svc.guard_summary());
        let drive: f64 = svc.drive_rms().iter().map(|r| r * r).sum();
        let row = json!({
            "loop": svc.loop_count(),
            "state": format!("{:?}", svc.state()),
            "level_db": svc.level_db(),
            "error_db": svc.error_db().iter().sum::<f64>() / svc.error_db().len().max(1) as f64,
            "drive_lbf2": drive,
            "law_ms": timing.law_ms,
            "estimate_ms": timing.estimate_ms,
            "synth_ms": timing.synth_ms,
            "plant_ms": timing.plant_ms,
            "capped": guards.capped,
            "repaired": guards.repair.lines_repaired,
            "peak_change_db": guards.peak_change_db,
        });
        if svc.loop_count().is_multiple_of(10) {
            eprintln!("{row}");
        }
        loops.push(row);
    }
    let summary = json!({
        "state": format!("{:?}", svc.state()),
        "failure": failure,
        "first_solve_s": first_solve_s,
        "wall_s": wall.elapsed().as_secs_f64(),
        "loop_count": svc.loop_count(),
        "law_stats": format!("{:?}", svc.law_stats()),
        "segment_samples": svc.loop_config().segment_seconds() * fs,
        "gate": serde_json::to_value(svc.gate()).ok(),
        "trip": serde_json::to_value(svc.trip()).ok(),
        "events": svc.log().events().iter().map(|e| format!("{e:?}")).collect::<Vec<_>>(),
        "loops": loops,
    });
    drop(svc);
    std::fs::write(summary_path, serde_json::to_string(&summary).expect("JSON"))
        .expect("the summary");
}
