//! The five built-in MIMO Sine laws at the largest setup the engine accepts,
//! 24 controls by 12 drives: each is built natively and as a law file, and
//! the law file gives its native form's drives on every update of a recorded
//! sequence (the law mathematics' exact linear structure, closed loop), as
//! `law_files` holds them at three by three.
//!
//! One test is a measurement, ignored by default: `matrix`'s build and call
//! times by size on a structure with modes across the whole sine band,
//! natively and as a law file, against the setup's call budget (a quarter of
//! the 10 ms update, 164 frames at 16384 Hz: 2.502 ms), with the allocations
//! a call makes. Run it with `--ignored --nocapture`.

use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_law_sdk::sine_mimo::builtin::{self, LawDecl};
use hertzforge_law_sdk::Params;
use hertzforge_mimo_sine::law::{LawValues, MimoSineLaw};
use hertzforge_mimo_sine::tick::TickOutputs;
use hertzforge_mimo_sine::{LawSetup, MimoSineLawFactory, TestType};
use hertzforge_mimo_sine_law_math::law::{Inputs, Outputs, SineLaw};
use hertzforge_mimo_sine_law_math::reference_structure::{
    run_closed_loop, Dynamics, ModalStructure, Played, Schedule,
};
use hertzforge_mimo_sine_law_math::Setup;
use hertzforge_mimo_sine_laws::parity::bench::{law_setup, record, Recording};
use hertzforge_mimo_sine_laws::parity::{relative_difference, run_sequence, RecordedInputs};
use hertzforge_mimo_sine_laws::{
    compare_laws, load_law_file, BuiltinFactory, WasmSineMimoLawFactory,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(|c| c.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.with(|c| c.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn file_of(d: &LawDecl) -> PathBuf {
    let file = format!("hertzforge_law_mimo_sine_{}.wasm", d.id);
    let exe = std::env::current_exe().expect("the test binary knows its own path");
    for dir in exe.ancestors().skip(1) {
        let p = dir.join("wasm32-wasip2").join("release").join(&file);
        if p.is_file() {
            return p;
        }
    }
    panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
}

const NC: usize = 24;
const ND: usize = 12;
const BUDGET: Duration = Duration::from_secs(5);
/// As `law_files`: rounding only, and the modal fit's platform difference.
const ROUNDING: f64 = 1e-12;
const MODAL_FIT: f64 = 1e-5;
/// The full-rank structure's modes.
const MODES: usize = 14;
/// `matrix`'s default modal model is held on three modes, as `law_files`
/// holds it. The fit is an iterative search, and on more modes the two
/// platforms' last-place rounding moves it further than [`MODAL_FIT`] at any
/// size: on this structure's sweep, 6 modes give 1.9e-3 at 3 by 3 and 9 modes
/// 6.8e-2 (measured on a Mac), against 6.9e-8 at 3 by 3 and 1.9e-6 at 24 by 12
/// on three.
const FIT_MODES: usize = 3;

/// A coupled structure of `modes` modes log-spaced over 35 to 110 Hz,
/// `controls` by `drives`. With [`MODES`] every pair of a control and a drive
/// has a mode of its own share, so the response is of full rank at every
/// frequency.
fn structure(controls: usize, drives: usize, modes: usize) -> ModalStructure {
    let shape = |n: usize| -> Vec<f64> {
        (0..modes * n)
            .map(|i| {
                let (r, c) = ((i / n) as f64, (i % n) as f64);
                (1.3 * r + 0.7 * c + 0.4).sin() + if (i / n) % n == i % n { 1.5 } else { 0.0 }
            })
            .collect()
    };
    ModalStructure {
        f_hz: (0..modes)
            .map(|r| 35.0 * (110.0f64 / 35.0).powf(r as f64 / (modes - 1) as f64))
            .collect(),
        zeta: (0..modes).map(|r| 0.02 + 0.01 * (r % 3) as f64).collect(),
        drive_shape: shape(drives),
        output_shape: shape(controls),
        drives,
        controls,
    }
}

const DWELL: Schedule = Schedule::Dwell { f: 63.0, secs: 1.0 };
const STEPS: Schedule = Schedule::Steps {
    f0: 40.0,
    f1: 90.0,
    per_octave: 6.0,
    secs: 0.3,
};
const SWEEP: Schedule = Schedule::Sweep {
    f0: 40.0,
    f1: 85.0,
    oct_min: 4.0,
};

fn host_params(p: &[(&str, ParamValue)]) -> Vec<Param> {
    p.iter()
        .map(|(k, v)| Param {
            key: (*k).into(),
            value: v.clone(),
        })
        .collect()
}

fn sdk_params(d: &LawDecl, p: &[(&str, ParamValue)]) -> Params {
    let mut all = builtin::defaults(d);
    for (k, v) in p {
        let v = hertzforge_mimo_sine_laws::choice::sdk_value(v);
        all.0.iter_mut().find(|q| q.key == *k).unwrap().value = v;
    }
    all
}

/// Record `d` on the 24 by 12 structure through `schedule`, then hold its
/// native form to the mathematics and its law file to the native form, on
/// every update, within `tol` of the update's largest drive.
fn parity(
    d: &'static LawDecl,
    p: &[(&str, ParamValue)],
    modes: usize,
    schedule: Schedule,
    tt: TestType,
    tol: f64,
) {
    let st = structure(NC, ND, modes);
    let setup: Setup = st.setup((30.0, 120.0), false);
    let params = sdk_params(d, p);
    let Recording {
        setup: ls,
        inputs,
        asked,
    } = record(d, &params, &st, &setup, schedule, tt, 20_000).unwrap();
    assert_eq!((ls.controls, ls.drives), (NC, ND));
    // The engine takes the run's law setup, as it does before it builds a law.
    ls.validate().unwrap();
    assert!(inputs.len() > 20, "{}: a short sequence", d.id);
    let native = BuiltinFactory::new(d, params).unwrap();
    let mut law = native.build(&ls).unwrap();
    let answers = run_sequence(law.as_mut(), &ls, native.manifest().values.len(), &inputs).unwrap();
    for (k, (a, m)) in answers.iter().zip(&asked).enumerate() {
        assert_eq!(a.drive.len(), ND);
        let e = relative_difference(&a.drive, m);
        assert!(
            e <= 1e-12,
            "{}: native against the math at {k}: {e:e}",
            d.id
        );
    }
    let host = LawHost::new(HostConfig::default()).unwrap();
    let file = WasmSineMimoLawFactory::new(
        load_law_file(&host, &file_of(d)).unwrap(),
        host_params(p),
        BUDGET,
    );
    let r = compare_laws(&native, &file, &ls, &inputs).unwrap();
    eprintln!(
        "{} {tt:?} at {NC} x {ND} on {modes} modes: {} updates, file against native {:e}",
        d.id, r.updates, r.worst_relative
    );
    assert_eq!(r.updates, inputs.len());
    assert!(
        r.worst_relative <= tol,
        "{}: {:e} at {}",
        d.id,
        r.worst_relative,
        r.worst_update
    );
    assert_eq!(r.mismatched_updates, 0, "{}", d.id);
}

#[test]
fn mimo_sine_matrix_at_24_by_12_natively_and_as_a_law_file() {
    let d = &builtin::MATRIX;
    let measured = [("model", ParamValue::Choice("identification".into()))];
    parity(d, &measured, MODES, SWEEP, TestType::Sweep, ROUNDING);
    parity(d, &measured, MODES, STEPS, TestType::Stepped, ROUNDING);
    parity(d, &measured, MODES, DWELL, TestType::Dwell, ROUNDING);
    parity(d, &[], FIT_MODES, SWEEP, TestType::Sweep, MODAL_FIT);
}

#[test]
fn mimo_sine_paired_ratio_steepest_and_secant_at_24_by_12() {
    parity(
        &builtin::PAIRED,
        &[],
        MODES,
        STEPS,
        TestType::Stepped,
        ROUNDING,
    );
    parity(
        &builtin::RATIO,
        &[],
        MODES,
        DWELL,
        TestType::Dwell,
        ROUNDING,
    );
    parity(
        &builtin::STEEPEST_DESCENT,
        &[],
        MODES,
        DWELL,
        TestType::Dwell,
        ROUNDING,
    );
    parity(
        &builtin::SECANT,
        &[],
        MODES,
        DWELL,
        TestType::Dwell,
        ROUNDING,
    );
}

/// A seeded `n` by `m` structure with `modes` modes log-spaced over 15 to
/// 2100 Hz, damping 1 to 3 percent, shapes uniform in -1 to 1 (the modal
/// fit's timing structure, sized).
fn big(n: usize, m: usize, modes: usize) -> ModalStructure {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut u = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x >> 11) as f64 / (1u64 << 53) as f64
    };
    let f_hz: Vec<f64> = (0..modes)
        .map(|r| 15.0 * (2100.0f64 / 15.0).powf((r as f64 + 0.3 + 0.4 * u()) / modes as f64))
        .collect();
    let zeta = (0..modes).map(|_| 0.01 + 0.02 * u()).collect();
    let drive_shape = (0..modes * m).map(|_| 2.0 * u() - 1.0).collect();
    let output_shape = (0..modes * n).map(|_| 2.0 * u() - 1.0).collect();
    ModalStructure {
        f_hz,
        zeta,
        drive_shape,
        output_shape,
        drives: m,
        controls: n,
    }
}

/// Each call's time, s, and the allocations over every call after the
/// engage, of `law` over `seq`.
fn timed(
    law: &mut dyn MimoSineLaw,
    setup: &LawSetup,
    declared: usize,
    seq: &[RecordedInputs],
) -> (Vec<f64>, usize) {
    let mut out = TickOutputs::new(setup.drives, setup.controls);
    let mut values = LawValues::new(declared);
    let mut times = Vec::with_capacity(seq.len());
    let mut allocs = 0;
    for (k, x) in seq.iter().enumerate() {
        let tick = x.as_tick();
        out.reset();
        values.clear();
        let before = ALLOCS.load(Ordering::Relaxed);
        COUNTING.with(|c| c.set(k > 0));
        let t = Instant::now();
        let r = if k == 0 {
            law.engage(&tick, &mut out, &mut values)
        } else {
            law.tick(&tick, &mut out, &mut values)
        };
        let s = t.elapsed().as_secs_f64();
        COUNTING.with(|c| c.set(false));
        allocs += ALLOCS.load(Ordering::Relaxed) - before;
        r.unwrap_or_else(|f| panic!("update {k}: {f:?}"));
        if k > 0 {
            times.push(s);
        }
    }
    (times, allocs)
}

fn summary(v: &mut [f64]) -> (f64, f64, f64) {
    v.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.99), *v.last().unwrap())
}

