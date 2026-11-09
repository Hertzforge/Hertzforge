//! What can go wrong loading or running a law.

use std::fmt;
use std::time::Duration;

/// An error from loading or running a law. Library paths return these; the
/// host never panics on a law's behavior.
///
/// After [`LawError::Trap`], [`LawError::Deadline`] or [`LawError::Memory`] the
/// instance is unusable and every later call returns [`LawError::Poisoned`].
/// The other errors leave the instance as it was.
#[derive(Debug, Clone, PartialEq)]
pub enum LawError {
    /// The bytes are not a WebAssembly component this host can compile.
    Compile(String),
    /// The component does not implement the world asked for (`random-single`
    /// or `random-mimo` of `hertzforge:control-law@0.1.0`), or imports
    /// something the host does not provide.
    NotALaw(String),
    /// The law's manifest breaks a rule (see the crate documentation).
    ManifestInvalid(String),
    /// A supplied parameter value does not fit its declaration.
    ParamInvalid {
        /// The parameter's key as supplied.
        key: String,
        /// Why it was refused.
        reason: String,
    },
    /// The setup handed to [`crate::LoadedLaw::instantiate`] is malformed.
    SetupInvalid(String),
    /// Loop inputs handed to a call are malformed (a caller error, not the
    /// law's).
    InputsInvalid(String),
    /// The law trapped: a panic, an out-of-bounds access, an `unreachable`, a
    /// stack overflow. Carries the trap and the tail of what the law wrote to
    /// stderr during the call.
    Trap(String),
    /// The call ran past its budget and was stopped.
    Deadline {
        /// The budget the call was given.
        budget: Duration,
    },
    /// The law asked for more memory than its cap and then failed.
    Memory {
        /// The cap, bytes.
        limit_bytes: usize,
        /// The total the refused request would have reached, bytes.
        requested_bytes: usize,
    },
    /// The law returned an error of its own.
    Law(String),
    /// The law returned a result that breaks the output rules: wrong length,
    /// a non-finite or negative drive, a non-finite level, a line off the grid,
    /// an undeclared or non-finite diagnostic.
    BadOutput {
        /// Which rule (the first broken, in the order the rules are listed).
        rule: OutputRule,
        /// What was wrong, in words.
        detail: String,
    },
    /// A multiple-output law returned a result that breaks the output rules:
    /// a drive of the wrong dimensions, a non-finite value, a matrix that is
    /// not Hermitian, a drive outside the band, or a diagnostic that is
    /// undeclared, repeated, of the wrong layout or not finite. Whether the
    /// drive is positive semi-definite is not checked here: the controller
    /// repairs or refuses it.
    BadMimoOutput {
        /// Which rule (the first broken, in the order the rules are listed).
        rule: MimoOutputRule,
        /// What was wrong, in words.
        detail: String,
    },
    /// A classical shock law returned a result that breaks the output rules:
    /// a drive of the wrong length, a value that is not a finite number, too
    /// many values, or a value the manifest does not declare (or reported
    /// twice). The change caps and every other protection of the drive belong
    /// to the controller.
    BadClassicalOutput {
        /// Which rule (the first broken, in the order the rules are listed).
        rule: ClassicalOutputRule,
        /// What was wrong, in words.
        detail: String,
    },
    /// An SRS shock law returned a result that breaks the output rules: not
    /// one gain per component, a value that is not a finite number, a gain at
    /// or under zero, too many values, or a value the manifest does not
    /// declare (or reported twice). The gain caps and every other protection
    /// of the drive belong to the controller.
    BadSrsOutput {
        /// Which rule (the first broken, in the order the rules are listed).
        rule: SrsOutputRule,
        /// What was wrong, in words.
        detail: String,
    },
    /// A Sine law returned a result that breaks the output rules: a drive
    /// that is not a finite number or is below zero, too many values, or a
    /// value the manifest does not declare (or reported twice). The change
    /// and rise caps and every other protection belong to the controller.
    BadSineOutput {
        /// Which rule (the first broken, in the order the rules are listed).
        rule: SineOutputRule,
        /// What was wrong, in words.
        detail: String,
    },
    /// A TWR law returned a result that breaks the output rules: a drive of
    /// the wrong length or holding a value that is not a finite number, too
    /// many values, or a value the manifest does not declare (or reported
    /// twice). The change caps, the predicted peak and every other protection
    /// belong to the controller.
    BadTwrOutput {
        /// Which rule (the first broken, in the order the rules are listed).
        rule: TwrOutputRule,
        /// What was wrong, in words.
        detail: String,
    },
    /// An earlier trap, deadline or memory error left this instance
    /// unusable. Carries that first error.
    Poisoned(String),
    /// The host itself failed (for example, it could not start its timer
    /// thread).
    Host(String),
}

