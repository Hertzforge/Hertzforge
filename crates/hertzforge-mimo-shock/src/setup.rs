//! What a MIMO Shock law is built for: fixed for the run.
//!
//! Mirrors `mimo-shock-setup` of `wit/control-law/shock-mimo.wit`. Validation
//! ([`Setup::validate`]) names the first rule broken ([`SetupRule`]).

use crate::limits::{MAX_CONTROLS, MAX_DRIVES, MAX_RECORD_LEN, MIN_RECORD_LEN};
use hertzforge_sdm::MatrixStack;
use serde::{Deserialize, Serialize};
use std::fmt;

/// What a test asks of its control channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// A time history at every control: a compensated classical pulse.
    Waveform,
    /// A Shock Response Spectrum at every control.
    Srs,
}

impl TargetKind {
    /// Both kinds.
    pub const ALL: [TargetKind; 2] = [TargetKind::Waveform, TargetKind::Srs];

    /// The stable wire name: `waveform` or `srs`.
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Waveform => "waveform",
            TargetKind::Srs => "srs",
        }
    }
}

/// A waveform target: the full-level record every control is to follow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveformTarget {
    /// `controls * record_len` values, control after control, EU.
    pub records: Vec<f64>,
}

/// The decay of one synthesis component.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentShape {
    /// A damped sinusoid with this damping ratio (above 0, below 1).
    Decay(f64),
    /// A windowed sine of this many half-cycles (odd, 3 or more).
    HalfCycles(u32),
}

/// One component of an SRS synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SrsComponent {
    /// Frequency, Hz.
    pub hz: f64,
    /// Peak amplitude at full level, EU; negative for an inverted component.
    pub amplitude: f64,
    /// Start of the component after the record's start, s.
    pub delay_s: f64,
    pub shape: ComponentShape,
}

/// An SRS target: the spectrum each control is judged against and the
/// full-level waveform the core synthesized to meet it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrsTarget {
    /// The SRS natural frequencies, Hz, ascending.
    pub natural_hz: Vec<f64>,
    /// The SRS quality factor.
    pub q: f64,
    /// The full-level target SRS, `controls * natural_hz.len()` values,
    /// control after control, EU peak.
    pub points: Vec<f64>,
    /// The tolerance below the target, dB (negative or zero).
    pub tol_lo_db: f64,
    /// The tolerance above the target, dB (positive or zero).
    pub tol_hi_db: f64,
    /// The synthesized full-level waveform at every control,
    /// `controls * record_len` values, control after control, EU.
    pub waveforms: Vec<f64>,
    /// The synthesis table behind each control's waveform, one per control.
    pub components: Vec<Vec<SrsComponent>>,
}

/// The target of a test: one kind for every control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Waveform(WaveformTarget),
    Srs(SrsTarget),
}

impl Target {
    /// The target's kind.
    pub fn kind(&self) -> TargetKind {
        match self {
            Target::Waveform(_) => TargetKind::Waveform,
            Target::Srs(_) => TargetKind::Srs,
        }
    }

    /// The full-level waveform at every control, control after control: the
    /// target itself, or the waveform synthesized for an SRS.
    pub fn waveforms(&self) -> &[f64] {
        match self {
            Target::Waveform(w) => &w.records,
            Target::Srs(s) => &s.waveforms,
        }
    }
}

/// Hold timing: each control's arrival is held to a set delay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HoldTiming {
    /// The delay each control is held to, s, one per control.
    pub delays_s: Vec<f64>,
    /// The window a control's measured delay may move within, s.
    pub window_s: f64,
}

/// How the arrival times of the controls are treated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Timing {
    /// Each control is judged on its own; its delay is reported. `window_s` is
    /// the window the delay is searched over.
    Free { window_s: f64 },
    /// Each control is held to its set delay.
    Hold(HoldTiming),
}

impl Timing {
    /// The stable wire name: `free` or `hold`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Timing::Free { .. } => "free",
            Timing::Hold(_) => "hold",
        }
    }
}

/// The core's plant estimate on the pulse grid, from one identification shot
/// with every drive played at once. The path delays are in `frf`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlantStack {
    /// H, controls by drives, on `record_len / 2 + 1` lines, EU per volt.
    pub frf: MatrixStack,
    /// Multiple coherence of each control on all drives, 0 to 1:
    /// `controls * lines` values, control after control.
    pub coherence: Vec<f64>,
    /// Lines the core trusts, one per line.
    pub trusted: Vec<bool>,
}

