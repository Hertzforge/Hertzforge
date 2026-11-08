//! Shared fixtures for the closed-loop tests: a plant whose response is the
//! drive times a gain (plus optional noise), adversarial laws around the
//! built-in, a test clock, and the run sequence.
#![allow(dead_code)]

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_shock_common::law_guard::{LawClock, LawDescriptor, LawFault};
use hertzforge_shock_common::{LadderAdvance, ShockRunMode};
use hertzforge_srs::iterate::SynthesisOpts;
use hertzforge_srs::service::pacer::VirtualPacer;
use hertzforge_srs::service::plant::{FiredResponses, PlantError};
use hertzforge_srs::service::report::RunState;
use hertzforge_srs::{
    FirePath, PulsePlant, SrsControl, SrsInputs, SrsLaw, SrsLawFactory, SrsLawSetup, SrsOutputs,
    SrsRatioFactory, SrsShockService, SrsTestConfig,
};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// The rate every fixture runs at.
pub const FS: f64 = 20_000.0;

/// What the plant was handed: every drive, in order (the identification
/// burst first).
pub type Fired = Arc<Mutex<Vec<Vec<f64>>>>;

/// A plant whose control response is `gain` times the drive, plus
/// deterministic noise of `noise` EU peak.
pub struct Linear {
    pub gain: f64,
    pub noise: f64,
    state: u64,
    pub fired: Fired,
}

