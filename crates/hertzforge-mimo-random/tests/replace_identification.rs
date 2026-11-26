//! Research: replacing the identification's frequency response at the gate, and the
//! read-only view of the response the law is handed. A response that does not fit
//! the test is refused, as `load_identification` refuses one, and the gate is kept;
//! one that fits is adopted, its condition numbers are recomputed from it, the gate
//! is judged again, the law's first solve uses it, and the replacement is on the
//! run's timeline with its source.
mod common;

use common::*;
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_law_host::{LawIdentity, Param};
use hertzforge_mimo_random::{FrfMode, MimoError, MimoEventKind, MimoRandomService, MimoState};
use hertzforge_random_laws::{ChoiceError, MimoLawFactory};
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
};
use hertzforge_sdm::{MatrixStack, C64};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Every frequency response the law was handed, in call order (seed first).
type Handed = Arc<Mutex<Vec<MatrixStack>>>;

/// The configured law, keeping a copy of the response it is handed on every call.
struct Recording(Box<dyn MimoControlLaw + Send>, Handed);

impl MimoControlLaw for Recording {
    fn needs(&self) -> &[MimoInput] {
        self.0.needs()
    }
    fn shapes(&self) -> &[MimoShape] {
        self.0.shapes()
    }
    fn seed(&mut self, i: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        self.1.lock().unwrap().push(i.frf.clone());
        self.0.seed(i)
    }
    fn iterate(&mut self, i: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        self.1.lock().unwrap().push(i.frf.clone());
        self.0.iterate(i)
    }
}

struct RecordingFactory(Arc<dyn MimoLawFactory>, Handed);

