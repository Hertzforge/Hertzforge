//! The guards the core keeps on every control law, each against a law or a
//! station built to break it: the output checks (length, finite numbers, the
//! value count), the time budget on a test clock, a panic, the miss rule with
//! the drive in force replayed, the seed's bound, the per-call peak and bin
//! rise caps, the predicted response peak, the predicted full-level drive at
//! proceed, the envelope watched during a play, the alignment of every play,
//! the at-rest taper, and the records of every call.
//!
//! The station is `sim_e2e`'s plant; `config()` starts 6 dB under the
//! predicted drive, so the first plays do not gate and the law is called.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use common::{config, no_backoff, player, sim_backend, Twisted};
use hertzforge_hal::Backend;
use hertzforge_shock_common::law_guard::{LawClock, LawDescriptor, LawFault, LawValue, OutputRule};
use hertzforge_twr::schedule::TwrRunMode;
use hertzforge_twr::service::config::TwrConfig;
use hertzforge_twr::{
    BackendReferencePlayer, PlayInputs, PlayOutputs, TripKind, TwrControl, TwrCorrectFactory,
    TwrLaw, TwrLawFactory, TwrLawSetup, TwrService, TwrState,
};

/// A clock a test moves by hand.
#[derive(Default)]
struct TestClock(AtomicU64);

impl TestClock {
    fn advance(&self, s: f64) {
        let now = f64::from_bits(self.0.load(Ordering::SeqCst));
        self.0.store((now + s).to_bits(), Ordering::SeqCst);
    }
}

impl LawClock for TestClock {
    fn now_s(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::SeqCst))
    }
}

/// How the test law misbehaves.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    /// The built-in, unchanged.
    Ok,
    /// Every correction one sample short.
    WrongLength,
    /// Every correction with a NaN in it.
    Nan,
    /// The seed with a NaN in it.
    SeedNan,
    /// The seed times this.
    SeedTimes(f64),
    /// Every correction takes twice its budget on the test clock.
    Slow,
    /// Every correction panics.
    Panic,
    /// Two corrections with a NaN, then one usable, over and over.
    MissMissOk,
    /// Every correction times this.
    Times(f64),
    /// Every correction with a tone added near the top of the record's band.
    BinSpike,
    /// Every correction reporting nine values.
    TooMany,
}

struct TestLaw {
    inner: Box<dyn TwrLaw + Send>,
    mode: Mode,
    calls: u32,
    clock: Arc<TestClock>,
    budget_s: f64,
}

impl TwrLaw for TestLaw {
    fn name(&self) -> &str {
        "test law"
    }
    fn seed(&mut self, inputs: &PlayInputs<'_>) -> Result<PlayOutputs, LawFault> {
        let mut out = self.inner.seed(inputs)?;
        match self.mode {
            Mode::SeedNan => out.drive[10] = f64::NAN,
            Mode::SeedTimes(k) => out.drive.iter_mut().for_each(|v| *v *= k),
            _ => {}
        }
        Ok(out)
    }
    fn iterate(&mut self, inputs: &PlayInputs<'_>) -> Result<PlayOutputs, LawFault> {
        self.calls += 1;
        let mut out = self.inner.iterate(inputs)?;
        match self.mode {
            Mode::WrongLength => {
                out.drive.pop();
            }
            Mode::Nan => out.drive[10] = f64::NAN,
            Mode::Slow => self.clock.advance(2.0 * self.budget_s),
            Mode::Panic => panic!("the test law panics"),
            Mode::MissMissOk if !self.calls.is_multiple_of(3) => out.drive[10] = f64::NAN,
            Mode::Times(k) => out.drive.iter_mut().for_each(|v| *v *= k),
            Mode::BinSpike => {
                let n = out.drive.len();
                let pk = out.drive.iter().fold(0.0f64, |m, v| m.max(v.abs()));
                let k0 = (n / 2 - 10) as f64;
                for (i, v) in out.drive.iter_mut().enumerate() {
                    *v += 0.05 * pk * (std::f64::consts::TAU * k0 * i as f64 / n as f64).sin();
                }
            }
            Mode::TooMany => {
                out.values = (0..9)
                    .map(|i| LawValue::new(format!("v{i}"), 0.0))
                    .collect();
            }
            _ => {}
        }
        Ok(out)
    }
}

