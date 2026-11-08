//! The open loop's recorded trace: what the classical service does around the
//! drive, pulse by pulse, on the `service_sim` fixtures, held fixed while the
//! closed loop is built beside it.
//!
//! With `control: None` the service plays the compensated reference in g
//! written as volts and closes no loop. Building the closed loop rewrites the
//! pipeline around that drive (the grid, the level and limit scaling, the
//! judgement, the ladder), and this trace is what says the open loop did not
//! move. Per pulse it records the SHA-256 of the drive frames the station
//! accepted (as `f32` little-endian bytes, the quiet surround included), the
//! scheduled and commanded levels, the drive-limit scale, the measured peak and
//! velocity change, the three verdicts, the SRS band counts, and per run the
//! trips, the end state and the train wall times (on the virtual pacer).
//!
//! Fixtures, as `service_sim.rs` names them: (a) the full AUTO ladder, (b) the
//! tolerance abort, (c) the feasibility block and the override, (d) a pulse
//! train, (e) a gunfire burst and a refused fast rate, (f) the pre-fire drive
//! limit, (h) two paced ladder pulses (here on the virtual pacer).
//!
//! The trace was recorded once, before the closed loop changed anything, into
//! `tests/data/classical_open_loop_trace.txt`. On the platform it was recorded
//! on (its first line says which) the trace must match it byte for byte. On
//! another platform the floating-point library may round a last bit
//! differently, so the drive hashes are not compared there and every number is
//! held to a relative 1e-9 instead; everything discrete must still match.
//!
//! `HERTZFORGE_RECORD_TRACE=1` rewrites the file. Only do that when the open
//! loop is meant to change, and say so in the commit.

use hertzforge_channel_table::ChannelTable;
use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_shaker_db::{MovingMass, RatingValue, ShakerRecord};
use hertzforge_shock_common::{RateModel, ShockLevelSchedule, TrainSpec};
use hertzforge_shock_host::service::config::GunfireSpec;
use hertzforge_shock_host::service::pacer::VirtualPacer;
use hertzforge_shock_host::{
    BackendPulseFirer, ClassicalPulseSpec, ClassicalShape, ClassicalShockService,
    ClassicalTestConfig,
};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const FS: f64 = 5120.0;

/// Every drive frame each fire offered the station and it accepted, one entry
/// per fire. A fire starts with the firer arming the station.
type Fires = Arc<Mutex<Vec<Vec<f32>>>>;

/// The Sim backend with a tap on what it accepts.
struct Recording {
    inner: SimBackend,
    fires: Fires,
}

