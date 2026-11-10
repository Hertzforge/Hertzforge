//! What the operator asked of a running Explorer, and what the run did with it.
//!
//! Every intent the service takes (a steer, a sweep, a hold, a stop) is applied between
//! tick batches and lands on the next generated frame, so it can be placed exactly in the
//! run's own time: the generated frame its change first appears in, and the input frames
//! the station had acquired when it was taken. [`IntentRecord`] carries both, with what
//! was asked and what the step adopted, and the service keeps the records in a bounded
//! ring ([`crate::service::ExplorerHostService::take_intents`]) that the run host hands
//! on. Nothing here is written to disk: the pump is the station's reading thread.

use crate::step::SweepDirection;

/// Why a controlled stop began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopCause {
    /// The operator asked for it.
    Operator,
    /// Nobody held control for the grace period.
    SafeStop,
    /// The service around the run is shutting down.
    Shutdown,
    /// The secondary limit monitor saw the derived motion pass its limit.
    Limit,
}

impl StopCause {
    /// The cause as one lower-case word.
    pub fn as_str(self) -> &'static str {
        match self {
            StopCause::Operator => "operator",
            StopCause::SafeStop => "safe_stop",
            StopCause::Shutdown => "shutdown",
            StopCause::Limit => "limit",
        }
    }
}

/// A frequency to steer to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrequencyPart {
    /// An absolute frequency, Hz.
    Hz(f64),
    /// Hz added to the target frequency in force.
    StepHz(f64),
    /// Octaves from the target frequency in force: the target times `2^oct`.
    StepOct(f64),
}

/// A level to steer to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LevelPart {
    /// An absolute level, volts peak.
    Vpk(f64),
    /// dB from the target level in force. Refused while that target is 0 V, where no
    /// ratio means anything.
    StepDb(f64),
}

/// One value an intent asked for and the value the step adopted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adoption {
    /// What was asked, in absolute terms (a step resolved against the target in force).
    pub asked: f64,
    /// What the step adopted after its clamp.
    pub adopted: f64,
    /// Whether the clamp moved it.
    pub clamped: bool,
}

impl Adoption {
    pub(crate) fn of(asked: f64, adopted: f64) -> Adoption {
        Adoption {
            asked,
            adopted,
            // A non-finite ask adopted as a finite edge is clamped; `!=` says so for NaN.
            clamped: asked.to_bits() != adopted.to_bits(),
        }
    }
}

/// What an intent did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IntentKind {
    /// A frequency part, a level part, or both, landing in the same frame.
    Steer {
        frequency: Option<Adoption>,
        level: Option<Adoption>,
    },
    /// A sweep started or re-aimed toward a band edge.
    Sweep {
        direction: SweepDirection,
        rate_oct_per_min: Adoption,
    },
    /// The sweep ended; the tone holds at `hz` (the target in force after the hold).
    Hold { hz: f64 },
    /// A controlled stop began.
    Stop { cause: StopCause },
}

/// One intent the step took, placed in the run's own time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntentRecord {
    /// Counts every intent the run took, from 0, including any the bounded ring dropped.
    pub seq: u64,
    /// The first generated frame made under the change (the frames generated before it).
    /// A level moves that frame's own sample. A retune is phase-continuous: that frame is
    /// the first at the new frequency and its sample continues the old phase, so the
    /// waveform's value first differs one frame later.
    pub frame: u64,
    /// The input frames the station had acquired when the intent was taken.
    pub input_frame: u64,
    pub kind: IntentKind,
}

/// The answer to a steer: the targets in force after it, which parts were clamped, and
/// where in the run it lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Steered {
    /// The target frequency in force after the steer, Hz.
    pub hz: f64,
    /// The target level in force after the steer, volts peak.
    pub vpk: f64,
    /// The frequency part was clamped (false when none was sent).
    pub clamped_hz: bool,
    /// The level part was clamped (false when none was sent).
    pub clamped_vpk: bool,
    pub frame: u64,
    pub input_frame: u64,
}
