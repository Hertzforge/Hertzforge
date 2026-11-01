//! Sine run-service configuration.
//!
//! One serde-serializable [`SineTestConfig`] carries everything the
//! [`super::SineService`] needs: the profile, the run [`SineMode`], the
//! control-channel set + composite mode, the limit-profile registry, the blend
//! knob, compression preset, tracking bandwidth, the level ladder, sysID +
//! coherence knobs, the drive Vpk guard, the servo-fault debounce, the analytic
//! kinematic monitor toggle, the capture rules, the COLA aux-channel config, and
//! user metadata.
//!
//! Nothing here reaches into hertzforge-dsp/hertzforge-engine; the service builds the live
//! objects from these plain records at `arm`.

use crate::control::{CompositeMode, CompressionPreset, FeedforwardConfig, NotchGovernorConfig};
use crate::profile::SineProfile;
use crate::scheduler::{BidirSweepConfig, DwellPlan, LadderConfig, RstdConfig, StepSineConfig};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_shaker_db::{MovingMass, ShakerRecord};
use serde::{Deserialize, Serialize};

/// Serde mirror for the foreign [`BandwidthMode`] (hertzforge-dsp derives no serde;
/// the remote-derive keeps the config serializable without an hertzforge-dsp edit).
#[derive(Serialize, Deserialize)]
#[serde(remote = "BandwidthMode")]
enum BandwidthModeDef {
    FixedHz(f64),
    Proportional { pct: f64, max_hz: f64 },
}

/// Config validation errors.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// A generic bad-config with a human message.
    BadConfig(String),
    /// The profile failed its own validation.
    BadProfile(String),
    /// The channel table was structurally unsound or role-inconsistent.
    BadTable(String),
    /// The control-channel set referenced a channel not in the table, or was
    /// empty / had a bad weight.
    BadControlSet(String),
    /// The COLA width invariant is unsatisfiable on this backend.
    BadCola(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::BadConfig(s) => write!(f, "sine config error: {s}"),
            ConfigError::BadProfile(s) => write!(f, "sine profile error: {s}"),
            ConfigError::BadTable(s) => write!(f, "channel table error: {s}"),
            ConfigError::BadControlSet(s) => write!(f, "control-channel set error: {s}"),
            ConfigError::BadCola(s) => write!(f, "COLA config error: {s}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The run mode (which scheduler drives the frequency): pinned as one of
/// four families. Each carries its own config; the service builds the matching
/// scheduler state at engage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SineMode {
    /// Bidirectional sweep.
    Sweep(BidirSweepConfig),
    /// Dwell at a fixed frequency.
    Dwell(DwellPlan),
    /// Step-sine (research extra).
    StepSine(StepSineConfig),
    /// Resonance search-track-dwell.
    Rstd(RstdConfig),
}

impl SineMode {
    /// The frequency band this mode sweeps / searches over, for the sysID grid
    /// and the profile evaluation span. Dwell returns a degenerate one-point
    /// band around its frequency.
    pub fn band(&self) -> (f64, f64) {
        match self {
            SineMode::Sweep(c) => c.band,
            SineMode::StepSine(c) => c.band,
            SineMode::Rstd(c) => c.band,
            SineMode::Dwell(p) => (p.freq_hz, p.freq_hz),
        }
    }
}

/// A single control channel: the channel index (into the channel table) and its
/// composite weight. Weights are normalized by the composite.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ControlChannel {
    pub index: u32,
    pub weight: f64,
}

/// The control set: the channels the servo controls to, and how they composite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlSet {
    pub channels: Vec<ControlChannel>,
    pub mode: CompositeMode,
}

impl ControlSet {
    /// The control channel indices, in order.
    pub fn indices(&self) -> Vec<u32> {
        self.channels.iter().map(|c| c.index).collect()
    }

    /// The control weights, in order.
    pub fn weights(&self) -> Vec<f64> {
        self.channels.iter().map(|c| c.weight).collect()
    }
}

