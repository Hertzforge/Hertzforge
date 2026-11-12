//! The multiple-output side of the law host, end to end, against law files
//! built by `scripts/build_laws.sh`: the zero-drive law (`laws/mimo-zero`)
//! and the misbehaving fixture (`guests/mimo-misbehave`), which implements
//! the interface's raw bindings so it can return what the SDK's types cannot
//! express.
//!
//! A test that needs a law file fails, naming the script, when the file has
//! not been built. Nothing here is skipped for a missing file.

use hertzforge_law_host::mimo::{MimoDims, HERMITIAN_TOLERANCE};
use hertzforge_law_host::{
    AnyLaw, DiagnosticKind, HostConfig, LawError, LawHost, LoadedMimoLaw, MimoLawInstance,
    MimoOutputRule, Param, ParamValue, WasmMimoLaw,
};
use hertzforge_sdm::law::{
    DiagnosticData, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup,
    MimoShape, Tolerances,
};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The bytes of `<name>.wasm` from `<target>/wasm32-wasip2/release/` (see
/// `law_host.rs`, which finds law files the same way).
fn law_bytes(name: &str) -> Vec<u8> {
    let file = format!("{name}.wasm");
    let mut tried: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("HERTZFORGE_LAW_WASM_DIR") {
        tried.push(PathBuf::from(dir).join(&file));
    } else {
        let exe = std::env::current_exe().expect("the test binary knows its own path");
        for dir in exe.ancestors().skip(1).take(4) {
            tried.push(dir.join("wasm32-wasip2").join("release").join(&file));
        }
    }
    for path in &tried {
        if path.is_file() {
            return std::fs::read(path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        }
    }
    panic!(
        "the law file {file} is not built. Run `bash scripts/build_laws.sh` (scripts/rust_tests.sh \
         runs it first); it builds every guest law into <target>/wasm32-wasip2/release/. \
         Looked in: {tried:#?}"
    );
}

const ZERO: &str = "hertzforge_law_mimo_zero";
const MISBEHAVE: &str = "law_fixture_mimo_misbehave";
const RATIO: &str = "hertzforge_law_ratio";

const BUDGET: Duration = Duration::from_secs(5);

fn host() -> LawHost {
    LawHost::new(HostConfig::default()).expect("the host starts")
}

fn load(host: &LawHost, name: &str) -> LoadedMimoLaw {
    host.load_mimo(&law_bytes(name))
        .unwrap_or_else(|e| panic!("{name} loads: {e}"))
}

fn within<T: Send + 'static>(
    limit: Duration,
    what: &str,
    f: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit)
        .unwrap_or_else(|_| panic!("{what} did not return within {limit:?}"))
}

// -------------------------------------------------------------------------
// A small grid: 3 controls, 2 drives, 8 lines, band [2, 6)
// -------------------------------------------------------------------------

const LINES: usize = 8;
const NC: usize = 3;
const ND: usize = 2;
const BAND: (usize, usize) = (2, 6);

/// Everything a call needs, owned.
struct Grid {
    setup: MimoSetup,
    response: HermitianStack,
    frf: MatrixStack,
    coherence: Vec<f64>,
    last_drive: HermitianStack,
    sysid_response: HermitianStack,
    sysid_noise: HermitianStack,
}

impl Grid {
    fn new() -> Self {
        let mut reference = HermitianStack::zeros(LINES, NC);
        for line in BAND.0..BAND.1 {
            for i in 0..NC {
                reference.set(line, i, i, C64::new(0.01 * (1 + i) as f64, 0.0));
            }
            reference.set(line, 0, 1, C64::new(0.002, 0.001));
        }
        let mut frf = MatrixStack::zeros(LINES, NC, ND);
        for line in 0..LINES {
            for r in 0..NC {
                for c in 0..ND {
                    let v = if r == c { 1.0 } else { 0.1 * (r + c) as f64 };
                    frf.set(line, r, c, C64::new(v, 0.05 * line as f64));
                }
            }
        }
        let mut response = reference.clone();
        response
            .scale_lines(&[0.9; LINES])
            .expect("one factor a line");
        Grid {
            setup: MimoSetup {
                controls: NC,
                drives: ND,
                lines: LINES,
                df_hz: 2.5,
                band: BAND,
                reference,
                tolerances: Tolerances::none(LINES, NC),
            },
            response,
            frf,
            coherence: vec![0.95; LINES * NC],
            last_drive: HermitianStack::zeros(LINES, ND),
            sysid_response: HermitianStack::zeros(LINES, NC),
            sysid_noise: HermitianStack::zeros(LINES, NC),
        }
    }

