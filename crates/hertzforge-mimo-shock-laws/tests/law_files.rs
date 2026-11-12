//! A MIMO Shock law file through the laws crate: resolved from a catalog by
//! its id or loaded by path, listed with the built-ins, its facts read from its
//! manifest and its check record, and its faults reported to the core by name.

use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_mimo_shock::{
    MimoShockLawFactory, PlantStack, Setup, Target, TargetKind, Timing, WaveformTarget,
};
use hertzforge_mimo_shock_laws::{
    all_facts, builtin_facts, check_record, entries, load_law_file, resolve, BuiltinMimoShock,
    CaseVerdict, MimoShockCheckCase, ResolveError,
};
use hertzforge_random_laws::{CheckStatus, LawCatalog, LawWorld};
use hertzforge_sdm::{MatrixStack, C64};
use hertzforge_shock_common::law_guard::LawFault;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().expect("the test binary knows its own path");
    for dir in exe.ancestors().skip(1).take(4) {
        let p = dir.join("wasm32-wasip2").join("release").join(&file);
        if p.is_file() {
            return p;
        }
    }
    panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "hertzforge-mimo-shock-laws-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn catalog(root: &Path) -> LawCatalog {
    LawCatalog::with_host(LawHost::new(HostConfig::default()).unwrap(), root).unwrap()
}

const LEN: usize = 64;

fn setup() -> Setup {
    let (nc, nd, lines) = (2, 3, LEN / 2 + 1);
    let mut frf = MatrixStack::zeros(lines, nc, nd);
    for l in 0..lines {
        for r in 0..nc {
            frf.set(l, r, r, C64::new(1.0, 0.0));
        }
    }
    Setup {
        fs_hz: 1024.0,
        record_len: LEN,
        n_drives: nd,
        n_controls: nc,
        band_lo_hz: 10.0,
        band_hi_hz: 400.0,
        target: Target::Waveform(WaveformTarget {
            records: (0..nc * LEN)
                .map(|i| ((i % LEN) as f64 * 0.3).sin())
                .collect(),
        }),
        timing: Timing::Free { window_s: 0.002 },
        plant: PlantStack {
            frf,
            coherence: vec![1.0; nc * lines],
            trusted: vec![true; lines],
        },
        drive_ceiling_v: vec![1.0; nd],
        ladder_db: vec![-6.0, 0.0],
    }
}

fn inputs(s: &Setup, pulse_index: u64) -> hertzforge_mimo_shock::PulseInputs {
    hertzforge_mimo_shock::PulseInputs {
        pulse_index,
        level_db: -6.0,
        next_level_db: 0.0,
        drives: vec![0.0; s.n_drives * LEN],
        responses: vec![0.0; s.n_controls * LEN],
        srs: None,
        verdicts: vec![
            hertzforge_mimo_shock::ControlVerdict {
                passed: false,
                error_db: 1.0,
                delay_s: 0.0,
                peak_ratio: 1.0,
            };
            s.n_controls
        ],
        limited: false,
    }
}

#[test]
fn mimo_shock_a_law_file_resolves_from_the_catalog_and_runs_as_the_core_law() {
    let scratch = Scratch::new("resolve");
    let mut cat = catalog(scratch.path());
    let id = cat
        .import(&law_file("hertzforge_law_mimo_shock_paired"))
        .unwrap()
        .id;
    let f = resolve(&cat, &id, Vec::new(), Duration::from_secs(5)).unwrap();
    let m = f.manifest();
    assert_eq!(m.targets, vec![TargetKind::Waveform, TargetKind::Srs]);
    assert_eq!(m.values, vec!["drive_rms_full_level".to_string()]);
    let d = f.describe();
    assert_eq!((d.id.as_str(), d.name.as_str()), (id.as_str(), "paired"));
    assert_eq!(d.sha256.as_deref(), Some(id.as_str()));
    assert_eq!(d.params["weight"], 0.8);
    let s = setup();
    let mut law = f.build(&s).unwrap();
    assert_eq!(law.name(), "paired");
    assert_eq!(law.seed(-6.0).unwrap().drives.len(), 3 * LEN);
    assert_eq!(law.iterate(&inputs(&s, 1)).unwrap().drives.len(), 3 * LEN);

    // A built-in id goes to the built-in branch; an unknown id is unknown; a
    // law file of another world is named as such.
    assert_eq!(
        resolve(&cat, "shape", Vec::new(), Duration::from_secs(1)).err(),
        Some(ResolveError::NotBuilt("shape"))
    );
    assert_eq!(
        resolve(&cat, "nope", Vec::new(), Duration::from_secs(1)).err(),
        Some(ResolveError::Unknown("nope".into()))
    );
    let srs = cat
        .import(&law_file("hertzforge_law_shock_srs_ratio"))
        .unwrap()
        .id;
    match resolve(&cat, &srs, Vec::new(), Duration::from_secs(1)) {
        Err(ResolveError::WrongWorld { law_is_for, .. }) => assert_eq!(law_is_for, "SRS shock"),
        other => panic!("{:?}", other.err()),
    }
}