/// A limit profile bound to one channel. `profile` is either a channel-owned
/// table or a name-shared one; the service resolves shared names against the
/// registry. Here we carry the resolved [`SineProfile`] directly so
/// a channel may share another's table by cloning it, or own its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitProfile {
    /// The channel this limit applies to (into the channel table).
    pub channel_index: u32,
    /// The limit profile (evaluated at the current frequency as a bound).
    pub profile: SineProfile,
    /// An optional shared-table name for display / dedup (N channels may share
    /// one table). Informational; the resolved profile is
    /// the source of truth.
    #[serde(default)]
    pub shared_name: Option<String>,
}

/// When to capture streaming telemetry (mirrors Random's rule set, sine terms).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum CaptureRule {
    /// Never capture.
    Off,
    /// Capture the entire test (from engage to stop).
    EntireTest,
    /// Manual: the operator starts/stops capture explicitly.
    Manual,
    /// Auto: capture the first `n_s` seconds after each level entry settles.
    AutoFirstSeconds { n_s: f64 },
}

/// Capture configuration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CaptureConfig {
    pub rule: CaptureRule,
    /// Settle debounce (s) before the AUTO window opens after a level entry.
    pub level_settle_s: f64,
    /// Periodic snapshot-save interval (s); `None` = no periodic save.
    pub periodic_save_s: Option<f64>,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        CaptureConfig {
            rule: CaptureRule::Off,
            level_settle_s: 1.0,
            periodic_save_s: None,
        }
    }
}

/// The COLA reference-output config (subject to the COLA width invariant).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColaConfig {
    /// Whether COLA is enabled (adds a second drive channel). Default off.
    pub enabled: bool,
    /// Constant COLA amplitude (drive scale units; volts-peak on hardware).
    pub volts_pk: f64,
    /// COLA waveform, frequency-locked to the sweep NCO phase.
    pub waveform: ColaWaveform,
}

impl Default for ColaConfig {
    fn default() -> Self {
        ColaConfig {
            enabled: false,
            volts_pk: 1.0,
            waveform: ColaWaveform::Sine,
        }
    }
}

/// COLA waveform: all frequency-locked to the sweep via the SAME NCO
/// phase, constant amplitude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColaWaveform {
    /// A sine at the swept frequency (uses the NCO's sin directly).
    Sine,
    /// A square wave: `sign(sin(phase))`.
    Square,
    /// A constant DC reference (frequency-independent).
    DcRef,
}

/// A user-defined metadata field carried into the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaField {
    pub key: String,
    pub value: String,
}

