//! The setup of a Modal Test: what the control service accepts at configure, in typed
//! form, with every value checked here and every refusal named.
//!
//! The wire form is parsed by the control service; this module holds the rules. A rule
//! that needs the channel table (a reference with no node, a drive row's range) is in
//! [`crate::plan`].

use std::fmt;

/// The block sizes a test may use, frames (powers of two).
pub const BLOCK_MIN: usize = 256;
pub const BLOCK_MAX: usize = 65_536;
/// The rates a station is asked for, Hz.
pub const RATE_MIN_HZ: f64 = 256.0;
pub const RATE_MAX_HZ: f64 = 204_800.0;
/// The drive ceiling's top, volts.
pub const CEILING_MAX_V: f64 = 10.0;
/// Averages a measurement may ask for.
pub const AVERAGES_MAX: u32 = 10_000;
/// The controlled stop: this far down, over [`STOP_RAMP_S`], then zero. The stop every
/// open-loop module ramps by (Playback's, Random's).
pub const STOP_RAMP_DEPTH_DB: f64 = 40.0;
pub const STOP_RAMP_S: f64 = 3.0;
/// The start ramp's longest, seconds.
pub const RAMP_MAX_S: f64 = 60.0;
/// Delay frames' most.
pub const DELAY_FRAMES_MAX: u32 = 100;

/// Excitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A hammer: nothing is driven.
    Impact,
    /// One or more shakers driven open loop.
    Shaker,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Impact => "impact",
            Mode::Shaker => "shaker",
        }
    }
}

/// The frequency response estimator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Estimator {
    H1,
    H2,
    Hv,
}

impl Estimator {
    pub fn name(self) -> &'static str {
        match self {
            Estimator::H1 => "H1",
            Estimator::H2 => "H2",
            Estimator::Hv => "Hv",
        }
    }
}

/// A point of the roving plan: the node every roving channel takes, and a direction
/// when exactly one channel roves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    pub node: u32,
    pub direction: Option<String>,
}

/// The roving plan: the channels (table indices) that move, and the points they visit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roving {
    pub channels: Vec<u32>,
    pub points: Vec<Point>,
}

/// How a hit is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptMode {
    /// An overloaded or double hit is rejected, anything else accepted. The operator's
    /// accept or reject overrides either way until the next hit lands.
    Auto,
    /// Every hit waits for the operator's accept or reject.
    Manual,
}

impl AcceptMode {
    pub fn name(self) -> &'static str {
        match self {
            AcceptMode::Auto => "auto",
            AcceptMode::Manual => "manual",
        }
    }
}

/// The force window on the reference: one from the trigger sample over `width_pct` of
/// the block, then a half-cosine fall over `taper_pct` of that width
/// ([`crate::window`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForceWindowSet {
    pub on: bool,
    pub width_pct: f64,
    pub taper_pct: f64,
}

impl Default for ForceWindowSet {
    fn default() -> Self {
        ForceWindowSet {
            on: true,
            width_pct: 10.0,
            taper_pct: 10.0,
        }
    }
}

/// The exponential window on every channel, set by its end value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpWindowSet {
    pub on: bool,
    /// The window's value at the block's end, percent.
    pub end_pct: f64,
}

impl Default for ExpWindowSet {
    fn default() -> Self {
        ExpWindowSet {
            on: false,
            end_pct: 10.0,
        }
    }
}

/// The force level a hit is judged against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForceTarget {
    /// Peak force, in the hammer row's unit.
    pub peak: f64,
    /// Percent either side of the target that is on target. A signal only: a hit off
    /// its target is never rejected for it.
    pub tolerance_pct: f64,
}

/// The tolerance a target takes when none is given, percent.
pub const TARGET_TOLERANCE_PCT: f64 = 20.0;

