//! The core's guards on any law, each on its own, and the run's other ends:
//! holds, the law's faults, the rate, the replay, stop and abort. Every run is
//! on the simulator's shock plants through the host firer.

mod support;

use hertzforge_hal::Backend;
use hertzforge_hal_sim::plants::ShockPlant;
use hertzforge_mimo_shock::builtin::InverseFactory;
use hertzforge_mimo_shock::{
    replay, EndOutcome, EngineError, LadderPlan, MimoShockEngine, RunRecord, RunState, ShotError,
    ShotPath,
};
use hertzforge_module_abi::StationClass;
use hertzforge_shock_common::shot::ShotHooks;
use hertzforge_shock_host::service::firer::{BackendPulseFirer, FireControl, PulseRows};
use hertzforge_shock_host::PulseFirer;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use support::*;

/// Over the limits' peaks in identification and on the first pulse at -12 dB
/// (about 2.6 at most), under what -6 dB is predicted to give them (about 5.2).
const LIMIT_ABORT: f64 = 4.5;

fn linear_classical(
    edit: impl FnOnce(&mut hertzforge_mimo_shock::MimoShockConfig),
    law: Arc<dyn hertzforge_mimo_shock::MimoShockLawFactory>,
) -> (MimoShockEngine, Arc<ManualClock>) {
    let mut cfg = config(4, 0, 4, LEN_CLASSICAL);
    edit(&mut cfg);
    ready(
        ShockPlant::Linear4x4,
        cfg,
        classical(&[20.0, 18.0, 20.0, 18.0]),
        law,
    )
}

fn end_of(e: &mut MimoShockEngine) -> (EndOutcome, String) {
    let end = e.run_to_end(None).unwrap();
    println!("{}", trace(e));
    (end.outcome, end.reason)
}

#[test]
fn mimo_shock_engine_a_drive_over_its_ceiling_is_scaled_to_it() {
    let (mut e, _) = linear_classical(
        |c| c.drives.iter_mut().for_each(|d| d.ceiling_v = 0.5),
        Arc::new(InverseFactory::default()),
    );
    let r = e.fire_next(None).unwrap();
    assert!(r.limit_scale.iter().any(|k| *k < 1.0), "{r:?}");
    assert!(
        r.drive_vpk.iter().all(|v| *v <= 0.5 + 1e-12),
        "{:?}",
        r.drive_vpk
    );
}

#[test]
fn mimo_shock_engine_a_drive_over_its_abort_fires_nothing() {
    let (mut e, _) = linear_classical(
        |c| {
            c.drives.iter_mut().for_each(|d| {
                d.ceiling_v = 0.5;
                d.abort_v = 0.6;
            })
        },
        Arc::new(InverseFactory::default()),
    );
    let r = e.fire_next(None).unwrap();
    let end = r.end.expect("ended");
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::Aborted, "drive_abort")
    );
    assert!(e.records().pulses.is_empty(), "nothing was fired");
    assert_eq!(e.state(), RunState::Finished);
}

#[test]
fn mimo_shock_engine_a_seed_over_the_first_drive_bound_fires_nothing() {
    let (e, _) = linear_classical(|_| {}, Arc::new(Twisted::new(Twist::SeedTimes(3.0))));
    let end = e.end().expect("ended at the seed");
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::LawFault, "first_drive")
    );
    assert!(e.records().pulses.is_empty());
}

#[test]
fn mimo_shock_engine_the_growth_cap_refuses_a_drive_that_more_than_doubles() {
    let (mut e, _) = linear_classical(|_| {}, Arc::new(Twisted::new(Twist::Grow(3.0))));
    let (o, r) = end_of(&mut e);
    assert_eq!((o, r.as_str()), (EndOutcome::LawFault, "peak_rise"));
    assert!(e.pulses()[1..].iter().all(|p| !p.law_used));
    assert!(worst_drive_growth(&e) <= 2.0);
}