struct TestFactory {
    mode: Mode,
    clock: Arc<TestClock>,
    params: TwrCorrectFactory,
}

impl TwrLawFactory for TestFactory {
    fn build(&self, setup: &TwrLawSetup<'_>) -> Result<Box<dyn TwrLaw + Send>, LawFault> {
        Ok(Box::new(TestLaw {
            inner: self.params.build(setup)?,
            mode: self.mode,
            calls: 0,
            clock: Arc::clone(&self.clock),
            budget_s: setup.call_budget_s,
        }))
    }
    fn describe(&self) -> LawDescriptor {
        let mut d = self.params.describe();
        d.id = "test-law".into();
        d
    }
}

type Svc<B> = TwrService<BackendReferencePlayer<B>>;

fn closed(mut cfg: TwrConfig) -> TwrConfig {
    cfg.control = Some(TwrControl::default());
    cfg
}

fn service<B: Backend>(backend: B, cfg: TwrConfig, mode: Mode) -> Svc<B> {
    let clock = Arc::new(TestClock::default());
    let params = TwrCorrectFactory::from_law_config(&cfg.law);
    TwrService::new(player(backend), closed(cfg))
        .unwrap()
        .with_law_clock(clock.clone())
        .with_law_factory(Arc::new(TestFactory {
            mode,
            clock,
            params,
        }))
}

/// Run to the end, releasing the review hold, and return every play's drive.
fn run<B: Backend>(svc: &mut Svc<B>) -> Vec<Vec<f64>> {
    let mut drives = Vec::new();
    for _ in 0..200 {
        match svc.state() {
            TwrState::Review => svc.proceed().unwrap(),
            TwrState::Complete
            | TwrState::Aborted
            | TwrState::Stopped
            | TwrState::Holding
            | TwrState::CoherenceHold => break,
            _ => {
                let before = svc.plays().len();
                svc.advance().unwrap();
                if svc.plays().len() > before {
                    drives.push(svc.drive_time().to_vec());
                }
            }
        }
    }
    drives
}

fn law_fault<B: Backend>(svc: &Svc<B>) -> (LawFault, u32) {
    assert_eq!(svc.state(), TwrState::Aborted, "{:?}", svc.event_log());
    match svc.trip().map(|t| t.kind.clone()) {
        Some(TripKind::LawFault {
            reason,
            consecutive,
        }) => (reason, consecutive),
        k => panic!("expected a law fault, got {k:?}"),
    }
}

/// Three unusable corrections in a row end the run on a law fault; the drive
/// in force replays, bit for bit, through the misses; nothing plays after.
fn three_misses<B: Backend>(svc: &mut Svc<B>, rule: impl Fn(&LawFault) -> bool) {
    let drives = run(svc);
    let (reason, consecutive) = law_fault(svc);
    assert!(rule(&reason), "{reason:?}");
    assert_eq!(consecutive, 3);
    assert_eq!(
        drives.len(),
        3,
        "three plays, a call after each, then the stop"
    );
    assert_eq!(
        drives[0], drives[1],
        "the drive in force replays after a miss"
    );
    assert_eq!(drives[1], drives[2]);
    let rows = svc.plays();
    assert!(rows[1]
        .law
        .as_ref()
        .is_some_and(|c| !c.used && c.miss.is_some()));
}

fn bad_output(rule: OutputRule) -> impl Fn(&LawFault) -> bool {
    move |f| f == &LawFault::BadOutput(rule)
}

#[test]
fn a_wrong_length_drive_is_a_miss() {
    let mut svc = service(sim_backend(), config(), Mode::WrongLength);
    three_misses(&mut svc, bad_output(OutputRule::WrongLength));
}

#[test]
fn a_drive_with_a_nan_is_a_miss() {
    let mut svc = service(sim_backend(), config(), Mode::Nan);
    three_misses(&mut svc, bad_output(OutputRule::NonFinite));
}

#[test]
fn more_than_eight_values_is_a_miss() {
    let mut svc = service(sim_backend(), config(), Mode::TooMany);
    three_misses(&mut svc, bad_output(OutputRule::TooManyValues));
}

