//! The law host end to end, against law files built by
//! `scripts/build_laws.sh`: the per-line ratio law and the misbehaving
//! fixtures in `crates/hertzforge-law-host/guests/`.
//!
//! A test that needs a law file fails, naming the script, when the file has
//! not been built. Nothing here is skipped for a missing file.

use hertzforge_law_host::{
    law_file_sha256, HostConfig, LawError, LawHost, LawInstance, LoadedLaw, LoopInputs,
    LoopOutputs, OutputRule, Param, ParamValue, Setup,
};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

// -------------------------------------------------------------------------
// Finding the built law files
// -------------------------------------------------------------------------

/// The bytes of `<name>.wasm`, built by scripts/build_laws.sh into
/// `<target>/wasm32-wasip2/release/`. The target directory is found from
/// this test binary's own path (`<target>/<profile>/deps/<binary>`), so it is
/// the one cargo built the test into, `CARGO_TARGET_DIR` or not.
/// `HERTZFORGE_LAW_WASM_DIR` overrides the directory.
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

const RATIO: &str = "hertzforge_law_ratio";
const MISBEHAVE: &str = "law_fixture_misbehave";
const BAD_MANIFEST: &str = "law_fixture_bad_manifest";

/// A generous budget for calls that are expected to finish.
const BUDGET: Duration = Duration::from_secs(5);

fn host() -> LawHost {
    LawHost::new(HostConfig::default()).expect("the host starts")
}

fn load(host: &LawHost, name: &str) -> LoadedLaw {
    host.load(&law_bytes(name))
        .unwrap_or_else(|e| panic!("{name} loads: {e}"))
}

/// Run `f` on its own thread and fail, rather than hang, if it does not
/// return within `limit`. Only guards against a hang: the limits are far
/// above what the calls take.
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
// A small grid for the fixtures
// -------------------------------------------------------------------------

const SMALL_N: usize = 16;
const SMALL_BAND: (usize, usize) = (2, 14);

struct Grid {
    target: Vec<f64>,
    measured: Vec<f64>,
    h_mag2: Vec<f64>,
    coherence: Vec<f64>,
}

impl Grid {
    fn small() -> Self {
        let target = (0..SMALL_N)
            .map(|k| {
                if (SMALL_BAND.0..SMALL_BAND.1).contains(&k) {
                    0.01
                } else {
                    0.0
                }
            })
            .collect();
        Grid {
            target,
            measured: vec![0.008; SMALL_N],
            h_mag2: vec![1.0; SMALL_N],
            coherence: vec![0.9; SMALL_N],
        }
    }

    fn setup(&self) -> Setup<'_> {
        Setup {
            target_psd: &self.target,
            df_hz: 5.0,
            band: SMALL_BAND,
            control_channels: 1,
        }
    }

    fn inputs(&self, loop_index: u64) -> LoopInputs<'_> {
        LoopInputs {
            target_psd: &self.target,
            measured_psd: &self.measured,
            h_mag2: &self.h_mag2,
            coherence: &self.coherence,
            current_level_db: 0.0,
            df_hz: 5.0,
            notched_bins: &[],
            band: SMALL_BAND,
            dof: 16.0,
            loop_index,
        }
    }
}

fn mode(m: &str) -> Vec<Param> {
    vec![Param::new("mode", ParamValue::Choice(m.into()))]
}

/// A misbehaving-fixture instance in `m` mode, already seeded.
fn misbehave_in(host: &LawHost, m: &str) -> LawInstance {
    let grid = Grid::small();
    let mut law = load(host, MISBEHAVE)
        .instantiate(&grid.setup(), &mode(m), BUDGET)
        .unwrap_or_else(|e| panic!("the fixture builds in {m} mode: {e}"));
    law.seed(&grid.inputs(0), BUDGET)
        .unwrap_or_else(|e| panic!("the fixture seeds in {m} mode: {e}"));
    law
}

/// One iterate on the small grid, guarded against a hang.
fn iterate_once(
    mut law: LawInstance,
    budget: Duration,
) -> (LawInstance, Result<LoopOutputs, LawError>, Duration) {
    within(Duration::from_secs(60), "the iterate call", move || {
        let grid = Grid::small();
        let t0 = Instant::now();
        let r = law.iterate(&grid.inputs(1), budget);
        let took = t0.elapsed();
        (law, r, took)
    })
}