impl Linear {
    pub fn new(gain: f64) -> Self {
        Linear {
            gain,
            noise: 0.0,
            state: 0x2545_f491_4f6c_dd1d,
            fired: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn noisy(gain: f64, noise: f64) -> Self {
        Linear {
            noise,
            ..Linear::new(gain)
        }
    }

    fn next(&mut self) -> f64 {
        // xorshift64*, mapped to [-1, 1).
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let x = self.state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11;
        (x as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

impl PulsePlant for Linear {
    fn fs_hz(&self) -> f64 {
        FS
    }
    fn ensure_open(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn fire(
        &mut self,
        drive: &[f64],
        ring_out_samples: usize,
        _cancel: Option<&AtomicBool>,
    ) -> Result<FiredResponses, PlantError> {
        self.fired.lock().unwrap().push(drive.to_vec());
        let mut control: Vec<f64> = drive.iter().map(|v| v * self.gain).collect();
        control.resize(drive.len() + ring_out_samples, 0.0);
        if self.noise > 0.0 {
            for v in control.iter_mut() {
                *v += self.noise * self.next();
            }
        }
        Ok(FiredResponses {
            control_g: control,
            monitors: Vec::new(),
        })
    }
    fn stop(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn emergency_stop(&mut self) -> Result<(), String> {
        Ok(())
    }
}

pub fn siso_table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut drv = Channel::new(1);
    drv.role = ChannelRole::Drive;
    drv.description = "Drive".into();
    let mut ctrl = Channel::new(2);
    ctrl.role = ChannelRole::Control;
    ctrl.description = "Control".into();
    t.channels.push(drv);
    t.channels.push(ctrl);
    t
}

/// A flat target of `target_g` over 100 to 2000 Hz at 1/3 octave, AUTO -6 then
/// 0 dB, two passing pulses a level, at most 5 a level, the drive ceiling 1 V
/// and abort 2 V, every other closed-loop setting at its default.
pub fn closed_cfg(target_g: f64) -> SrsTestConfig {
    let mut c = SrsTestConfig::flat(100.0, 2000.0, target_g);
    c.synthesis = SynthesisOpts {
        max_iters: 120,
        tol_db: 1.5,
        max_gain_db_per_iter: 6.0,
        q: 10.0,
    };
    c.schedule.mode = ShockRunMode::Auto;
    c.schedule.table_db = vec![-6.0, 0.0];
    c.schedule.start_db = -6.0;
    c.schedule.pulses_per_level = vec![2];
    c.schedule.advance = LadderAdvance::OnPass;
    c.schedule.max_pulses_per_level = 5;
    c.compute_pvss = false;
    c.control = Some(SrsControl::new(1.0, 2.0));
    c
}

/// The service on `plant`, on a virtual pacer (each fire costs 10 ms).
pub fn service(cfg: SrsTestConfig, plant: Linear) -> SrsShockService {
    let mut s = SrsShockService::new(
        cfg,
        siso_table(),
        FirePath::Host(Box::new(plant)),
        Some(std::env::temp_dir().join(format!("hertzforge-srs-closed-{}", std::process::id()))),
    )
    .unwrap();
    s.set_pacer(Box::new(VirtualPacer::new(0.010)));
    s
}

/// Feasibility, arm, identification: the state it ends in (the review hold,
/// or an end).
pub fn identify(s: &mut SrsShockService) -> RunState {
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid().unwrap();
    s.state()
}

/// The most pulses [`run`] fires, whatever the run says.
pub const RUN_PULSE_BOUND: usize = 100;

/// The whole run: identify, proceed from the review hold, fire until it ends
/// (or [`RUN_PULSE_BOUND`] pulses, so a run whose guard is broken fails its
/// test instead of running on).
pub fn run(s: &mut SrsShockService) -> RunState {
    if identify(s) == RunState::AwaitingReview {
        s.proceed_after_review().unwrap();
    }
    for _ in 0..RUN_PULSE_BOUND {
        if !matches!(s.state(), RunState::Armed | RunState::Firing) || !s.fire_pulse().unwrap() {
            break;
        }
    }
    s.state()
}

/// A clock that moves only when a law moves it.
#[derive(Default)]
pub struct TestClock(Mutex<f64>);

impl TestClock {
    pub fn advance(&self, s: f64) {
        *self.0.lock().unwrap() += s;
    }
}

impl LawClock for TestClock {
    fn now_s(&self) -> f64 {
        *self.0.lock().unwrap()
    }
}

/// How an adversarial law misbehaves. Each keeps the built-in's seed unless
/// it says otherwise, and misbehaves on every iteration.
#[derive(Clone)]
pub enum Mode {
    /// A NaN gain.
    Nan,
    /// A zero gain.
    Zero,
    /// A negative gain.
    Negative,
    /// One gain short.
    Short,
    /// Every gain +`db` a pulse.
    Rise(f64),
    /// Each iteration takes 0.3 s on the test clock.
    Slow(Arc<TestClock>),
    /// Panics.
    Panic,
    /// Seeds 6 dB low and never moves.
    NeverCorrects,
    /// A NaN gain at the seed.
    SeedNan,
}

/// The built-in law, made to misbehave by `mode`.
pub struct Adversary(pub Mode);

struct AdversaryLaw {
    inner: Box<dyn SrsLaw + Send>,
    mode: Mode,
}

impl SrsLawFactory for Adversary {
    fn build(&self, setup: &SrsLawSetup<'_>) -> Result<Box<dyn SrsLaw + Send>, LawFault> {
        Ok(Box::new(AdversaryLaw {
            inner: SrsRatioFactory::default().build(setup)?,
            mode: self.0.clone(),
        }))
    }
    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "adversary".into(),
            name: "adversary".into(),
            version: "0".into(),
            sha256: None,
            params: serde_json::json!({}),
        }
    }
}

impl SrsLaw for AdversaryLaw {
    fn name(&self) -> &str {
        "adversary"
    }
    fn seed(&mut self, inputs: &SrsInputs<'_>) -> Result<SrsOutputs, LawFault> {
        let mut out = self.inner.seed(inputs)?;
        match self.mode {
            Mode::SeedNan => out.gains[0] = f64::NAN,
            Mode::NeverCorrects => out.gains.iter_mut().for_each(|g| *g *= 0.5),
            _ => {}
        }
        Ok(out)
    }
    fn iterate(&mut self, inputs: &SrsInputs<'_>) -> Result<SrsOutputs, LawFault> {
        let mut out = self.inner.iterate(inputs)?;
        match &self.mode {
            Mode::Nan => out.gains[0] = f64::NAN,
            Mode::Zero => out.gains[0] = 0.0,
            Mode::Negative => out.gains[0] = -1.0,
            Mode::Short => {
                out.gains.pop();
            }
            Mode::Rise(db) => out
                .gains
                .iter_mut()
                .for_each(|g| *g = 10f64.powf(db / 20.0)),
            Mode::Slow(clock) => clock.advance(0.3),
            Mode::Panic => panic!("the adversary panicked"),
            Mode::NeverCorrects => out.gains.iter_mut().for_each(|g| *g = 1.0),
            Mode::SeedNan => {}
        }
        Ok(out)
    }
}

/// The largest absolute value of `x`.
pub fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}