#[test]
fn a_call_slow_on_the_test_clock_is_a_miss() {
    let mut svc = service(sim_backend(), config(), Mode::Slow);
    three_misses(&mut svc, |f| matches!(f, LawFault::OverBudget { .. }));
    let (reason, _) = law_fault(&svc);
    let LawFault::OverBudget { took_s, budget_s } = reason else {
        unreachable!()
    };
    // A 0.75 s section between quiet pads: the budget is a quarter of 4 s.
    assert!((budget_s - 1.0).abs() < 1e-12, "{budget_s}");
    assert!((took_s - 2.0).abs() < 1e-9, "{took_s}");
}

#[test]
fn a_panicking_call_is_caught_as_a_miss() {
    let mut svc = service(sim_backend(), config(), Mode::Panic);
    three_misses(
        &mut svc,
        |f| matches!(f, LawFault::Crashed(m) if m.contains("panics")),
    );
}

#[test]
fn a_usable_call_resets_the_count() {
    let mut svc = service(sim_backend(), config(), Mode::MissMissOk);
    run(&mut svc);
    assert_eq!(svc.state(), TwrState::Complete, "{:?}", svc.event_log());
    let misses = svc
        .plays()
        .iter()
        .filter(|r| r.law.as_ref().is_some_and(|c| !c.used))
        .count();
    assert!(misses >= 2, "{misses} misses");
}

#[test]
fn a_seed_that_cannot_be_used_plays_nothing() {
    let mut svc = service(sim_backend(), config(), Mode::SeedNan);
    run(&mut svc);
    let (reason, consecutive) = law_fault(&svc);
    assert_eq!(reason, LawFault::BadOutput(OutputRule::NonFinite));
    assert_eq!(consecutive, 1);
    assert!(svc.plays().is_empty());
}

#[test]
fn a_first_drive_over_its_bound_plays_nothing() {
    // The seed is 6 dB under the predicted drive; times 5 it is 8 dB over,
    // and its predicted response peak about 2.5 times the target's.
    let mut svc = service(sim_backend(), config(), Mode::SeedTimes(5.0));
    run(&mut svc);
    assert_eq!(svc.state(), TwrState::Aborted);
    assert!(svc.plays().is_empty(), "nothing played");
    match svc.trip().map(|t| t.kind.clone()) {
        Some(TripKind::FirstDrive {
            predicted_peak,
            target_peak,
            ratio_limit,
        }) => {
            assert_eq!(ratio_limit, 2.0);
            assert!(
                predicted_peak > 2.0 * target_peak,
                "{predicted_peak} {target_peak}"
            );
        }
        k => panic!("expected the first-drive bound, got {k:?}"),
    }
}

#[test]
fn a_peak_rise_over_the_cap_is_refused() {
    let mut svc = service(sim_backend(), config(), Mode::Times(10.0));
    three_misses(&mut svc, bad_output(OutputRule::PeakRise));
}

#[test]
fn a_bin_rise_over_the_cap_is_refused() {
    let mut svc = service(sim_backend(), config(), Mode::BinSpike);
    three_misses(&mut svc, bad_output(OutputRule::BinRise));
}

#[test]
fn a_predicted_peak_over_the_envelope_is_refused() {
    // The plant measures 0.8 of what was identified, so the first play does
    // not gate; 1.7 times the built-in's correction predicts a response peak
    // over 1.5 times the target's, under both rise caps.
    let station = Twisted::new(sim_backend(), 1, 0.8, 0);
    let mut svc = service(station, no_backoff(), Mode::Times(1.7));
    three_misses(&mut svc, bad_output(OutputRule::PredictedPeak));
}

/// The ceiling tests' configuration: an identification drive small enough to
/// pass under the ceilings they set.
fn ceiling_config() -> TwrConfig {
    let mut c = config();
    c.sysid.drive_rms = 0.03;
    c
}

/// The full-level drive the inverse predicts, read off a run that has none of
/// the ceiling in its way.
fn full_level_drive_peak() -> f64 {
    let mut probe = service(sim_backend(), ceiling_config(), Mode::Ok);
    probe.advance().unwrap();
    probe.proceed().unwrap();
    // The drive in force is the seed, its backoff under the predicted drive;
    // at full level and without the backoff it is the predicted drive.
    let backoff = 10f64.powf(-config().law.initial_drive_backoff_db / 20.0);
    probe.drive_in_force_vpk().unwrap() / 10f64.powf(-12.0 / 20.0) / backoff
}