/// The full sine test configuration. serde-serializable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SineTestConfig {
    /// The control profile.
    pub profile: SineProfile,
    /// The run mode + its scheduler config.
    pub mode: SineMode,
    /// The control-channel set + composite mode.
    pub control: ControlSet,
    /// Limit profiles on other channels.
    #[serde(default)]
    pub limit_profiles: Vec<LimitProfile>,
    /// The Hinv-vs-feedback blend weight `w` in `[0, 1]`.
    pub blend_w: f64,
    /// The compression preset (the servo `(ki, max_rate)` pair).
    pub compression: CompressionPreset,
    /// The tracking-filter bandwidth mode.
    #[serde(with = "BandwidthModeDef")]
    pub tracking_bw: BandwidthMode,
    /// The feedforward trust-mask config (sysID coherence + |H| floors).
    #[serde(default)]
    pub feedforward: FeedforwardConfig,
    /// The notch governor config.
    #[serde(default)]
    pub notch: NotchGovernorConfig,
    /// The level ladder.
    pub ladder: LadderConfig,
    /// The sysID drive level in dB relative to the full profile (default -20).
    pub sysid_level_db: f64,
    /// The coherence floor for the report-and-hold gate (default 0.5).
    pub coherence_floor: f64,
    /// The drive Vpk ceiling (clamp) and abort trip.
    pub drive_vpk_ceiling: f64,
    pub drive_vpk_abort: f64,
    /// Consecutive faulted control ticks before the debounced servo-fault abort.
    /// `0` disables the fault abort.
    pub fault_debounce_ticks: u32,
    /// Whether to run the analytic kinematic monitor against the
    /// shaker's max velocity / displacement.
    pub kinematic_monitor: bool,
    /// Capture rules.
    #[serde(default)]
    pub capture: CaptureConfig,
    /// COLA aux-output config.
    #[serde(default)]
    pub cola: ColaConfig,
    /// The shaker record for feasibility + the kinematic monitor bounds (`None`
    /// => feasibility not evaluated, kinematic monitor disabled).
    #[serde(default)]
    pub shaker: Option<ShakerRecord>,
    /// The moving-mass stack for feasibility.
    #[serde(default)]
    pub moving_mass: MovingMass,
    /// The control-tick size in frames (~2-5 ms at Sim rates). This is
    /// the CONTROL-update cadence and is distinct from the engine's acquisition
    /// block, which comes from `ConfiguredActual`.
    ///
    /// The old note here said the service "enforces the engine runs at
    /// frames_per_tick = 1 (SimBackend per-block drive latch)". That latch has
    /// since been removed; the HERTZFORGE-SIM path still requests one frame per tick,
    /// for the reason named by `hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES`.
    pub control_tick_frames: usize,
    /// The acquisition sample rate this run ASKS the station for, Hz.
    /// **`None` = take the station's own rate**, which is the behaviour every
    /// config written before this field existed had, and still has.
    ///
    /// # Why this is a request and never an answer
    ///
    /// The device is the authority. What this field does is state which rate the
    /// operator wants ASKED FOR at `configure`; the answer comes back as
    /// `ConfiguredActual::actual_rate_hz` and is adopted by `adopt_configured_rate`
    /// exactly as it always was. On this chassis the two routinely differ (the 9234
    /// rides a 13.1072 MHz/(256n) ladder: ask for 8192 and it programs 8533.33), and
    /// every Hz-denominated thing the service owns is re-derived from the ANSWER.
    /// So a `Some(fs)` here can change what the run asks for and can never change
    /// what it believes it got.
    ///
    /// # Why the field exists at all
    ///
    /// The v1 call was "no rate knob - the station rate is the rate". The operator
    /// reversed it at acceptance (2026-08-09): every other module gives him
    /// the choice, and a sine run whose rate he cannot set is a sine run he has to
    /// go and reconfigure the station for.
    #[serde(default)]
    pub fs_hz: Option<f64>,
    /// Hold at [`crate::RunState::AwaitingReview`] after the coherence gate
    /// PASSES, until an operator releases it. **Default ON.**
    ///
    /// The gate answers "is this plant coherent enough for the loop to trust its
    /// own sysID"; it does not answer "is the article ready to be driven at
    /// level". Between those two questions is the last moment a human can look at
    /// the |H| and coherence the run is about to close a loop on. Random ships the
    /// same hold and the expensive-article case is the one where being
    /// wrong is unrecoverable, so the default is ON and a production operator
    /// turns it off once.
    ///
    /// **Not the same thing as [`crate::RunState::CoherenceHold`], and they must
    /// never share a release verb.** CoherenceHold is the gate FAILING; releasing
    /// it with a click would be a one-button bypass of a safety refusal, and only
    /// a Reset clears it.
    #[serde(default = "default_review_hold")]
    pub review_hold: bool,
    /// User metadata carried into the report.
    #[serde(default)]
    pub metadata: Vec<MetaField>,
}

fn default_review_hold() -> bool {
    true
}

/// Ratio of delay-induced beat to tracking corner at which the check turns
/// ADVISORY. `D = 1/(1 + ratio^2)` puts this at a 0.34 dB
/// systematic under-read - small, but the first point where the number the
/// servo controls to is measurably not the number on the shaker.
pub const TRACKING_LAG_ADVISORY_RATIO: f64 = 0.2;

/// Ratio at which the check REFUSES. `D = 1/(1 + 0.5^2) = 0.8` is a 1.94 dB
/// under-read: the servo would be commanded to overdrive the true response by
/// that much at the top of the band, and the only protection that can see it is
/// the raw-ring watchdog (which does not read the demodulated path). That is
/// exactly the tracking-lag overdrive seen on the chassis, and it is not a run
/// worth starting.
pub const TRACKING_LAG_REFUSE_RATIO: f64 = 0.5;