/// Impact mode's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpactSet {
    /// The trigger level, percent of the hammer row's range.
    pub trigger_pct: f64,
    /// The hammer's polarity: the sign its force takes on a hit. The trigger, the peak,
    /// the force target and the double-hit check all watch this sign.
    pub polarity: Polarity,
    /// The pre-trigger, percent of the block.
    pub pretrigger_pct: f64,
    /// Blocks after a captured block before the trigger re-arms.
    pub holdoff_blocks: f64,
    /// A second impact above this percent of the main peak (or six times the
    /// pre-trigger RMS, the larger), after the pulse has fallen below
    /// [`crate::hits::PULSE_END_FRACTION`] of it, is a double hit ([`crate::hits`]).
    pub double_hit_pct: f64,
    /// The force spectrum's drop from its maximum to the band's top above which the
    /// tip is reported (a warning, never a rejection), dB.
    pub tip_warn_db: f64,
    pub accept: AcceptMode,
    pub force_window: ForceWindowSet,
    pub exp_window: ExpWindowSet,
    /// Go to the next point, armed, when a point's averages are in.
    pub auto_advance: bool,
    pub target: Option<ForceTarget>,
}

impl Default for ImpactSet {
    fn default() -> Self {
        ImpactSet {
            trigger_pct: 10.0,
            polarity: Polarity::Positive,
            pretrigger_pct: 5.0,
            holdoff_blocks: 1.0,
            double_hit_pct: 5.0,
            tip_warn_db: 20.0,
            accept: AcceptMode::Auto,
            force_window: ForceWindowSet::default(),
            exp_window: ExpWindowSet::default(),
            auto_advance: true,
            target: None,
        }
    }
}

/// The sign a hammer's force takes on a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    Positive,
    Negative,
}

impl Polarity {
    pub fn name(self) -> &'static str {
        match self {
            Polarity::Positive => "positive",
            Polarity::Negative => "negative",
        }
    }

    /// `1` or `-1`: a force times it is positive on a hit.
    pub fn sign(self) -> f64 {
        match self {
            Polarity::Positive => 1.0,
            Polarity::Negative => -1.0,
        }
    }
}

/// A shaker source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Random,
    BurstRandom,
    PseudoRandom,
    PeriodicRandom,
    PeriodicChirp,
    BurstChirp,
}

impl SourceKind {
    pub fn name(self) -> &'static str {
        match self {
            SourceKind::Random => "random",
            SourceKind::BurstRandom => "burst_random",
            SourceKind::PseudoRandom => "pseudo_random",
            SourceKind::PeriodicRandom => "periodic_random",
            SourceKind::PeriodicChirp => "periodic_chirp",
            SourceKind::BurstChirp => "burst_chirp",
        }
    }

    pub fn from_name(s: &str) -> Option<SourceKind> {
        [
            SourceKind::Random,
            SourceKind::BurstRandom,
            SourceKind::PseudoRandom,
            SourceKind::PeriodicRandom,
            SourceKind::PeriodicChirp,
            SourceKind::BurstChirp,
        ]
        .into_iter()
        .find(|k| k.name() == s)
    }

    /// Whether the source may drive more than one shaker. Pseudo random repeats one
    /// frame, so every average sees the same forces and the force matrix is singular;
    /// the chirps are one per test in this version.
    pub fn several_drives(self) -> bool {
        matches!(
            self,
            SourceKind::Random | SourceKind::BurstRandom | SourceKind::PeriodicRandom
        )
    }

    /// Whether the averages are frame-synchronous (uniform window, no overlap).
    pub fn frame_synchronous(self) -> bool {
        self != SourceKind::Random
    }

    /// Whether a burst: on for part of the frame, quiet after.
    pub fn is_burst(self) -> bool {
        matches!(self, SourceKind::BurstRandom | SourceKind::BurstChirp)
    }

    /// The most a sample of the source can be, as a multiple of its RMS level: bounded
    /// by construction ([`crate::source`]). A random source is clipped at
    /// [`RANDOM_CREST`]; a chirp is a sine.
    pub fn crest(self) -> f64 {
        match self {
            SourceKind::PeriodicChirp | SourceKind::BurstChirp => std::f64::consts::SQRT_2,
            _ => RANDOM_CREST,
        }
    }
}