/// The calls over `budget_ms`, and the five slowest calls as (index, ms):
/// whether the worst is one stall or a call the law makes again and again.
fn over(v: &[f64], budget_ms: f64) -> (usize, Vec<(usize, f64)>) {
    let mut k: Vec<(usize, f64)> = v.iter().map(|s| s * 1e3).enumerate().collect();
    let n = k.iter().filter(|(_, ms)| *ms > budget_ms).count();
    k.sort_by(|a, b| b.1.total_cmp(&a.1));
    k.truncate(5);
    (
        n,
        k.into_iter()
            .map(|(i, ms)| (i, (ms * 100.0).round() / 100.0))
            .collect(),
    )
}

/// The mathematics' own law, each call timed and the allocations of every
/// call after the engage counted.
struct Timed {
    law: builtin::BuiltLaw,
    times: Vec<f64>,
    allocs: usize,
}

impl Timed {
    fn call(
        &mut self,
        engage: bool,
        f: impl FnOnce(&mut builtin::BuiltLaw) -> Result<(), String>,
    ) -> Result<(), String> {
        let before = ALLOCS.load(Ordering::Relaxed);
        COUNTING.with(|c| c.set(!engage));
        let t = Instant::now();
        let r = f(&mut self.law);
        let s = t.elapsed().as_secs_f64();
        COUNTING.with(|c| c.set(false));
        if !engage {
            self.allocs += ALLOCS.load(Ordering::Relaxed) - before;
            self.times.push(s);
        }
        r
    }
}