/// The verdict of the sweep-rate-vs-tracking-bandwidth check.
///
/// # What is being checked
///
/// The tracking filter demodulates the incoming sample against a reference at
/// the CURRENT NCO frequency, but the sample is an echo of drive emitted `lag`
/// frames earlier. Over the transport lag `tau` the sweep has moved the
/// commanded frequency by `df`, so the wanted phasor arrives at `df` off
/// baseband and the two-pole low-pass attenuates it by
///
/// ```text
///   D = 1 / (1 + (df / bw)^2)
/// ```
///
/// The amplitude readout therefore UNDER-READS by that factor, and an amplitude
/// servo closing on an under-read overdrives the true response by `1/D`. Nothing
/// on the demodulated path can notice - the servo, the scalar abort and the
/// kinematic monitor all read the same droopped number - so the only protection
/// left is the raw-ring watchdog, and by the time it fires the article has
/// already seen the overdrive.
///
/// The check is a comparison of two frequencies, `df` and `bw`, both of which
/// the config already determines. It is not a model of the plant.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackingLagVerdict {
    /// The demodulator keeps up across the whole band.
    Ok {
        /// Worst `df / bw` over the band.
        ratio: f64,
    },
    /// Marginal: run it, but the readout is measurably low at the worst
    /// frequency and a report reader deserves to know.
    Advisory {
        ratio: f64,
        /// Frequency (Hz) at which `ratio` peaks.
        at_hz: f64,
        message: String,
    },
    /// Refused: the servo would be controlling to a number the shaker is not at.
    Refuse {
        ratio: f64,
        at_hz: f64,
        message: String,
    },
}

impl TrackingLagVerdict {
    /// The worst `df / bw` the check found.
    pub fn ratio(&self) -> f64 {
        match self {
            TrackingLagVerdict::Ok { ratio }
            | TrackingLagVerdict::Advisory { ratio, .. }
            | TrackingLagVerdict::Refuse { ratio, .. } => *ratio,
        }
    }

    /// The operator-readable sentence, if there is one.
    pub fn message(&self) -> Option<&str> {
        match self {
            TrackingLagVerdict::Ok { .. } => None,
            TrackingLagVerdict::Advisory { message, .. }
            | TrackingLagVerdict::Refuse { message, .. } => Some(message),
        }
    }

    /// Whether this verdict refuses the run.
    pub fn is_refusal(&self) -> bool {
        matches!(self, TrackingLagVerdict::Refuse { .. })
    }
}