#[test]
fn auto_ends_at_proceed_when_the_full_level_drive_is_over_the_ceiling() {
    let full = full_level_drive_peak();
    let mut cfg = ceiling_config();
    cfg.limits.max_drive_vpk = Some(0.8 * full);
    let mut svc = service(sim_backend(), cfg, Mode::Ok);
    run(&mut svc);
    assert_eq!(svc.state(), TwrState::Aborted);
    assert!(svc.plays().is_empty(), "nothing played for it");
    match svc.trip().map(|t| t.kind.clone()) {
        Some(TripKind::DriveVpk {
            vpk,
            ceiling_vpk,
            predicted,
        }) => {
            assert!(predicted);
            assert_eq!(ceiling_vpk, 0.8 * full);
            assert!((vpk - full).abs() < 1e-6 * full, "{vpk} against {full}");
        }
        k => panic!("expected the predicted drive over the ceiling, got {k:?}"),
    }
}

/// MANUAL does not end at proceed; a level move whose projected drive passes
/// the ceiling is refused with both numbers, and nothing moves. The ceiling
/// sits under the full-level projection of the drive in force after one
/// correction, read off a probe of the same steps.
#[test]
fn manual_refuses_a_level_move_over_the_ceiling() {
    let manual = |ceiling: Option<f64>| {
        let mut cfg = ceiling_config();
        cfg.schedule.mode = TwrRunMode::Manual;
        if let Some(c) = ceiling {
            cfg.limits.max_drive_vpk = Some(c);
        }
        let mut svc = service(sim_backend(), cfg, Mode::Ok);
        svc.advance().unwrap();
        svc.proceed().unwrap();
        svc.advance().unwrap();
        svc
    };
    let probe = manual(None);
    let full = probe.drive_in_force_vpk().unwrap() / 10f64.powf(-12.0 / 20.0);
    let mut svc = manual(Some(0.9 * full));
    assert_eq!(
        svc.state(),
        TwrState::Iterating,
        "MANUAL does not end at proceed"
    );
    let before = svc.status().level_db;
    let e = svc.jump_to_level(4).unwrap_err().to_string();
    assert!(e.contains("exceeds the max drive ceiling"), "{e}");
    assert!(
        e.contains(&format!("{:.4}", 0.9 * full)),
        "names the ceiling: {e}"
    );
    assert_eq!(svc.status().level_db, before);
}

#[test]
fn the_envelope_is_watched_during_a_play() {
    // The plant's gain steps up 6 dB after the identification.
    let station = Twisted::new(sim_backend(), 1, 2.0, 0);
    let mut svc = service(station, no_backoff(), Mode::Ok);
    svc.advance().unwrap();
    // What the identification clocked, its rest pad included.
    let after_id = svc.station_frames();
    run(&mut svc);
    assert_eq!(svc.state(), TwrState::Aborted, "{:?}", svc.event_log());
    let trip = svc.trip().unwrap().clone();
    let TripKind::Envelope {
        in_play,
        samples,
        at_sample,
        ..
    } = trip.kind
    else {
        panic!("expected the envelope, got {:?}", trip.kind);
    };
    assert!(in_play, "found during the play");
    assert_eq!(samples, 8);
    assert!(svc.plays().is_empty(), "the play cut short has no row");
    assert_eq!(trip.play, Some(0), "the play it cut short");
    // Cut within the debounce and one block: the station clocked the
    // identification, then at most the lag, the trip sample and one block.
    let pad = svc.station_frames() - after_id;
    let lag = svc.lag_frames().unwrap() as u64;
    let block = hertzforge_hal::default_block_frames(common::FS) as u64;
    assert!(
        pad <= lag + at_sample as u64 + 1 + block,
        "{pad} frames clocked; lag {lag}, trip at {at_sample}, block {block}"
    );
    assert!(pad < svc.grid().n as u64, "cut short");
}

