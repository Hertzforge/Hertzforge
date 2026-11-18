//! The loop's delay as the law is told it: the trackers' stated delay factor
//! is their measured one, and the transport-delay case (50 ms on every drive,
//! engaged at -6 dB, a 6 s dwell) settles only when the delay is told.

mod common;

use common::*;
use hertzforge_dsp::tone::{ToneTracker, TRACKER_DELAY_FACTOR};
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_hal_sim::{PlantModel, SimBackend, SimConfig};
use hertzforge_mimo_sine::law::{LawDescriptor, LawManifest, LawSetup};
use hertzforge_mimo_sine::records::Outcome;
use hertzforge_mimo_sine::service::{EngineOptions, MimoSineService};
use hertzforge_mimo_sine::*;
use hertzforge_module_abi::StationClass;
use hertzforge_sine::scheduler::{LadderConfig, RungFor};
use std::f64::consts::TAU;
use std::sync::Arc;

/// A tone a little off the tracker's reference frequency turns the tracked
/// phase at the offset; the reading lags that turn by the tracker's group
/// delay. Measured at several bandwidths and rates against the stated factor.
#[test]
fn mimo_sine_the_trackers_stated_delay_factor_is_its_measured_one() {
    for (fs, bn) in [(16384.0, 10.0), (16384.0, 40.0), (51200.0, 5.0)] {
        let mut t = ToneTracker::new(fs, bn);
        let (f0, df) = (200.0, 0.02 * bn);
        let n = (40.0 / bn * fs) as usize;
        let mut last = (0.0, 0.0);
        for k in 0..n {
            let ts = k as f64 / fs;
            let ref_phase = TAU * f0 * ts;
            t.update((ref_phase + TAU * df * ts).sin(), ref_phase);
            last = (ts, t.phase());
        }
        // Read phase = 2 pi df (t - tau), wrapped.
        let want = TAU * df * last.0;
        let mut lagged = (want - last.1).rem_euclid(TAU);
        if lagged > std::f64::consts::PI {
            lagged -= TAU;
        }
        let tau = lagged / (TAU * df);
        let factor = tau * bn;
        println!("fs {fs} B {bn}: measured factor {factor:.4}, stated {TRACKER_DELAY_FACTOR}");
        assert!(
            factor <= TRACKER_DELAY_FACTOR && factor > 0.95 * TRACKER_DELAY_FACTOR,
            "{factor}"
        );
    }
}

/// `matrix` with its first drive 6 dB low, so the loop has a correction to
/// make; built with the loop's delay told, or as it was before it was told
/// (the testbed tracker's factor and no transport).
struct Low(Arc<dyn MimoSineLawFactory>, bool);

struct LowLaw(Box<dyn MimoSineLaw + Send>);

impl MimoSineLaw for LowLaw {
    fn name(&self) -> &str {
        "low"
    }
    fn engage(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut law::LawValues,
    ) -> Result<(), LawFault> {
        self.0.engage(x, o, v)?;
        o.drive.iter_mut().for_each(|d| *d *= 0.5);
        Ok(())
    }
    fn tick(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut law::LawValues,
    ) -> Result<(), LawFault> {
        self.0.tick(x, o, v)
    }
}

impl MimoSineLawFactory for Low {
    fn build(&self, s: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        let mut s = s.clone();
        if !self.1 {
            // As before the loop's delay was told: the testbed tracker's
            // factor and no transport.
            s.transport_delay_s = 0.0;
            s.tracker_delay_factor = law::default_tracker_delay_factor();
        }
        Ok(Box::new(LowLaw(self.0.build(&s)?)))
    }
    fn describe(&self) -> LawDescriptor {
        self.0.describe()
    }
    fn manifest(&self) -> LawManifest {
        self.0.manifest()
    }
}

const DELAY_S: f64 = 0.05;

const DWELL_HZ: f64 = 100.0;

/// The measurement noise in the delay case, g rms per sample: a hundredth of
/// the closed-loop tests'. At their noise the identification's group delay
/// (the largest phase slope over the elements of a line, read off a 0.25 Hz
/// grid) is set by the noise in the small elements' phase, about 0.13 s on
/// this structure, and it hides a 50 ms transport; quiet, it is the
/// structure's own, and the case shows what it is about.
const QUIET: f64 = NOISE / 100.0;

