//! The core's divergence judgement of a law that controls bands.
//!
//! A band law sets one level per band and leaves the lines inside a band to the
//! structure, so on a plant with resonances inside its bands its line error stays
//! above the divergence floor while it holds every band level, and never falls. The
//! core judges such a law on band error, over the bands the law declares
//! (`MimoControlLaw::control_bands`), with the floor and the loop count unchanged.
//!
//! Both runs are the built-in band-average law at the default protections on the
//! 4 x 4 modal plant, to a reference that is flat across each of the law's bands
//! (the band averages of a response the plant can reach), with no envelope lines,
//! so the divergence judgement is the only one that can end the run.
mod common;

use common::*;
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal_sim::plants::{NamedPlant, StandardPlant};
use hertzforge_law_host::{LawIdentity, Param};
use hertzforge_mimo_random::law::builtin_factory;
use hertzforge_mimo_random::{
    DivergenceMeasure, MimoConfig, MimoRandomService, MimoState, MimoTripKind,
};
use hertzforge_random_laws::{ChoiceError, LawChoice, MimoLawFactory};
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
    Tolerances,
};
use hertzforge_sdm::{HermitianStack, C64};
use std::sync::Arc;
use std::time::Duration;

/// The band-average law's test: the plant, and a reference flat across each band
/// the law declares.
fn banded_test() -> (MimoConfig, ChannelTable, NamedPlant, Grid) {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    cfg.law = LawChoice::new("band_average", Vec::new());
    let (lines, n) = (grid.lines(), cfg.reference.sdm.n());
    let setup = MimoSetup {
        controls: n,
        drives: cfg.drives.len(),
        lines,
        df_hz: grid.df(),
        band: grid.band(),
        reference: cfg.reference.sdm.clone(),
        tolerances: cfg.reference.tolerances.clone(),
    };
    let bands = builtin_factory(&cfg.law, n)
        .expect("the law is built in")
        .build(&setup, Duration::from_secs(1))
        .expect("the law builds")
        .control_bands()
        .expect("the band-average law declares its bands");
    let mut flat = HermitianStack::zeros(lines, n);
    for &(first, end) in &bands {
        for i in 0..n {
            let mean = (first..end)
                .map(|line| cfg.reference.sdm.get(line, i, i).re)
                .sum::<f64>()
                / (end - first) as f64;
            for line in first..end {
                flat.set(line, i, i, C64::new(mean, 0.0));
            }
        }
    }
    cfg.reference.sdm = flat;
    cfg.reference.tolerances = Tolerances::none(lines, n);
    (cfg, table, p, grid)
}

/// Run to full level and for `loops` loops there, or until the run ends. Returns
/// the loops at full level.
fn run_at_full_level(svc: &mut MimoRandomService, loops: u64) -> u64 {
    let st = svc.run_auto(0).expect("the run engages");
    assert_eq!(st, MimoState::Engaged, "{:?}", svc.log().events());
    let mut at_full = 0;
    while svc.state().is_running() && at_full < loops && svc.loop_count() < loops + 40 {
        svc.loop_tick().expect("a loop");
        if svc.state() == MimoState::FullLevel {
            at_full += 1;
        }
    }
    at_full
}

/// Mutation: judge every law line by line in `loop_tick` (set `law_bands` to
/// `None`). The run is then stopped for divergence within its first loops at full
/// level, as every band-law run at the default protections was.
#[test]
fn a_band_law_holding_its_band_levels_is_not_stopped_for_divergence() {
    let (cfg, table, p, grid) = banded_test();
    let floor = cfg.protections.divergence_floor_db;
    let mut svc = service(cfg, table, &p, &grid, "band-div-holds");
    let at_full = run_at_full_level(&mut svc, 30);
    eprintln!(
        "band law, 30 loops at full level: state {:?}, line error per control {:?} dB",
        svc.state(),
        svc.error_db()
    );
    assert_eq!(at_full, 30, "the run ended early: {:?}", svc.trip());
    assert_eq!(svc.state(), MimoState::FullLevel, "{:?}", svc.trip());
    // The line error is at or above the floor and is not what the run was judged
    // on: judged on it, the run would have been stopped.
    assert!(
        svc.error_db().iter().any(|&e| e >= floor),
        "the plant's resonances no longer hold a control's line error at the {floor} dB \
         floor ({:?}); the test needs a plant that does",
        svc.error_db()
    );
}

/// dB the drifting law raises its drive by each loop.
const DRIFT_DB: f64 = 1.0;

/// The band-average law's first drive, raised by [`DRIFT_DB`] every loop: a band
/// law whose band levels drift away from the reference.
struct Drifting {
    inner: Box<dyn MimoControlLaw + Send>,
    first: Option<HermitianStack>,
    loops: u32,
}

impl MimoControlLaw for Drifting {
    fn needs(&self) -> &[MimoInput] {
        self.inner.needs()
    }
    fn shapes(&self) -> &[MimoShape] {
        self.inner.shapes()
    }
    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        let out = self.inner.seed(inputs)?;
        self.first = Some(out.drive.clone());
        Ok(out)
    }
    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        let mut out = self.inner.iterate(inputs)?;
        self.loops += 1;
        let mut drive = self.first.clone().expect("seeded");
        let gain = 10f64.powf(DRIFT_DB * f64::from(self.loops) / 10.0);
        let _ = drive.scale_lines(&vec![gain; drive.lines()]);
        out.drive = drive;
        Ok(out)
    }
    fn control_bands(&self) -> Option<Vec<(usize, usize)>> {
        self.inner.control_bands()
    }
}

struct DriftingFactory(Arc<dyn MimoLawFactory>);

impl MimoLawFactory for DriftingFactory {
    fn identity(&self) -> LawIdentity {
        LawIdentity {
            name: "drifting band law".into(),
            version: "0".into(),
            author: "test".into(),
            sha256: String::new(),
        }
    }
    fn params(&self) -> &[Param] {
        self.0.params()
    }
    fn build(
        &self,
        setup: &MimoSetup,
        budget: Duration,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError> {
        Ok(Box::new(Drifting {
            inner: self.0.build(setup, budget)?,
            first: None,
            loops: 0,
        }))
    }
}

/// Mutation: skip `check_divergence` for a law that declares bands. A band law is
/// still stopped when its band levels leave the reference and keep going: the
/// judgement moved to band error, it was not taken away.
#[test]
fn a_band_law_whose_band_levels_drift_is_stopped_for_divergence() {
    let (cfg, table, p, grid) = banded_test();
    let (floor, loops) = (
        cfg.protections.divergence_floor_db,
        cfg.protections.divergence_loops,
    );
    let inner = builtin_factory(&cfg.law, cfg.reference.sdm.n()).unwrap();
    let mut svc = service(cfg, table, &p, &grid, "band-div-drifts")
        .with_law_factory(Arc::new(DriftingFactory(inner)));
    let at_full = run_at_full_level(&mut svc, 30);
    assert!(
        at_full < 30,
        "a drifting band law ran on: {:?}",
        svc.error_db()
    );
    assert_eq!(svc.state(), MimoState::Aborted, "{:?}", svc.trip());
    match svc.trip().map(|t| &t.kind) {
        Some(MimoTripKind::Divergence {
            consecutive_loops,
            error_db,
            measure,
            ..
        }) => {
            assert_eq!(*measure, DivergenceMeasure::Bands);
            assert!(*consecutive_loops >= loops);
            assert!(*error_db >= floor, "{error_db}");
        }
        other => panic!("expected a divergence stop on band error, got {other:?}"),
    }
}