/// Assess a sweep law against a tracking bandwidth and a measured/estimated
/// transport lag. See [`TrackingLagVerdict`] for the mechanism.
///
/// `lag_frames` is `tau * fs_hz`: the sysID-measured drive-to-response pipeline
/// lag when there is one, otherwise the engage lead-in plus whatever the device
/// contributes. A lag of zero (or a non-sweeping mode) can never fail this
/// check, which is correct: with no delay there is no beat.
///
/// The band is SCANNED rather than evaluated at one frequency. Under a log law
/// with a fixed corner the worst case is the band top, but under a proportional
/// corner the ratio is flat until the `max_hz` cap or the image-rejection clamp
/// binds, and under a linear law it is worst at the band BOTTOM. Scanning costs
/// nothing at config time and removes the need for the check to know which of
/// those it is looking at.
pub fn assess_tracking_lag(
    band: (f64, f64),
    rate: hertzforge_dsp::sweep::SweepRate,
    bw: BandwidthMode,
    lag_frames: usize,
    fs_hz: f64,
) -> TrackingLagVerdict {
    if lag_frames == 0 || !(fs_hz.is_finite() && fs_hz > 0.0) {
        return TrackingLagVerdict::Ok { ratio: 0.0 };
    }
    let (lo, hi) = band;
    if !(lo.is_finite() && hi.is_finite()) || lo <= 0.0 || hi < lo {
        return TrackingLagVerdict::Ok { ratio: 0.0 };
    }
    let tau_s = lag_frames as f64 / fs_hz;
    const SCAN: usize = 129;
    let mut worst = 0.0f64;
    let mut worst_hz = lo;
    let mut worst_df = 0.0f64;
    let mut worst_bw = 0.0f64;
    for k in 0..SCAN {
        let f = if hi > lo {
            lo * (hi / lo).powf(k as f64 / (SCAN - 1) as f64)
        } else {
            lo
        };
        // Where the commanded frequency was one transport lag ago. Works for
        // both laws and both sweep directions; the magnitude is what beats.
        let past = rate.freq_at(f, -tau_s);
        if !past.is_finite() || past <= 0.0 {
            continue;
        }
        let df = (f - past).abs();
        let eff_bw = bw.effective_bw(f);
        if !(eff_bw.is_finite() && eff_bw > 0.0) {
            continue;
        }
        let ratio = df / eff_bw;
        if ratio > worst {
            worst = ratio;
            worst_hz = f;
            worst_df = df;
            worst_bw = eff_bw;
        }
    }
    if worst <= TRACKING_LAG_ADVISORY_RATIO {
        return TrackingLagVerdict::Ok { ratio: worst };
    }
    let droop_db = 20.0 * (1.0 / (1.0 + worst * worst)).log10();
    // The remedy targets the ADVISORY line, i.e. what it takes to be clean -
    // not the refusal line, which would only move the run from refused to warned.
    let slow_by = worst / TRACKING_LAG_ADVISORY_RATIO;
    let bw_needed = worst_df / TRACKING_LAG_ADVISORY_RATIO;
    let (mode, remedy) = match bw {
        BandwidthMode::FixedHz(b) => (
            format!("FixedHz({b} Hz)"),
            // A fixed corner is the mode with a frequency ANCHOR: the beat grows
            // with f while the corner does not, so the droop worsens all the way
            // up the band. A proportional corner scales with f and the ratio goes
            // flat, which is why the deployed configs never see this.
            format!(
                "slow the sweep by {slow_by:.1}x, widen the corner to {bw_needed:.1} Hz, \
                 or switch to a proportional corner (which scales with frequency and \
                 removes the anchor entirely)"
            ),
        ),
        BandwidthMode::Proportional { pct, max_hz } => {
            // Say WHICH of the three rules produced the corner, because that is
            // which knob to reach for. (`effective_bw`: pct rule, then the
            // max_hz cap, then the f/2 image-rejection clamp.)
            let pct_rule = worst_hz * pct / 100.0;
            let binding =
                if worst_bw >= 0.5 * worst_hz - 1e-12 && 0.5 * worst_hz < pct_rule.min(max_hz) {
                    "the f/2 image-rejection clamp"
                } else if worst_bw >= max_hz - 1e-12 && max_hz < pct_rule {
                    "the max_hz cap"
                } else {
                    "the percentage rule"
                };
            (
                format!("Proportional({pct}% capped at {max_hz} Hz)"),
                format!(
                    "slow the sweep by {slow_by:.1}x, or widen the corner at \
                     {worst_hz:.1} Hz from {worst_bw:.2} Hz to {bw_needed:.1} Hz \
                     ({binding} is what sets it there)"
                ),
            )
        }
    };
    let rate_txt = match rate {
        hertzforge_dsp::sweep::SweepRate::LogOctPerMin(r) => format!("{r} oct/min"),
        hertzforge_dsp::sweep::SweepRate::LinearHzPerSec(r) => format!("{r} Hz/s"),
    };
    // BOTH NUMBERS, always. A refusal that says only "too fast" leaves the
    // operator guessing which of the two knobs to turn and by how much.
    let message = format!(
        "sweep rate {rate_txt} against tracking bandwidth {mode}: at {worst_hz:.1} Hz the \
         {lag_frames}-frame ({:.1} ms at {fs_hz:.1} Hz) drive-to-response lag puts the \
         response {worst_df:.2} Hz off the demodulation reference, against an effective \
         tracking corner of {worst_bw:.2} Hz. Ratio {worst:.2} (advisory above \
         {TRACKING_LAG_ADVISORY_RATIO}, refused above {TRACKING_LAG_REFUSE_RATIO}); the \
         amplitude readout under-reads by {droop_db:.1} dB there, so the servo would \
         overdrive the true response by that much and only the raw-ring watchdog could \
         see it. Fix: {remedy}.",
        tau_s * 1e3,
    );
    if worst > TRACKING_LAG_REFUSE_RATIO {
        TrackingLagVerdict::Refuse {
            ratio: worst,
            at_hz: worst_hz,
            message,
        }
    } else {
        TrackingLagVerdict::Advisory {
            ratio: worst,
            at_hz: worst_hz,
            message,
        }
    }
}