#[test]
fn mimo_shock_engine_the_growth_cap_refuses_a_band_that_grows() {
    // A 50 mV tone at 1.5 kHz, where the drives hold almost nothing: the peak
    // barely moves, the band grows by far more than 10 dB.
    let (mut e, _) = linear_classical(|_| {}, Arc::new(Twisted::new(Twist::AddTone(0.05, 1500.0))));
    let (o, r) = end_of(&mut e);
    assert_eq!((o, r.as_str()), (EndOutcome::LawFault, "bin_rise"));
}

#[test]
fn mimo_shock_engine_a_drive_predicted_over_an_abort_line_is_refused() {
    // Every answer 1.5 times the law's: 3.5 dB, under the growth cap, over the
    // classical abort line of 2.3 dB.
    let (mut e, _) = linear_classical(|_| {}, Arc::new(Twisted::new(Twist::Scale(1.5))));
    let (o, r) = end_of(&mut e);
    assert_eq!((o, r.as_str()), (EndOutcome::LawFault, "predicted_peak"));
    assert!(worst_peak_ratio(&e) < 10f64.powf(2.3 / 20.0));
}

fn six_by_three_with_limits(abort: f64) -> MimoShockEngine {
    // The 6 x 3 structure with three controls (as many as drives, so the
    // targets are reachable) and three limit channels.
    let mut cfg = config(3, 3, 3, LEN_CLASSICAL);
    cfg.limits.iter_mut().for_each(|l| l.abort_peak = abort);
    let mut e = MimoShockEngine::new(cfg, Arc::new(InverseFactory::default()))
        .unwrap()
        .with_target_spec(classical(&[10.0; 3]))
        .with_pacing(false);
    e.arm(
        station(ShockPlant::Jointed6x3, FS, 4),
        table(3, 3, 3),
        StationClass::HertzforgeSim,
    )
    .unwrap();
    e.identify().unwrap();
    e
}

#[test]
fn mimo_shock_engine_a_limit_over_its_abort_in_identification_ends_the_run() {
    let e = six_by_three_with_limits(0.5);
    let end = e.end().expect("ended in identification");
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::Aborted, "limit_abort")
    );
}

#[test]
fn mimo_shock_engine_a_limit_predicted_over_its_abort_fires_nothing() {
    let mut e = six_by_three_with_limits(LIMIT_ABORT);
    assert!(e.end().is_none(), "{:?}", e.end());
    e.proceed().unwrap();
    let end = e.run_to_end(None).unwrap();
    println!("{}", trace(&e));
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::SafeStop, "predicted_abort")
    );
    assert!(end.detail.contains("limit"), "{end:?}");
    // Nothing measured on a limit ever passed its abort.
    for p in &e.records().pulses {
        for (ch, row) in p.channels.iter().zip(&p.rows) {
            if *ch >= 4 {
                assert!(row.iter().all(|v| v.abs() <= LIMIT_ABORT), "limit {ch}");
            }
        }
    }
}

#[test]
fn mimo_shock_engine_predicted_kinematics_over_their_limit_fire_nothing() {
    let (mut e, _) = linear_classical(
        |c| c.guards.max_displacement_in = Some(1e-6),
        Arc::new(InverseFactory::default()),
    );
    let r = e.fire_next(None).unwrap();
    let end = r.end.expect("ended");
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::SafeStop, "predicted_abort")
    );
    assert!(end.detail.contains("peak to peak"), "{end:?}");
}

#[test]
fn mimo_shock_engine_the_divergence_watch_stops_an_error_rising_at_one_level() {
    // 1.25 times a pulse (1.9 dB): under the growth cap; the abort lines
    // moved out of the way so the watch is what acts.
    let (mut e, _) = linear_classical(
        |c| {
            c.guards.control_abort_db = Some(20.0);
            c.ladder = LadderPlan {
                max_pulses_per_level: 6,
                ..LadderPlan::default()
            };
        },
        Arc::new(Twisted::new(Twist::Grow(1.25))),
    );
    let (o, r) = end_of(&mut e);
    assert_eq!((o, r.as_str()), (EndOutcome::SafeStop, "divergence"));
}