// -------------------------------------------------------------------------
// Loading, manifests, identity
// -------------------------------------------------------------------------

#[test]
fn the_ratio_law_loads_with_its_manifest_and_identity() {
    let bytes = law_bytes(RATIO);
    let law = host().load(&bytes).expect("the ratio law loads");
    let m = law.manifest();
    assert_eq!(m.name, "per-line ratio");
    assert_eq!(m.version, "0.1.0");
    let keys: Vec<&str> = m.params.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(keys, ["gain", "max_step_db", "coherence_floor"]);
    let diags: Vec<&str> = m.diagnostics.iter().map(|d| d.key.as_str()).collect();
    assert_eq!(diags, ["untrusted_lines", "clamped_lines"]);
    let id = law.identity();
    assert_eq!(id.name, m.name);
    assert_eq!(id.version, m.version);
    assert_eq!(id.sha256, law_file_sha256(&bytes));
    assert_eq!(id.sha256.len(), 64);
    assert_eq!(id.short_hash(), &id.sha256[..12]);
}

#[test]
fn the_identity_hash_is_the_files_sha256_and_stable() {
    // Known answer (FIPS 180-2, "abc"): the hash is SHA-256 itself.
    assert_eq!(
        law_file_sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let bytes = law_bytes(RATIO);
    let h = host();
    let a = h.load(&bytes).expect("loads").identity().clone();
    let b = h.load(&bytes).expect("loads again").identity().clone();
    assert_eq!(a, b, "the same file gives the same identity");
    // A custom section appended to the component changes the file, not the
    // law: same manifest, a different hash.
    let mut changed = bytes.clone();
    let name = b"hertzforge-test";
    let mut content = vec![name.len() as u8];
    content.extend_from_slice(name);
    content.push(b'x');
    changed.push(0x00);
    changed.push(content.len() as u8);
    changed.extend_from_slice(&content);
    let c = h
        .load(&changed)
        .expect("the changed file still loads")
        .identity()
        .clone();
    assert_eq!(
        (c.name.as_str(), c.version.as_str()),
        (a.name.as_str(), a.version.as_str())
    );
    assert_ne!(
        c.sha256, a.sha256,
        "the hash is of the file, not the manifest"
    );
}

#[test]
fn bytes_that_are_not_a_law_are_refused() {
    let h = host();
    let garbage = h.load(b"not a law at all");
    assert!(
        matches!(garbage, Err(LawError::Compile(_))),
        "{:?}",
        garbage.err()
    );
    let core_module = h.load(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);
    assert!(
        matches!(core_module, Err(LawError::Compile(_))),
        "a core module is not a component: {:?}",
        core_module.err()
    );
    // A valid, empty component: it compiles, but exports nothing.
    let empty = h.load(&[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00]);
    assert!(
        matches!(empty, Err(LawError::NotALaw(_))),
        "an empty component is not a law: {:?}",
        empty.err()
    );
}

#[test]
fn a_manifest_that_breaks_a_rule_is_refused_at_load() {
    match host().load(&law_bytes(BAD_MANIFEST)) {
        Err(LawError::ManifestInvalid(why)) => {
            assert!(why.contains("name is empty"), "{why}")
        }
        Err(e) => panic!("expected the manifest to be refused, got {e}"),
        Ok(_) => panic!("a law with an empty name loaded"),
    }
}

#[test]
fn parameter_values_are_checked_before_the_law_is_built() {
    let h = host();
    let law = load(&h, RATIO);
    let grid = Grid::small();
    let too_much = [Param::new("gain", ParamValue::Number(1.5))];
    match law.instantiate(&grid.setup(), &too_much, BUDGET) {
        Err(LawError::ParamInvalid { key, .. }) => assert_eq!(key, "gain"),
        Err(e) => panic!("expected gain to be refused, got {e}"),
        Ok(_) => panic!("a gain of 1.5 was accepted"),
    }
    let unknown = [Param::new("damping", ParamValue::Number(1.0))];
    assert!(matches!(
        law.instantiate(&grid.setup(), &unknown, BUDGET),
        Err(LawError::ParamInvalid { .. })
    ));
    let bad_setup = Setup {
        band: (0, SMALL_N + 1),
        ..grid.setup()
    };
    assert!(matches!(
        law.instantiate(&bad_setup, &[], BUDGET),
        Err(LawError::SetupInvalid(_))
    ));
    let inst = law
        .instantiate(
            &grid.setup(),
            &[Param::new("gain", ParamValue::Number(0.25))],
            BUDGET,
        )
        .expect("good values build the law");
    assert_eq!(
        inst.params(),
        [
            Param::new("gain", ParamValue::Number(0.25)),
            Param::new("max_step_db", ParamValue::Number(2.0)),
            Param::new("coherence_floor", ParamValue::Number(0.5)),
        ]
    );
}

#[test]
fn a_law_can_refuse_its_own_setup() {
    // The host accepts an all-zero target; the ratio law does not, and says
    // why. The refusal is the law's own error, not a trap.
    let h = host();
    let zero = vec![0.0; SMALL_N];
    let setup = Setup {
        target_psd: &zero,
        df_hz: 5.0,
        band: SMALL_BAND,
        control_channels: 1,
    };
    match load(&h, RATIO).instantiate(&setup, &[], BUDGET) {
        Err(LawError::Law(why)) => assert!(why.contains("no in-band energy"), "{why}"),
        Err(e) => panic!("expected the law's own refusal, got {e}"),
        Ok(_) => panic!("a zero target was accepted"),
    }
}

// -------------------------------------------------------------------------
// The ratio law against a native reference
// -------------------------------------------------------------------------

/// The per-line ratio law written again, natively, from its description
/// (laws/ratio/src/lib.rs's module doc). The WebAssembly law must agree with
/// it.
struct NativeRatio {
    target: Vec<f64>,
    lo: usize,
    hi: usize,
    full_rms: f64,
    gain: f64,
    max_step_db: f64,
    coherence_floor: f64,
    reference: Vec<f64>,
    drive: Vec<f64>,
    scale: f64,
}

impl NativeRatio {
    fn new(target: &[f64], band: (usize, usize), df: f64, params: (f64, f64, f64)) -> Self {
        let (lo, hi) = band;
        NativeRatio {
            target: target.to_vec(),
            lo,
            hi,
            full_rms: (target[lo..hi].iter().sum::<f64>() * df).sqrt(),
            gain: params.0,
            max_step_db: params.1,
            coherence_floor: params.2,
            reference: vec![0.0; target.len()],
            drive: vec![0.0; target.len()],
            scale: 1.0,
        }
    }

    // One index into four per-line arrays reads plainer than a zip of four.
    #[allow(clippy::needless_range_loop)]
    fn seed(&mut self, h2: &[f64]) {
        let peak = h2[self.lo..self.hi].iter().copied().fold(0.0, f64::max);
        let floor = peak * 1e-3;
        for k in self.lo..self.hi {
            self.reference[k] = self.target[k] / h2[k].max(floor);
            self.drive[k] = self.reference[k] * self.scale * self.scale;
        }
    }

    /// Returns (untrusted, clamped) lines.
    fn iterate(
        &mut self,
        measured: &[f64],
        coherence: &[f64],
        notched: &[bool],
    ) -> (Vec<usize>, Vec<usize>) {
        let (mut untrusted, mut clamped) = (Vec::new(), Vec::new());
        for k in self.lo..self.hi {
            if coherence[k] < self.coherence_floor {
                untrusted.push(k);
                continue;
            }
            if notched.get(k).copied().unwrap_or(false) {
                continue;
            }
            let t = (self.target[k] * self.scale * self.scale).max(1e-30);
            let m = measured[k].max(1e-30);
            let raw = self.gain * 10.0 * (t / m).log10();
            let step = raw.clamp(-self.max_step_db, self.max_step_db);
            if step != raw {
                clamped.push(k);
            }
            self.drive[k] *= 10f64.powf(step / 10.0);
        }
        (untrusted, clamped)
    }

    fn retarget(&mut self, level_db: f64) {
        let s = 10f64.powf(level_db / 20.0);
        let r = (s / self.scale).powi(2);
        for d in &mut self.drive[self.lo..self.hi] {
            *d *= r;
        }
        self.scale = s;
    }

    fn level_cmd_db(&self) -> f64 {
        let r: f64 = self.reference[self.lo..self.hi].iter().sum();
        let d: f64 = self.drive[self.lo..self.hi].iter().sum();
        10.0 * (d / r).log10()
    }
}

/// A single-mode plant on a 400-line grid: the target, the true and the
/// estimated `|H|^2` (the estimate off by up to 1.5 dB), the coherence (ten
/// lines below the trust floor) and the band.
struct Plant {
    n: usize,
    df: f64,
    band: (usize, usize),
    target: Vec<f64>,
    h_true: Vec<f64>,
    h_est: Vec<f64>,
    coherence: Vec<f64>,
}

impl Plant {
    fn new(n: usize) -> Self {
        let df = 1000.0 / n as f64;
        let band = (n / 50, n * 9 / 10);
        let (f0, zeta) = (300.0, 0.05);
        let mut p = Plant {
            n,
            df,
            band,
            target: vec![0.0; n],
            h_true: vec![0.0; n],
            h_est: vec![0.0; n],
            coherence: vec![0.95; n],
        };
        for k in 0..n {
            let f = k as f64 * df;
            let r = f / f0;
            p.h_true[k] = 1.0 / ((1.0 - r * r).powi(2) + (2.0 * zeta * r).powi(2));
            p.h_est[k] = p.h_true[k] * 10f64.powf(0.15 * (0.05 * k as f64).sin());
            if (band.0..band.1).contains(&k) {
                p.target[k] = 0.01 * (1.0 + 3.0 * (-((f - 400.0) / 60.0).powi(2)).exp());
            }
        }
        for c in &mut p.coherence[n / 4..n / 4 + 10] {
            *c = 0.2;
        }
        p
    }

    /// The measured response to `drive` on loop `l`: the true plant with a
    /// deterministic +/-0.5 dB wobble standing in for estimation noise.
    fn measure(&self, drive: &[f64], l: usize) -> Vec<f64> {
        (0..self.n)
            .map(|k| {
                drive[k] * self.h_true[k] * 10f64.powf(0.05 * (0.37 * k as f64 + l as f64).sin())
            })
            .collect()
    }

    fn setup(&self) -> Setup<'_> {
        Setup {
            target_psd: &self.target,
            df_hz: self.df,
            band: self.band,
            control_channels: 1,
        }
    }
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    a == b || (a - b).abs() <= rel * a.abs().max(b.abs())
}