impl SineTestConfig {
    /// Run [`assess_tracking_lag`] against this config's own band, sweep rate and
    /// tracking bandwidth.
    ///
    /// Only [`SineMode::Sweep`] can fail it. Dwell holds `df/dt = 0`; step-sine
    /// dwells at each step; RSTD's search is a slow hunt around a resonance and
    /// its dwell is stationary - none of them move the commanded frequency far
    /// enough within one transport lag for the beat to exist.
    pub fn assess_tracking_lag(&self, lag_frames: usize, fs_hz: f64) -> TrackingLagVerdict {
        match &self.mode {
            SineMode::Sweep(c) => {
                assess_tracking_lag(c.band, c.rate, self.tracking_bw, lag_frames, fs_hz)
            }
            _ => TrackingLagVerdict::Ok { ratio: 0.0 },
        }
    }

    /// Validate the config: profile grammar, blend weight, control set, the
    /// drive-guard ordering, and the control tick. Does NOT validate against the
    /// channel table (the service does that in `new`, where it has the table).
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.profile
            .validate()
            .map_err(|e| ConfigError::BadProfile(e.to_string()))?;
        if self.blend_w.is_nan() || !(0.0..=1.0).contains(&self.blend_w) {
            return Err(ConfigError::BadConfig(format!(
                "blend_w must be in [0,1] (got {})",
                self.blend_w
            )));
        }
        if self.control.channels.is_empty() {
            return Err(ConfigError::BadControlSet("no control channels".into()));
        }
        for c in &self.control.channels {
            if c.weight <= 0.0 || c.weight.is_nan() {
                return Err(ConfigError::BadControlSet(format!(
                    "control channel {} weight must be > 0 (got {})",
                    c.index, c.weight
                )));
            }
        }
        if self.drive_vpk_ceiling <= 0.0 || self.drive_vpk_ceiling.is_nan() {
            return Err(ConfigError::BadConfig(format!(
                "drive_vpk_ceiling must be > 0 (got {})",
                self.drive_vpk_ceiling
            )));
        }
        if self.drive_vpk_abort < self.drive_vpk_ceiling || self.drive_vpk_abort.is_nan() {
            return Err(ConfigError::BadConfig(format!(
                "drive_vpk_abort must be >= drive_vpk_ceiling (got {} < {})",
                self.drive_vpk_abort, self.drive_vpk_ceiling
            )));
        }
        if self.coherence_floor.is_nan() || !(0.0..=1.0).contains(&self.coherence_floor) {
            return Err(ConfigError::BadConfig(format!(
                "coherence_floor must be in [0,1] (got {})",
                self.coherence_floor
            )));
        }
        if self.control_tick_frames == 0 {
            return Err(ConfigError::BadConfig(
                "control_tick_frames must be >= 1".into(),
            ));
        }
        // A REQUESTED rate must be a rate. `None` is the station's own and is not
        // this check's business; a `Some` that is zero, negative or NaN would reach
        // `AcquisitionConfig::new` and be refused there by the driver, with the
        // driver's words instead of the operator's field name.
        if let Some(fs) = self.fs_hz {
            if !fs.is_finite() || fs <= 0.0 {
                return Err(ConfigError::BadConfig(format!(
                    "fs_hz must be a finite rate > 0 when set (got {fs}); leave it unset \
                     to take the station's own rate"
                )));
            }
        }
        if self.cola.enabled && (self.cola.volts_pk <= 0.0 || self.cola.volts_pk.is_nan()) {
            return Err(ConfigError::BadCola(format!(
                "COLA volts_pk must be > 0 when enabled (got {})",
                self.cola.volts_pk
            )));
        }
        Ok(())
    }

    /// The number of drive (output) channels this config implies: 1 control
    /// drive, plus 1 for COLA when enabled (the COLA width invariant).
    pub fn out_channels(&self) -> usize {
        if self.cola.enabled {
            2
        } else {
            1
        }
    }
}