/// The output rules a law's result is checked against, in the order they are
/// checked ([`LawError::BadOutput`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRule {
    /// The drive has one value per line.
    WrongLength,
    /// Every drive line is a finite number.
    NonFiniteDrive,
    /// No drive line is negative.
    NegativeDrive,
    /// The level command is a finite number.
    NonFiniteLevel,
    /// Every reported line is on the grid, and every named value is declared,
    /// reported once and finite.
    Diagnostics,
}

/// The output rules a multiple-output law's result is checked against, in the
/// order they are checked ([`LawError::BadMimoOutput`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MimoOutputRule {
    /// The drive has one packed `drives` by `drives` matrix per line of the
    /// grid.
    Dimensions,
    /// Every drive value is a finite number.
    NonFinite,
    /// Every line is Hermitian: `||G - G^H|| / ||G||` (Frobenius) at most
    /// [`crate::mimo::HERMITIAN_TOLERANCE`]. Packed, the only way to break
    /// this is an imaginary part on the diagonal.
    NotHermitian,
    /// Every line outside the band is zero.
    OutOfBand,
    /// Every diagnostic is declared, reported once, in its declared layout
    /// and length, and finite.
    Diagnostics,
}

/// The output rules a classical shock law's result is checked against, in the
/// order they are checked ([`LawError::BadClassicalOutput`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicalOutputRule {
    /// The drive has `record-len` samples.
    WrongLength,
    /// Every drive sample and every value is a finite number.
    NonFinite,
    /// At most [`crate::shock_classical::MAX_CLASSICAL_VALUES`] values.
    TooManyValues,
    /// Every value is declared by the manifest and reported once.
    UndeclaredValue,
}

impl ClassicalOutputRule {
    /// The stable wire and record name, the controller's own for the same
    /// rule: `wrong_length`, `non_finite`, `too_many_values`,
    /// `undeclared_value`.
    pub fn as_str(self) -> &'static str {
        match self {
            ClassicalOutputRule::WrongLength => "wrong_length",
            ClassicalOutputRule::NonFinite => "non_finite",
            ClassicalOutputRule::TooManyValues => "too_many_values",
            ClassicalOutputRule::UndeclaredValue => "undeclared_value",
        }
    }
}

/// The output rules an SRS shock law's result is checked against, in the
/// order they are checked ([`LawError::BadSrsOutput`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrsOutputRule {
    /// One gain per component.
    WrongLength,
    /// Every gain and every value is a finite number.
    NonFinite,
    /// Every gain is above zero.
    NotPositive,
    /// At most [`crate::shock_srs::MAX_SRS_VALUES`] values.
    TooManyValues,
    /// Every value is declared by the manifest and reported once.
    UndeclaredValue,
}

impl SrsOutputRule {
    /// The stable wire and record name, the controller's own for the same
    /// rule: `wrong_length`, `non_finite`, `not_positive`, `too_many_values`,
    /// `undeclared_value`.
    pub fn as_str(self) -> &'static str {
        match self {
            SrsOutputRule::WrongLength => "wrong_length",
            SrsOutputRule::NonFinite => "non_finite",
            SrsOutputRule::NotPositive => "not_positive",
            SrsOutputRule::TooManyValues => "too_many_values",
            SrsOutputRule::UndeclaredValue => "undeclared_value",
        }
    }
}

