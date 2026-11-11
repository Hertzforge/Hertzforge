//! The floor's rms abort is armed with a part sweeping the whole run: it is
//! judged on the clean lines that each hold its degrees of freedom, once most
//! of the band does, so one line a part has just freed cannot disarm it. A
//! floor driven 3 dB high trips it; a healthy run does not.

mod common;
use common::*;
use hertzforge_mixed::{
    parts::{Direction, PartSchedule, PartSweepTarget, SweepLaw},
    MixedLawFactories, MixedService, MixedTripKind,
};
use hertzforge_random::control::{ControlLaw, LawFault, LevelCommand, LoopInputs, LoopOutputs};
use hertzforge_random::service::law::{LawFactory, LawSetup};
use hertzforge_random::HinvLawFactory;
use std::sync::Arc;

/// The built-in floor law shown half the measured density: it drives the
/// floor until it reads 3 dB over its reference.
struct High(Box<dyn ControlLaw + Send>);
impl ControlLaw for High {
    fn init(&mut self, i: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        self.0.init(i)
    }
    fn iterate(&mut self, i: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        let half: Vec<f64> = i.measured_psd.iter().map(|p| 0.5 * p).collect();
        let j = LoopInputs {
            measured_psd: &half,
            ..*i
        };
        self.0.iterate(&j)
    }
    fn retarget_level(&mut self, c: LevelCommand) -> Result<LoopOutputs, LawFault> {
        self.0.retarget_level(c)
    }
    fn name(&self) -> &str {
        "high"
    }
}
struct HighFactory;
impl LawFactory for HighFactory {
    fn build(
        &self,
        s: &LawSetup<'_>,
    ) -> Result<Box<dyn ControlLaw + Send>, hertzforge_random::ControlError> {
        Ok(Box::new(High(HinvLawFactory.build(s)?)))
    }
}

fn sweeping(high: bool) -> MixedService {
    let sweep = PartSchedule::Sweep {
        lo_hz: 150.0,
        hi_hz: 450.0,
        start_hz: 150.0,
        direction: Direction::Up,
        law: SweepLaw::LinearHzPerS,
        rate: 4.0,
        target: PartSweepTarget::Duration { s: f64::INFINITY },
    };
    let mut c = config(vec![floor(0.01), tone("s", sweep, 1.0)]);
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    // The per-line abort stays out of it: only the rms abort can end the run.
    c.limits.max_lines_out = usize::MAX;
    if let hertzforge_mixed::PartBody::Floor(f) = &mut c.parts[0].body {
        f.alarm_db = (1.0, 1.0);
        f.abort_db = (2.0, 2.0);
    }
    let mut laws = MixedLawFactories::builtin();
    if high {
        laws.floor = Arc::new(HighFactory);
    }
    engaged_with(c, DELAY, laws)
}

#[test]
fn a_floor_driven_3_db_high_trips_the_rms_abort_with_a_tone_sweeping() {
    let mut s = sweeping(true);
    let mut judged = 0;
    for _ in 0..240 {
        if !run_for(&mut s, 0.25) {
            break;
        }
        judged = judged.max(s.live_status().floor.rms_judged_lines);
    }
    let t = s.trip().cloned();
    eprintln!(
        "floor driven high: end {:?}; judged on up to {judged} lines",
        s.end_reason()
    );
    assert!(
        matches!(t.map(|t| t.kind), Some(MixedTripKind::FloorRmsAbort { error_db }) if error_db > 2.0),
        "{:?}",
        s.end_reason()
    );
}

#[test]
fn a_healthy_floor_with_a_tone_sweeping_keeps_the_rms_abort_armed_and_untripped() {
    let mut s = sweeping(false);
    let (mut armed, mut lines, mut in_band) = (0usize, 0usize, 0usize);
    for _ in 0..160 {
        assert!(run_for(&mut s, 0.25), "{:?}", s.end_reason());
        let f = s.live_status().floor;
        if f.rms_judged_lines > 0 {
            armed += 1;
            lines = f.rms_judged_lines;
            in_band = f.lines_in_band;
        }
    }
    eprintln!("healthy: armed on {armed} of 160 readings, last on {lines} of {in_band} lines");
    assert!(s.trip().is_none(), "{:?}", s.end_reason());
    assert!(
        armed >= 100 && lines * 4 >= in_band * 3,
        "{armed} {lines} {in_band}"
    );
}