/// The crest a random source is clipped at: no sample exceeds this many times its RMS
/// level.
pub const RANDOM_CREST: f64 = 4.0;

/// How far inside the drive ceiling a level's bounded crest must stay: level times
/// crest is at most the ceiling times one less this (0.1 %). The source is bounded in
/// the f32 the station is handed, so no accepted level reaches the ceiling by rounding.
pub const CEILING_MARGIN: f64 = 1e-3;

/// One driven row and its level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriveSet {
    /// The drive row (table index).
    pub row: u32,
    /// Volts RMS (over the burst for a burst source).
    pub level_vrms: f64,
}

/// Shaker mode's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ShakerSet {
    pub drives: Vec<DriveSet>,
    pub source: SourceKind,
    /// The burst's part of the frame, percent.
    pub burst_pct: f64,
    pub ramp_s: f64,
    pub ceiling_v: f64,
    pub delay_frames: u32,
    /// Overlap of the averages of continuous random, percent.
    pub overlap_pct: f64,
    pub seed: u64,
}

impl Default for ShakerSet {
    fn default() -> Self {
        ShakerSet {
            drives: Vec::new(),
            source: SourceKind::BurstRandom,
            burst_pct: 50.0,
            ramp_s: 1.0,
            ceiling_v: 1.0,
            delay_frames: 2,
            overlap_pct: 50.0,
            seed: 1,
        }
    }
}

/// The default level per drive, volts RMS.
pub const LEVEL_DEFAULT_VRMS: f64 = 0.05;
/// The default averages: a hammer's, and a shaker's.
pub const AVERAGES_IMPACT: u32 = 5;
pub const AVERAGES_SHAKER: u32 = 30;
/// The default decay warning, percent: a burst frame's or a hit's response whose last
/// 5 % is above this share of its loudest 5 % carries `response_not_decayed`.
pub const DECAY_WARN_PCT_DEFAULT: f64 = 2.0;
/// The decay warning's bounds, percent.
pub const DECAY_WARN_PCT_MIN: f64 = 0.01;
pub const DECAY_WARN_PCT_MAX: f64 = 100.0;

/// A whole setup.
#[derive(Debug, Clone, PartialEq)]
pub struct ModalSetup {
    pub mode: Mode,
    /// The rate asked of the station, Hz.
    pub fs_hz: f64,
    /// The block (frame) length, frames.
    pub block: usize,
    /// The band, Hz.
    pub band: (f64, f64),
    pub estimator: Estimator,
    /// Averages per measurement.
    pub averages: u32,
    pub roving: Option<Roving>,
    pub impact: ImpactSet,
    pub shaker: ShakerSet,
    /// The decay check's threshold, percent (a warning, never a rejection).
    pub decay_warn_pct: f64,
}

/// A refusal by name: the reason, the field it points at and the words.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub reason: &'static str,
    pub field: String,
    pub message: String,
    /// The table row it points at, when it points at one.
    pub channel: Option<u32>,
}

impl Refusal {
    pub fn field(
        reason: &'static str,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Refusal {
            reason,
            field: field.into(),
            message: message.into(),
            channel: None,
        }
    }

    pub fn channel(reason: &'static str, channel: Option<u32>, message: impl Into<String>) -> Self {
        Refusal {
            reason,
            field: "channels".into(),
            message: message.into(),
            channel,
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.reason, self.message)
    }
}

fn pct_in(r: &mut Vec<Refusal>, field: &str, v: f64, lo: f64, hi: f64, what: &str) {
    if !(v.is_finite() && v >= lo && v <= hi) {
        r.push(Refusal::field(
            "out_of_range",
            field,
            format!("{what} is {v}; it is {lo} to {hi}."),
        ));
    }
}

impl ModalSetup {
    /// The setup with every default and the given mode, rate and block.
    pub fn new(mode: Mode, fs_hz: f64, block: usize) -> ModalSetup {
        ModalSetup {
            mode,
            fs_hz,
            block,
            band: (0.0, fs_hz / 2.56),
            estimator: Estimator::H1,
            averages: match mode {
                Mode::Impact => AVERAGES_IMPACT,
                Mode::Shaker => AVERAGES_SHAKER,
            },
            roving: None,
            impact: ImpactSet::default(),
            shaker: ShakerSet::default(),
            decay_warn_pct: DECAY_WARN_PCT_DEFAULT,
        }
    }