#[test]
fn the_wasm_ratio_law_agrees_with_a_native_reference() {
    // Agreement within 1e-9, relative per line and absolute in dB on the
    // level. Not bit-identical: log10 and powf come from different math
    // libraries in the law file and in this test.
    const TOL: f64 = 1e-9;
    let plant = Plant::new(400);
    let (gain, max_step, floor) = (0.6, 1.0, 0.5);
    let params = [
        Param::new("gain", ParamValue::Number(gain)),
        Param::new("max_step_db", ParamValue::Number(max_step)),
        Param::new("coherence_floor", ParamValue::Number(floor)),
    ];
    let mut wasm = load(&host(), RATIO)
        .instantiate(&plant.setup(), &params, BUDGET)
        .expect("the ratio law builds");
    let mut native = NativeRatio::new(&plant.target, plant.band, plant.df, (gain, max_step, floor));

    let zeros = vec![0.0; plant.n];
    let seed_in = LoopInputs {
        target_psd: &plant.target,
        measured_psd: &zeros,
        h_mag2: &plant.h_est,
        coherence: &plant.coherence,
        current_level_db: 0.0,
        df_hz: plant.df,
        notched_bins: &[],
        band: plant.band,
        dof: 16.0,
        loop_index: 0,
    };
    let seeded = wasm.seed(&seed_in, BUDGET).expect("seeds");
    native.seed(&plant.h_est);
    let seed_drive = native.drive.clone();
    for k in 0..plant.n {
        assert!(
            close(seeded.drive_psd[k], native.drive[k], TOL),
            "seed line {k}"
        );
    }

    let mut notched = vec![false; plant.n];
    let (mut clamped_total, mut first_err, mut last_err) = (0usize, None, 0.0);
    for l in 1..=24usize {
        let level_db = match l {
            10 => Some(-6.0),
            18 => Some(0.0),
            _ => None,
        };
        if let Some(level_db) = level_db {
            let rms = native.full_rms * 10f64.powf(level_db / 20.0);
            let out = wasm
                .retarget_level(level_db, rms, BUDGET)
                .expect("retargets");
            native.retarget(level_db);
            for k in 0..plant.n {
                assert!(
                    close(out.drive_psd[k], native.drive[k], TOL),
                    "retarget at loop {l}, line {k}"
                );
            }
            assert!(
                (out.level_cmd_db - native.level_cmd_db()).abs() <= TOL,
                "retarget at loop {l}: level"
            );
        }
        if l == 6 {
            for x in &mut notched[200..205] {
                *x = true;
            }
        }
        // Both laws see the same measurement, made from the reference's drive.
        let measured = plant.measure(&native.drive, l);
        let inputs = LoopInputs {
            measured_psd: &measured,
            notched_bins: &notched,
            current_level_db: 20.0 * native.scale.log10(),
            loop_index: l as u64,
            ..seed_in
        };
        let out = wasm.iterate(&inputs, BUDGET).expect("iterates");
        let (untrusted, clamped) = native.iterate(&measured, &plant.coherence, &notched);

        for k in 0..plant.n {
            assert!(
                close(out.drive_psd[k], native.drive[k], TOL),
                "loop {l} line {k}: law {} reference {}",
                out.drive_psd[k],
                native.drive[k]
            );
        }
        assert!(
            (out.level_cmd_db - native.level_cmd_db()).abs() <= TOL,
            "loop {l} level"
        );
        assert_eq!(
            out.diagnostics.untrusted_bins, untrusted,
            "loop {l} untrusted"
        );
        assert_eq!(out.diagnostics.clamped_bins, clamped, "loop {l} clamped");
        assert_eq!(
            out.diagnostics.value("untrusted_lines"),
            Some(untrusted.len() as f64)
        );
        assert_eq!(
            out.diagnostics.value("clamped_lines"),
            Some(clamped.len() as f64)
        );
        clamped_total += clamped.len();
        let err = out
            .diagnostics
            .inband_err_db
            .expect("an in-band error is reported");
        first_err.get_or_insert(err);
        last_err = err;
    }
    // The comparison is not vacuous: the law moved the drive, hit its step
    // limit, held the untrusted lines, and converged.
    assert!(clamped_total > 0, "no line was ever clamped");
    let moved = (plant.band.0..plant.band.1)
        .filter(|&k| !close(native.drive[k], seed_drive[k], 1e-3))
        .count();
    assert!(
        moved > plant.n / 2,
        "only {moved} lines moved from the seed"
    );
    let first_err = first_err.expect("loops ran");
    assert!(
        last_err < first_err,
        "no convergence: {first_err} dB then {last_err} dB"
    );
}