/// The output rules a Sine law's result is checked against, in the order
/// they are checked ([`LawError::BadSineOutput`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SineOutputRule {
    /// The drive and every value are finite numbers.
    NonFinite,
    /// The drive is at least zero.
    Negative,
    /// At most [`crate::sine::MAX_SINE_VALUES`] values.
    TooManyValues,
    /// Every value is declared by the manifest and reported once.
    UndeclaredValue,
}

impl SineOutputRule {
    /// The stable wire and record name, the controller's own for the same
    /// rule: `non_finite`, `negative`, `too_many_values`, `undeclared_value`.
    pub fn as_str(self) -> &'static str {
        match self {
            SineOutputRule::NonFinite => "non_finite",
            SineOutputRule::Negative => "negative",
            SineOutputRule::TooManyValues => "too_many_values",
            SineOutputRule::UndeclaredValue => "undeclared_value",
        }
    }
}

/// The output rules a TWR law's result is checked against, in the order they
/// are checked ([`LawError::BadTwrOutput`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TwrOutputRule {
    /// The drive is `record-len` samples.
    WrongLength,
    /// The drive and every value are finite numbers.
    NonFinite,
    /// At most [`crate::twr::MAX_TWR_VALUES`] values.
    TooManyValues,
    /// Every value is declared by the manifest and reported once.
    UndeclaredValue,
}

impl TwrOutputRule {
    /// The stable wire and record name, the controller's own for the same
    /// rule: `wrong_length`, `non_finite`, `too_many_values`,
    /// `undeclared_value`.
    pub fn as_str(self) -> &'static str {
        match self {
            TwrOutputRule::WrongLength => "wrong_length",
            TwrOutputRule::NonFinite => "non_finite",
            TwrOutputRule::TooManyValues => "too_many_values",
            TwrOutputRule::UndeclaredValue => "undeclared_value",
        }
    }
}

impl LawError {
    /// Whether this error leaves the instance unusable.
    pub fn poisons(&self) -> bool {
        matches!(
            self,
            LawError::Trap(_) | LawError::Deadline { .. } | LawError::Memory { .. }
        )
    }
}

impl fmt::Display for LawError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LawError::Compile(s) => write!(f, "the law file did not compile: {s}"),
            LawError::NotALaw(s) => write!(
                f,
                "the file is not a control law of the kind this host was asked to load: {s}"
            ),
            LawError::ManifestInvalid(s) => write!(f, "the law's manifest is invalid: {s}"),
            LawError::ParamInvalid { key, reason } => {
                write!(f, "parameter `{key}` is invalid: {reason}")
            }
            LawError::SetupInvalid(s) => write!(f, "the law setup is invalid: {s}"),
            LawError::InputsInvalid(s) => write!(f, "the loop inputs are invalid: {s}"),
            LawError::Trap(s) => write!(f, "the law trapped: {s}"),
            LawError::Deadline { budget } => write!(
                f,
                "the law ran past its {:.3} ms budget and was stopped",
                budget.as_secs_f64() * 1e3
            ),
            LawError::Memory {
                limit_bytes,
                requested_bytes,
            } => write!(
                f,
                "the law asked for {requested_bytes} bytes of memory, over its cap of \
                 {limit_bytes} bytes"
            ),
            LawError::Law(s) => write!(f, "the law returned an error: {s}"),
            LawError::BadOutput { detail, .. }
            | LawError::BadMimoOutput { detail, .. }
            | LawError::BadClassicalOutput { detail, .. }
            | LawError::BadSrsOutput { detail, .. }
            | LawError::BadSineOutput { detail, .. }
            | LawError::BadTwrOutput { detail, .. } => {
                write!(f, "the law's output was refused: {detail}")
            }
            LawError::Poisoned(s) => write!(
                f,
                "the law instance is unusable after an earlier failure: {s}"
            ),
            LawError::Host(s) => write!(f, "the law host failed: {s}"),
        }
    }
}

impl std::error::Error for LawError {}