    /// The block's trigger sample: the pre-trigger length, frames.
    pub fn pretrigger_frames(&self) -> usize {
        ((self.impact.pretrigger_pct / 100.0) * self.block as f64).round() as usize
    }

    /// The line spacing, Hz.
    pub fn df_hz(&self) -> f64 {
        self.fs_hz / self.block as f64
    }

    /// The lines kept: every line from the band's bottom to its top, inclusive.
    pub fn lines(&self) -> std::ops::RangeInclusive<usize> {
        let df = self.df_hz();
        let lo = (self.band.0 / df - 1e-9).ceil().max(0.0) as usize;
        let hi = ((self.band.1 / df + 1e-9).floor() as usize).min(self.block / 2);
        lo..=hi
    }

    /// The frames of the start ramp, rounded up to whole blocks for a frame-synchronous
    /// source so averaging begins on a frame's start.
    pub fn ramp_frames(&self) -> u64 {
        let raw = (self.shaker.ramp_s * self.fs_hz).round().max(1.0) as u64;
        if self.shaker.source.frame_synchronous() {
            let b = self.block as u64;
            raw.div_ceil(b) * b
        } else {
            raw
        }
    }

    /// The value checks that need no table. Empty when the setup passes.
    pub fn refusals(&self) -> Vec<Refusal> {
        let mut r = Vec::new();
        if !(self.fs_hz.is_finite() && (RATE_MIN_HZ..=RATE_MAX_HZ).contains(&self.fs_hz)) {
            r.push(Refusal::field(
                "out_of_range",
                "fs_hz",
                format!(
                    "The rate is {} Hz; a station is asked for {RATE_MIN_HZ} to {RATE_MAX_HZ} Hz.",
                    self.fs_hz
                ),
            ));
        }
        if !(self.block.is_power_of_two() && (BLOCK_MIN..=BLOCK_MAX).contains(&self.block)) {
            r.push(Refusal::field(
                "block_invalid",
                "block",
                format!(
                    "The block is {} frames; it is a power of two from {BLOCK_MIN} to {BLOCK_MAX}.",
                    self.block
                ),
            ));
        }
        let (lo, hi) = self.band;
        let nyq = self.fs_hz / 2.0;
        if !(lo.is_finite() && hi.is_finite() && lo >= 0.0 && hi > lo && hi <= nyq) {
            r.push(Refusal::field(
                "band_invalid",
                "band",
                format!(
                    "The band is {lo} to {hi} Hz; its bottom is at least 0, its top above the \
                     bottom and at most half the rate ({nyq} Hz)."
                ),
            ));
        } else if self.block.is_power_of_two() && self.lines().count() < 2 {
            r.push(Refusal::field(
                "band_invalid",
                "band",
                "The band holds fewer than two lines at this rate and block.",
            ));
        }
        if self.averages == 0 || self.averages > AVERAGES_MAX {
            r.push(Refusal::field(
                "out_of_range",
                "averages",
                format!(
                    "Averages are {}; a measurement takes 1 to {AVERAGES_MAX}.",
                    self.averages
                ),
            ));
        }
        if let Some(rv) = &self.roving {
            if rv.channels.is_empty() {
                r.push(Refusal::field(
                    "roving_invalid",
                    "roving.channels",
                    "A roving plan names at least one channel that roves.",
                ));
            }
            if rv.points.is_empty() {
                r.push(Refusal::field(
                    "roving_invalid",
                    "roving.points",
                    "A roving plan names at least one point.",
                ));
            }
            if rv.channels.len() > 1 && rv.points.iter().any(|p| p.direction.is_some()) {
                r.push(Refusal::field(
                    "roving_invalid",
                    "roving.points",
                    "A point gives a direction only when exactly one channel roves; with \
                     several, each keeps its own direction.",
                ));
            }
            let mut seen = rv.channels.clone();
            seen.sort_unstable();
            seen.dedup();
            if seen.len() != rv.channels.len() {
                r.push(Refusal::field(
                    "roving_invalid",
                    "roving.channels",
                    "A channel is named twice in the roving plan.",
                ));
            }
            for (k, p) in rv.points.iter().enumerate() {
                if let Some(d) = &p.direction {
                    if !direction_ok(d) {
                        r.push(Refusal::field(
                            "roving_invalid",
                            format!("roving.points[{k}].direction"),
                            format!(
                                "\"{d}\" is not a direction; a direction is X, Y or Z with a sign, \
                                 as X+."
                            ),
                        ));
                    }
                }
            }
        }
        pct_in(
            &mut r,
            "decay_warn_pct",
            self.decay_warn_pct,
            DECAY_WARN_PCT_MIN,
            DECAY_WARN_PCT_MAX,
            "The decay warning (percent of the loudest window)",
        );
        match self.mode {
            Mode::Impact => self.impact_refusals(&mut r),
            Mode::Shaker => self.shaker_refusals(&mut r),
        }
        r
    }