impl Backend for Recording {
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
        self.fires.lock().unwrap().push(Vec::new());
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let n = self.inner.write_drive(drive)?;
        if let Some(last) = self.fires.lock().unwrap().last_mut() {
            last.extend_from_slice(&drive[..n.min(drive.len())]);
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

fn spec_20g_11ms() -> ClassicalPulseSpec {
    let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
    spec.compensation = CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: 4.0,
        post_frac: 4.0,
        symmetric: true,
    };
    spec
}

fn fast_floors() -> RateModel {
    RateModel {
        cooling_floor_s: 0.5,
        settling_floor_s: 0.2,
        compute_floor_s: 0.1,
        device_recompute_floor_s: 20.0,
        read_cadence_ok: true,
    }
}

fn big_shaker() -> ShakerRecord {
    let mut s = ShakerRecord::new("big");
    s.force_shock = Some(RatingValue::peak(100_000.0));
    s.max_velocity_ips = Some(RatingValue::peak(400.0));
    s.max_displacement_in_pp = Some(RatingValue::peak(3.0));
    s
}

type Svc = ClassicalShockService<BackendPulseFirer<Recording>>;

fn service(cfg: ClassicalTestConfig, feedthrough: f64, tag: &str) -> (Svc, Fires) {
    #[allow(deprecated)]
    let table = ChannelTable::nonuniform_default_8ch();
    let fires: Fires = Arc::default();
    let backend = Recording {
        inner: SimConfig::new()
            .sample_rate_hz(FS)
            .resonances(Vec::new())
            .noise_level(0.0)
            .feedthrough(feedthrough)
            .shared_clock(true)
            .build(),
        fires: Arc::clone(&fires),
    };
    let firer = BackendPulseFirer::new(
        backend,
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    let mut s = ClassicalShockService::new(
        cfg,
        table,
        firer,
        Some(std::env::temp_dir().join(format!(
            "hertzforge-shock-host-trace-{tag}-{}",
            std::process::id()
        ))),
    )
    .unwrap();
    s.set_pacer(Box::new(VirtualPacer::new(0.010)));
    (s, fires)
}

fn sha256_f32(x: &[f32]) -> String {
    let mut h = Sha256::new();
    for v in x {
        h.update(v.to_le_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn opt<T: std::fmt::Debug>(v: Option<T>) -> String {
    match v {
        Some(v) => format!("{v:?}"),
        None => "none".into(),
    }
}

/// Append one run's trace to `out` under `tag`.
fn trace(out: &mut String, tag: &str, s: &Svc, fires: &Fires) {
    let report = s.report_data();
    // One fire per arm after the service's own bring-up arm (and its stop).
    let fires = fires.lock().unwrap();
    let played: Vec<&Vec<f32>> = fires.iter().filter(|f| !f.is_empty()).collect();
    assert_eq!(
        played.len(),
        report.pulses.len(),
        "{tag}: fires the station took against pulse rows"
    );
    for (row, drive) in report.pulses.iter().zip(played) {
        let srs = match (row.srs_bands_pass, row.srs_bands_total) {
            (Some(p), Some(t)) => format!("{p}/{t}"),
            _ => "none".into(),
        };
        writeln!(
            out,
            "{tag} pulse={} origin={:?} sched_db={:?} cmd_db={:?} scale={:?} amp_g={:?} \
             peak_g={} dv_ips={} disp_in={} env={} vel={} sub={} overall={} srs={srs} \
             srs_worst_db={} frames={} drive_sha={}",
            row.pulse,
            row.origin,
            row.scheduled_level_db,
            row.commanded_db,
            row.drive_limit_scale,
            row.commanded_amplitude_g,
            opt(row.peak_g),
            opt(row.velocity_change_ips),
            opt(row.pk_pk_disp_in),
            opt(row.envelope_pass),
            opt(row.velocity_change_pass),
            opt(row.substitution_pass),
            opt(row.tolerance_overall),
            opt(row.srs_worst_dev_db),
            drive.len(),
            sha256_f32(drive),
        )
        .unwrap();
    }
    for (i, t) in report.trains.iter().enumerate() {
        let walls: Vec<String> = t.wall_times_s.iter().map(|w| format!("{w:?}")).collect();
        writeln!(
            out,
            "{tag} train={i} count={} spacing_s={:?} pulses={:?} level_db={:?} walls={}",
            t.plan.count,
            t.plan.realized_spacing_s,
            t.pulse_numbers,
            t.level_db,
            walls.join(",")
        )
        .unwrap();
    }
    for a in &report.drive_limit_actions {
        writeln!(
            out,
            "{tag} limit pulse={} phase={} scale={:?}",
            a.pulse, a.phase, a.scale
        )
        .unwrap();
    }
    let trip = report
        .trip
        .as_ref()
        .map(|t| {
            format!(
                "{} at_pulse={} at_db={:?}",
                flat("trip", &serde_json::to_value(&t.kind).unwrap()),
                t.at_pulse,
                t.at_level_db
            )
        })
        .unwrap_or_else(|| "trip=none".into());
    writeln!(
        out,
        "{tag} end state={:?} pulses={} checked={} passed={} {trip}",
        s.state(),
        report.pulses.len(),
        report.tolerance.checked,
        report.tolerance.passed
    )
    .unwrap();
}

/// A serde value as space-free `key=value` tokens, numbers in shortest
/// round-trip form, so the rounding comparison can read each one. An object's
/// keys are written sorted: the workspace builds `serde_json` with
/// `preserve_order` (field order) and this package alone builds it without
/// (sorted), and the trace must read the same either way.
fn flat(key: &str, v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Object(m) if m.len() == 1 && m.values().all(|x| x.is_object()) => {
            let (name, inner) = m.iter().next().unwrap();
            format!("{key}={name} {}", flat(key, inner))
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            keys.iter()
                .map(|k| flat(&format!("{key}.{k}"), &m[k.as_str()]))
                .collect::<Vec<_>>()
                .join(" ")
        }
        Value::Number(n) => match n.as_f64() {
            Some(f) if !n.is_u64() && !n.is_i64() => format!("{key}={f:?}"),
            _ => format!("{key}={n}"),
        },
        Value::String(t) => format!("{key}={}", t.replace(' ', "_")),
        other => format!("{key}={other}"),
    }
}

fn record_all() -> String {
    let mut out = String::new();

    // (a) The full AUTO ladder.
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.shaker = Some(big_shaker());
    cfg.moving_mass = MovingMass {
        armature_lb: 100.0,
        test_article_lb: 50.0,
        ..MovingMass::default()
    };
    let (mut s, f) = service(cfg, 1.0, "a");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_auto(100).unwrap();
    trace(&mut out, "a", &s, &f);

    // (b) The tolerance abort on a 1.3x plant.
    let (mut s, f) = service(ClassicalTestConfig::new(spec_20g_11ms()), 1.3, "b");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_auto(100).unwrap();
    trace(&mut out, "b", &s, &f);

    // (c) The feasibility block, the override, one pulse.
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    let mut small = ShakerRecord::new("small");
    small.force_shock = Some(RatingValue::peak(5000.0));
    small.max_velocity_ips = Some(RatingValue::peak(400.0));
    small.max_displacement_in_pp = Some(RatingValue::peak(3.0));
    cfg.shaker = Some(small);
    cfg.moving_mass = MovingMass {
        armature_lb: 100.0,
        test_article_lb: 400.0,
        ..MovingMass::default()
    };
    let (mut s, f) = service(cfg, 1.0, "c");
    let feas = s.assess_feasibility().unwrap();
    writeln!(
        out,
        "c feasibility exceeded={} state={:?} arm_refused={}",
        feas.exceeded(),
        s.state(),
        s.arm().is_err()
    )
    .unwrap();
    s.arm_with_override("operator accepts 2x force exceedance")
        .unwrap();
    s.fire_next_pulse().unwrap();
    s.stop().unwrap();
    trace(&mut out, "c", &s, &f);

    // (d) A pulse train at the floor, then a refused short spacing.
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.schedule = ShockLevelSchedule::manual_default();
    cfg.rate_model = fast_floors();
    cfg.train = TrainSpec::max_rate(4);
    let (mut s, f) = service(cfg, 1.0, "d");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.fire_train().unwrap();
    writeln!(
        out,
        "d refused_short_spacing={}",
        s.fire_train_with(TrainSpec::delayed(3, 0.1)).is_err()
    )
    .unwrap();
    s.stop().unwrap();
    trace(&mut out, "d", &s, &f);

    // (e) A gunfire burst at 30 rounds/min; 600 rounds/min refused.
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.schedule = ShockLevelSchedule::manual_default();
    cfg.rate_model = fast_floors();
    cfg.gunfire = Some(GunfireSpec {
        rounds_per_min: 30.0,
        rounds: 3,
    });
    let (mut s, f) = service(cfg.clone(), 1.0, "e");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.fire_gunfire_burst().unwrap();
    s.stop().unwrap();
    trace(&mut out, "e", &s, &f);
    cfg.gunfire = Some(GunfireSpec {
        rounds_per_min: 600.0,
        rounds: 3,
    });
    let (mut s, f) = service(cfg, 1.0, "e2");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    writeln!(out, "e2 refused={}", s.fire_gunfire_burst().is_err()).unwrap();
    s.stop().unwrap();
    trace(&mut out, "e2", &s, &f);

    // (f) The pre-fire drive limit scales the full-level pulse to 15 g.
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.drive_limit.peak_accel_g = Some(15.0);
    let (mut s, f) = service(cfg, 1.0, "f");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_auto(100).unwrap();
    trace(&mut out, "f", &s, &f);

    // (h) Two ladder pulses paced at a 0.5 s floor.
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.rate_model = fast_floors();
    let (mut s, f) = service(cfg, 1.0, "h");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.fire_next_pulse().unwrap();
    s.fire_next_pulse().unwrap();
    let walls: Vec<String> = s
        .report_data()
        .pulses
        .iter()
        .map(|r| format!("{:?}", r.at_wall_time_s))
        .collect();
    writeln!(out, "h walls={}", walls.join(",")).unwrap();
    s.stop().unwrap();
    trace(&mut out, "h", &s, &f);

    out
}

fn platform() -> String {
    format!(
        "# recorded on {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("classical_open_loop_trace.txt")
}

/// Two lines agree when every token does: words exactly, numbers to a
/// relative 1e-9, drive hashes not at all.
fn same_within_rounding(want: &str, got: &str) -> Result<(), String> {
    let (wt, gt): (Vec<&str>, Vec<&str>) = (want.split(' ').collect(), got.split(' ').collect());
    if wt.len() != gt.len() {
        return Err("token counts differ".into());
    }
    for (w, g) in wt.iter().zip(&gt) {
        if w.starts_with("drive_sha=") && g.starts_with("drive_sha=") {
            continue;
        }
        if w == g {
            continue;
        }
        let (wk, wv) = w.split_once('=').unwrap_or(("", w));
        let (gk, gv) = g.split_once('=').unwrap_or(("", g));
        if wk != gk {
            return Err(format!("{w} against {g}"));
        }
        let nums = |v: &str| -> Option<Vec<f64>> {
            v.split(',')
                .map(|x| x.trim_matches(|c| c == '[' || c == ']').parse::<f64>().ok())
                .collect()
        };
        match (nums(wv), nums(gv)) {
            (Some(a), Some(b)) if a.len() == b.len() => {
                for (x, y) in a.iter().zip(&b) {
                    let tol = 1e-9 * x.abs().max(y.abs()).max(1e-12);
                    if (x - y).abs() > tol {
                        return Err(format!("{w} against {g}"));
                    }
                }
            }
            _ => return Err(format!("{w} against {g}")),
        }
    }
    Ok(())
}

#[test]
fn the_open_loop_trace_is_unchanged() {
    let got = format!("{}\n{}", platform(), record_all());
    let path = golden_path();
    if std::env::var_os("HERTZFORGE_RECORD_TRACE").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
        eprintln!("recorded {}", path.display());
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (record it once with HERTZFORGE_RECORD_TRACE=1)",
            path.display()
        )
    });
    let recorded_here = want.lines().next() == Some(platform().as_str());
    if recorded_here {
        if want != got {
            for (i, (w, g)) in want.lines().zip(got.lines()).enumerate() {
                if w != g {
                    panic!(
                        "the open-loop trace moved at line {}:\n  was {w}\n  now {g}",
                        i + 1
                    );
                }
            }
            panic!(
                "the open-loop trace moved: {} lines recorded, {} now",
                want.lines().count(),
                got.lines().count()
            );
        }
        return;
    }
    let (wl, gl): (Vec<&str>, Vec<&str>) = (
        want.lines().skip(1).collect(),
        got.lines().skip(1).collect(),
    );
    assert_eq!(
        wl.len(),
        gl.len(),
        "the open-loop trace has a different number of lines"
    );
    for (i, (w, g)) in wl.iter().zip(&gl).enumerate() {
        if let Err(e) = same_within_rounding(w, g) {
            panic!(
                "the open-loop trace moved at line {}: {e}\n  was {w}\n  now {g}",
                i + 2
            );
        }
    }
}

#[test]
fn the_rounding_comparison_still_sees_a_real_change() {
    let a = "a pulse=0 sched_db=-12.0 peak_g=5.0237728630191 env=true drive_sha=00";
    let b = "a pulse=0 sched_db=-12.0 peak_g=5.023772863019105 env=true drive_sha=ff";
    same_within_rounding(a, b).expect("a last-bit difference and another hash are rounding");
    let moved = "a pulse=0 sched_db=-12.0 peak_g=5.0238 env=true drive_sha=00";
    assert!(same_within_rounding(a, moved).is_err());
    let flipped = "a pulse=0 sched_db=-12.0 peak_g=5.0237728630191 env=false drive_sha=00";
    assert!(same_within_rounding(a, flipped).is_err());
}