#[test]
fn mimo_shock_engine_a_law_that_panics_ends_the_run_at_once() {
    let (mut e, _) = linear_classical(|_| {}, Arc::new(Twisted::new(Twist::Panic)));
    let (o, r) = end_of(&mut e);
    assert_eq!((o, r.as_str()), (EndOutcome::LawFault, "panicked"));
    assert_eq!(e.pulses().len(), 1);
}

#[test]
fn mimo_shock_engine_a_law_over_its_budget_three_times_ends_the_run() {
    let law = Twisted::new(Twist::Slow(1.0));
    let slot = law.clock.clone();
    let (mut e, clock) = linear_classical(|_| {}, Arc::new(law));
    *slot.lock().unwrap() = Some(clock);
    let (o, r) = end_of(&mut e);
    assert_eq!((o, r.as_str()), (EndOutcome::LawFault, "over_budget"));
    assert_eq!(e.pulses().len(), 3);
}

#[test]
fn mimo_shock_engine_a_law_of_the_wrong_length_ends_the_run() {
    let (mut e, _) = linear_classical(|_| {}, Arc::new(Twisted::new(Twist::Short)));
    let (o, r) = end_of(&mut e);
    assert_eq!((o, r.as_str()), (EndOutcome::LawFault, "wrong_length"));
}

#[test]
fn mimo_shock_engine_a_law_answering_a_non_finite_drive_ends_the_run() {
    let (mut e, _) = linear_classical(|_| {}, Arc::new(Twisted::new(Twist::NonFinite)));
    let (o, r) = end_of(&mut e);
    assert_eq!((o, r.as_str()), (EndOutcome::LawFault, "non_finite"));
}

#[test]
fn mimo_shock_engine_the_pulse_cap_ends_the_run() {
    let (mut e, _) = linear_classical(
        |c| c.guards.law.max_pulses = 2,
        Arc::new(InverseFactory::default()),
    );
    let (o, r) = end_of(&mut e);
    assert_eq!((o, r.as_str()), (EndOutcome::SafeStop, "max_pulses"));
    assert_eq!(e.pulses().len(), 2);
}

#[test]
fn mimo_shock_engine_a_target_outside_the_band_holds_the_run() {
    let mut cfg = config(4, 0, 4, LEN_CLASSICAL);
    cfg.band_hi_hz = 40.0;
    let mut e = MimoShockEngine::new(cfg, Arc::new(InverseFactory::default()))
        .unwrap()
        .with_target_spec(classical(&[20.0; 4]))
        .with_pacing(false);
    e.arm(
        station(ShockPlant::Linear4x4, FS, 1),
        table(4, 0, 4),
        StationClass::HertzforgeSim,
    )
    .unwrap();
    e.identify().unwrap();
    let end = e.end().expect("held");
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::Held, "reference_out_of_band")
    );
}

#[test]
fn mimo_shock_engine_a_recorded_run_replays_to_the_same_drives() {
    let (mut e, _) = linear_classical(|_| {}, Arc::new(InverseFactory::default()));
    let (o, _) = end_of(&mut e);
    assert_eq!(o, EndOutcome::Completed);
    let rec = e.records().clone();
    assert_eq!(rec.pulses.len(), e.pulses().len());
    assert_eq!(rec.law_calls.len(), e.pulses().len());
    let r = replay(&rec, &InverseFactory::default()).unwrap();
    assert!(r.reproduced(), "{r:?}");
    assert_eq!(r.compared, rec.law_calls.len());
    // Through the files.
    let dir = std::env::temp_dir().join(format!("mimo-shock-replay-{}", std::process::id()));
    rec.write_dir(&dir).unwrap();
    let back = RunRecord::read_dir(&dir).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(back.law_calls.len(), rec.law_calls.len());
    let r = replay(&back, &InverseFactory::default()).unwrap();
    println!("from the files: {r:?}");
    assert!(r.reproduced() && r.worst_relative <= 1e-12, "{r:?}");
    // A changed law does not reproduce.
    let mut other = InverseFactory::default();
    other.params.gain = 0.5;
    let r = replay(&back, &other).unwrap();
    assert!(!r.reproduced());
}