/// What a MIMO Shock law is built for: fixed for the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    /// The rate the records are at, Hz.
    pub fs_hz: f64,
    /// Samples in every record: a power of two, [`MIN_RECORD_LEN`] to
    /// [`MAX_RECORD_LEN`].
    pub record_len: usize,
    /// Drives the law commands, 1 to [`MAX_DRIVES`].
    pub n_drives: usize,
    /// Control channels, 1 to [`MAX_CONTROLS`].
    pub n_controls: usize,
    /// The band the law corrects, Hz.
    pub band_lo_hz: f64,
    pub band_hi_hz: f64,
    pub target: Target,
    pub timing: Timing,
    pub plant: PlantStack,
    /// Each drive's ceiling, volts peak, one per drive.
    pub drive_ceiling_v: Vec<f64>,
    /// The level ladder, dB, ascending, ending at 0.
    pub ladder_db: Vec<f64>,
}

/// The rule a [`Setup`] broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupRule {
    /// `n_controls` is 0 or over [`MAX_CONTROLS`].
    ControlsOutOfRange,
    /// `n_drives` is 0 or over [`MAX_DRIVES`].
    DrivesOutOfRange,
    /// `record_len` is not a power of two from [`MIN_RECORD_LEN`] to
    /// [`MAX_RECORD_LEN`].
    RecordLenInvalid,
    /// `fs_hz` is not a finite number above 0.
    RateInvalid,
    /// The band is not `0 <= lo < hi <= fs / 2`.
    BandInvalid,
    /// A target array is not the length its shape asks for.
    TargetShape,
    /// A target value is not finite.
    TargetNotFinite,
    /// The SRS frequencies are empty, not ascending or not above 0.
    SrsFrequencies,
    /// The SRS quality factor is not above 0.5 and at most 100.
    SrsQ,
    /// The SRS tolerance is not `tol_lo_db <= 0 <= tol_hi_db`.
    SrsTolerance,
    /// A synthesis component is not usable.
    SrsComponent,
    /// The timing window is not a finite number above 0, or a hold delay list
    /// is not one finite delay per control.
    Timing,
    /// The plant is not `record_len / 2 + 1` lines of controls by drives.
    PlantShape,
    /// A plant value is not finite.
    PlantNotFinite,
    /// The coherence is not `controls * lines` values from 0 to 1.
    Coherence,
    /// `trusted` is not one flag per line.
    TrustedShape,
    /// The drive ceilings are not one value above 0 per drive.
    DriveCeiling,
    /// The ladder is empty, not ascending, or does not end at 0 dB.
    Ladder,
    /// The law does not accept the target's kind.
    TargetNotAccepted,
    /// A run's configuration names one table channel twice among its controls,
    /// drives and limits.
    DuplicateChannel,
}

impl SetupRule {
    /// The stable name.
    pub fn as_str(self) -> &'static str {
        match self {
            SetupRule::ControlsOutOfRange => "controls_out_of_range",
            SetupRule::DrivesOutOfRange => "drives_out_of_range",
            SetupRule::RecordLenInvalid => "record_len_invalid",
            SetupRule::RateInvalid => "rate_invalid",
            SetupRule::BandInvalid => "band_invalid",
            SetupRule::TargetShape => "target_shape",
            SetupRule::TargetNotFinite => "target_not_finite",
            SetupRule::SrsFrequencies => "srs_frequencies",
            SetupRule::SrsQ => "srs_q",
            SetupRule::SrsTolerance => "srs_tolerance",
            SetupRule::SrsComponent => "srs_component",
            SetupRule::Timing => "timing",
            SetupRule::PlantShape => "plant_shape",
            SetupRule::PlantNotFinite => "plant_not_finite",
            SetupRule::Coherence => "coherence",
            SetupRule::TrustedShape => "trusted_shape",
            SetupRule::DriveCeiling => "drive_ceiling",
            SetupRule::Ladder => "ladder",
            SetupRule::TargetNotAccepted => "target_not_accepted",
            SetupRule::DuplicateChannel => "duplicate_channel",
        }
    }
}

