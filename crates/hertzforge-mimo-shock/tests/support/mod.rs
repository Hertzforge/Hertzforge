//! What the engine tests share: stations on the simulator's shock plants, the
//! channel tables, the targets, a manual clock and test laws.

#![allow(dead_code)]

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
use hertzforge_dsp::srs::{octave_freqs, OctaveFraction};
use hertzforge_hal::Backend;
use hertzforge_hal_sim::plants::ShockPlant;
use hertzforge_mimo_shock::builtin::InverseFactory;
use hertzforge_mimo_shock::law::LawManifest;
use hertzforge_mimo_shock::{
    ClassicalControl, DriveChannel, GuardPlan, IdentificationPlan, LadderPlan, LimitChannel,
    MimoShockConfig, MimoShockEngine, MimoShockLaw, MimoShockLawFactory, PulseInputs, PulseOutputs,
    Setup, SrsSpec, Target, TargetSpec, Timing, WaveformTarget,
};
use hertzforge_module_abi::StationClass;
use hertzforge_shock_common::law_guard::{LawClock, LawDescriptor, LawFault};
use hertzforge_shock_host::spec::{ClassicalPulseSpec, ClassicalShape};
use hertzforge_shock_host::tolerance::ToleranceStandard;
use hertzforge_srs::iterate::{SynthesisEngine, SynthesisOpts};
use hertzforge_srs::table::WaveletTable;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The shock plants' calibration rate.
pub const FS: f64 = 12_800.0;
/// Classical records: 0.64 s.
pub const LEN_CLASSICAL: usize = 8192;
/// SRS records: 1.28 s (the wavelet composite runs about 0.3 s).
pub const LEN_SRS: usize = 16384;

/// A simulator station of `plant`, its caps rate `caps_hz`.
pub fn station(plant: ShockPlant, caps_hz: f64, seed: u64) -> Box<dyn Backend + Send> {
    Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(caps_hz)
            .noise_level(1e-4)
            .shared_clock(true)
            .seed(seed)
            .plant(plant.build().model)
            .build(),
    )
}