    fn impact_refusals(&self, r: &mut Vec<Refusal>) {
        let i = &self.impact;
        pct_in(
            r,
            "impact.trigger_level_pct",
            i.trigger_pct,
            0.1,
            100.0,
            "The trigger level (percent of the hammer row's range)",
        );
        pct_in(
            r,
            "impact.pretrigger_pct",
            i.pretrigger_pct,
            0.0,
            50.0,
            "The pre-trigger (percent of the block)",
        );
        pct_in(
            r,
            "impact.holdoff_blocks",
            i.holdoff_blocks,
            0.0,
            10.0,
            "The hold-off (blocks)",
        );
        pct_in(
            r,
            "impact.double_hit_pct",
            i.double_hit_pct,
            1.0,
            50.0,
            "The double-hit fraction (percent of the main peak)",
        );
        pct_in(
            r,
            "impact.tip_warn_db",
            i.tip_warn_db,
            1.0,
            80.0,
            "The tip warning (dB)",
        );
        if i.force_window.on {
            pct_in(
                r,
                "impact.force_window.fall_pct",
                i.force_window.taper_pct,
                0.0,
                100.0,
                "The force window's fall (percent of its width)",
            );
            match crate::window::force_window_samples(&i.force_window, self.block) {
                None => r.push(Refusal::field(
                    "out_of_range",
                    "impact.force_window.width_pct",
                    format!(
                        "The force window's width is {} percent of the block; it must be a \
                         number above zero that is at least one sample.",
                        i.force_window.width_pct
                    ),
                )),
                Some((width, fall)) => {
                    let pre = self.pretrigger_frames();
                    if pre + width + fall > self.block {
                        r.push(Refusal::field(
                            "out_of_range",
                            "impact.force_window",
                            format!(
                                "The force window runs past the block: the trigger at sample \
                                 {pre}, {width} samples at one and a fall of {fall} end at \
                                 {}, after the block's {} samples.",
                                pre + width + fall,
                                self.block
                            ),
                        ));
                    }
                }
            }
        }
        if i.exp_window.on {
            pct_in(
                r,
                "impact.exponential_window.end_pct",
                i.exp_window.end_pct,
                0.01,
                99.0,
                "The exponential window's end value (percent)",
            );
        }
        if let Some(t) = &i.target {
            if !(t.peak.is_finite() && t.peak > 0.0) {
                r.push(Refusal::field(
                    "out_of_range",
                    "impact.target.peak",
                    format!("The target peak force is {}; it is above 0.", t.peak),
                ));
            }
            pct_in(
                r,
                "impact.target.tolerance_pct",
                t.tolerance_pct,
                1.0,
                100.0,
                "The target's tolerance (percent)",
            );
        }
    }