// -------------------------------------------------------------------------
// Containment
// -------------------------------------------------------------------------

#[test]
fn a_trapping_law_is_reported_and_its_instance_poisoned() {
    let h = host();
    let (mut law, r, _) = iterate_once(misbehave_in(&h, "trap"), BUDGET);
    match r {
        Err(LawError::Trap(why)) => assert!(
            why.contains("deliberate trap from the misbehaving fixture"),
            "the trap carries what the law wrote: {why}"
        ),
        other => panic!("expected a trap, got {other:?}"),
    }
    assert!(law.is_poisoned());
    let grid = Grid::small();
    assert!(matches!(
        law.iterate(&grid.inputs(2), BUDGET),
        Err(LawError::Poisoned(_))
    ));
    assert!(matches!(
        law.retarget_level(0.0, 1.0, BUDGET),
        Err(LawError::Poisoned(_))
    ));
}

#[test]
fn a_law_that_never_returns_is_stopped_by_its_deadline() {
    let h = host();
    let budget = Duration::from_millis(50);
    let (law, r, _) = iterate_once(misbehave_in(&h, "spin"), budget);
    assert_eq!(r, Err(LawError::Deadline { budget }));
    assert!(law.is_poisoned());
}

/// Timing list: the deadline lands within a few ticks of the budget, well
/// within a second.
#[test]
fn the_deadline_stops_a_spinning_law_within_a_tick_of_its_budget() {
    let h = host();
    let budget = Duration::from_millis(50);
    let mut worst = Duration::ZERO;
    for _ in 0..5 {
        let (_, r, took) = iterate_once(misbehave_in(&h, "spin"), budget);
        assert_eq!(r, Err(LawError::Deadline { budget }));
        assert!(took >= budget, "stopped early, after {took:?}");
        worst = worst.max(took - budget);
    }
    println!("deadline overshoot, worst of 5 at a 50 ms budget: {worst:?}");
    assert!(worst < Duration::from_millis(100), "overshoot {worst:?}");
}