/// Why a [`Setup`] was refused: the rule and the facts.
#[derive(Debug, Clone, PartialEq)]
pub struct SetupError {
    pub rule: SetupRule,
    pub message: String,
}

impl SetupError {
    pub(crate) fn new(rule: SetupRule, message: impl Into<String>) -> Self {
        SetupError {
            rule,
            message: message.into(),
        }
    }
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.rule.as_str())
    }
}

impl std::error::Error for SetupError {}

fn finite(xs: &[f64]) -> bool {
    xs.iter().all(|x| x.is_finite())
}

impl Setup {
    /// Lines on the pulse grid: `record_len / 2 + 1`.
    pub fn lines(&self) -> usize {
        self.record_len / 2 + 1
    }

    /// Line spacing, Hz.
    pub fn df_hz(&self) -> f64 {
        self.fs_hz / self.record_len as f64
    }

    /// Check every rule, in the order the fields are declared.
    pub fn validate(&self) -> Result<(), SetupError> {
        use SetupRule as R;
        let (nc, nd, len) = (self.n_controls, self.n_drives, self.record_len);
        if nc == 0 || nc > MAX_CONTROLS {
            return Err(SetupError::new(
                R::ControlsOutOfRange,
                format!("{nc} control channels; a run controls 1 to {MAX_CONTROLS}"),
            ));
        }
        if nd == 0 || nd > MAX_DRIVES {
            return Err(SetupError::new(
                R::DrivesOutOfRange,
                format!("{nd} drives; a run commands 1 to {MAX_DRIVES}"),
            ));
        }
        if !len.is_power_of_two() || !(MIN_RECORD_LEN..=MAX_RECORD_LEN).contains(&len) {
            return Err(SetupError::new(
                R::RecordLenInvalid,
                format!(
                    "a record of {len} samples; a record is a power of two from \
                     {MIN_RECORD_LEN} to {MAX_RECORD_LEN}"
                ),
            ));
        }
        if !(self.fs_hz.is_finite() && self.fs_hz > 0.0) {
            return Err(SetupError::new(
                R::RateInvalid,
                format!("a rate of {} Hz", self.fs_hz),
            ));
        }
        let (lo, hi) = (self.band_lo_hz, self.band_hi_hz);
        if !(lo.is_finite() && hi.is_finite() && 0.0 <= lo && lo < hi && hi <= self.fs_hz / 2.0) {
            return Err(SetupError::new(
                R::BandInvalid,
                format!(
                    "a band of {lo} to {hi} Hz; it rises from 0 Hz or more to at most half \
                     the rate"
                ),
            ));
        }
        self.validate_target()?;
        match &self.timing {
            Timing::Free { window_s } => {
                if !(window_s.is_finite() && *window_s > 0.0) {
                    return Err(SetupError::new(
                        R::Timing,
                        format!("a timing window of {window_s} s"),
                    ));
                }
            }
            Timing::Hold(h) => {
                if !(h.window_s.is_finite() && h.window_s > 0.0) {
                    return Err(SetupError::new(
                        R::Timing,
                        format!("a timing window of {} s", h.window_s),
                    ));
                }
                if h.delays_s.len() != nc || !finite(&h.delays_s) {
                    return Err(SetupError::new(
                        R::Timing,
                        format!(
                            "{} hold delays for {nc} controls; one finite delay per control",
                            h.delays_s.len()
                        ),
                    ));
                }
            }
        }
        let lines = self.lines();
        let p = &self.plant;
        if p.frf.dims() != (lines, nc, nd) {
            let (l, r, c) = p.frf.dims();
            return Err(SetupError::new(
                R::PlantShape,
                format!(
                    "a plant of {l} lines by {r} x {c}; the grid needs {lines} lines by \
                     {nc} controls x {nd} drives"
                ),
            ));
        }
        if !p
            .frf
            .data()
            .iter()
            .all(|z| z.re.is_finite() && z.im.is_finite())
        {
            return Err(SetupError::new(
                R::PlantNotFinite,
                "the plant holds a value that is not finite",
            ));
        }
        if p.coherence.len() != nc * lines || !p.coherence.iter().all(|g| (0.0..=1.0).contains(g)) {
            return Err(SetupError::new(
                R::Coherence,
                format!(
                    "{} coherence values; the grid needs {} from 0 to 1",
                    p.coherence.len(),
                    nc * lines
                ),
            ));
        }
        if p.trusted.len() != lines {
            return Err(SetupError::new(
                R::TrustedShape,
                format!("{} trusted flags for {lines} lines", p.trusted.len()),
            ));
        }
        if self.drive_ceiling_v.len() != nd
            || !self
                .drive_ceiling_v
                .iter()
                .all(|v| v.is_finite() && *v > 0.0)
        {
            return Err(SetupError::new(
                R::DriveCeiling,
                format!(
                    "{} drive ceilings for {nd} drives; one above 0 V per drive",
                    self.drive_ceiling_v.len()
                ),
            ));
        }
        let l = &self.ladder_db;
        if l.is_empty()
            || !finite(l)
            || l.windows(2).any(|w| w[1] <= w[0])
            || l.last() != Some(&0.0)
        {
            return Err(SetupError::new(
                R::Ladder,
                format!("a ladder of {l:?} dB; it ascends and ends at 0 dB"),
            ));
        }
        Ok(())
    }