/// A table of `nc` Control rows (1..), `nl` Monitor rows, then `nd` Drive rows.
pub fn table(nc: usize, nl: usize, nd: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    for k in 0..nc + nl + nd {
        let mut c = Channel::new(k as u32 + 1);
        c.role = if k < nc {
            ChannelRole::Control
        } else if k < nc + nl {
            ChannelRole::Monitor
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        t.channels.push(c);
    }
    t
}

/// A classical spec: a half-sine of `amp_g[c]` g, 11 ms, at every control,
/// with rectangular shoulders four times its duration (about -8 % of its
/// peak, inside the standard's quiet band, as the single-output runs use).
pub fn classical(amp_g: &[f64]) -> TargetSpec {
    TargetSpec::Classical {
        controls: amp_g
            .iter()
            .map(|&a| {
                let mut pulse = ClassicalPulseSpec::new(ClassicalShape::HalfSine, a, 11.0);
                pulse.compensation = CompensationConfig {
                    shape: CompensationShape::Rectangular,
                    pre_frac: 4.0,
                    post_frac: 4.0,
                    symmetric: true,
                };
                ClassicalControl {
                    pulse,
                    inverted: false,
                }
            })
            .collect(),
        standard: ToleranceStandard::Mil810_516,
        lead_s: 0.05,
    }
}

/// The SRS natural frequencies: third octaves, 50 Hz to 2 kHz.
pub fn srs_freqs() -> Vec<f64> {
    octave_freqs(50.0, 2000.0, OctaveFraction::Third)
}

/// An SRS spec: at each control, +6 dB an octave from 50 Hz to 300 Hz, then
/// flat at `flat_g[c]` g to 2 kHz; tolerance -3 to +3 dB; wavelet synthesis.
pub fn srs(flat_g: &[f64]) -> TargetSpec {
    let f = srs_freqs();
    let points = flat_g
        .iter()
        .map(|&a| {
            f.iter()
                .map(|&fz| if fz < 300.0 { a * fz / 300.0 } else { a })
                .collect()
        })
        .collect();
    let tables = flat_g
        .iter()
        .map(|_| {
            SynthesisEngine::Wavsyn(
                WaveletTable::default_over_band(50.0, 2000.0, OctaveFraction::Third)
                    .expect("a table"),
            )
        })
        .collect();
    TargetSpec::Srs {
        spec: SrsSpec {
            natural_hz: f,
            q: 10.0,
            points,
            tol_lo_db: -3.0,
            tol_hi_db: 3.0,
            tables,
            synthesis: SynthesisOpts::default(),
            inverted: Vec::new(),
        },
        lead_s: 0.05,
    }
}

/// The twist pattern: controls 1 and 3 upside down. The 4 x 4 structure's
/// first mode (the one the joints soften most) is a twist of the plate, which
/// in-phase targets at its four controls barely excite.
pub const TWIST: [bool; 4] = [false, true, false, true];

/// A classical spec on the twist pattern: 20 g half-sines, 11 ms.
pub fn classical_twist() -> TargetSpec {
    let mut t = classical(&[20.0; 4]);
    if let TargetSpec::Classical { controls, .. } = &mut t {
        for (c, inv) in controls.iter_mut().zip(TWIST) {
            c.inverted = inv;
        }
    }
    t
}

/// An SRS spec on the twist pattern: +6 dB an octave from 50 Hz to 100 Hz,
/// then flat at 50 g to 2 kHz, at every control.
pub fn srs_twist() -> TargetSpec {
    let mut t = srs(&[50.0; 4]);
    if let TargetSpec::Srs { spec, .. } = &mut t {
        for row in spec.points.iter_mut() {
            for (v, f) in row.iter_mut().zip(&spec.natural_hz) {
                *v = if *f < 100.0 { 50.0 * f / 100.0 } else { 50.0 };
            }
        }
        spec.inverted = TWIST.to_vec();
    }
    t
}

/// A run's configuration on `nc` controls (table 1..), `nl` limits and `nd`
/// drives, record `len`; the target is a placeholder the spec replaces.
pub fn config(nc: usize, nl: usize, nd: usize, len: usize) -> MimoShockConfig {
    MimoShockConfig {
        fs_hz: FS,
        record_len: len,
        band_lo_hz: 5.0,
        band_hi_hz: 2500.0,
        controls: (1..=nc as u32).collect(),
        drives: (0..nd)
            .map(|d| DriveChannel {
                channel: (nc + nl + d) as u32 + 1,
                ceiling_v: 10.0,
                abort_v: 15.0,
            })
            .collect(),
        limits: (0..nl)
            .map(|l| LimitChannel {
                channel: (nc + l) as u32 + 1,
                abort_peak: 1e6,
            })
            .collect(),
        target: Target::Waveform(WaveformTarget {
            records: vec![0.0; nc * len],
        }),
        timing: Timing::Free { window_s: 0.002 },
        ladder: LadderPlan::default(),
        identification: IdentificationPlan {
            drive_vrms: 1.0,
            frames: 40,
            coherence_floor: 0.5,
            review: false,
        },
        guards: GuardPlan::default(),
        min_spacing_s: 1.0,
    }
}

/// A clock that moves only when told to: law calls take no time unless a test
/// law advances it.
#[derive(Default)]
pub struct ManualClock(AtomicU64);

impl ManualClock {
    pub fn advance(&self, s: f64) {
        let now = f64::from_bits(self.0.load(Ordering::SeqCst));
        self.0.store((now + s).to_bits(), Ordering::SeqCst);
    }
}

impl LawClock for ManualClock {
    fn now_s(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::SeqCst))
    }
}

/// Arm, identify and seed an engine on `plant` with `spec` and `law`; the
/// engine and its clock.
pub fn ready(
    plant: ShockPlant,
    cfg: MimoShockConfig,
    spec: TargetSpec,
    law: Arc<dyn MimoShockLawFactory>,
) -> (MimoShockEngine, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::default());
    let (nc, nl, nd) = (cfg.controls.len(), cfg.limits.len(), cfg.drives.len());
    let mut e = MimoShockEngine::new(cfg, law)
        .expect("a good configuration")
        .with_target_spec(spec)
        .with_clock(clock.clone())
        .with_pacing(false);
    e.arm(
        station(plant, FS, 11),
        table(nc, nl, nd),
        StationClass::HertzforgeSim,
    )
    .expect("armed");
    e.identify().expect("identified");
    assert!(e.end().is_none(), "held: {:?}", e.end());
    e.proceed().expect("proceeded");
    (e, clock)
}

/// What a test law does to the drives a real law answers.
#[derive(Clone)]
pub enum Twist {
    /// Multiply every answer by this on each iteration (compounding).
    Grow(f64),
    /// Multiply every answer by this (the same factor every time).
    Scale(f64),
    /// Panic on the first iteration.
    Panic,
    /// Advance the clock by this many seconds on every iteration.
    Slow(f64),
    /// Answer one sample short.
    Short,
    /// Answer with a value that is not finite.
    NonFinite,
    /// Multiply the seed by this.
    SeedTimes(f64),
    /// Add a tone of this many volts at this frequency, Hz, from the first
    /// iteration on.
    AddTone(f64, f64),
    /// Report this as its predicted over-test of every answer, dB.
    Predict(f64),
    /// Add to the seed a copy of itself delayed by this many seconds at this
    /// gain.
    SeedEcho(f64, f64),
}

