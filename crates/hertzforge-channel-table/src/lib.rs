//! `hertzforge-channel-table` - the universal channel table.
//!
//! One table, filled once, used by every module. The schema is a superset of a typical
//! commercial controller's channel-config dialog: per
//! channel identity, interface, sensitivity, range, role, and tri-field abort limits.
//! The table holds channel IDENTITY, not the test
//! definition (that scope line is deliberate).
//!
//! A row holds what the controller runs the channel at and, beside it, the
//! columns the analysis side reads (node, direction, the sensor's identity and
//! the rest, [`AnalysisColumns`]), so one table reaches a run and comes back from
//! it with nothing dropped. The control service keeps the analysis columns as the
//! table's identity and never runs anything from them.
//!
//! JSON is this crate's one serialized form. The table's files (`.hforge`, CSV,
//! tab-delimited text, Excel) are written and read by the analysis side, under
//! its own headings, from the protocol's rows; this crate keeps no second
//! spelling of them.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub mod uff;

/// The schema this build reads and writes.
///
/// Bumped to 2 by the schema change that added
/// [`Channel::physical`], [`Channel::analysis_tier`] and this field itself. A
/// table that carries no version at all is a v1 table (the field did not exist
/// then) and is upgraded by [`ChannelTable::migrate_to_current`].
pub const SCHEMA_VERSION: u32 = 2;

/// The version assumed for a table that does not state one. Deliberately a named
/// constant rather than a bare `1` at the serde default: the "absent means v1"
/// rule is the whole migration contract and it should be greppable.
pub const SCHEMA_VERSION_UNVERSIONED: u32 = 1;

fn unversioned() -> u32 {
    SCHEMA_VERSION_UNVERSIONED
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interface {
    SingleEnded,
    Iepe,
}

/// Which physical front-end channel a table row is bound to.
///
/// The two halves are stored SEPARATELY rather than as one resolved string
/// because `hertzforge_hal::discovery` makes a backend's channel id opaque: only the
/// backend that produced an id may parse it. A consumer that needs to know
/// "which device is this row on" must be able to ask without splitting anything,
/// which is exactly what `device_id` is for.
///
/// `None` on a row means UNBOUND, and unbound is a real state, not a defect: the
/// Hertzforge-Sim station has no physical channels to bind to, and a table authored
/// before a station was selected is legitimately unbound everywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalChannelRef {
    /// Backend-opaque device identity (`DiscoveredDevice::id`).
    pub device_id: String,
    /// Backend-opaque channel identity (`DiscoveredChannel::id`) WITHOUT the
    /// device prefix - the trailing segment the backend's own label uses.
    pub channel: String,
}

impl PhysicalChannelRef {
    pub fn new(device_id: impl Into<String>, channel: impl Into<String>) -> Self {
        PhysicalChannelRef {
            device_id: device_id.into(),
            channel: channel.into(),
        }
    }

    /// The RESOLVED physical name, the form a backend hands back and the form
    /// the grid renders on the row: `<device_id>/<channel>`.
    pub fn resolved(&self) -> String {
        format!("{}/{}", self.device_id, self.channel)
    }

    /// Split a resolved physical name at its FIRST separator.
    ///
    /// A flat form (a spreadsheet cell, `"<device>/<channel>"`) has one cell per
    /// field, so a two-field reference survives there as one string. JSON and
    /// the protocol keep the two halves apart.
    ///
    /// A cell with no separator is a PARSE ERROR rather than a guess. Guessing
    /// (whole string as the device, or as the channel) would silently produce a
    /// binding that names a device that does not exist.
    pub fn parse_resolved(s: &str) -> Result<Self, TableError> {
        match s.split_once('/') {
            Some((dev, chan)) if !dev.is_empty() && !chan.is_empty() => {
                Ok(PhysicalChannelRef::new(dev, chan))
            }
            _ => Err(TableError::Parse(format!(
                "bad physical channel '{s}': expected <device>/<channel>"
            ))),
        }
    }
}

/// How much analysis a row gets per block.
///
/// The FIELD ships with the v2 schema bump; the POLICY that assigns anything
/// other than [`AnalysisTier::Full`] is later work. It exists now so a 300-400
/// channel table can be authored and round-tripped before the DSP policy that
/// consumes it lands - the alternative is a second schema bump later, which is
/// exactly what "one schema-bump owner" was decided to avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AnalysisTier {
    /// Every block, full rate: spectra, limits, everything.
    #[default]
    Full,
    /// Analyzed at 1/N of the block cadence.
    Decimated(u32),
    /// Analyzed in a round-robin slice of a wide channel set.
    Rotating,
}

impl AnalysisTier {
    /// The protocol's spelling: `full`, `rotating`, or `decimated:N`.
    pub fn name(self) -> String {
        match self {
            AnalysisTier::Full => "full".into(),
            AnalysisTier::Decimated(n) => format!("decimated:{n}"),
            AnalysisTier::Rotating => "rotating".into(),
        }
    }

    /// Read a tier from its spelling. Blank is `Full`. The short forms `dec:N`
    /// and `rot` are read too. An unrecognized spelling is refused rather than
    /// read as `Full`: an intended `decimated:8` mistyped and silently promoted to
    /// full analysis is a 400-channel budget blown with nothing said.
    pub fn from_name(s: &str) -> Result<AnalysisTier, TableError> {
        let t = s.trim().to_ascii_lowercase();
        if t.is_empty() || t == "full" || t == "f" {
            return Ok(AnalysisTier::Full);
        }
        if t == "rot" || t == "rotating" {
            return Ok(AnalysisTier::Rotating);
        }
        if let Some(n) = t
            .strip_prefix("dec:")
            .or_else(|| t.strip_prefix("decimated:"))
        {
            let n: u32 = n
                .trim()
                .parse()
                .map_err(|_| TableError::Parse(format!("bad decimation factor '{s}'")))?;
            if n == 0 {
                return Err(TableError::Parse(
                    "decimation factor 0 is not a cadence".into(),
                ));
            }
            return Ok(AnalysisTier::Decimated(n));
        }
        Err(TableError::Parse(format!(
            "bad analysis tier '{s}': expected full, decimated:N or rotating"
        )))
    }
}