impl MimoLawFactory for RecordingFactory {
    fn identity(&self) -> LawIdentity {
        self.0.identity()
    }
    fn params(&self) -> &[Param] {
        self.0.params()
    }
    fn build(
        &self,
        s: &MimoSetup,
        b: Duration,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError> {
        Ok(Box::new(Recording(self.0.build(s, b)?, self.1.clone())))
    }
}

/// Start, measure the noise floor and the identification: the run waits at the gate.
fn to_gate(svc: &mut MimoRandomService) {
    svc.start().unwrap();
    if svc.state() == MimoState::Feasibility {
        svc.override_with("a test on the simulator").unwrap();
    }
    svc.run_noise_floor().unwrap();
    svc.run_identification().unwrap();
    assert_eq!(svc.state(), MimoState::Gate);
}

/// A 4 x 4 run with every response the law is handed recorded.
fn recorded_service(tag: &str, frf_mode: Option<FrfMode>) -> (MimoRandomService, Handed) {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    if let Some(m) = frf_mode {
        cfg.frf_mode = m;
    }
    let inner = hertzforge_mimo_random::law::builtin_factory(&cfg.law, 4).unwrap();
    let handed: Handed = Arc::default();
    let svc = service(cfg, table, &p, &grid, tag)
        .with_law_factory(Arc::new(RecordingFactory(inner, handed.clone())));
    (svc, handed)
}

/// The control rows of a physical response (the tests set no limit channels, so
/// that is every row).
fn control_rows(h: &MatrixStack, n_c: usize) -> MatrixStack {
    let (lines, _, cols) = h.dims();
    let mut out = MatrixStack::zeros(lines, n_c, cols);
    for l in 0..lines {
        for r in 0..n_c {
            for c in 0..cols {
                out.set(l, r, c, h.get(l, r, c));
            }
        }
    }
    out
}

fn bits(v: &[f64]) -> Vec<u64> {
    v.iter().map(|x| x.to_bits()).collect()
}

/// Two identification sets are the same, the condition numbers (NaN outside the
/// band) bit for bit. The arrays are too large to print on a failure.
fn same_set(
    a: &hertzforge_mimo_random::IdentificationSet,
    b: &hertzforge_mimo_random::IdentificationSet,
) -> bool {
    let (mut a, mut b) = (a.clone(), b.clone());
    let same_condition = bits(&a.condition) == bits(&b.condition);
    a.condition.clear();
    b.condition.clear();
    same_condition && a == b
}

#[test]
fn a_replaced_response_is_adopted_its_conditioning_recomputed_and_the_gate_judged_again() {
    let (mut svc, handed) = recorded_service("replace-adopt", None);
    to_gate(&mut svc);
    let band = svc.band();
    let measured = svc.identification().unwrap().clone();
    let gate0 = svc.gate().unwrap().clone();
    assert!(gate0.passed, "{gate0:?}");
    assert!(!svc.identification_loaded());

    // A response with drive 1 nearly gone on every line: ill-conditioned everywhere.
    let mut weak = measured.frf.clone();
    let (lines, rows, _) = weak.dims();
    for l in 0..lines {
        for r in 0..rows {
            weak.set(l, r, 1, weak.get(l, r, 1) * 1.0e-6);
        }
    }
    svc.replace_identification_frf(weak.clone(), "arm/frf_stored.c16")
        .unwrap();
    assert_eq!(svc.state(), MimoState::Gate);
    let set = svc.identification().unwrap();
    assert!(set.frf == weak);
    // Everything else the identification measured is kept.
    assert!(set.g_cc == measured.g_cc);
    assert!(set.noise == measured.noise);
    assert!(bits(&set.coherence) == bits(&measured.coherence));
    assert_eq!(set.setup_hash, measured.setup_hash);
    // The condition numbers are the replacement's own.
    let want =
        hertzforge_mimo_random::pool::condition_numbers(&control_rows(&weak, 4), band).unwrap();
    assert!(bits(&set.condition) == bits(&want));
    assert!(bits(&set.condition) != bits(&measured.condition));
    // The gate is judged again on it, and fails on conditioning alone.
    let gate1 = svc.gate().unwrap().clone();
    assert!(!gate1.passed, "{gate1:?}");
    assert_eq!(gate1.ill_conditioned_lines.len(), band.1 - band.0);
    assert!(gate1.low_coherence.is_empty());
    assert!(gate1.worst_condition > 1.0e4 * gate0.worst_condition);
    assert!(svc.identification_loaded());
    // The law is handed the replacement.
    assert!(svc.frf_law().unwrap() == &control_rows(&weak, 4));
    // The replacement and its source are on the timeline.
    assert!(
        svc.log().events().iter().any(|e| matches!(
            &e.kind,
            MimoEventKind::Note { text }
                if text.contains("replaced") && text.contains("arm/frf_stored.c16")
        )),
        "{:?}",
        svc.log().events()
    );

    // Handing the measured response back restores the measured judgement exactly.
    svc.replace_identification_frf(measured.frf.clone(), "the measured one")
        .unwrap();
    assert!(bits(&svc.identification().unwrap().condition) == bits(&measured.condition));
    assert_eq!(svc.gate().unwrap(), &gate0);

    // Once more with a response scaled by 2 (as well conditioned as the measured one):
    // the law's first solve is made on it.
    let mut doubled = measured.frf.clone();
    for v in doubled.data_mut() {
        *v *= 2.0;
    }
    svc.replace_identification_frf(doubled.clone(), "doubled")
        .unwrap();
    assert!(svc.gate().unwrap().passed);
    assert!(
        handed.lock().unwrap().is_empty(),
        "no solve before the gate"
    );
    svc.proceed().unwrap();
    assert_eq!(svc.state(), MimoState::Predict);
    let first = handed.lock().unwrap().first().cloned().expect("the seed");
    assert!(
        first == control_rows(&doubled, 4),
        "the seed was not handed the replacement"
    );
}

#[test]
fn a_response_that_does_not_fit_is_refused_and_the_gate_kept() {
    let (mut svc, _) = recorded_service("replace-refuse", None);
    // Before the gate there is nothing to replace.
    svc.start().unwrap();
    if svc.state() == MimoState::Feasibility {
        svc.override_with("a test on the simulator").unwrap();
    }
    let any = MatrixStack::zeros(Grid::standard().lines(), 4, 4);
    match svc.replace_identification_frf(any, "early") {
        Err(MimoError::BadState(m)) => assert!(m.contains("at the gate"), "{m}"),
        r => panic!("expected a refusal, got {r:?}"),
    }
    svc.run_noise_floor().unwrap();
    svc.run_identification().unwrap();
    assert_eq!(svc.state(), MimoState::Gate);
    let measured = svc.identification().unwrap().clone();
    let gate0 = svc.gate().unwrap().clone();
    let law0 = svc.frf_law().unwrap().clone();
    let n_events = svc.log().events().len();

    let (lines, rows, cols) = measured.frf.dims();
    for (what, dims) in [
        ("another grid", (lines - 1, rows, cols)),
        ("another control count", (lines, rows + 1, cols)),
        ("another drive count", (lines, rows, cols - 1)),
    ] {
        let wrong = MatrixStack::zeros(dims.0, dims.1, dims.2);
        match svc.replace_identification_frf(wrong, what) {
            Err(MimoError::Blocked(m)) => assert!(m.contains("does not fit"), "{what}: {m}"),
            r => panic!("{what}: expected a refusal, got {r:?}"),
        }
        assert_eq!(svc.state(), MimoState::Gate, "{what}");
        assert!(same_set(svc.identification().unwrap(), &measured), "{what}");
        assert_eq!(svc.gate().unwrap(), &gate0, "{what}");
        assert!(svc.frf_law().unwrap() == &law0, "{what}");
        assert!(!svc.identification_loaded(), "{what}");
    }
    assert_eq!(
        svc.log().events().len(),
        n_events,
        "a refusal leaves no event"
    );

    // A response measured on the setup before a control was excluded at the gate does
    // not fit the setup after it.
    svc.exclude_controls(&[0], "a test on the simulator")
        .unwrap();
    assert_eq!(svc.state(), MimoState::Gate);
    let reduced = svc.identification().unwrap().clone();
    match svc.replace_identification_frf(measured.frf.clone(), "before the exclusion") {
        Err(MimoError::Blocked(m)) => assert!(m.contains("does not fit"), "{m}"),
        r => panic!("expected a refusal, got {r:?}"),
    }
    assert!(same_set(svc.identification().unwrap(), &reduced));

    // Past the gate a replacement is refused.
    svc.override_with("a test on the simulator").ok();
    svc.proceed().unwrap();
    assert_eq!(svc.state(), MimoState::Predict);
    match svc.replace_identification_frf(reduced.frf.clone(), "late") {
        Err(MimoError::BadState(m)) => assert!(m.contains("at the gate"), "{m}"),
        r => panic!("expected a refusal, got {r:?}"),
    }
}

#[test]
fn the_getters_return_what_the_law_is_handed() {
    // Held: the identification's response, at the seed and on every loop.
    let (mut svc, handed) = recorded_service("frf-law-held", None);
    assert!(
        svc.frf_law().is_none(),
        "no response before the identification"
    );
    assert!(svc.law_space_is_physical());
    to_gate(&mut svc);
    let ident = control_rows(&svc.identification().unwrap().frf, 4);
    assert!(svc.frf_law().unwrap() == &ident);
    svc.proceed().unwrap();
    svc.proceed().unwrap();
    for _ in 0..3 {
        svc.loop_tick().unwrap();
    }
    let calls = handed.lock().unwrap().clone();
    assert!(calls.len() >= 3, "{} calls", calls.len());
    assert!(calls.iter().all(|h| h == &ident));
    assert!(svc.frf_law().unwrap() == calls.last().unwrap());

    // Updated: the getter after a loop is what the law is handed on the next.
    let (mut svc, handed) =
        recorded_service("frf-law-update", Some(FrfMode::Update { blend: 1.0 }));
    to_gate(&mut svc);
    let ident = svc.frf_law().unwrap().clone();
    svc.proceed().unwrap();
    svc.proceed().unwrap();
    // The update is made from a loop's measurement before the law is called in that
    // loop, so after each loop the getter holds what the law was just handed.
    let mut changed = 0;
    for _ in 0..6 {
        let before = handed.lock().unwrap().len();
        svc.loop_tick().unwrap();
        let calls = handed.lock().unwrap().clone();
        if calls.len() > before {
            let last = calls.last().unwrap();
            assert!(
                svc.frf_law().unwrap() == last,
                "the getter is not what the law was handed"
            );
            if last != &ident {
                changed += 1;
            }
        }
    }
    assert!(changed > 0, "the update never changed the response");

    // A drive transformation: the law works in its own space, and the getter says so.
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    let mut t_d = MatrixStack::zeros(1, 4, 4);
    for k in 0..4 {
        t_d.set(0, k, k, C64::new(0.5, 0.0));
    }
    cfg.drive_transform = Some(t_d);
    let inner = hertzforge_mimo_random::law::builtin_factory(&cfg.law, 4).unwrap();
    let handed: Handed = Arc::default();
    let mut svc = service(cfg, table, &p, &grid, "frf-law-transformed")
        .with_law_factory(Arc::new(RecordingFactory(inner, handed.clone())));
    assert!(!svc.law_space_is_physical());
    to_gate(&mut svc);
    let physical = control_rows(&svc.identification().unwrap().frf, 4);
    let law = svc.frf_law().unwrap().clone();
    assert!(law != physical);
    let (lo, hi) = svc.band();
    let mid = (lo + hi) / 2;
    let a = law.get(mid, 0, 0);
    let b = physical.get(mid, 0, 0) * 0.5;
    assert!((a - b).norm() <= 1e-12 * b.norm(), "{a} against {b}");
    svc.override_with("a test on the simulator").ok();
    svc.proceed().unwrap();
    let first = handed.lock().unwrap().first().cloned().expect("the seed");
    assert!(
        first == law,
        "the seed was not handed the law-space response"
    );
}