/// A law that is `inverse` twisted.
pub struct Twisted {
    pub inner: InverseFactory,
    pub twist: Twist,
    pub clock: Arc<Mutex<Option<Arc<ManualClock>>>>,
}

impl Twisted {
    pub fn new(twist: Twist) -> Self {
        Twisted {
            inner: InverseFactory::default(),
            twist,
            clock: Arc::new(Mutex::new(None)),
        }
    }
}

struct TwistedLaw {
    inner: Box<dyn MimoShockLaw + Send>,
    twist: Twist,
    factor: f64,
    clock: Arc<Mutex<Option<Arc<ManualClock>>>>,
    fs_hz: f64,
    record_len: usize,
}

impl MimoShockLaw for TwistedLaw {
    fn name(&self) -> &str {
        "twisted"
    }

    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, LawFault> {
        let mut o = self.inner.seed(level_db)?;
        if let Twist::SeedTimes(k) = self.twist {
            o.drives.iter_mut().for_each(|v| *v *= k);
        }
        if let Twist::SeedEcho(delay_s, gain) = self.twist {
            let n = self.record_len;
            let k = (delay_s * self.fs_hz).round() as usize;
            for row in o.drives.chunks_mut(n) {
                let orig = row.to_vec();
                for i in k..n {
                    row[i] += gain * orig[i - k];
                }
            }
        }
        Ok(o)
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, LawFault> {
        let mut o = self.inner.iterate(inputs)?;
        match self.twist {
            Twist::Grow(k) => {
                self.factor *= k;
                let f = self.factor;
                o.drives.iter_mut().for_each(|v| *v *= f);
            }
            Twist::Scale(k) => o.drives.iter_mut().for_each(|v| *v *= k),
            Twist::Panic => panic!("a test law gives up"),
            Twist::Slow(s) => {
                if let Some(c) = self.clock.lock().unwrap().as_ref() {
                    c.advance(s);
                }
            }
            Twist::Short => {
                o.drives.pop();
            }
            Twist::NonFinite => o.drives[7] = f64::NAN,
            Twist::SeedTimes(_) | Twist::SeedEcho(..) => {}
            Twist::Predict(v) => o
                .values
                .push(hertzforge_shock_common::law_guard::LawValue::new(
                    hertzforge_mimo_shock::engine::LAW_PREDICTED_OVER_TEST,
                    v,
                )),
            Twist::AddTone(v, hz) => {
                for (i, x) in o.drives.iter_mut().enumerate() {
                    let t = (i % self.record_len) as f64 / self.fs_hz;
                    *x += v * (std::f64::consts::TAU * hz * t).sin();
                }
            }
        }
        Ok(o)
    }
}

impl MimoShockLawFactory for Twisted {
    fn build(&self, setup: &Setup) -> Result<Box<dyn MimoShockLaw + Send>, LawFault> {
        Ok(Box::new(TwistedLaw {
            inner: self.inner.build(setup)?,
            twist: self.twist.clone(),
            factor: 1.0,
            clock: self.clock.clone(),
            fs_hz: setup.fs_hz,
            record_len: setup.record_len,
        }))
    }

    fn describe(&self) -> LawDescriptor {
        let mut d = self.inner.describe();
        d.id = "twisted".into();
        d.name = "twisted".into();
        d
    }