/// Why a row's [`AnalysisTier`] came out the way it did.
///
/// Returned alongside the tier by [`derived_analysis_tier`] so the derivation is
/// auditable rather than a bare value: today every basis leads to `Full`, and
/// the reason they do is different per basis. When the decimation policy lands, only the
/// [`TierBasis::NoPolicyYet`] arm changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierBasis {
    /// A control row closes the loop; it is never analyzed at reduced cadence.
    ControlRole,
    /// A drive row is the output; its record is the drive itself.
    DriveRole,
    /// A row with an armed abort limit must be evaluated every block.
    LimitArmed,
    /// A plain monitor row with no armed limit. `Full` today because no
    /// decimation policy exists yet, NOT because the row needs full analysis.
    NoPolicyYet,
}

/// The tier a row would get from its own contents, with the reason.
///
/// Pure and total: it reads the row and answers. Nothing calls it to OVERWRITE
/// an operator's explicit choice - it is the default a new/migrated row starts
/// from and the value the decimation policy will compare against.
pub fn derived_analysis_tier(ch: &Channel) -> (AnalysisTier, TierBasis) {
    let basis = match ch.role {
        ChannelRole::Control => TierBasis::ControlRole,
        ChannelRole::Drive => TierBasis::DriveRole,
        _ if ch.has_armed_limit() => TierBasis::LimitArmed,
        _ => TierBasis::NoPolicyYet,
    };
    // Every basis resolves to Full in this build. The three named bases are
    // REQUIRED to be Full; `NoPolicyYet` is Full because there is no policy, and
    // that is the arm the decimation policy replaces.
    (AnalysisTier::Full, basis)
}

/// A row's engineering unit. The five named variants are the units the
/// controller has always named; any other unit is [`Units::Custom`], kept by its
/// name exactly as written. Any unit the analysis side reads can be held, and the
/// names in [`UNIT_VOCABULARY`] are the ones every part of the program knows the
/// quantity of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Units {
    AccelG,
    VelocityIps,
    DisplacementMil,
    ForceLbf,
    VoltageV,
    Custom(String),
}

/// The channel table's unit vocabulary: the names every part of the program reads
/// as the same quantity, grouped as the analysis side offers them for each channel
/// type (acceleration, velocity, displacement, force, pressure, voltage, strain,
/// temperature). A unit outside it is still held and still runs; the control
/// service says so with a warning, and the analysis side reads it with its own
/// unit reader or refuses it when that cannot.
pub const UNIT_VOCABULARY: [&str; 29] = [
    "g",
    "in/s**2",
    "m/s**2",
    "mm/s**2",
    "in/s",
    "m/s",
    "mm/s",
    "mil",
    "in",
    "m",
    "mm",
    "lbf",
    "N",
    "kN",
    "psi",
    "Pa",
    "kPa",
    "MPa",
    "V",
    "mV",
    "strain",
    "microstrain",
    "m/m",
    "degC",
    "degF",
    "K",
    "degR",
    "in/s^2",
    "m/s^2",
];

impl Units {
    /// The unit's name as the protocol writes it.
    pub fn name(&self) -> String {
        match self {
            Units::AccelG => "g".into(),
            Units::VelocityIps => "in/s".into(),
            Units::DisplacementMil => "mil".into(),
            Units::ForceLbf => "lbf".into(),
            Units::VoltageV => "V".into(),
            Units::Custom(s) => s.clone(),
        }
    }

    /// The unit a name gives. Only the five names the controller has always used
    /// map to their variants; every other name, other spellings of those five
    /// included, is kept as written. Exact on purpose: a name read here reaches a
    /// running test, and folding `G` onto `g` would change what an existing table
    /// runs with.
    pub fn from_name(s: &str) -> Units {
        match s {
            "g" => Units::AccelG,
            "in/s" => Units::VelocityIps,
            "mil" => Units::DisplacementMil,
            "lbf" => Units::ForceLbf,
            "V" => Units::VoltageV,
            other => Units::Custom(other.to_owned()),
        }
    }