#[test]
fn a_law_that_grows_past_its_memory_cap_is_stopped() {
    let limit = 32 * 1024 * 1024;
    let h = LawHost::new(HostConfig {
        memory_limit_bytes: limit,
        ..HostConfig::default()
    })
    .expect("the host starts");
    let (law, r, _) = iterate_once(misbehave_in(&h, "grow"), BUDGET);
    match r {
        Err(LawError::Memory {
            limit_bytes,
            requested_bytes,
        }) => {
            assert_eq!(limit_bytes, limit);
            assert!(requested_bytes > limit);
        }
        other => panic!("expected a memory error, got {other:?}"),
    }
    assert!(law.is_poisoned());
}

#[test]
fn a_law_cannot_open_files_or_the_network() {
    let h = host();
    let (_, r, _) = iterate_once(misbehave_in(&h, "open_file"), BUDGET);
    match r {
        Err(LawError::Law(why)) => assert!(why.contains("every escape failed"), "{why}"),
        Ok(out) => panic!(
            "the law reached the system: {} of its attempts succeeded",
            out.diagnostics.value("escaped").unwrap_or(-1.0)
        ),
        Err(e) => panic!("expected the law's own report of failure, got {e}"),
    }
}

#[test]
fn a_sleeping_law_does_not_hold_the_caller() {
    // The law sleeps 30 s. With real sleeps the call would return after 30 s
    // (a sleep is a host call, out of the deadline's reach), and the guard
    // here fails at 10 s; the frozen clock makes the sleep return at once.
    let h = host();
    let law = misbehave_in(&h, "sleep");
    let r = within(Duration::from_secs(10), "a sleeping law", move || {
        let mut law = law;
        law.iterate(&Grid::small().inputs(1), Duration::from_secs(60))
    });
    assert!(r.is_ok(), "{r:?}");
}