/// A station that programs 12 800 Hz whatever it is asked.
struct Coerced(BackendPulseFirer<Box<dyn Backend + Send>>);

impl ShotPath for Coerced {
    fn arm(&mut self, _fs_hz: f64) -> Result<f64, String> {
        let f = std::mem::replace(
            &mut self.0,
            BackendPulseFirer::new(
                station(ShockPlant::Linear4x4, FS, 9),
                table(4, 0, 4),
                StationClass::HertzforgeSim,
            )
            .unwrap(),
        );
        self.0 = f.with_requested_rate(FS);
        self.0.arm().map_err(|e| e.to_string())?;
        self.0.actual_fs_hz().ok_or_else(|| "no rate".into())
    }
    fn set_quiet_frames(&mut self, frames: usize) {
        self.0.set_quiet_frames(frames);
    }
    fn fire<'a>(
        &mut self,
        drive: &[f64],
        n_drive: usize,
        cancel: Option<&'a AtomicBool>,
        hooks: Option<&'a mut dyn ShotHooks>,
    ) -> Result<PulseRows, ShotError> {
        let mut ctl = FireControl {
            cancel,
            observer: None,
            hooks,
        };
        self.0
            .fire_rows(drive, n_drive, &mut ctl)
            .map_err(|e| ShotError::Failed(e.to_string()))
    }
    fn stop(&mut self) -> Result<(), String> {
        self.0.stop().map_err(|e| e.to_string())
    }
    fn emergency_stop(&mut self) -> Result<(), String> {
        self.0.emergency_stop().map_err(|e| e.to_string())
    }
}

fn coerced() -> Box<dyn ShotPath> {
    Box::new(Coerced(
        BackendPulseFirer::new(
            station(ShockPlant::Linear4x4, FS, 9),
            table(4, 0, 4),
            StationClass::HertzforgeSim,
        )
        .unwrap(),
    ))
}

#[test]
fn mimo_shock_engine_a_coerced_rate_is_adopted_and_the_target_rebuilt_at_it() {
    let mut cfg = config(4, 0, 4, LEN_CLASSICAL);
    cfg.fs_hz = 10_000.0;
    let mut e = MimoShockEngine::new(cfg.clone(), Arc::new(InverseFactory::default()))
        .unwrap()
        .with_target_spec(classical(&[20.0; 4]))
        .with_pacing(false);
    e.arm_with(coerced()).unwrap();
    assert_eq!(e.fs_hz(), FS);
    assert_eq!(e.df_hz(), FS / LEN_CLASSICAL as f64);
    // The pulse is 11 ms at the adopted rate: about 94 samples above half its
    // peak (73 had it been built at the rate asked).
    let w = e.target().unwrap().waveforms();
    let above = w[..LEN_CLASSICAL].iter().filter(|v| **v > 10.0).count();
    let expect = (FS * 0.011 * 2.0 / 3.0) as usize;
    assert!(
        above.abs_diff(expect) <= 2,
        "{above} samples above half the peak"
    );
    // A target given sample by sample cannot be re-derived: refused.
    let mut e = MimoShockEngine::new(cfg, Arc::new(InverseFactory::default())).unwrap();
    let r = e.arm_with(coerced());
    assert!(matches!(r, Err(EngineError::Config(_))), "{r:?}");
}

#[test]
fn mimo_shock_engine_stop_and_abort_end_the_run_and_release_the_station() {
    let (mut e, _) = linear_classical(|_| {}, Arc::new(InverseFactory::default()));
    e.fire_next(None).unwrap();
    e.stop().unwrap();
    assert_eq!(e.state(), RunState::Finished);
    assert_eq!(e.end().unwrap().outcome, EndOutcome::OperatorStop);
    assert!(matches!(e.fire_next(None), Err(EngineError::WrongState(_))));
    let (mut e, _) = linear_classical(|_| {}, Arc::new(InverseFactory::default()));
    e.abort().unwrap();
    assert_eq!(e.end().unwrap().outcome, EndOutcome::OperatorAbort);
}