#[test]
fn a_moved_delay_ends_the_run_on_alignment() {
    // The control channel arrives two frames later after the identification.
    let station = Twisted::new(sim_backend(), 1, 1.0, 2);
    let mut svc = service(station, no_backoff(), Mode::Ok);
    run(&mut svc);
    assert_eq!(svc.state(), TwrState::Aborted, "{:?}", svc.event_log());
    let trip = svc.trip().unwrap().clone();
    let TripKind::Alignment {
        lag_frames,
        identification_lag_frames,
        drift_frames,
        limit_frames,
    } = trip.kind
    else {
        panic!("expected the alignment, got {:?}", trip.kind);
    };
    assert!((drift_frames - 2.0).abs() < 0.25, "{drift_frames}");
    assert_eq!(limit_frames, 1.0);
    assert!((lag_frames - identification_lag_frames - drift_frames).abs() < 1e-9);
    let row = &svc.plays()[0];
    assert_eq!(trip.play, Some(0));
    assert_eq!(row.drift_frames, Some(drift_frames));
    assert_eq!(
        svc.plays().len(),
        1,
        "nothing corrected from it, nothing after it"
    );
}

#[test]
fn an_unmoved_delay_completes_with_no_drift() {
    let mut svc = service(sim_backend(), config(), Mode::Ok);
    run(&mut svc);
    assert_eq!(svc.state(), TwrState::Complete, "{:?}", svc.event_log());
    let id_lag = svc.lag_frames().unwrap() as f64;
    for r in svc.plays() {
        let drift = r.drift_frames.expect("every play's lag is fitted");
        assert!(drift.abs() < 0.1, "play {}: {drift}", r.index);
        assert!((r.lag_frames.unwrap() - id_lag - drift).abs() < 1e-9);
    }
}

#[test]
fn every_play_starts_and_ends_at_rest() {
    let mut taper_off = config();
    taper_off.control = Some(TwrControl {
        rest_taper: false,
        ..TwrControl::default()
    });
    let mut off = TwrService::new(player(sim_backend()), taper_off).unwrap();
    off.advance().unwrap();
    off.proceed().unwrap();
    off.advance().unwrap();
    let plain = off.drive_time().to_vec();
    assert!(plain[0] != 0.0, "the inverse rings into the lead-in");

    let mut svc = service(sim_backend(), config(), Mode::Ok);
    let drives = run(&mut svc);
    assert!(drives.len() > 3);
    for (i, d) in drives.iter().enumerate() {
        assert_eq!(d[0], 0.0, "play {i} starts at rest");
        assert_eq!(*d.last().unwrap(), 0.0, "play {i} ends at rest");
    }
    // The first play's drive is the same seed at the same level: outside the
    // outer half of each pad it is the drive untapered, bit for bit.
    let g = svc.grid().clone();
    let head = g.pad_pre / 2;
    let tail = (g.n - g.pad_pre - g.section_len) / 2;
    for k in head..g.n - tail {
        assert_eq!(drives[0][k].to_bits(), plain[k].to_bits(), "sample {k}");
    }
}

#[test]
fn every_call_is_recorded_between_plays() {
    let root =
        std::env::temp_dir().join(format!("hertzforge_twr_law_records_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut svc = service(sim_backend(), config(), Mode::Ok);
    let dir = svc.start_capture(&root).unwrap();
    run(&mut svc);
    svc.stop_capture().unwrap();
    assert_eq!(svc.state(), TwrState::Complete);

    let law: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("law.json")).unwrap()).unwrap();
    assert_eq!(law["law"]["id"], "test-law");
    assert_eq!(law["guards"]["max_misses"], 3);
    let index = std::fs::read_to_string(dir.join("law_calls.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = index
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let calls = svc.plays().iter().filter(|r| r.law.is_some()).count();
    assert_eq!(lines.len(), calls, "the seed and every correction");
    assert_eq!(lines[0]["call"], "seed");
    assert!(lines.iter().all(|l| l["used"] == true));
    let bin = std::fs::metadata(dir.join("law_calls.bin")).unwrap().len();
    assert_eq!(
        bin,
        (lines.len() * svc.grid().n * 4) as u64,
        "each call's drive, f32"
    );

    // Each row carries the call behind its drive.
    let first = svc.plays()[0]
        .law
        .clone()
        .expect("the seed is behind the first play");
    assert!(first.used);
    assert_eq!(first.values.len(), 3);
    let report = svc.report();
    let line = report.law.expect("the report's law line");
    assert_eq!(line.id, "test-law");
    assert_eq!(line.misses, 0);
    let _ = std::fs::remove_dir_all(&root);
}
