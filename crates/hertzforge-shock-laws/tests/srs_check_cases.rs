//! The SRS law check, run with the built-in law on the simulator: every case
//! passes. And a law that does not correct fails the case built to catch it.

mod common;

use common::srs_station_for;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault};
use hertzforge_shock_laws::{
    judge_srs, run_srs_check_case, srs_check_record, BuiltinSrsFactory, SrsCaseEnd, SrsCheckCase,
};
use hertzforge_srs::{SrsInputs, SrsLaw, SrsLawFactory, SrsLawSetup, SrsOutputs};
use std::sync::Arc;

#[test]
fn the_built_in_law_passes_every_case() {
    let mut cases = Vec::new();
    for case in SrsCheckCase::ALL {
        let m = run_srs_check_case(
            case,
            Arc::new(BuiltinSrsFactory::default()),
            srs_station_for(case),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", case.name()));
        let j = judge_srs(case, &m);
        println!("{}: {}", case.name(), j.detail);
        assert!(j.passed, "{}: {} ({m:?})", case.name(), j.detail);
        cases.push(j);
    }
    let r = srs_check_record("00".repeat(32), "2026-11-07T00:00:00Z", cases);
    assert!(r.passed);
}

/// The seed, then every gain held at 1.
struct Frozen(Box<dyn SrsLaw + Send>);
impl SrsLaw for Frozen {
    fn name(&self) -> &str {
        "frozen"
    }
    fn seed(&mut self, i: &SrsInputs<'_>) -> Result<SrsOutputs, LawFault> {
        self.0.seed(i)
    }
    fn iterate(&mut self, i: &SrsInputs<'_>) -> Result<SrsOutputs, LawFault> {
        let n = self.0.iterate(i)?.gains.len();
        Ok(SrsOutputs {
            gains: vec![1.0; n],
            values: Vec::new(),
        })
    }
}
struct FrozenFactory;
impl SrsLawFactory for FrozenFactory {
    fn build(&self, s: &SrsLawSetup<'_>) -> Result<Box<dyn SrsLaw + Send>, LawFault> {
        Ok(Box::new(Frozen(BuiltinSrsFactory::default().build(s)?)))
    }
    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "frozen".into(),
            name: "frozen".into(),
            version: "0".into(),
            sha256: None,
            params: serde_json::json!({}),
        }
    }
}

#[test]
fn a_law_that_does_not_correct_fails_the_identification_error_case() {
    let case = SrsCheckCase::IdentificationError;
    let m = run_srs_check_case(case, Arc::new(FrozenFactory), srs_station_for(case)).unwrap();
    let j = judge_srs(case, &m);
    println!("frozen: {}", j.detail);
    assert!(!j.passed, "{}", j.detail);
    assert_eq!(m.end, SrsCaseEnd::NotConverged, "{m:?}");
    assert_eq!(m.full_level_pulses, 0, "it never leaves the first level");
}