    fn shaker_refusals(&self, r: &mut Vec<Refusal>) {
        let s = &self.shaker;
        if s.drives.is_empty() {
            r.push(Refusal::field(
                "no_drive",
                "shaker.drives",
                "Shaker mode drives at least one drive row.",
            ));
        }
        if !(s.ceiling_v.is_finite() && s.ceiling_v > 0.0 && s.ceiling_v <= CEILING_MAX_V) {
            r.push(Refusal::field(
                "out_of_range",
                "shaker.ceiling_v",
                format!(
                    "The drive ceiling is {} V; it is above 0 and at most {CEILING_MAX_V} V.",
                    s.ceiling_v
                ),
            ));
        }
        for (k, d) in s.drives.iter().enumerate() {
            if !(d.level_vrms.is_finite() && d.level_vrms > 0.0) {
                r.push(Refusal::field(
                    "out_of_range",
                    format!("shaker.drives[{k}].level_vrms"),
                    format!(
                        "Drive row {}'s level is {} V RMS; it is above 0.",
                        d.row, d.level_vrms
                    ),
                ));
            } else if d.level_vrms * s.source.crest() > s.ceiling_v * (1.0 - CEILING_MARGIN) {
                r.push(Refusal::field(
                    "level_over_ceiling",
                    format!("shaker.drives[{k}].level_vrms"),
                    format!(
                        "Drive row {} at {} V RMS can reach {:.4} V with the {} source's crest of \
                         {:.3}, not inside the drive ceiling of {} V by its margin of {} % \
                         (at most {:.6} V). Lower the level, or raise the ceiling if the shaker \
                         and amplifier allow it.",
                        d.row,
                        d.level_vrms,
                        d.level_vrms * s.source.crest(),
                        s.source.name(),
                        s.source.crest(),
                        s.ceiling_v,
                        CEILING_MARGIN * 100.0,
                        s.ceiling_v * (1.0 - CEILING_MARGIN)
                    ),
                ));
            }
        }
        let mut rows: Vec<u32> = s.drives.iter().map(|d| d.row).collect();
        rows.sort_unstable();
        rows.dedup();
        if rows.len() != s.drives.len() {
            r.push(Refusal::field(
                "drive_repeated",
                "shaker.drives",
                "A drive row is named twice.",
            ));
        }
        if s.drives.len() > 1 && !s.source.several_drives() {
            r.push(Refusal::field(
                "source_refused",
                "shaker.source",
                format!(
                    "The {} source drives one shaker only: {}",
                    s.source.name(),
                    if s.source == SourceKind::PseudoRandom {
                        "it repeats one frame, so every average sees the same forces and the \
                         force matrix cannot be inverted."
                    } else {
                        "a chirp on several shakers is not offered in this version."
                    }
                ),
            ));
        }
        if s.drives.len() > 1 && self.averages < 2 * s.drives.len() as u32 {
            r.push(Refusal::field(
                "too_few_averages",
                "averages",
                format!(
                    "{} averages for {} references: a test with several shakers takes at least \
                     twice as many averages as references ({}).",
                    self.averages,
                    s.drives.len(),
                    2 * s.drives.len()
                ),
            ));
        }
        if s.drives.len() > 1 && self.estimator != Estimator::H1 {
            r.push(Refusal::field(
                "estimator_refused",
                "estimator",
                format!(
                    "{} is not offered with several references; matrix H1 is used.",
                    self.estimator.name()
                ),
            ));
        }
        if s.source.is_burst() {
            pct_in(
                r,
                "shaker.burst_pct",
                s.burst_pct,
                5.0,
                95.0,
                "The burst (percent of the frame)",
            );
        }
        pct_in(
            r,
            "shaker.ramp_s",
            s.ramp_s,
            0.0,
            RAMP_MAX_S,
            "The start ramp (seconds)",
        );
        if s.delay_frames > DELAY_FRAMES_MAX {
            r.push(Refusal::field(
                "out_of_range",
                "shaker.delay_frames",
                format!(
                    "Delay frames are {}; at most {DELAY_FRAMES_MAX}.",
                    s.delay_frames
                ),
            ));
        }
        if s.source == SourceKind::Random
            && !(s.overlap_pct == 0.0
                || (s.overlap_pct - 50.0).abs() < 1e-9
                || (s.overlap_pct - 75.0).abs() < 1e-9)
        {
            r.push(Refusal::field(
                "out_of_range",
                "shaker.overlap_pct",
                format!(
                    "The overlap is {} percent; random takes 0, 50 or 75.",
                    s.overlap_pct
                ),
            ));
        }
    }
}