fn delayed_run(f: Arc<dyn MimoSineLawFactory>, hz: f64) -> (Option<Outcome>, f64, f64, f64) {
    let s = SineStructure::Linear4x4;
    // The plate's mass line alone (its modes taken out): a coupled response
    // with no resonance near the tone, so the identification shows almost no
    // group delay and the loop's delay is the transport and the tracker.
    let mut p = s.linear_structure();
    p.modes.clear();
    p.transport_delay_samples = (std::env::var("MSINE_DELAY")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(DELAY_S)
        * FS)
        .round() as usize;
    let b: SimBackend = SimConfig::new()
        .sample_rate_hz(FS)
        .plant(PlantModel::Jointed(p))
        .noise_level(QUIET)
        .seed(7)
        .shared_clock(true)
        .build();
    let mut st = setup(s, dwell(hz, 6.0), "matrix");
    st.schedule.ladder = LadderConfig {
        sysid_level_db: -20.0,
        start_db: -6.0,
        table_db: vec![-6.0],
        per_rung: RungFor::Seconds(1e6),
        full_for: RungFor::Seconds(1e6),
    };
    for c in &mut st.controls {
        c.profile = flat(1.0, 1.5, 20.0);
    }
    st.guards.abort_ticks = 20;
    let mut svc = MimoSineService::new(
        st,
        table(s),
        Box::new(b),
        f,
        // On the simulator the law's time budget is relaxed, as the other simulator
        // harnesses relax theirs, so a call a busy machine made late is not a miss here.
        EngineOptions {
            law_budget: Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET),
            ..EngineOptions::new(StationClass::HertzforgeSim)
        },
    )
    .unwrap();
    svc.arm().unwrap();
    let lag = svc.identify().unwrap().lag_frames;
    svc.engage().unwrap();
    let told = svc.law_setup().unwrap().transport_delay_s;
    let r = run(svc, 10.0);
    // The last two seconds of the dwell: the worst error and how much the
    // drive still moves from update to update.
    let tail: Vec<_> = r
        .ticks
        .iter()
        .filter(|t| t.time_s >= 2.0 && t.time_s < 6.0)
        .collect();
    let early = r
        .ticks
        .iter()
        .filter(|t| t.time_s < 2.0 && !t.settling)
        .flat_map(|t| t.errors_db().to_vec())
        .fold(0.0f64, |a, e| a.max(e.abs()));
    let g = r
        .ticks
        .iter()
        .filter(|t| !t.settling && !t.values[0].is_nan())
        .map(|t| t.values[0])
        .fold(0.0f64, f64::max);
    let gd = r
        .ticks
        .iter()
        .rfind(|t| !t.values[1].is_nan())
        .map(|t| t.values[1]);
    println!("{hz} Hz: worst in the first 2 s outside settle {early:.2} dB; largest gain {g:.4}; group delay {gd:?}");
    let worst = tail
        .iter()
        .flat_map(|t| t.errors_db().to_vec())
        .fold(0.0f64, |a, e| a.max(e.abs()));
    println!(
        "lag {lag} samples, transport told {told:.4} s: outcome {:?}, worst error from 2 s {worst:.2} dB",
        r.outcome
    );
    (r.outcome, worst, told, lag as f64 / FS)
}

#[test]
fn mimo_sine_a_transport_delay_told_to_the_law_settles_the_loop() {
    let (o, worst, told, lag_s) = delayed_run(Arc::new(Low(law("matrix"), true)), DWELL_HZ);
    assert!((told - lag_s).abs() < 1e-12);
    assert!(told >= DELAY_S);
    assert_eq!(o, Some(Outcome::Completed));
    assert!(worst < 0.5, "{worst}");
}

#[test]
fn mimo_sine_a_transport_delay_not_told_rings_or_loses_the_loop() {
    let (o, worst, _, _) = delayed_run(Arc::new(Low(law("matrix"), false)), DWELL_HZ);
    assert!(
        o != Some(Outcome::Completed) || worst > 1.5,
        "{o:?} {worst}"
    );
}