    fn validate_target(&self) -> Result<(), SetupError> {
        use SetupRule as R;
        let (nc, len) = (self.n_controls, self.record_len);
        let shape = |what: &str, got: usize, want: usize| {
            SetupError::new(
                R::TargetShape,
                format!("{got} {what}; the target needs {want}"),
            )
        };
        let waves = self.target.waveforms();
        if waves.len() != nc * len {
            return Err(shape("waveform values", waves.len(), nc * len));
        }
        if !finite(waves) {
            return Err(SetupError::new(
                R::TargetNotFinite,
                "a target waveform holds a value that is not finite",
            ));
        }
        let Target::Srs(s) = &self.target else {
            return Ok(());
        };
        let nf = s.natural_hz.len();
        if nf == 0
            || !finite(&s.natural_hz)
            || s.natural_hz[0] <= 0.0
            || s.natural_hz.windows(2).any(|w| w[1] <= w[0])
        {
            return Err(SetupError::new(
                R::SrsFrequencies,
                "the SRS frequencies are empty, not above 0 Hz or not ascending",
            ));
        }
        if !(s.q.is_finite() && s.q > 0.5 && s.q <= 100.0) {
            return Err(SetupError::new(
                R::SrsQ,
                format!(
                    "an SRS quality factor of {}; above 0.5 and at most 100",
                    s.q
                ),
            ));
        }
        if s.points.len() != nc * nf {
            return Err(shape("SRS points", s.points.len(), nc * nf));
        }
        if !s.points.iter().all(|x| x.is_finite() && *x > 0.0) {
            return Err(SetupError::new(
                R::TargetNotFinite,
                "an SRS target point is not a finite value above 0",
            ));
        }
        if !(s.tol_lo_db.is_finite()
            && s.tol_hi_db.is_finite()
            && s.tol_lo_db <= 0.0
            && s.tol_hi_db >= 0.0)
        {
            return Err(SetupError::new(
                R::SrsTolerance,
                format!(
                    "an SRS tolerance of {} to {} dB; the lower is 0 dB or below, the upper \
                     0 dB or above",
                    s.tol_lo_db, s.tol_hi_db
                ),
            ));
        }
        if s.components.len() != nc {
            return Err(shape("component tables", s.components.len(), nc));
        }
        let nyquist = self.fs_hz / 2.0;
        for (c, table) in s.components.iter().enumerate() {
            for (i, k) in table.iter().enumerate() {
                let shape_ok = match k.shape {
                    ComponentShape::Decay(z) => z.is_finite() && z > 0.0 && z < 1.0,
                    ComponentShape::HalfCycles(n) => n >= 3 && n % 2 == 1,
                };
                let ok = k.hz.is_finite()
                    && k.hz > 0.0
                    && k.hz < nyquist
                    && k.amplitude.is_finite()
                    && k.delay_s.is_finite()
                    && k.delay_s >= 0.0
                    && shape_ok;
                if !ok {
                    return Err(SetupError::new(
                        R::SrsComponent,
                        format!("component {i} of control {c} is not usable: {k:?}"),
                    ));
                }
            }
        }
        Ok(())
    }
}
