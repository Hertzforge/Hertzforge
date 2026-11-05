//! Multiple-output laws in the catalog, end to end on a scratch data root:
//! the built-in MIMO laws listed apart from the single-output ones, law files
//! of either world imported and listed by the world they implement, each
//! built-in MIMO manifest equal to its law file's, a choice of MIMO law
//! resolved to a factory that builds a running law, a law of the other test
//! type refused, and the arming gate applied alike.

mod common;

use common::*;
use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_random_laws::{
    builtin_mimo_manifest, default_mimo_choice, may_arm_on_hardware, ArmRefusal, BuiltinMimo,
    CheckCase, CheckRecord, CheckStatus, ChoiceError, LawCatalog, LawChoice, LawWorld,
    DEFAULT_MIMO_LAW_ID,
};
use hertzforge_sdm::law::{MimoLoopInputs, MimoSetup, Tolerances};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use std::path::Path;
use std::time::Duration;

const ZERO_LAW: &str = "hertzforge_law_mimo_zero";
const MIMO_MISBEHAVE: &str = "law_fixture_mimo_misbehave";

fn open(root: &Path) -> LawCatalog {
    let host = LawHost::new(HostConfig::default()).expect("the host starts");
    LawCatalog::with_host(host, root).expect("the catalog opens")
}

/// A 3 control, 2 drive setup on 16 lines, with what a call needs.
struct Small {
    setup: MimoSetup,
    frf: MatrixStack,
    coherence: Vec<f64>,
}

impl Small {
    fn new() -> Self {
        let lines = 16;
        let mut reference = HermitianStack::zeros(lines, 3);
        let mut frf = MatrixStack::zeros(lines, 3, 2);
        for line in 0..lines {
            for i in 0..3 {
                reference.set(line, i, i, C64::new(1e-2, 0.0));
            }
            for r in 0..3 {
                for c in 0..2 {
                    let v = if r == c { 1.0 } else { 0.2 };
                    frf.set(line, r, c, C64::new(v, 0.1 * r as f64));
                }
            }
        }
        Small {
            setup: MimoSetup {
                controls: 3,
                drives: 2,
                lines,
                df_hz: 5.0,
                band: (2, 14),
                reference,
                tolerances: Tolerances::none(lines, 3),
            },
            frf,
            coherence: vec![0.95; lines * 3],
        }
    }

    fn inputs<'a>(&'a self, response: &'a HermitianStack) -> MimoLoopInputs<'a> {
        MimoLoopInputs {
            reference: &self.setup.reference,
            tolerances: &self.setup.tolerances,
            response,
            response_dof: 64.0,
            frf: &self.frf,
            multiple_coherence: &self.coherence,
            df_hz: self.setup.df_hz,
            band: self.setup.band,
            loop_index: 0,
            last_drive: None,
            response_drive_csd: None,
            sysid_response: Some(response),
            sysid_drive: None,
            sysid_noise: None,
        }
    }
}

#[test]
fn mimo_laws_are_listed_apart_from_single_output_ones() {
    let scratch = Scratch::new("mimo-list");
    let mut cat = open(scratch.path());
    let ids: Vec<String> = cat.mimo_entries().into_iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        [
            "pinv_closed",
            "pinv_open",
            "shape_svd",
            "buzz_match",
            "min_drive"
        ]
    );
    assert_eq!(DEFAULT_MIMO_LAW_ID, "pinv_closed");
    for e in cat.mimo_entries() {
        assert!(e.builtin);
        assert_eq!(e.world, LawWorld::Mimo);
        assert_eq!(e.check, CheckStatus::BuiltIn);
        let m = e.mimo.expect("a MIMO entry carries its MIMO manifest");
        assert_eq!(m.law, e.manifest);
        assert_eq!(m.shapes.len(), 3, "{}", e.id);
    }
    let single: Vec<String> = cat.entries().into_iter().map(|e| e.id).collect();
    assert_eq!(single, ["hinv", "ratio"]);

    // Law files of both worlds land on their own lists.
    let zero = cat
        .import(&law_file(ZERO_LAW))
        .expect("the zero law imports");
    let fixture = cat
        .import(&law_file(MIMO_MISBEHAVE))
        .expect("the MIMO fixture imports");
    let ratio = cat.import(&law_file(RATIO)).expect("ratio imports");
    assert_eq!(zero.world, LawWorld::Mimo);
    assert_eq!(zero.check, CheckStatus::Unchecked);
    assert_eq!(ratio.world, LawWorld::Single);
    assert!(ratio.mimo.is_none());
    let mimo: Vec<(String, bool)> = cat
        .mimo_entries()
        .into_iter()
        .map(|e| (e.manifest.name, e.builtin))
        .collect();
    assert_eq!(
        mimo,
        [
            ("pseudo-inverse, closed loop".to_string(), true),
            ("pseudo-inverse, open loop".to_string(), true),
            ("shape-constrained (SVD)".to_string(), true),
            ("buzz-test matched".to_string(), true),
            ("minimum drive".to_string(), true),
            ("misbehaving multiple-output fixture".to_string(), false),
            ("zero drive".to_string(), false),
        ]
    );
    let single: Vec<String> = cat.entries().into_iter().map(|e| e.id).collect();
    assert_eq!(
        single,
        ["hinv".to_string(), "ratio".to_string(), ratio.id.clone()]
    );

    // Another catalog on the same root finds each in its world.
    let again = open(scratch.path());
    assert_eq!(again.mimo_entries(), cat.mimo_entries());
    assert_eq!(again.entries(), cat.entries());
    assert_eq!(again.entry(&fixture.id), Some(fixture));
}