    /// Inputs with every optional input supplied: the host must pass only
    /// the ones the law declares.
    fn inputs(&self, loop_index: u64) -> MimoLoopInputs<'_> {
        MimoLoopInputs {
            reference: &self.setup.reference,
            tolerances: &self.setup.tolerances,
            response: &self.response,
            response_dof: 64.0,
            frf: &self.frf,
            multiple_coherence: &self.coherence,
            df_hz: self.setup.df_hz,
            band: self.setup.band,
            loop_index,
            last_drive: Some(&self.last_drive),
            response_drive_csd: None,
            sysid_response: Some(&self.sysid_response),
            sysid_drive: None,
            sysid_noise: Some(&self.sysid_noise),
        }
    }
}

fn mode(m: &str) -> Vec<Param> {
    vec![Param::new("mode", ParamValue::Choice(m.into()))]
}

/// A misbehaving-fixture instance in `m` mode, already seeded.
fn misbehave_in(host: &LawHost, m: &str) -> MimoLawInstance {
    let grid = Grid::new();
    let mut law = load(host, MISBEHAVE)
        .instantiate(&grid.setup, &mode(m), BUDGET)
        .unwrap_or_else(|e| panic!("the fixture builds in {m} mode: {e}"));
    law.seed(&grid.inputs(0), BUDGET)
        .unwrap_or_else(|e| panic!("the fixture seeds in {m} mode: {e}"));
    law
}

/// One iterate on the small grid, guarded against a hang.
fn iterate_once(
    mut law: MimoLawInstance,
    budget: Duration,
) -> (MimoLawInstance, Result<MimoLoopOutputs, LawError>, Duration) {
    within(Duration::from_secs(60), "the iterate call", move || {
        let grid = Grid::new();
        let t0 = Instant::now();
        let r = law.iterate(&grid.inputs(1), budget);
        (law, r, t0.elapsed())
    })
}

fn scalar(out: &MimoLoopOutputs, key: &str) -> f64 {
    match out
        .diagnostics
        .iter()
        .find(|d| d.key == key)
        .map(|d| &d.data)
    {
        Some(DiagnosticData::Scalar(v)) => *v,
        other => panic!("diagnostic {key}: {other:?}"),
    }
}

// -------------------------------------------------------------------------
// Loading: the world decides which loader takes a file
// -------------------------------------------------------------------------

#[test]
fn the_zero_law_loads_as_a_multiple_output_law_with_its_manifest() {
    let bytes = law_bytes(ZERO);
    let law = host().load_mimo(&bytes).expect("the zero law loads");
    let m = law.manifest();
    assert_eq!(m.law.name, "zero drive");
    assert_eq!(m.law.version, "0.1.0");
    assert_eq!(
        m.shapes,
        [
            MimoShape::Square,
            MimoShape::MoreControlsThanDrives,
            MimoShape::FewerControlsThanDrives
        ]
    );
    assert_eq!(m.needs, [MimoInput::SysidResponse]);
    let keys: Vec<(&str, DiagnosticKind)> = m
        .diagnostic_layouts
        .iter()
        .map(|l| (l.key.as_str(), l.kind))
        .collect();
    assert_eq!(
        keys,
        [
            ("loop", DiagnosticKind::Scalar),
            ("in_band", DiagnosticKind::PerLine),
            ("response_psd", DiagnosticKind::PerControlLine),
            ("drive_psd", DiagnosticKind::PerDriveLine),
            ("sysid_trace", DiagnosticKind::Scalar),
        ]
    );
    assert_eq!(law.identity().sha256.len(), 64);
}

#[test]
fn each_loader_takes_only_its_own_world_and_says_what_the_file_is() {
    let h = host();
    match h.load(&law_bytes(ZERO)) {
        Err(LawError::NotALaw(why)) => assert!(why.contains("multiple-output"), "{why}"),
        other => panic!("a MIMO law loaded as single-output: {:?}", other.err()),
    }
    match h.load_mimo(&law_bytes(RATIO)) {
        Err(LawError::NotALaw(why)) => assert!(why.contains("single-output"), "{why}"),
        other => panic!("a single-output law loaded as MIMO: {:?}", other.err()),
    }
    assert!(matches!(h.load_any(&law_bytes(ZERO)), Ok(AnyLaw::Mimo(_))));
    match h.load_any(&law_bytes(RATIO)) {
        Ok(l @ AnyLaw::Single(_)) => assert_eq!(l.world(), "random-single"),
        other => panic!("{:?}", other.err()),
    }
    let empty = h.load_any(&[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00]);
    match empty {
        Err(LawError::NotALaw(why)) => {
            assert!(why.contains("not a single-output law"), "{why}");
            assert!(why.contains("not a multiple-output law"), "{why}");
        }
        other => panic!("an empty component loaded: {:?}", other.err()),
    }
}