/// Whether `d` spells a direction: X, Y or Z with an optional sign.
pub fn direction_ok(d: &str) -> bool {
    let b = d.as_bytes();
    matches!(b.first(), Some(b'X' | b'Y' | b'Z' | b'x' | b'y' | b'z'))
        && (b.len() == 1 || (b.len() == 2 && matches!(b[1], b'+' | b'-')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shaker(source: SourceKind, drives: usize) -> ModalSetup {
        let mut s = ModalSetup::new(Mode::Shaker, 2048.0, 2048);
        s.shaker.source = source;
        s.shaker.drives = (0..drives)
            .map(|k| DriveSet {
                row: 10 + k as u32,
                level_vrms: 0.05,
            })
            .collect();
        s
    }

    fn reasons(s: &ModalSetup) -> Vec<&'static str> {
        s.refusals().iter().map(|r| r.reason).collect()
    }

    #[test]
    fn modal_test_stop_ramp_is_the_shared_one() {
        assert_eq!(
            STOP_RAMP_DEPTH_DB,
            hertzforge_playback::plan::STOP_RAMP_DEPTH_DB
        );
        assert_eq!(STOP_RAMP_S, hertzforge_playback::plan::STOP_RAMP_S);
    }

    #[test]
    fn modal_test_defaults_pass() {
        assert!(ModalSetup::new(Mode::Impact, 2048.0, 2048)
            .refusals()
            .is_empty());
        assert!(shaker(SourceKind::BurstRandom, 2).refusals().is_empty());
    }

    #[test]
    fn modal_test_force_window_past_the_block_or_without_width_is_refused() {
        let mut s = ModalSetup::new(Mode::Impact, 1024.0, 1024);
        // Trigger at 51, width 923, fall 50: ends at exactly 1024, passes.
        s.impact.pretrigger_pct = 5.0;
        s.impact.force_window.width_pct = 923.0 / 10.24;
        s.impact.force_window.taper_pct = 100.0 * 50.0 / 923.0;
        assert!(s.refusals().is_empty(), "{:?}", s.refusals());
        // One sample more is refused, not clamped.
        s.impact.force_window.width_pct = 924.0 / 10.24;
        let r = s.refusals();
        assert_eq!(reasons(&s), vec!["out_of_range"]);
        assert_eq!(r[0].field, "impact.force_window");
        for w in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            s.impact.force_window.width_pct = w;
            assert_eq!(reasons(&s), vec!["out_of_range"], "{w}");
        }
        // Off, nothing is judged.
        s.impact.force_window.on = false;
        assert!(s.refusals().is_empty());
    }

    #[test]
    fn modal_test_level_over_ceiling_is_refused_by_the_bounded_crest() {
        let mut s = shaker(SourceKind::Random, 1);
        // 0.25 V RMS x crest 4 = 1.0 V: at the ceiling, not inside it by the margin.
        s.shaker.drives[0].level_vrms = 0.25;
        assert_eq!(reasons(&s), vec!["level_over_ceiling"]);
        // Inside by the margin passes; a hair past the margin does not.
        let edge = s.shaker.ceiling_v * (1.0 - CEILING_MARGIN) / RANDOM_CREST;
        s.shaker.drives[0].level_vrms = edge;
        assert!(s.refusals().is_empty(), "{:?}", s.refusals());
        s.shaker.drives[0].level_vrms = edge * (1.0 + 1e-9);
        assert_eq!(reasons(&s), vec!["level_over_ceiling"]);
        s.shaker.drives[0].level_vrms = 0.2501;
        assert_eq!(reasons(&s), vec!["level_over_ceiling"]);
        // A chirp's crest is a sine's.
        let mut c = shaker(SourceKind::PeriodicChirp, 1);
        c.shaker.drives[0].level_vrms = 0.70;
        assert!(c.refusals().is_empty());
        c.shaker.drives[0].level_vrms = 0.71;
        assert_eq!(reasons(&c), vec!["level_over_ceiling"]);
    }

    #[test]
    fn modal_test_sources_refused_for_several_shakers() {
        for k in [
            SourceKind::PseudoRandom,
            SourceKind::PeriodicChirp,
            SourceKind::BurstChirp,
        ] {
            assert_eq!(reasons(&shaker(k, 2)), vec!["source_refused"], "{k:?}");
            assert!(shaker(k, 1).refusals().is_empty(), "{k:?}");
        }
        for k in [
            SourceKind::Random,
            SourceKind::BurstRandom,
            SourceKind::PeriodicRandom,
        ] {
            assert!(shaker(k, 2).refusals().is_empty(), "{k:?}");
        }
    }

    #[test]
    fn modal_test_too_few_averages_for_the_references_is_refused() {
        let mut s = shaker(SourceKind::BurstRandom, 3);
        s.averages = 5;
        assert_eq!(reasons(&s), vec!["too_few_averages"]);
        s.averages = 6;
        assert!(s.refusals().is_empty());
    }

    #[test]
    fn modal_test_decay_warning_threshold_is_checked() {
        let mut s = shaker(SourceKind::BurstRandom, 2);
        assert_eq!(s.decay_warn_pct, DECAY_WARN_PCT_DEFAULT);
        for bad in [0.0, -1.0, 100.5, f64::NAN] {
            s.decay_warn_pct = bad;
            let r = s.refusals();
            assert_eq!(r.len(), 1, "{bad}");
            assert_eq!(r[0].field, "decay_warn_pct");
        }
        s.decay_warn_pct = 0.5;
        assert!(s.refusals().is_empty());
    }

    #[test]
    fn modal_test_ceiling_and_block_and_band_are_checked() {
        let mut s = shaker(SourceKind::BurstRandom, 1);
        s.shaker.ceiling_v = 10.5;
        assert!(reasons(&s).contains(&"out_of_range"));
        let mut b = ModalSetup::new(Mode::Impact, 2048.0, 3000);
        assert!(reasons(&b).contains(&"block_invalid"));
        b.block = 128;
        assert!(reasons(&b).contains(&"block_invalid"));
        let mut d = ModalSetup::new(Mode::Impact, 2048.0, 2048);
        d.band = (10.0, 1500.0);
        assert_eq!(reasons(&d), vec!["band_invalid"]);
    }

    #[test]
    fn modal_test_force_target_is_checked() {
        let mut s = ModalSetup::new(Mode::Impact, 2048.0, 2048);
        s.impact.target = Some(ForceTarget {
            peak: 0.0,
            tolerance_pct: 20.0,
        });
        assert_eq!(reasons(&s), vec!["out_of_range"]);
        s.impact.target = Some(ForceTarget {
            peak: 50.0,
            tolerance_pct: 0.5,
        });
        assert_eq!(reasons(&s), vec!["out_of_range"]);
        s.impact.target = Some(ForceTarget {
            peak: 50.0,
            tolerance_pct: 100.0,
        });
        assert!(s.refusals().is_empty());
    }

    #[test]
    fn modal_test_lines_cover_the_band() {
        let mut s = ModalSetup::new(Mode::Impact, 1024.0, 1024);
        s.band = (5.0, 200.0);
        assert_eq!(s.lines(), 5..=200);
        s.band = (0.0, 512.0);
        assert_eq!(s.lines(), 0..=512);
    }

    #[test]
    fn modal_test_roving_directions_need_one_channel() {
        let mut s = ModalSetup::new(Mode::Impact, 2048.0, 2048);
        s.roving = Some(Roving {
            channels: vec![1, 2],
            points: vec![Point {
                node: 3,
                direction: Some("Z+".into()),
            }],
        });
        assert_eq!(reasons(&s), vec!["roving_invalid"]);
    }
}