    /// True when the name is in [`UNIT_VOCABULARY`].
    pub fn is_known(&self) -> bool {
        UNIT_VOCABULARY.contains(&self.name().as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Polarity {
    Positive,
    Negative,
}

/// Where a channel sits in a control topology. `Control` channels close the loop
/// (averaged / extremal / limiting), `Monitor` is recorded only, `Drive` is an output,
/// `Reference` is a MIMO reference input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelRole {
    Control,
    Monitor,
    Drive,
    Reference,
}

/// The columns the analysis side reads, carried by the control service as the
/// row's identity: where the channel is (node and direction), what it measures,
/// the sensor and its calibration, and any other column by name. Every member is
/// optional, and nothing the controller runs is read from them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnalysisColumns {
    /// The geometry node the channel is at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<u32>,
    /// The direction it measures along, as the analysis side spells it (`X+`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub direction: String,
    /// The quantity it measures (`acceleration`, `force`, ...).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel_type: String,
    /// What the channel is to a frequency response: `reference`, `response`
    /// or `monitor`. Apart from the loop's role, because a controlled force can
    /// be a response's reference.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
    /// The input's range, volts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daq_range_v: Option<f64>,
    /// The sensor's own full-scale output, volts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor_range_v: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub serial_number: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub make: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Which leg of a triaxial sensor (`X`, `Y` or `Z`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub triax_dof: String,
    /// When the sensor's calibration runs out, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expiration: String,
    /// Any other column, by its name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

impl AnalysisColumns {
    /// True when no member holds anything.
    pub fn is_blank(&self) -> bool {
        self == &AnalysisColumns::default()
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Tri-field per-channel abort limits: acceleration (g), velocity (in/s),
/// displacement (mil). `None` means no limit on that axis. Mirrors the SMART dialog's
/// g / in-s / mil trio.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct AbortLimits {
    pub accel_g: Option<f64>,
    pub velocity_ips: Option<f64>,
    pub displacement_mil: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub index: u32,
    pub enabled: bool,
    pub interface: Interface,
    /// IEPE bias volts; `None` when interface is `SingleEnded`.
    pub iepe_bias_v: Option<f64>,
    pub units: Units,
    /// STRICTLY per-channel. Never a global. `eu = volts / (sensitivity_mv_per_eu/1000)`.
    pub sensitivity_mv_per_eu: f64,
    /// Full-scale range in the channel's engineering unit.
    pub range_max: f64,
    pub polarity: Polarity,
    pub description: String,
    pub role: ChannelRole,
    /// Control-group id for multi-group averaging control. `None` = default group.
    pub control_group: Option<u32>,
    pub abort: AbortLimits,
    /// Which physical front-end channel this row is bound to. `None` = unbound
    /// (schema v2). Defaulted on load so a v1 table parses unchanged.
    #[serde(default)]
    pub physical: Option<PhysicalChannelRef>,
    /// How much analysis this row gets per block (schema v2). See
    /// [`derived_analysis_tier`] for the default derivation; the POLICY that
    /// assigns anything but `Full` does not exist yet.
    #[serde(default)]
    pub analysis_tier: AnalysisTier,
    /// Marks the row as a limit channel: an input a test's limits are set on.
    /// A mark only. The limits themselves stay in each test's setup, and no
    /// module reads this flag.
    #[serde(default, skip_serializing_if = "is_false")]
    pub limit: bool,
    /// The analysis side's columns, carried with the row and never run from.
    #[serde(default, skip_serializing_if = "AnalysisColumns::is_blank")]
    pub analysis: AnalysisColumns,
}

impl Channel {
    pub fn new(index: u32) -> Self {
        Channel {
            index,
            enabled: true,
            interface: Interface::SingleEnded,
            iepe_bias_v: None,
            units: Units::AccelG,
            sensitivity_mv_per_eu: 100.0,
            range_max: 100.0,
            polarity: Polarity::Positive,
            description: String::new(),
            role: ChannelRole::Monitor,
            control_group: None,
            abort: AbortLimits::default(),
            physical: None,
            analysis_tier: AnalysisTier::Full,
            limit: false,
            analysis: AnalysisColumns::default(),
        }
    }

    /// Sensitivity expressed as EU per volt (the reciprocal used in conversion).
    pub fn eu_per_volt(&self) -> f64 {
        1000.0 / self.sensitivity_mv_per_eu
    }

    /// True when any of the three abort axes carries a limit.
    pub fn has_armed_limit(&self) -> bool {
        self.abort.accel_g.is_some()
            || self.abort.velocity_ips.is_some()
            || self.abort.displacement_mil.is_some()
    }

    /// The resolved physical name for display, or empty when unbound.
    pub fn physical_name(&self) -> String {
        self.physical
            .as_ref()
            .map(|p| p.resolved())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelTable {
    /// The schema this table's contents follow. Absent in serialized form means
    /// [`SCHEMA_VERSION_UNVERSIONED`] - see [`ChannelTable::migrate_to_current`].
    #[serde(default = "unversioned")]
    pub schema_version: u32,
    pub channels: Vec<Channel>,
}

impl Default for ChannelTable {
    /// A NEW table is authored at the current schema. Only a table that came
    /// from somewhere else can be at an older one.
    fn default() -> Self {
        ChannelTable {
            schema_version: SCHEMA_VERSION,
            channels: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableError {
    EmptyEnabled,
    ZeroSensitivity(u32),
    /// An enabled row (of any role) whose sensitivity is below zero, with the value. A
    /// channel read inverted says so in its own `polarity` column, never in the sign of its
    /// sensitivity, so the sign is refused rather than folded into the conversion.
    NegativeSensitivity {
        index: u32,
        value: f64,
    },
    NonPositiveRange(u32),
    DuplicateIndex(u32),
    Parse(String),
    /// `control_group` ids used across the table are not the contiguous run
    /// `0..=max` (a gap in the group numbering, e.g. groups {0, 2} with no 1).
    /// Carries the first missing id.
    NonContiguousControlGroup(u32),
    /// Reference-role channels are present but no Control-role channel exists to
    /// close the MIMO loop against them.
    ReferenceWithoutControl,
    /// The table states a schema NEWER than this build reads. Refused rather
    /// than parsed on a best-effort basis: a table from a future build may bind
    /// channels this build would silently drop, and a dropped binding is a run
    /// on the wrong physical channel.
    UnsupportedSchemaVersion {
        found: u32,
        supported: u32,
    },
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableError::EmptyEnabled => write!(f, "no channels are enabled"),
            TableError::ZeroSensitivity(i) => write!(f, "channel {i} has zero sensitivity"),
            TableError::NegativeSensitivity { index, value } => write!(
                f,
                "channel {index}'s sensitivity_mv_per_eu is {value}, which is negative; a \
                 sensitivity is never negative on an enabled row, and a channel read inverted \
                 says so in its polarity column"
            ),
            TableError::NonPositiveRange(i) => write!(f, "channel {i} has non-positive range"),
            TableError::DuplicateIndex(i) => write!(f, "duplicate channel index {i}"),
            TableError::Parse(s) => write!(f, "parse error: {s}"),
            TableError::NonContiguousControlGroup(missing) => {
                write!(
                    f,
                    "control-group ids are not contiguous (missing group {missing})"
                )
            }
            TableError::ReferenceWithoutControl => write!(
                f,
                "reference channels exist but no control channel closes the loop"
            ),
            TableError::UnsupportedSchemaVersion { found, supported } => write!(
                f,
                "channel table states schema v{found}; this build reads up to v{supported}. \
                 A newer table may bind physical channels this build does not understand, so \
                 it is refused rather than partly loaded."
            ),
        }
    }
}

impl std::error::Error for TableError {}

/// Non-fatal validation findings. These do not block a table from being used but
/// flag configurations that are almost certainly operator mistakes. Returned by
/// [`ChannelTable::validate_report`] alongside the hard errors.
///
/// The IEPE bias/interface mismatch is intentionally a warning, not a hard error:
/// the SMART dialog leaves IEPE-bias blank in single-ended mode (`N/A`), and a
/// mid-edit table may briefly carry an inconsistent pair. The operator, not the
/// parser, decides whether that is a problem.
#[derive(Debug, Clone, PartialEq)]
pub enum TableWarning {
    /// Interface is IEPE but no bias voltage is set on this channel.
    IepeMissingBias(u32),
    /// A bias voltage is set but the interface is single-ended (bias is ignored).
    BiasOnSingleEnded(u32),
    /// Reference channels exist but no Control channel is present. MIMO control
    /// needs at least one control channel to close the loop against the references.
    ReferenceWithoutControl,
    /// An enabled channel is a Drive output. Informational: drive channels are
    /// outputs, not sensor inputs, so their sensitivity/range fields are advisory.
    DriveChannelPresent(u32),
    /// The row's unit is not in [`UNIT_VOCABULARY`]. The row runs in it as
    /// written; its quantity is unknown to the rest of the program.
    UnitUnknown(u32),
    /// An enabled drive row is not in volts: a drive is written to the device in
    /// volts, whatever the row says.
    DriveNotInVolts(u32),
    /// A row with an abort limit is not in g: the kinematic limits read the
    /// row's samples as acceleration in g.
    AbortNotInG(u32),
}

impl fmt::Display for TableWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableWarning::IepeMissingBias(i) => {
                write!(f, "channel {i} is IEPE but has no bias voltage set")
            }
            TableWarning::BiasOnSingleEnded(i) => {
                write!(f, "channel {i} is single-ended but has an IEPE bias set (ignored)")
            }
            TableWarning::ReferenceWithoutControl => write!(
                f,
                "reference channels are present but no control channel closes the loop"
            ),
            TableWarning::DriveChannelPresent(i) => {
                write!(f, "channel {i} is a drive output (sensitivity/range are advisory)")
            }
            TableWarning::UnitUnknown(i) => write!(
                f,
                "channel {i}'s unit is not one the program knows the quantity of; it runs in it as written"
            ),
            TableWarning::DriveNotInVolts(i) => write!(
                f,
                "channel {i} is a drive whose unit is not V; a drive is written in volts"
            ),
            TableWarning::AbortNotInG(i) => write!(
                f,
                "channel {i} has an abort limit and its unit is not g; the abort limits read it as g"
            ),
        }
    }
}

/// The full result of [`ChannelTable::validate_report`]: hard errors that make the
/// table unusable, plus non-fatal warnings worth surfacing to the operator. A table
/// is safe to use when `errors` is empty; `warnings` may still be non-empty.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidationReport {
    pub errors: Vec<TableError>,
    pub warnings: Vec<TableWarning>,
}

impl ValidationReport {
    /// True when there are no hard errors (warnings are allowed).
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

impl ChannelTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upgrade a table parsed from an older schema to [`SCHEMA_VERSION`].
    ///
    /// This is the ONE place a version is interpreted, and the load path calls
    /// it: [`ChannelTable::from_json`]. A table that reached a module without
    /// passing through here is a table nobody checked the version of.
    ///
    /// v1 -> v2 needs no content rewrite: [`Channel::physical`] defaults to
    /// `None` (unbound is the honest state for a table authored before physical
    /// binding existed - inventing a binding here would bind rows to devices the
    /// author never saw) and [`Channel::analysis_tier`] defaults to
    /// [`AnalysisTier::Full`], which is what every v1 row effectively was.
    ///
    /// A version NEWER than this build is refused
    /// ([`TableError::UnsupportedSchemaVersion`]), never truncated.
    pub fn migrate_to_current(&mut self) -> Result<(), TableError> {
        if self.schema_version > SCHEMA_VERSION {
            return Err(TableError::UnsupportedSchemaVersion {
                found: self.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        // v0 (a hand-written table that stated 0) and v1 land in the same place:
        // the v2 fields are already at their serde/pad defaults by the time the
        // table gets here, so the upgrade is the version stamp itself.
        self.schema_version = SCHEMA_VERSION;
        Ok(())
    }

    /// True when this table predates the v2 schema and would be upgraded by
    /// [`ChannelTable::migrate_to_current`]. Display-only.
    pub fn needs_migration(&self) -> bool {
        self.schema_version < SCHEMA_VERSION
    }

    /// Hard structural invariants. Returns the first violating error, preserving the
    /// original short-circuit contract callers depend on.
    ///
    /// The full invariant set (both hard errors and non-fatal warnings) is available
    /// via [`ChannelTable::validate_report`]; this method is the hard-error gate.
    ///
    /// Hard errors, in check order:
    /// 1. Channel indices are unique.
    /// 2. At least one channel is enabled.
    /// 3. Every enabled channel has nonzero sensitivity (the EU conversion divides by
    ///    it) and a strictly positive range; no enabled channel, a drive included, has a
    ///    negative sensitivity (polarity is the row's own column).
    /// 4. `control_group` ids form the contiguous run `0..=max` with no gaps.
    /// 5. If any Reference-role channel exists, at least one Control-role channel
    ///    exists to close the MIMO loop (coherence).
    pub fn validate(&self) -> Result<(), TableError> {
        let report = self.validate_report();
        match report.errors.into_iter().next() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Full validation: collects ALL hard errors and ALL non-fatal warnings in a
    /// single pass. Use this for a table editor that wants to surface every problem
    /// at once; use [`ChannelTable::validate`] for the go/no-go gate.
    ///
    /// Warnings (non-fatal): IEPE/bias interface mismatch per channel, the
    /// presence of drive-output channels (informational), a unit outside
    /// [`UNIT_VOCABULARY`], a drive not in volts, and an abort limit on a row not
    /// in g. Warnings never block use.
    pub fn validate_report(&self) -> ValidationReport {
        let mut report = ValidationReport::default();

        // --- pass 1: per-channel + index uniqueness ---
        let mut seen = std::collections::BTreeSet::new();
        let mut any_enabled = false;
        let mut n_control = 0u32;
        let mut n_reference_role = 0u32;
        for ch in &self.channels {
            if !seen.insert(ch.index) {
                report.errors.push(TableError::DuplicateIndex(ch.index));
            }
            match ch.role {
                ChannelRole::Control => n_control += 1,
                ChannelRole::Reference => n_reference_role += 1,
                ChannelRole::Drive => {
                    if ch.enabled {
                        report
                            .warnings
                            .push(TableWarning::DriveChannelPresent(ch.index));
                    }
                }
                ChannelRole::Monitor => {}
            }
            if ch.enabled {
                any_enabled = true;
                if ch.sensitivity_mv_per_eu < 0.0 {
                    report.errors.push(TableError::NegativeSensitivity {
                        index: ch.index,
                        value: ch.sensitivity_mv_per_eu,
                    });
                }
                // Sensitivity is meaningful only for input channels; a drive output
                // carries no sensor sensitivity. Gate the hard check to non-drive rows.
                if ch.role != ChannelRole::Drive {
                    if ch.sensitivity_mv_per_eu == 0.0 {
                        report.errors.push(TableError::ZeroSensitivity(ch.index));
                    }
                    if ch.range_max <= 0.0 {
                        report.errors.push(TableError::NonPositiveRange(ch.index));
                    }
                }
            }
            if !ch.units.is_known() {
                report.warnings.push(TableWarning::UnitUnknown(ch.index));
            }
            if ch.enabled && ch.role == ChannelRole::Drive && ch.units != Units::VoltageV {
                report
                    .warnings
                    .push(TableWarning::DriveNotInVolts(ch.index));
            }
            if ch.has_armed_limit() && ch.units != Units::AccelG {
                report.warnings.push(TableWarning::AbortNotInG(ch.index));
            }
            // IEPE bias/interface coherence (a warning, not a hard error).
            match (ch.interface, ch.iepe_bias_v) {
                (Interface::Iepe, None) => {
                    report
                        .warnings
                        .push(TableWarning::IepeMissingBias(ch.index));
                }
                (Interface::SingleEnded, Some(_)) => {
                    report
                        .warnings
                        .push(TableWarning::BiasOnSingleEnded(ch.index));
                }
                _ => {}
            }
        }
        if !any_enabled {
            report.errors.push(TableError::EmptyEnabled);
        }

        // --- control-group contiguity: the set of Some(group) ids must be 0..=max ---
        let groups: std::collections::BTreeSet<u32> = self
            .channels
            .iter()
            .filter_map(|c| c.control_group)
            .collect();
        if let Some(&max) = groups.iter().next_back() {
            for expected in 0..=max {
                if !groups.contains(&expected) {
                    report
                        .errors
                        .push(TableError::NonContiguousControlGroup(expected));
                    break;
                }
            }
        }

        // --- MIMO coherence: references require a control channel to close on ---
        if n_reference_role > 0 && n_control == 0 {
            report.errors.push(TableError::ReferenceWithoutControl);
        }

        report
    }

    // --- JSON (serde, pretty) ---

    /// Serialize the whole table to pretty JSON, every member of every row.
    pub fn to_json(&self) -> Result<String, TableError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| TableError::Parse(format!("json serialize: {e}")))
    }

    /// Parse a table from JSON produced by [`ChannelTable::to_json`] (or any
    /// compatible source), migrating an older schema forward. Does NOT validate;
    /// call [`ChannelTable::validate`] after.
    pub fn from_json(s: &str) -> Result<Self, TableError> {
        let mut t: ChannelTable =
            serde_json::from_str(s).map_err(|e| TableError::Parse(format!("json parse: {e}")))?;
        t.migrate_to_current()?;
        Ok(t)
    }

    /// Neutral default table for `n` channels: indices 1..=n with the
    /// `Channel::new` defaults, CH1 as Control and the rest Monitor. Carries no
    /// hardware/vendor assumption; pick `n` from the backend's reported channel
    /// count. `n = 0` yields an empty table (invalid on `validate`, same as
    /// `ChannelTable::new`).
    pub fn default_with_channels(n: usize) -> Self {
        let mut t = ChannelTable::new();
        for i in 0..n {
            let mut c = Channel::new(i as u32 + 1);
            c.role = if i == 0 {
                ChannelRole::Control
            } else {
                ChannelRole::Monitor
            };
            // Default rows must derive a voltage range that is LEGAL ON ANY
            // DEVICE: 100 mV/EU x 10 EU = +/-1 V, inside every supported input
            // range (a below-minimum request coerces UP harmlessly). The
            // Channel::new default (100 EU -> +/-10 V) was refused by the
            // +/-5 V NI-9234 three separate times on 2026-07-25 (-200077:
            // the engine harness, the app's default rows, the daemon's seed
            // table) - same class as the factory placeholder fix.
            c.range_max = 10.0;
            t.channels.push(c);
        }
        t
    }

    /// Seed mirroring a captured delegated-controller dialog: CH1 = 1000 mV/g "Airbags",
    /// CH2/3 = 10.25/10.27 mV/g "Trunions"/"Vertical", CH4..8 = 10.0 mV/g default.
    /// The per-channel non-uniform sensitivity is deliberate and load-bearing
    /// (delegated truth-block tests encode these exact values), so this stays
    /// byte-identical; it is a device-captured flavor that does not belong in this
    /// hardware-agnostic crate.
    #[deprecated(
        note = "device-captured 8-channel constructor in a hardware-agnostic crate; \
                use ChannelTable::default_with_channels(n) and set per-channel values \
                from the actual front end"
    )]
    pub fn nonuniform_default_8ch() -> Self {
        let mut t = Self::default_with_channels(8);
        let spec: [(f64, f64, &str); 8] = [
            (1000.0, 5.0, "Airbags"),
            (10.25, 100.0, "Trunions"),
            (10.27, 100.0, "Vertical"),
            (10.0, 100.0, ""),
            (10.0, 100.0, ""),
            (10.0, 100.0, ""),
            (10.0, 100.0, ""),
            (10.0, 100.0, ""),
        ];
        for (c, (sens, range, desc)) in t.channels.iter_mut().zip(spec.iter()) {
            c.sensitivity_mv_per_eu = *sens;
            c.range_max = *range;
            c.description = desc.to_string();
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- seed + existing invariants --------------------------------------------

    #[test]
    fn default_with_channels_neutral_shape() {
        let t = ChannelTable::default_with_channels(4);
        assert_eq!(t.channels.len(), 4);
        assert_eq!(t.channels[0].role, ChannelRole::Control);
        assert!(t.channels[1..]
            .iter()
            .all(|c| c.role == ChannelRole::Monitor));
        // Neutral: uniform Channel::new defaults, no vendor seed values.
        assert!(t.channels.iter().all(|c| c.sensitivity_mv_per_eu == 100.0));
        assert_eq!(
            t.channels.iter().map(|c| c.index).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        t.validate().unwrap();
    }

    #[test]
    fn smart_seed_has_nonuniform_sensitivity() {
        // This test checks the deprecated seed itself.
        #[allow(deprecated)]
        let t = ChannelTable::nonuniform_default_8ch();
        assert_eq!(t.channels.len(), 8);
        assert_eq!(t.channels[0].sensitivity_mv_per_eu, 1000.0);
        assert_eq!(t.channels[1].sensitivity_mv_per_eu, 10.25);
        assert_eq!(t.channels[0].role, ChannelRole::Control);
        t.validate().unwrap();
    }

    #[test]
    fn smart_seed_reports_clean() {
        // This test checks the deprecated seed itself.
        #[allow(deprecated)]
        let report = ChannelTable::nonuniform_default_8ch().validate_report();
        assert!(report.is_ok(), "errors: {:?}", report.errors);
        assert!(
            report.warnings.is_empty(),
            "warnings: {:?}",
            report.warnings
        );
    }

    #[test]
    fn validate_rejects_zero_sensitivity() {
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.sensitivity_mv_per_eu = 0.0;
        t.channels.push(c);
        assert_eq!(t.validate(), Err(TableError::ZeroSensitivity(1)));
    }

    /// Decided by the project's owner: a negative sensitivity on an enabled row is refused,
    /// whatever its role; a disabled row holds any number. Falsified by dropping the
    /// `NegativeSensitivity` check in `validate_report`: the table validates.
    #[test]
    fn channel_table_validate_refuses_a_negative_sensitivity_on_an_enabled_row() {
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.sensitivity_mv_per_eu = -100.0;
        t.channels.push(c.clone());
        let e = t.validate().unwrap_err();
        assert_eq!(
            e,
            TableError::NegativeSensitivity {
                index: 1,
                value: -100.0
            }
        );
        let said = e.to_string();
        assert!(said.contains("channel 1") && said.contains("sensitivity_mv_per_eu"));
        assert!(said.contains("-100") && said.contains("polarity"), "{said}");
        c.role = ChannelRole::Drive;
        t.channels[0] = c.clone();
        assert!(matches!(
            t.validate(),
            Err(TableError::NegativeSensitivity { index: 1, .. })
        ));
        c.enabled = false;
        t.channels[0] = c;
        let mut on = Channel::new(2);
        on.sensitivity_mv_per_eu = 10.0;
        t.channels.push(on);
        assert_eq!(t.validate(), Ok(()));
    }

    #[test]
    fn validate_rejects_empty_and_dup_index() {
        assert_eq!(
            ChannelTable::new().validate(),
            Err(TableError::EmptyEnabled)
        );
        let mut t = ChannelTable::new();
        t.channels.push(Channel::new(1));
        t.channels.push(Channel::new(1));
        assert_eq!(t.validate(), Err(TableError::DuplicateIndex(1)));
    }

    // --- validate_report: MIMO + IEPE rules ------------------------------------

    #[test]
    fn reference_without_control_is_hard_error() {
        let mut t = ChannelTable::new();
        let mut r = Channel::new(1);
        r.role = ChannelRole::Reference;
        t.channels.push(r);
        let report = t.validate_report();
        assert!(report.errors.contains(&TableError::ReferenceWithoutControl));
        assert!(t.validate().is_err());
    }

    #[test]
    fn reference_with_control_is_ok() {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        let mut r = Channel::new(2);
        r.role = ChannelRole::Reference;
        t.channels.push(ctrl);
        t.channels.push(r);
        assert!(t.validate().is_ok());
    }

    #[test]
    fn iepe_bias_mismatch_is_warning_not_error() {
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.interface = Interface::Iepe;
        c.iepe_bias_v = None; // IEPE but no bias -> warn
        t.channels.push(c);
        let report = t.validate_report();
        assert!(report.is_ok(), "should have no hard errors");
        assert!(report.warnings.contains(&TableWarning::IepeMissingBias(1)));
        assert!(t.validate().is_ok());
    }

    #[test]
    fn bias_on_single_ended_is_warning() {
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.interface = Interface::SingleEnded;
        c.iepe_bias_v = Some(2.0);
        t.channels.push(c);
        let report = t.validate_report();
        assert!(report
            .warnings
            .contains(&TableWarning::BiasOnSingleEnded(1)));
    }

    // --- validate_report: control-group contiguity + drive coherence -----------

    #[test]
    fn non_contiguous_control_group_is_error() {
        let mut t = ChannelTable::new();
        let mut a = Channel::new(1);
        a.control_group = Some(0);
        let mut b = Channel::new(2);
        b.control_group = Some(2); // gap: no group 1
        t.channels.push(a);
        t.channels.push(b);
        assert_eq!(t.validate(), Err(TableError::NonContiguousControlGroup(1)));
    }

    #[test]
    fn contiguous_control_groups_ok() {
        let mut t = ChannelTable::new();
        for (i, g) in [(1u32, 0u32), (2, 1), (3, 1)] {
            let mut c = Channel::new(i);
            c.control_group = Some(g);
            t.channels.push(c);
        }
        assert!(t.validate().is_ok());
    }

    #[test]
    fn drive_channel_produces_warning_and_skips_sensitivity_check() {
        let mut t = ChannelTable::new();
        let mut d = Channel::new(1);
        d.role = ChannelRole::Drive;
        d.units = Units::VoltageV;
        d.sensitivity_mv_per_eu = 0.0; // would be a hard error for an input row
        d.range_max = 0.0;
        t.channels.push(d);
        let report = t.validate_report();
        assert!(
            report.is_ok(),
            "drive rows skip the sensitivity/range hard checks"
        );
        assert_eq!(report.warnings, [TableWarning::DriveChannelPresent(1)]);
    }

    // --- the unit vocabulary, and what control needs of a unit -----------------

    /// Every vocabulary name is known; the five controller names map onto their
    /// variants and back unchanged; any other name is kept exactly as written.
    ///
    /// Falsified by folding `G` onto `Units::AccelG` in `from_name`: the
    /// "kept as written" assertion fails.
    #[test]
    fn channel_table_units_keep_every_name_as_written() {
        for name in UNIT_VOCABULARY {
            let u = Units::from_name(name);
            assert!(u.is_known(), "{name}");
            assert_eq!(u.name(), name);
        }
        assert_eq!(Units::from_name("g"), Units::AccelG);
        assert_eq!(Units::from_name("V"), Units::VoltageV);
        for name in ["G", "ips", "ft/s", "A"] {
            assert_eq!(Units::from_name(name), Units::Custom(name.into()));
            assert_eq!(Units::from_name(name).name(), name);
        }
        assert!(!Units::from_name("A").is_known());
    }

    /// A unit outside the vocabulary, a drive not in volts and an abort limit on
    /// a row not in g are warnings, never errors: the table runs as before.
    #[test]
    fn channel_table_unit_needs_are_warnings_only() {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.units = Units::Custom("A".into());
        ctrl.abort.accel_g = Some(5.0);
        let mut drive = Channel::new(2);
        drive.role = ChannelRole::Drive;
        t.channels.extend([ctrl, drive]);
        let report = t.validate_report();
        assert!(report.is_ok(), "{:?}", report.errors);
        for w in [
            TableWarning::UnitUnknown(1),
            TableWarning::AbortNotInG(1),
            TableWarning::DriveNotInVolts(2),
        ] {
            assert!(
                report.warnings.contains(&w),
                "{w:?} in {:?}",
                report.warnings
            );
        }
    }

    // --- schema v2: physical binding + analysis tier + the migration ----------

    /// A NEW table is authored at the current schema; nothing else in the crate
    /// may quietly mint a v1 table.
    #[test]
    fn a_new_table_is_at_the_current_schema() {
        assert_eq!(ChannelTable::new().schema_version, SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, 2);
        assert!(!ChannelTable::new().needs_migration());
        // A fresh row is unbound and Full - both are the honest defaults, not
        // placeholders.
        let c = Channel::new(1);
        assert_eq!(c.physical, None);
        assert_eq!(c.analysis_tier, AnalysisTier::Full);
        assert!(c.analysis.is_blank() && !c.limit);
    }

    /// The resolved physical name splits at the FIRST separator and rebuilds
    /// byte-for-byte, so a channel id that itself contains a separator survives
    /// the flat form.
    #[test]
    fn a_physical_ref_round_trips_through_its_resolved_name() {
        let p = PhysicalChannelRef::new("cDAQ1Mod1", "ai0");
        assert_eq!(p.resolved(), "cDAQ1Mod1/ai0");
        assert_eq!(
            PhysicalChannelRef::parse_resolved("cDAQ1Mod1/ai0").unwrap(),
            p
        );
        // First-separator split: everything after it is the channel.
        let deep = PhysicalChannelRef::parse_resolved("Dev1/port0/line3").unwrap();
        assert_eq!(deep.device_id, "Dev1");
        assert_eq!(deep.channel, "port0/line3");
        assert_eq!(deep.resolved(), "Dev1/port0/line3");
    }

    /// A physical cell with no separator is REFUSED, never half-parsed into a
    /// binding that names a device nobody scanned.
    ///
    /// RED-MUTATION: make `parse_resolved` fall back to
    /// `PhysicalChannelRef::new("", s)` and this reds.
    #[test]
    fn a_separator_less_physical_cell_is_refused() {
        for bad in ["ai0", "/ai0", "cDAQ1Mod1/", ""] {
            assert!(
                PhysicalChannelRef::parse_resolved(bad).is_err(),
                "{bad:?} must not parse into a binding"
            );
        }
    }

    /// An unrecognized tier spelling is a refusal, not a silent `Full`.
    ///
    /// RED-MUTATION: give `from_name` a `_ => Ok(AnalysisTier::Full)` arm and
    /// this reds.
    #[test]
    fn analysis_tier_parses_its_spellings_and_refuses_the_rest() {
        let read = AnalysisTier::from_name;
        assert_eq!(read("").unwrap(), AnalysisTier::Full);
        assert_eq!(read(" full ").unwrap(), AnalysisTier::Full);
        assert_eq!(read("ROT").unwrap(), AnalysisTier::Rotating);
        assert_eq!(read("dec:8").unwrap(), AnalysisTier::Decimated(8));
        assert_eq!(read("decimated:16").unwrap(), AnalysisTier::Decimated(16));
        assert!(read("decim:8").is_err());
        assert!(read("decimated:0").is_err(), "0 is not a cadence");
        assert!(read("dec:x").is_err());
        // And every spelling the protocol writes reads back.
        for t in [
            AnalysisTier::Full,
            AnalysisTier::Rotating,
            AnalysisTier::Decimated(4),
        ] {
            assert_eq!(read(&t.name()).unwrap(), t);
        }
    }

    /// The derived tier reports its BASIS, so the decimation policy has one arm to replace
    /// rather than a value to reverse-engineer.
    #[test]
    fn the_derived_tier_names_its_basis() {
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        assert_eq!(
            derived_analysis_tier(&ctrl),
            (AnalysisTier::Full, TierBasis::ControlRole)
        );

        let mut drv = Channel::new(2);
        drv.role = ChannelRole::Drive;
        assert_eq!(derived_analysis_tier(&drv).1, TierBasis::DriveRole);

        let mut limited = Channel::new(3);
        limited.abort.accel_g = Some(10.0);
        assert!(limited.has_armed_limit());
        assert_eq!(derived_analysis_tier(&limited).1, TierBasis::LimitArmed);

        let plain = Channel::new(4);
        assert!(!plain.has_armed_limit());
        assert_eq!(derived_analysis_tier(&plain).1, TierBasis::NoPolicyYet);
        // Every basis is Full in this build; only the reasons differ.
        assert_eq!(derived_analysis_tier(&plain).0, AnalysisTier::Full);
    }

    /// JSON keeps every member: the physical ref's two halves, the tier, the
    /// limit mark and every analysis column.
    #[test]
    fn json_round_trips_every_member_and_the_version() {
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.role = ChannelRole::Control;
        c.physical = Some(PhysicalChannelRef::new("cDAQ1Mod1", "ai0"));
        c.analysis_tier = AnalysisTier::Decimated(2);
        c.limit = true;
        c.analysis = AnalysisColumns {
            node: Some(101),
            direction: "Z+".into(),
            channel_type: "acceleration".into(),
            role: "response".into(),
            daq_range_v: Some(5.0),
            sensor_range_v: Some(4.5),
            serial_number: "0042".into(),
            make: "Maker".into(),
            model: "M-1".into(),
            triax_dof: "Z".into(),
            expiration: "2027-03-01".into(),
            extra: BTreeMap::from([("bay".into(), "3".into())]),
        };
        t.channels.push(c);
        let json = t.to_json().unwrap();
        assert!(json.contains("\"schema_version\": 2"), "{json}");
        assert!(json.contains("\"device_id\": \"cDAQ1Mod1\""), "{json}");
        let back = ChannelTable::from_json(&json).unwrap();
        assert_eq!(t, back);
    }

    /// A row with no analysis columns and no limit mark serializes exactly as it
    /// did before they existed, so a record written from it is unchanged.
    ///
    /// Falsified by dropping `skip_serializing_if` from `Channel::analysis`: the
    /// member list grows by `analysis`.
    #[test]
    fn channel_table_row_without_analysis_columns_keeps_its_serialized_shape() {
        let v = serde_json::to_value(Channel::new(1)).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "abort",
                "analysis_tier",
                "control_group",
                "description",
                "enabled",
                "iepe_bias_v",
                "index",
                "interface",
                "physical",
                "polarity",
                "range_max",
                "role",
                "sensitivity_mv_per_eu",
                "units"
            ]
        );
    }

    /// THE MIGRATION: a v1 JSON table (no `schema_version`, no `physical`, no
    /// `analysis_tier`) loads at the current schema with the honest defaults.
    /// Members a v1 table had that this build no longer holds (`save`, `stream`,
    /// `reference`) are read past.
    ///
    /// RED-MUTATION: drop the `t.migrate_to_current()?` call from `from_json`
    /// and this reds on the version assertion.
    #[test]
    fn a_v1_json_table_migrates_with_defaults() {
        let v1 = r#"{
          "channels": [
            {"index":1,"enabled":true,"save":true,"stream":false,
             "interface":"SingleEnded","iepe_bias_v":null,"units":"AccelG",
             "sensitivity_mv_per_eu":100.0,"range_max":100.0,"polarity":"Positive",
             "description":"CH1","role":"Control","control_group":null,
             "abort":{"accel_g":null,"velocity_ips":null,"displacement_mil":null}}
          ],
          "reference": null
        }"#;
        let t = ChannelTable::from_json(v1).unwrap();
        assert_eq!(t.schema_version, SCHEMA_VERSION);
        assert_eq!(t.channels.len(), 1);
        assert_eq!(
            t.channels[0].physical, None,
            "a v1 row is UNBOUND; inventing a binding here would bind it to a device nobody saw"
        );
        assert_eq!(t.channels[0].analysis_tier, AnalysisTier::Full);
        assert_eq!(t.channels[0].description, "CH1");
        t.validate().unwrap();
    }

    /// A table from a FUTURE build is refused, never partly loaded. A silently
    /// dropped `physical` binding is a run on the wrong physical channel.
    ///
    /// RED-MUTATION: replace the `>` guard in `migrate_to_current` with a
    /// clamp (`self.schema_version = SCHEMA_VERSION`) and this reds.
    #[test]
    fn a_future_schema_is_refused_rather_than_truncated() {
        let mut t = ChannelTable::default_with_channels(1);
        t.schema_version = SCHEMA_VERSION + 1;
        let err = t.migrate_to_current().unwrap_err();
        assert_eq!(
            err,
            TableError::UnsupportedSchemaVersion {
                found: SCHEMA_VERSION + 1,
                supported: SCHEMA_VERSION
            }
        );
        assert!(err.to_string().contains("refused"), "{err}");
        // And through the JSON door, which is how one would actually arrive.
        let json = serde_json::to_string(&t).unwrap();
        assert!(ChannelTable::from_json(&json).is_err());
    }

    #[test]
    fn validate_report_collects_multiple_errors_at_once() {
        // Two enabled channels, both with problems, plus a duplicate index.
        let mut t = ChannelTable::new();
        let mut a = Channel::new(1);
        a.sensitivity_mv_per_eu = 0.0;
        let mut b = Channel::new(1); // duplicate index
        b.range_max = -1.0;
        t.channels.push(a);
        t.channels.push(b);
        let report = t.validate_report();
        assert!(report.errors.len() >= 3, "errors: {:?}", report.errors);
        assert!(report.errors.contains(&TableError::DuplicateIndex(1)));
        assert!(report.errors.contains(&TableError::ZeroSensitivity(1)));
        assert!(report.errors.contains(&TableError::NonPositiveRange(1)));
    }
}