// -------------------------------------------------------------------------
// The zero law end to end
// -------------------------------------------------------------------------

#[test]
fn the_zero_law_runs_and_every_diagnostic_layout_crosses_the_interface() {
    let grid = Grid::new();
    let law = load(&host(), ZERO);
    let mut inst = law
        .instantiate(
            &grid.setup,
            &[Param::new("gain", ParamValue::Number(2.0))],
            BUDGET,
        )
        .expect("builds");
    assert_eq!(inst.params(), [Param::new("gain", ParamValue::Number(2.0))]);
    for l in 0..3u64 {
        let out = if l == 0 {
            inst.seed(&grid.inputs(l), BUDGET)
        } else {
            inst.iterate(&grid.inputs(l), BUDGET)
        }
        .expect("the zero law returns");
        assert_eq!(out.drive, HermitianStack::zeros(LINES, ND));
        assert_eq!(scalar(&out, "loop"), l as f64);
        let get = |key: &str| {
            out.diagnostics
                .iter()
                .find(|d| d.key == key)
                .map(|d| d.data.clone())
                .unwrap_or_else(|| panic!("{key} reported"))
        };
        match get("in_band") {
            DiagnosticData::PerLine(v) => {
                assert_eq!(v, [0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0])
            }
            other => panic!("{other:?}"),
        }
        match get("response_psd") {
            DiagnosticData::PerControlLine(v) => {
                assert_eq!(v.len(), LINES * NC);
                let want = 2.0 * grid.response.get(3, 1, 1).re;
                assert_eq!(v[3 * NC + 1], want);
            }
            other => panic!("{other:?}"),
        }
        match get("drive_psd") {
            DiagnosticData::PerDriveLine(v) => assert_eq!(v, vec![0.0; LINES * ND]),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_test_shape_the_law_does_not_support_is_refused_and_so_are_bad_params() {
    let grid = Grid::new();
    let law = load(&host(), ZERO);
    let too_much = [Param::new("gain", ParamValue::Number(11.0))];
    assert!(matches!(
        law.instantiate(&grid.setup, &too_much, BUDGET),
        Err(LawError::ParamInvalid { .. })
    ));
    let mut bad = grid.setup.clone();
    bad.band = (0, LINES + 1);
    assert!(matches!(
        law.instantiate(&bad, &[], BUDGET),
        Err(LawError::SetupInvalid(_))
    ));
}

// -------------------------------------------------------------------------
// Optional inputs: only the declared ones reach the law
// -------------------------------------------------------------------------

#[test]
fn only_the_declared_optional_inputs_reach_the_law() {
    // The fixture declares the last drive and nothing else; the caller
    // supplies the last drive, the identification response and the noise.
    let (_, r, _) = iterate_once(misbehave_in(&host(), "ok"), BUDGET);
    let out = r.expect("ok mode returns");
    assert_eq!(scalar(&out, "has_last_drive"), 1.0);
    assert_eq!(scalar(&out, "has_sysid_response"), 0.0);
    assert_eq!(scalar(&out, "has_sysid_noise"), 0.0);

    // A declared input the caller leaves out is the caller's error, refused
    // before the law sees it, and the instance stays usable.
    let mut law = misbehave_in(&host(), "ok");
    let grid = Grid::new();
    let mut inputs = grid.inputs(1);
    inputs.last_drive = None;
    match law.iterate(&inputs, BUDGET) {
        Err(LawError::InputsInvalid(why)) => assert!(why.contains("LastDrive"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(!law.is_poisoned());
    law.iterate(&grid.inputs(2), BUDGET)
        .expect("the instance still works");
}

// -------------------------------------------------------------------------
// The adversarial fixture: each guard by its exact reason
// -------------------------------------------------------------------------

fn refused(m: &str) -> (MimoOutputRule, String) {
    let (law, r, _) = iterate_once(misbehave_in(&host(), m), BUDGET);
    assert!(!law.is_poisoned(), "{m}: a refused output is not a trap");
    match r {
        Err(LawError::BadMimoOutput { rule, detail }) => (rule, detail),
        other => panic!("{m}: expected a refused output, got {other:?}"),
    }
}

#[test]
fn a_nan_in_the_drive_is_refused() {
    let (rule, detail) = refused("nan");
    assert_eq!(rule, MimoOutputRule::NonFinite);
    assert!(
        detail.contains("drive line 2") && detail.contains("not a finite number"),
        "{detail}"
    );
}

#[test]
fn a_drive_that_is_not_hermitian_is_refused() {
    let (rule, detail) = refused("non_hermitian");
    assert_eq!(rule, MimoOutputRule::NotHermitian);
    assert!(detail.contains("drive line 2 is not Hermitian"), "{detail}");
    const { assert!(HERMITIAN_TOLERANCE <= 1e-9) };
}

#[test]
fn a_drive_of_the_wrong_dimensions_is_refused() {
    let (rule, detail) = refused("wrong_dims");
    assert_eq!(rule, MimoOutputRule::Dimensions);
    assert!(
        detail.contains("8 lines of 3 x 3") && detail.contains("8 lines of 2 x 2"),
        "{detail}"
    );
}

#[test]
fn a_drive_outside_the_band_is_refused() {
    let (rule, detail) = refused("out_of_band");
    assert_eq!(rule, MimoOutputRule::OutOfBand);
    assert!(
        detail.contains("drive line 0 is outside the band [2, 6)"),
        "{detail}"
    );
}

#[test]
fn diagnostics_that_are_not_as_declared_are_refused() {
    let (rule, detail) = refused("undeclared");
    assert_eq!(rule, MimoOutputRule::Diagnostics);
    assert!(
        detail.contains("`not_declared` is not declared"),
        "{detail}"
    );
    let (rule, detail) = refused("wrong_layout");
    assert_eq!(rule, MimoOutputRule::Diagnostics);
    assert!(
        detail.contains("declared per-line but reported scalar"),
        "{detail}"
    );
}

/// The host passes an indefinite drive on: it is well formed, and positive
/// semi-definiteness belongs to the controller, which clamps a small negative
/// eigenvalue and refuses a large one (design 8.3). This one is far past the
/// refusal threshold (-1e-3 against a largest of 3e-3, a third of the largest
/// where the rule refuses at a thousandth), so the MIMO service's own guard
/// test must refuse it; this test only shows the host does not hide it.
#[test]
fn an_indefinite_drive_is_passed_on_for_the_controller_to_judge() {
    let (law, r, _) = iterate_once(misbehave_in(&host(), "indefinite"), BUDGET);
    let out = r.expect("the host passes a well-formed indefinite drive");
    assert!(!law.is_poisoned());
    for line in BAND.0..BAND.1 {
        let a = out.drive.get(line, 0, 0).re;
        let b = out.drive.get(line, 1, 1).re;
        let c = out.drive.get(line, 0, 1);
        // The 2 x 2 matrix [[a, c], [c*, b]] has a negative eigenvalue
        // exactly when its determinant is negative.
        let det = a * b - c.norm_sqr();
        assert!(det < 0.0, "line {line}: determinant {det}");
        let half_tr = 0.5 * (a + b);
        let disc = (0.25 * (a - b) * (a - b) + c.norm_sqr()).sqrt();
        let (lmin, lmax) = (half_tr - disc, half_tr + disc);
        assert!((lmin - (-1e-3)).abs() < 1e-15, "{lmin}");
        assert!((lmax - 3e-3).abs() < 1e-15, "{lmax}");
    }
}

#[test]
fn a_law_that_spins_is_stopped_at_its_budget_and_poisoned() {
    let budget = Duration::from_millis(50);
    let (mut law, r, took) = iterate_once(misbehave_in(&host(), "spin"), budget);
    assert_eq!(r, Err(LawError::Deadline { budget }));
    assert!(
        took >= budget && took < budget + Duration::from_millis(500),
        "{took:?}"
    );
    assert!(law.is_poisoned());
    assert!(matches!(
        law.iterate(&Grid::new().inputs(2), BUDGET),
        Err(LawError::Poisoned(_))
    ));
}

#[test]
fn a_trapping_law_is_reported_with_what_it_wrote_and_poisoned() {
    let (law, r, _) = iterate_once(misbehave_in(&host(), "trap"), BUDGET);
    match r {
        Err(LawError::Trap(why)) => assert!(
            why.contains("deliberate trap from the multiple-output fixture"),
            "{why}"
        ),
        other => panic!("expected a trap, got {other:?}"),
    }
    assert!(law.is_poisoned());
}

#[test]
fn a_laws_own_error_is_passed_through() {
    let (law, r, _) = iterate_once(misbehave_in(&host(), "law_error"), BUDGET);
    assert_eq!(
        r,
        Err(LawError::Law(
            "deliberate error from the multiple-output fixture".into()
        ))
    );
    assert!(!law.is_poisoned());
}

// -------------------------------------------------------------------------
// The adapter: a law file as the controller's MIMO law
// -------------------------------------------------------------------------

#[test]
fn a_law_file_runs_as_the_controllers_mimo_law_and_keeps_its_first_fault() {
    let grid = Grid::new();
    let law = load(&host(), MISBEHAVE);
    let budget = Duration::from_millis(50);
    let mut w = WasmMimoLaw::build(&law, &grid.setup, &mode("spin"), budget).expect("builds");
    assert_eq!(w.needs(), [MimoInput::LastDrive]);
    assert_eq!(w.shapes().len(), 3);
    let dyn_law: &mut dyn MimoControlLaw = &mut w;
    dyn_law.seed(&grid.inputs(0)).expect("seeds");
    let first = within(Duration::from_secs(60), "the spin", move || {
        let grid = Grid::new();
        let e = w.iterate(&grid.inputs(1)).expect_err("the spin is stopped");
        let again = w.iterate(&grid.inputs(2)).expect_err("still stopped");
        (w, e, again)
    });
    let (w, e, again) = first;
    assert!(e.0.contains("budget"), "{e}");
    assert_eq!(w.last_error(), Some(&LawError::Deadline { budget }));
    // The second call reports the first fault, not "poisoned".
    assert_eq!(again, e);
    assert!(w.instance().is_poisoned());
}

/// Mutation: arm the call budget for `seed` in `WasmMimoLaw::seed`. The first
/// solve is then stopped at 50 ms and the error names that budget.
#[test]
fn a_law_files_first_solve_runs_to_its_own_deadline() {
    let call = Duration::from_millis(50);
    let seed = Duration::from_millis(400);
    let law = load(&host(), MISBEHAVE);

    // With no seed budget set, the first solve has the call budget.
    let grid = Grid::new();
    let w = WasmMimoLaw::build(&law, &grid.setup, &mode("spin_seed"), call).expect("builds");
    assert_eq!((w.budget(), w.seed_budget()), (call, call));
    let (w, e) = within(
        Duration::from_secs(60),
        "the spinning first solve",
        move || {
            let mut w = w;
            let e = w.seed(&Grid::new().inputs(0)).expect_err("stopped");
            (w, e)
        },
    );
    assert!(e.0.contains("budget"), "{e}");
    assert_eq!(w.last_error(), Some(&LawError::Deadline { budget: call }));

    // With one, the first solve runs to it; a call after it keeps the call budget.
    let w = WasmMimoLaw::build(&law, &grid.setup, &mode("spin_seed"), call)
        .expect("builds")
        .with_seed_budget(seed);
    assert_eq!((w.budget(), w.seed_budget()), (call, seed));
    let (w, e, took) = within(
        Duration::from_secs(60),
        "the spinning first solve",
        move || {
            let mut w = w;
            let t0 = Instant::now();
            let e = w.seed(&Grid::new().inputs(0)).expect_err("stopped");
            (w, e, t0.elapsed())
        },
    );
    assert!(e.0.contains("budget"), "{e}");
    assert_eq!(w.last_error(), Some(&LawError::Deadline { budget: seed }));
    assert!(took >= seed, "stopped after {took:?}, before its {seed:?}");

    let mut w = WasmMimoLaw::build(&law, &grid.setup, &mode("spin"), call)
        .expect("builds")
        .with_seed_budget(seed);
    w.seed(&grid.inputs(0)).expect("seeds");
    let (w, _) = within(Duration::from_secs(60), "the spin", move || {
        let e = w.iterate(&Grid::new().inputs(1)).expect_err("stopped");
        (w, e)
    });
    assert_eq!(w.last_error(), Some(&LawError::Deadline { budget: call }));
}

#[test]
fn mimo_handles_move_between_threads() {
    fn send<T: Send>() {}
    fn send_sync<T: Send + Sync>() {}
    send::<MimoLawInstance>();
    send::<WasmMimoLaw>();
    send_sync::<LoadedMimoLaw>();
    let _ = MimoDims {
        lines: LINES,
        controls: NC,
        drives: ND,
        band: BAND,
        df_hz: 1.0,
    };
}