    fn manifest(&self) -> LawManifest {
        let mut m = self.inner.manifest();
        m.values
            .push(hertzforge_mimo_shock::engine::LAW_PREDICTED_OVER_TEST.into());
        m
    }
}

/// The largest ratio of any drive's peak over the same drive's peak on the
/// previous pulse, with the level step taken out.
pub fn worst_drive_growth(e: &MimoShockEngine) -> f64 {
    let p = e.pulses();
    p.windows(2)
        .flat_map(|w| {
            let step = 10f64.powf((w[1].level_db - w[0].level_db) / 20.0);
            w[1].drive_vpk
                .iter()
                .zip(&w[0].drive_vpk)
                .map(move |(a, b)| a / (b * step))
        })
        .fold(0.0f64, f64::max)
}

/// The largest measured peak ratio of any control on any pulse.
pub fn worst_peak_ratio(e: &MimoShockEngine) -> f64 {
    e.pulses()
        .iter()
        .flat_map(|p| p.controls.iter().map(|c| c.verdict.peak_ratio))
        .fold(0.0f64, f64::max)
}

/// One line per pulse, for the test's output.
pub fn trace(e: &MimoShockEngine) -> String {
    let mut s = String::new();
    for p in e.pulses() {
        s.push_str(&format!(
            "pulse {:>2} at {:>6.1} dB pass {} err {:?} pk {:?} vpk {:?} used {} miss {:?}\n",
            p.index,
            p.level_db,
            p.passed,
            p.controls
                .iter()
                .map(|c| (c.verdict.error_db * 100.0).round() / 100.0)
                .collect::<Vec<_>>(),
            p.controls
                .iter()
                .map(|c| (c.verdict.peak_ratio * 1000.0).round() / 1000.0)
                .collect::<Vec<_>>(),
            p.drive_vpk
                .iter()
                .map(|v| (v * 1000.0).round() / 1000.0)
                .collect::<Vec<_>>(),
            p.law_used,
            p.law_miss
        ));
    }
    s.push_str(&format!("end {:?}\n", e.end()));
    s
}

/// A shot path that adds measurement noise to every input row of every shot
/// after the first two (the identification's probe and shot): Gaussian, its
/// standard deviation `fraction` of the row's own rms, from a fixed seed.
pub struct NoisyShot {
    pub inner: hertzforge_shock_host::BackendPulseFirer<Box<dyn Backend + Send>>,
    pub fraction: f64,
    pub shots: usize,
    pub rng: u64,
}

impl NoisyShot {
    pub fn new(plant: ShockPlant, nc: usize, nl: usize, nd: usize, fraction: f64) -> Self {
        NoisyShot {
            inner: hertzforge_shock_host::BackendPulseFirer::new(
                station(plant, FS, 21),
                table(nc, nl, nd),
                StationClass::HertzforgeSim,
            )
            .unwrap(),
            fraction,
            shots: 0,
            rng: 0x2545_f491_4f6c_dd1d,
        }
    }

    fn normal(&mut self) -> f64 {
        let mut u = || {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            ((self.rng >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (a, b) = (u(), u());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

impl hertzforge_mimo_shock::ShotPath for NoisyShot {
    fn arm(&mut self, fs_hz: f64) -> Result<f64, String> {
        use hertzforge_shock_host::PulseFirer;
        let f = std::mem::replace(
            &mut self.inner,
            hertzforge_shock_host::BackendPulseFirer::new(
                station(ShockPlant::Linear4x4, FS, 1),
                table(4, 0, 4),
                StationClass::HertzforgeSim,
            )
            .unwrap(),
        );
        self.inner = f.with_requested_rate(fs_hz);
        self.inner.arm().map_err(|e| e.to_string())?;
        self.inner.actual_fs_hz().ok_or_else(|| "no rate".into())
    }
    fn set_quiet_frames(&mut self, frames: usize) {
        use hertzforge_shock_host::PulseFirer;
        self.inner.set_quiet_frames(frames);
    }
    fn fire<'a>(
        &mut self,
        drive: &[f64],
        n_drive: usize,
        cancel: Option<&'a std::sync::atomic::AtomicBool>,
        hooks: Option<&'a mut dyn hertzforge_shock_common::shot::ShotHooks>,
    ) -> Result<hertzforge_shock_host::PulseRows, hertzforge_mimo_shock::ShotError> {
        let mut ctl = hertzforge_shock_host::service::firer::FireControl {
            cancel,
            observer: None,
            hooks,
        };
        let mut rows = self
            .inner
            .fire_rows(drive, n_drive, &mut ctl)
            .map_err(|e| hertzforge_mimo_shock::ShotError::Failed(e.to_string()))?;
        self.shots += 1;
        if self.shots > 2 {
            for r in 0..rows.rows.len() {
                let len = rows.rows[r].len().max(1) as f64;
                let rms = (rows.rows[r].iter().map(|v| v * v).sum::<f64>() / len).sqrt();
                for i in 0..rows.rows[r].len() {
                    let z = self.normal();
                    rows.rows[r][i] += self.fraction * rms * z;
                }
            }
        }
        Ok(rows)
    }
    fn stop(&mut self) -> Result<(), String> {
        use hertzforge_shock_host::PulseFirer;
        self.inner.stop().map_err(|e| e.to_string())
    }
    fn emergency_stop(&mut self) -> Result<(), String> {
        use hertzforge_shock_host::PulseFirer;
        self.inner.emergency_stop().map_err(|e| e.to_string())
    }
}

/// [`ready`] on a shot path of the test's.
pub fn ready_on(
    shot: Box<dyn hertzforge_mimo_shock::ShotPath>,
    cfg: MimoShockConfig,
    spec: TargetSpec,
    law: Arc<dyn MimoShockLawFactory>,
) -> MimoShockEngine {
    let mut e = MimoShockEngine::new(cfg, law)
        .expect("a good configuration")
        .with_target_spec(spec)
        .with_clock(Arc::new(ManualClock::default()))
        .with_pacing(false);
    e.arm_with(shot).expect("armed");
    e.identify().expect("identified");
    assert!(e.end().is_none(), "held: {:?}", e.end());
    e.proceed().expect("proceeded");
    e
}