#[test]
fn each_built_in_mimo_manifest_equals_its_law_files() {
    let host = LawHost::new(HostConfig::default()).expect("the host starts");
    for (b, file) in [
        (BuiltinMimo::PseudoInverse, "hertzforge_law_mimo_pinv"),
        (BuiltinMimo::OpenLoop, "hertzforge_law_mimo_open_loop"),
        (BuiltinMimo::ShapeConstrained, "hertzforge_law_mimo_shape"),
        (BuiltinMimo::BuzzMatched, "hertzforge_law_mimo_buzz"),
    ] {
        let bytes = std::fs::read(law_file(file)).expect("readable");
        let law = host
            .load_mimo(&bytes)
            .unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(law.manifest(), &builtin_mimo_manifest(b), "{b:?}");
    }
}

#[test]
fn a_mimo_choice_resolves_to_a_factory_that_builds_a_running_law() {
    let scratch = Scratch::new("mimo-choice");
    let mut cat = open(scratch.path());
    let small = Small::new();
    let budget = Duration::from_secs(5);

    // The default law, natively.
    let f = cat
        .resolve_mimo(&default_mimo_choice(), 3)
        .expect("the default resolves");
    assert_eq!(f.identity().name, "pseudo-inverse, closed loop");
    assert!(f.identity().sha256.is_empty());
    assert_eq!(
        f.params().len(),
        builtin_mimo_manifest(BuiltinMimo::PseudoInverse)
            .law
            .params
            .len()
    );
    let mut law = f.build(&small.setup, budget).expect("builds");
    let response = small.setup.reference.clone();
    let out = law.seed(&small.inputs(&response)).expect("seeds");
    assert_eq!(out.drive.n(), 2);
    assert!(out.drive.trace(5) > 0.0);

    // Parameters are checked against the declaration.
    let bad = LawChoice::new(
        "pinv_closed",
        vec![Param::new("convergence", ParamValue::Number(2.0))],
    );
    assert!(matches!(
        cat.resolve_mimo(&bad, 3),
        Err(ChoiceError::BadParam { ref key, .. }) if key == "convergence"
    ));

    // A law file, through the host.
    let zero = cat.import(&law_file(ZERO_LAW)).expect("imports");
    let f = cat
        .resolve_mimo(
            &LawChoice::new(
                zero.id.clone(),
                vec![Param::new("gain", ParamValue::Number(2.0))],
            ),
            3,
        )
        .expect("the zero law resolves");
    assert_eq!(f.identity().sha256, zero.id);
    let mut law = f.build(&small.setup, budget).expect("builds");
    let out = law.seed(&small.inputs(&response)).expect("seeds");
    assert_eq!(out.drive, HermitianStack::zeros(16, 2));

    // A law of the other test type is refused, both ways.
    for (id, is_for) in [("hinv", "single-output Random")] {
        match cat.resolve_mimo(&LawChoice::new(id, Vec::new()), 3) {
            Err(ChoiceError::WrongTestType { law_is_for, .. }) => assert_eq!(law_is_for, is_for),
            other => panic!("{id}: {:?}", other.err()),
        }
    }
    let mut cfg = manual_cfg();
    for id in ["pinv_closed".to_string(), zero.id.clone()] {
        match cat.resolve(&LawChoice::new(id.clone(), Vec::new()), &mut cfg) {
            Err(ChoiceError::WrongTestType { law_is_for, .. }) => {
                assert_eq!(law_is_for, "multiple-output Random")
            }
            Err(e) => panic!("{id}: {e}"),
            Ok(_) => panic!("{id}: a MIMO law resolved for a single-output run"),
        }
    }
    assert!(matches!(
        cat.resolve_mimo(&LawChoice::new("no_such_law", Vec::new()), 3),
        Err(ChoiceError::UnknownLaw(_))
    ));
}

#[test]
fn the_arming_gate_applies_to_mimo_laws_alike() {
    let scratch = Scratch::new("mimo-arm");
    let mut cat = open(scratch.path());
    for e in cat.mimo_entries() {
        assert_eq!(may_arm_on_hardware(&e), Ok(()), "{}", e.id);
    }
    let zero = cat.import(&law_file(ZERO_LAW)).expect("imports");
    assert_eq!(may_arm_on_hardware(&zero), Err(ArmRefusal::Unchecked));
    let record = CheckRecord::new(
        zero.id.clone(),
        "test",
        "2026-11-02T20:00:00Z",
        vec![CheckCase {
            name: "two close modes".into(),
            passed: true,
            detail: "converged".into(),
        }],
    );
    cat.record_check(&record).expect("the record is written");
    let zero = cat.entry(&zero.id).expect("still listed");
    assert_eq!(may_arm_on_hardware(&zero), Ok(()));
}