#[test]
fn time_and_randomness_are_fixed_and_nothing_is_inherited() {
    let h = host();
    let (_, r, _) = iterate_once(misbehave_in(&h, "probe"), BUDGET);
    let out = r.expect("the probe returns");
    let v = |k: &str| out.diagnostics.value(k).expect("the probe reports it");
    assert_eq!(v("wall_clock_ns"), 0.0, "the wall clock reads the epoch");
    assert_eq!(v("busy_loop_ns"), 0.0, "the monotonic clock does not move");
    assert_eq!(v("env_vars"), 0.0, "no environment is inherited");
    assert_eq!(v("args"), 0.0, "no arguments are inherited");
}

#[test]
fn two_instances_given_the_same_inputs_return_identical_outputs() {
    let h = host();
    // The probe: what a law could use to tell two runs apart is fixed.
    let (_, a, _) = iterate_once(misbehave_in(&h, "probe"), BUDGET);
    let (_, b, _) = iterate_once(misbehave_in(&h, "probe"), BUDGET);
    let (a, b) = (a.expect("probe a"), b.expect("probe b"));
    assert_eq!(
        a.diagnostics.value("hash_probe").map(f64::to_bits),
        b.diagnostics.value("hash_probe").map(f64::to_bits),
        "the random source differs between instances"
    );
    assert_eq!(a, b);

    // The ratio law over a whole run: bit for bit.
    let plant = Plant::new(200);
    let law = load(&h, RATIO);
    let run = || {
        let mut inst = law
            .instantiate(&plant.setup(), &[], BUDGET)
            .expect("builds");
        let mut drives = Vec::new();
        let zeros = vec![0.0; plant.n];
        let inputs = LoopInputs {
            target_psd: &plant.target,
            measured_psd: &zeros,
            h_mag2: &plant.h_est,
            coherence: &plant.coherence,
            current_level_db: 0.0,
            df_hz: plant.df,
            notched_bins: &[],
            band: plant.band,
            dof: 16.0,
            loop_index: 0,
        };
        let mut out = inst.seed(&inputs, BUDGET).expect("seeds");
        for l in 1..=8 {
            let measured = plant.measure(&out.drive_psd, l);
            let step = LoopInputs {
                measured_psd: &measured,
                loop_index: l as u64,
                ..inputs
            };
            out = inst.iterate(&step, BUDGET).expect("iterates");
            drives.push(
                out.drive_psd
                    .iter()
                    .map(|x| x.to_bits())
                    .collect::<Vec<_>>(),
            );
        }
        drives
    };
    assert_eq!(run(), run());
}