impl SineLaw for Timed {
    fn engage(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        self.call(true, |l| l.engage(x, out))
    }
    fn tick(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        self.call(false, |l| l.tick(x, out))
    }
    fn value_names(&self) -> &'static [&'static str] {
        self.law.value_names()
    }
}

/// `matrix` with its defaults on [`big`]`(n, m, 100)` over 20 to 2000 Hz on
/// 0.25 Hz lines, swept 100 to 400 Hz at 4 octaves a minute: the build and
/// every call timed, natively (the mathematics' law in its closed loop, at
/// any size) and as a law file (where the core takes the size). Sizes from
/// `MSINE_SIZES` (`24x12,72x24`), else 4 by 4, 16 by 8 and 24 by 12.
#[test]
#[ignore = "a measurement; run with --ignored --nocapture"]
fn mimo_sine_matrix_call_time_by_size() {
    let budget_ms = 164.0 / 16384.0 / 4.0 * 1e3;
    let band = (20.0, 2000.0);
    let host = LawHost::new(HostConfig::default()).unwrap();
    let d = &builtin::MATRIX;
    let sizes: Vec<(usize, usize)> = match std::env::var("MSINE_SIZES") {
        Ok(s) => s
            .split(',')
            .map(|p| {
                let (a, b) = p.split_once('x').unwrap();
                (a.parse().unwrap(), b.parse().unwrap())
            })
            .collect(),
        Err(_) => vec![(4, 4), (16, 8), (24, 12)],
    };
    let sched = Schedule::Sweep {
        f0: 100.0,
        f1: 400.0,
        oct_min: 4.0,
    };
    for (n, m) in sizes {
        let st = big(n, m, 100);
        let setup = st.setup(band, true);
        let lines = setup.identification.grid_hz.len();
        let params = builtin::defaults(d);
        let t = Instant::now();
        let law = builtin::config(d.id, &params)
            .unwrap()
            .build(&setup)
            .unwrap();
        let build_s = t.elapsed().as_secs_f64();
        let mut timed_law = Timed {
            law,
            times: Vec::new(),
            allocs: 0,
        };
        let names = timed_law.value_names().len();
        run_closed_loop(
            &mut timed_law,
            &st,
            &setup,
            sched,
            Played::AsAsked,
            Dynamics::QuasiSteady,
            names,
            20_000,
        )
        .unwrap();
        let calls = timed_law.times.len();
        let (n_over, slowest) = over(&timed_law.times, budget_ms);
        let (n50, n99, nmax) = summary(&mut timed_law.times);
        println!(
            "{n} x {m}, {lines} lines, natively: build {build_s:.2} s; {calls} calls, median {:.4} ms, \
             99th {:.4} ms, worst {:.4} ms against {budget_ms:.3} ms; {} allocations ({:.1} a call)",
            n50 * 1e3,
            n99 * 1e3,
            nmax * 1e3,
            timed_law.allocs,
            timed_law.allocs as f64 / calls as f64
        );
        println!(
            "{n} x {m} natively: {n_over} calls over the budget; slowest (call, ms) {slowest:?}"
        );
        if n > hertzforge_mimo_sine::MAX_CONTROLS || m > hertzforge_mimo_sine::MAX_DRIVES {
            println!("{n} x {m}: over the core's limits, no law file run");
            continue;
        }
        let rec = record(d, &params, &st, &setup, sched, TestType::Sweep, 20_000).unwrap();
        let ls = law_setup(&setup, TestType::Sweep);
        let file = WasmSineMimoLawFactory::new(
            load_law_file(&host, &file_of(d)).unwrap(),
            Vec::new(),
            Duration::from_secs(5),
        );
        let t = Instant::now();
        let built = file.build(&ls);
        let file_build_s = t.elapsed().as_secs_f64();
        match built {
            Ok(mut law) => {
                let (mut v, _) =
                    timed(law.as_mut(), &ls, file.manifest().values.len(), &rec.inputs);
                let (f_over, f_slowest) = over(&v, budget_ms);
                let (f50, f99, fmax) = summary(&mut v);
                println!(
                    "{n} x {m} as a law file: build {file_build_s:.2} s; {} calls, median {:.4} ms, \
                     99th {:.4} ms, worst {:.4} ms",
                    v.len(),
                    f50 * 1e3,
                    f99 * 1e3,
                    fmax * 1e3
                );
                println!("{n} x {m} as a law file: {f_over} calls over the budget; slowest (call, ms) {f_slowest:?}");
            }
            Err(e) => {
                println!("{n} x {m} as a law file: not built after {file_build_s:.2} s: {e:?}")
            }
        }
    }
}