#[test]
fn mimo_shock_a_law_file_loads_by_path_and_a_missing_one_is_named() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = load_law_file(&host, &law_file("hertzforge_law_mimo_shock_inverse")).unwrap();
    assert_eq!(law.manifest().law.name, "inverse");
    match load_law_file(&host, Path::new("/no/such/law.wasm")) {
        Err(ResolveError::File(m)) => assert!(m.contains("/no/such/law.wasm"), "{m}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    match load_law_file(&host, &law_file("hertzforge_law_shock_fdc")) {
        Err(ResolveError::File(m)) => assert!(m.contains("not a MIMO Shock law"), "{m}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn mimo_shock_the_picker_lists_the_builtins_first_and_states_their_failures() {
    let scratch = Scratch::new("facts");
    let mut cat = catalog(scratch.path());
    let id = cat
        .import(&law_file("hertzforge_law_mimo_shock_shape"))
        .unwrap()
        .id;
    let listed = entries(&cat);
    let ids: Vec<&str> = listed.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["inverse", "shape", "paired", id.as_str()]);
    assert!(listed.iter().all(|e| e.world == LawWorld::ShockMimo));
    assert!(listed[..3].iter().all(|e| e.builtin));

    let shape = builtin_facts(BuiltinMimoShock::Shape);
    assert_eq!(shape.targets, vec![TargetKind::Srs]);
    assert!(shape
        .known_failures
        .iter()
        .any(|f| f.case == MimoShockCheckCase::Jointed4x4Severe));
    assert!(shape.params.iter().any(|p| p.key == "anchor"));
    let paired = builtin_facts(BuiltinMimoShock::Paired);
    assert_eq!(paired.known_failures.len(), 16);

    // A law file's failures come from its check record.
    let failing = CaseVerdict {
        case: MimoShockCheckCase::Jointed6x3,
        kind: TargetKind::Srs,
        passed: false,
        failed: Some("converges".into()),
        number: 4.2,
        detail: "the last pulse did not pass".into(),
    };
    let passing = CaseVerdict {
        case: MimoShockCheckCase::Linear4x4,
        failed: None,
        passed: true,
        ..failing.clone()
    };
    let rec = check_record(&id, "2026-11-11T00:00:00Z", &[passing, failing]);
    cat.record_check(&rec).unwrap();
    let facts = all_facts(&cat);
    let file = facts.iter().find(|f| f.id == id).unwrap();
    assert!(!file.builtin);
    assert!(matches!(file.check, CheckStatus::Checked(_)));
    assert_eq!(file.known_failures.len(), 1);
    assert_eq!(file.known_failures[0].case, MimoShockCheckCase::Jointed6x3);
    assert!(file.known_failures[0].why.contains("converges"));
}

#[test]
fn mimo_shock_a_law_file_fault_reaches_the_core_by_name_and_sticks() {
    let scratch = Scratch::new("faults");
    let mut cat = catalog(scratch.path());
    let id = cat
        .import(&law_file("law_fixture_shock_mimo_misbehave"))
        .unwrap()
        .id;
    let s = setup();
    let mode = |m: &str| {
        vec![Param {
            key: "mode".into(),
            value: ParamValue::Choice(m.into()),
        }]
    };
    let build = |m: &str, budget| {
        resolve(&cat, &id, mode(m), budget)
            .unwrap()
            .build(&s)
            .unwrap()
    };
    let budget = Duration::from_secs(5);
    for (m, reason) in [
        ("wrong_drive_count", "wrong_length"),
        ("wrong_length", "wrong_length"),
        ("nan", "non_finite"),
        ("bad_delays", "wrong_length"),
        ("undeclared", "undeclared_value"),
        ("law_error", "error"),
        ("trap", "panicked"),
        ("grow", "over_memory"),
    ] {
        let mut law = build(m, budget);
        law.seed(-6.0).unwrap();
        let f = law.iterate(&inputs(&s, 1)).unwrap_err();
        assert_eq!(f.reason(), reason, "{m}: {f:?}");
    }
    let mut law = build("trap", budget);
    law.seed(-6.0).unwrap();
    let first = law.iterate(&inputs(&s, 1)).unwrap_err();
    assert_eq!(law.iterate(&inputs(&s, 2)).unwrap_err(), first);
    let mut law = build("seed_trap", budget);
    assert!(matches!(law.seed(-6.0), Err(LawFault::Crashed(_))));
    // A bad parameter refuses the run before any pulse.
    let r = resolve(&cat, &id, mode("no_such_mode"), budget)
        .unwrap()
        .build(&s);
    assert!(matches!(r, Err(LawFault::Failed(_))));
}

/// Timing list: the deadline is the factory's budget.
#[test]
fn mimo_shock_a_late_law_file_is_over_budget_for_the_core() {
    let scratch = Scratch::new("budget");
    let mut cat = catalog(scratch.path());
    let id = cat
        .import(&law_file("law_fixture_shock_mimo_misbehave"))
        .unwrap()
        .id;
    let s = setup();
    let params = vec![Param {
        key: "mode".into(),
        value: ParamValue::Choice("spin".into()),
    }];
    let mut law = resolve(&cat, &id, params, Duration::from_millis(50))
        .unwrap()
        .build(&s)
        .unwrap();
    law.seed(-6.0).unwrap();
    let f = law.iterate(&inputs(&s, 1)).unwrap_err();
    assert_eq!(f.reason(), "over_budget");
}