#[test]
fn each_bad_output_is_refused_without_poisoning_the_instance() {
    let h = host();
    for (m, want) in [
        ("nan", OutputRule::NonFiniteDrive),
        ("short", OutputRule::WrongLength),
        ("undeclared", OutputRule::Diagnostics),
    ] {
        let (law, r, _) = iterate_once(misbehave_in(&h, m), BUDGET);
        assert!(
            matches!(&r, Err(LawError::BadOutput { rule, .. }) if *rule == want),
            "{m}: {r:?}"
        );
        assert!(!law.is_poisoned(), "{m}: a refused output is not a trap");
    }
}

#[test]
fn a_laws_own_error_is_passed_through() {
    let h = host();
    let (law, r, _) = iterate_once(misbehave_in(&h, "law_error"), BUDGET);
    assert_eq!(
        r,
        Err(LawError::Law(
            "deliberate error from the misbehaving fixture".into()
        ))
    );
    assert!(!law.is_poisoned());
}

#[test]
fn output_is_kept_bounded_and_never_stops_the_law() {
    let h = LawHost::new(HostConfig {
        capture_bytes: 4096,
        ..HostConfig::default()
    })
    .expect("the host starts");
    let (law, r, _) = iterate_once(misbehave_in(&h, "print"), BUDGET);
    assert!(r.is_ok(), "{r:?}");
    let out = law.guest_output();
    assert_eq!(out.stdout.len(), 4096);
    assert_eq!(out.stderr.len(), 4096);
    assert_eq!(out.stdout_dropped, 64 * 1024 - 4096);
    assert_eq!(out.stderr_dropped, 64 * 1024 - 4096);
}

#[test]
fn malformed_inputs_are_refused_before_the_law_sees_them() {
    let h = host();
    let mut law = misbehave_in(&h, "ok");
    let grid = Grid::small();
    let short = vec![0.0; SMALL_N - 1];
    let ragged = LoopInputs {
        measured_psd: &short,
        ..grid.inputs(1)
    };
    assert!(matches!(
        law.iterate(&ragged, BUDGET),
        Err(LawError::InputsInvalid(_))
    ));
    assert!(!law.is_poisoned());
    law.iterate(&grid.inputs(1), BUDGET)
        .expect("the instance still works");
}

#[test]
fn law_handles_move_between_threads() {
    fn send<T: Send>() {}
    fn send_sync<T: Send + Sync>() {}
    send::<LawInstance>();
    send_sync::<LoadedLaw>();
    send_sync::<LawHost>();
}

// -------------------------------------------------------------------------
// Cost
// -------------------------------------------------------------------------

/// Timing list: the median cost of one `iterate` call on a 1,000-line grid,
/// host side included (checking and copying the inputs and outputs). A loop
/// takes tenths of a second; the bound is far below that.
#[test]
fn iterate_on_a_1000_line_grid_takes_well_under_a_loop() {
    let plant = Plant::new(1000);
    let mut law = load(&host(), RATIO)
        .instantiate(&plant.setup(), &[], BUDGET)
        .expect("builds");
    let zeros = vec![0.0; plant.n];
    let mut inputs = LoopInputs {
        target_psd: &plant.target,
        measured_psd: &zeros,
        h_mag2: &plant.h_est,
        coherence: &plant.coherence,
        current_level_db: 0.0,
        df_hz: plant.df,
        notched_bins: &[],
        band: plant.band,
        dof: 16.0,
        loop_index: 0,
    };
    let seeded = law.seed(&inputs, BUDGET).expect("seeds");
    let measured = plant.measure(&seeded.drive_psd, 1);
    inputs.measured_psd = &measured;
    let mut times = Vec::new();
    for i in 0..220 {
        let t0 = Instant::now();
        law.iterate(&inputs, BUDGET).expect("iterates");
        if i >= 20 {
            times.push(t0.elapsed());
        }
    }
    times.sort();
    let median = times[times.len() / 2];
    println!(
        "iterate on 1000 lines: median {median:?}, fastest {:?}, slowest {:?} over {} calls",
        times[0],
        times[times.len() - 1],
        times.len()
    );
    assert!(median < Duration::from_millis(10), "median {median:?}");
}
