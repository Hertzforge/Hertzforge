//! `eigen-channel-table` - the universal channel table.
//!
//! One table, filled once, used by every module. The schema is a superset of a typical
//! commercial controller's channel-config dialog: per
//! channel identity, interface, sensitivity, range, role, and tri-field abort limits,
//! plus MIMO reference fields. The table holds channel IDENTITY, not the test
//! definition (that scope line is deliberate).
//!
//! Types are pinned by the integrator. The TSV round-trip is implemented so the
//! spreadsheet copy/paste spine works today; CSV/JSON/UFF import-export and the full
//! invariant set are hardened by later channel-table work.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::fmt;

pub mod csv;
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
/// because `eigen_hal::discovery` makes a backend's channel id opaque: only the
/// backend that produced an id may parse it. A consumer that needs to know
/// "which device is this row on" must be able to ask without splitting anything,
/// which is exactly what `device_id` is for.
///
/// `None` on a row means UNBOUND, and unbound is a real state, not a defect: the
/// Eigen-Sim station has no physical channels to bind to, and a table authored
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
    /// This is the one place in the tree that splits a physical name, and it
    /// exists because the flat CSV/TSV forms have exactly one cell per field:
    /// a two-field reference has to survive as one string there. JSON keeps the
    /// two halves apart and is the lossless format.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Units {
    AccelG,
    VelocityIps,
    DisplacementMil,
    ForceLbf,
    VoltageV,
    Custom(String),
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
    pub save: bool,
    pub stream: bool,
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
}

impl Channel {
    pub fn new(index: u32) -> Self {
        Channel {
            index,
            enabled: true,
            save: true,
            stream: false,
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

/// MIMO reference specification. Held here for channel identity only; the numeric
/// reference/weighting/DOF-transform matrices for a given test are supplied by the
/// module at configure time. (Identity here, test definition elsewhere.)
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReferenceSpec {
    pub n_reference: u32,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelTable {
    /// The schema this table's contents follow. Absent in serialized form means
    /// [`SCHEMA_VERSION_UNVERSIONED`] - see [`ChannelTable::migrate_to_current`].
    #[serde(default = "unversioned")]
    pub schema_version: u32,
    pub channels: Vec<Channel>,
    pub reference: Option<ReferenceSpec>,
}

impl Default for ChannelTable {
    /// A NEW table is authored at the current schema. Only a table that came
    /// from somewhere else can be at an older one.
    fn default() -> Self {
        ChannelTable {
            schema_version: SCHEMA_VERSION,
            channels: Vec::new(),
            reference: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableError {
    EmptyEnabled,
    ZeroSensitivity(u32),
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
    /// `reference.n_reference` disagrees with the count of `Reference`-role channels.
    ReferenceCountMismatch { declared: u32, found: u32 },
    /// An enabled channel is a Drive output. Informational: drive channels are
    /// outputs, not sensor inputs, so their sensitivity/range fields are advisory.
    DriveChannelPresent(u32),
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
            TableWarning::ReferenceCountMismatch { declared, found } => write!(
                f,
                "reference spec declares {declared} references but {found} reference-role channels exist"
            ),
            TableWarning::DriveChannelPresent(i) => {
                write!(f, "channel {i} is a drive output (sensitivity/range are advisory)")
            }
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
    /// This is the ONE place a version is interpreted, and every load path calls
    /// it: [`ChannelTable::from_json`], [`ChannelTable::from_tsv`],
    /// [`ChannelTable::from_csv`] and both clipboard-paste entry points. A table
    /// that reached a module without passing through here is a table nobody
    /// checked the version of.
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
    ///    it) and a strictly positive range.
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
    /// Warnings (non-fatal): IEPE/bias interface mismatch per channel, a declared
    /// `n_reference` that disagrees with the reference-role channel count, and the
    /// presence of drive-output channels (informational). Warnings never block use.
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

        // --- reference-count consistency (warning): declared vs. found ---
        if let Some(ref spec) = self.reference {
            if spec.n_reference != n_reference_role {
                report.warnings.push(TableWarning::ReferenceCountMismatch {
                    declared: spec.n_reference,
                    found: n_reference_role,
                });
            }
        }

        report
    }

    // --- JSON (serde, pretty) ---

    /// Serialize the whole table (channels + reference spec) to pretty JSON. This is
    /// the canonical on-disk / interchange format; it preserves every field including
    /// the reference spec that the flat TSV/CSV forms drop.
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

    // --- column headers shared by the TSV/CSV flat forms ---

    /// Canonical short field keys, in column order, for the flat TSV/CSV forms. The
    /// `paste_from_clipboard_tsv` header mapper resolves human column names onto these.
    ///
    /// Schema v2 appended `physical` and `tier` as the LAST two columns, which is
    /// what makes a v1 file (16 columns, no version marker) loadable: the missing
    /// cells pad to their defaults exactly as Excel's dropped trailing empties
    /// already did.
    pub const FIELD_KEYS: [&'static str; 18] = [
        "idx",
        "en",
        "save",
        "stream",
        "iface",
        "iepe_v",
        "units",
        "sens_mv_eu",
        "range",
        "pol",
        "role",
        "group",
        "abort_g",
        "abort_ips",
        "abort_mil",
        "desc",
        "physical",
        "tier",
    ];

    /// The column count of the schema-v1 flat form. A parsed row no wider than
    /// this is a v1 row, which is how the versionless flat forms date themselves.
    pub const V1_COLUMNS: usize = 16;

    const TSV_HEADER: &'static str =
        "idx\ten\tsave\tstream\tiface\tiepe_v\tunits\tsens_mv_eu\trange\tpol\trole\tgroup\tabort_g\tabort_ips\tabort_mil\tdesc\tphysical\ttier";

    /// Excel-compatible TSV for spreadsheet copy/paste (goal 6). One header row, one
    /// row per channel. Description must not contain tabs or newlines.
    pub fn to_tsv(&self) -> String {
        let mut out = String::from(Self::TSV_HEADER);
        for c in &self.channels {
            out.push('\n');
            out.push_str(&channel_to_ordered_fields(c).join("\t"));
        }
        out
    }

    /// Parse the TSV produced by `to_tsv`. Hardened for real spreadsheet paste:
    ///
    /// - tolerates a present-or-absent header row (detected by a leading `idx` cell),
    /// - tolerates CRLF line endings and a trailing blank line,
    /// - tolerates trailing whitespace/tabs on a row (Excel adds these),
    /// - tolerates rows with FEWER than 18 columns by padding trailing empties
    ///   (Excel drops empty trailing cells on copy - and a schema-v1 file has 16),
    ///   but rejects rows with MORE.
    ///
    /// The widest row DATES the file: no wider than [`ChannelTable::V1_COLUMNS`]
    /// means a v1 table, which then goes through
    /// [`ChannelTable::migrate_to_current`] like any other older-schema load.
    ///
    /// `desc` is taken verbatim, so a description may itself contain no tab. For
    /// reordered/renamed columns use [`ChannelTable::paste_from_clipboard_tsv`].
    pub fn from_tsv(s: &str) -> Result<Self, TableError> {
        let ncols = Self::FIELD_KEYS.len();
        let mut channels = Vec::new();
        let mut widest = 0usize;
        for line in s.lines() {
            let line = line.trim_end_matches('\r');
            // Excel often leaves trailing tabs/spaces; strip them but keep interior tabs.
            let line = line.trim_end_matches([' ', '\t']);
            if line.is_empty() {
                continue;
            }
            if is_header_row(line, '\t') {
                continue;
            }
            let mut f: Vec<&str> = line.split('\t').collect();
            if f.len() > ncols {
                return Err(TableError::Parse(format!(
                    "expected at most {ncols} columns, got {}",
                    f.len()
                )));
            }
            widest = widest.max(f.len());
            while f.len() < ncols {
                f.push(""); // pad dropped trailing empties
            }
            channels.push(channel_from_ordered_fields(&f)?);
        }
        let mut t = ChannelTable {
            schema_version: flat_schema_version(widest),
            channels,
            reference: None,
        };
        t.migrate_to_current()?;
        Ok(t)
    }

    /// Clipboard-paste entry point that tolerates REORDERED and RENAMED columns.
    ///
    /// Requires a header row. Each header cell is matched case-insensitively (and
    /// ignoring spaces/underscores/`#`) against a set of aliases per field, so a paste
    /// from a spreadsheet whose columns are, e.g., `Channel #`, `Sensitivity (mV/EU)`,
    /// `Enabled`, `Description` in any order maps onto the schema where unambiguous.
    ///
    /// Unmapped columns are ignored. Missing columns take the field default from
    /// [`Channel::new`]. `index` is required (either an `idx`-family column, or row
    /// position + 1 as a fallback). Ambiguous headers (two columns mapping to the same
    /// field) are rejected. On any structural problem a [`TableError::Parse`] is
    /// returned with a human-readable reason.
    pub fn paste_from_clipboard_tsv(s: &str) -> Result<Self, TableError> {
        Self::paste_from_clipboard_delim(s, '\t')
    }

    /// Comma-delimited sibling of [`ChannelTable::paste_from_clipboard_tsv`]: same
    /// header-aliasing, reordering, and RFC-4180 quote handling, for clipboard or file
    /// content whose columns are renamed/reordered but comma-separated. Use this (not
    /// [`ChannelTable::from_csv`]) when the CSV has friendly/renamed headers; `from_csv`
    /// expects the fixed 16-column schema order.
    pub fn paste_from_clipboard_csv(s: &str) -> Result<Self, TableError> {
        Self::paste_from_clipboard_delim(s, ',')
    }

    /// Shared header-mapping paste for both tab- and comma-delimited clipboard data.
    fn paste_from_clipboard_delim(s: &str, delim: char) -> Result<Self, TableError> {
        // Find the first non-blank line; it must be the header.
        let mut lines = s
            .lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.trim().is_empty());
        let header_line = lines
            .next()
            .ok_or_else(|| TableError::Parse("empty paste (no header row)".into()))?;

        let header_cells: Vec<String> = split_row(header_line, delim);
        // Map each column position -> the schema field it feeds (if any).
        let mut col_field: Vec<Option<Field>> = Vec::with_capacity(header_cells.len());
        let mut assigned: std::collections::BTreeMap<Field, usize> = Default::default();
        for (col, name) in header_cells.iter().enumerate() {
            let field = Field::from_header(name);
            if let Some(fd) = field {
                if let Some(prev) = assigned.insert(fd, col) {
                    return Err(TableError::Parse(format!(
                        "ambiguous header: columns {prev} and {col} both map to `{}`",
                        fd.key()
                    )));
                }
            }
            col_field.push(field);
        }

        let mut channels = Vec::new();
        for (row_i, line) in lines.enumerate() {
            let cells = split_row(line, delim);
            if cells.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            let mut ch = Channel::new(row_i as u32 + 1); // positional index fallback
            let mut saw_index = false;
            for (col, field) in col_field.iter().enumerate() {
                let Some(field) = field else { continue };
                let raw = cells.get(col).map(|s| s.as_str()).unwrap_or("").trim();
                if raw.is_empty() && *field != Field::Description {
                    continue; // leave the Channel::new default in place
                }
                field.apply(&mut ch, raw)?;
                if *field == Field::Index {
                    saw_index = true;
                }
            }
            let _ = saw_index; // positional fallback already applied above
            channels.push(ch);
        }
        // A header-mapped paste is versionless by construction (columns are named,
        // not positional), so it is treated as the OLDEST schema and migrated:
        // whichever v2 columns the paste did carry are already applied, and the
        // ones it did not are at their defaults.
        let mut t = ChannelTable {
            schema_version: SCHEMA_VERSION_UNVERSIONED,
            channels,
            reference: None,
        };
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

// --- small serializers/parsers (kept private; may be replaced with serde later) ---
fn b2s(b: bool) -> &'static str {
    if b {
        "1"
    } else {
        "0"
    }
}
fn s2b(s: &str) -> bool {
    let s = s.trim();
    matches!(
        s,
        "1" | "true"
            | "TRUE"
            | "True"
            | "yes"
            | "Yes"
            | "YES"
            | "y"
            | "Y"
            | "on"
            | "ON"
            | "x"
            | "X"
    )
}
fn iface_str(i: Interface) -> &'static str {
    match i {
        Interface::SingleEnded => "SE",
        Interface::Iepe => "IEPE",
    }
}
fn iface_from(s: &str) -> Interface {
    let s = s.trim();
    if s.eq_ignore_ascii_case("iepe") || s.eq_ignore_ascii_case("icp") {
        Interface::Iepe
    } else {
        Interface::SingleEnded
    }
}
fn pol_str(p: Polarity) -> &'static str {
    match p {
        Polarity::Positive => "+",
        Polarity::Negative => "-",
    }
}
fn pol_from(s: &str) -> Polarity {
    let s = s.trim();
    if s == "-" || s.eq_ignore_ascii_case("neg") || s.eq_ignore_ascii_case("negative") {
        Polarity::Negative
    } else {
        Polarity::Positive
    }
}
fn role_str(r: ChannelRole) -> &'static str {
    match r {
        ChannelRole::Control => "control",
        ChannelRole::Monitor => "monitor",
        ChannelRole::Drive => "drive",
        ChannelRole::Reference => "reference",
    }
}
fn role_from(s: &str) -> ChannelRole {
    match s.trim().to_ascii_lowercase().as_str() {
        "control" | "ctrl" | "c" => ChannelRole::Control,
        "drive" | "output" | "out" | "d" => ChannelRole::Drive,
        "reference" | "ref" | "r" => ChannelRole::Reference,
        _ => ChannelRole::Monitor,
    }
}
pub(crate) fn units_str(u: &Units) -> String {
    match u {
        Units::AccelG => "g".into(),
        Units::VelocityIps => "ips".into(),
        Units::DisplacementMil => "mil".into(),
        Units::ForceLbf => "lbf".into(),
        Units::VoltageV => "V".into(),
        Units::Custom(s) => s.clone(),
    }
}
fn units_from(s: &str) -> Units {
    match s.trim() {
        "g" | "G" => Units::AccelG,
        "ips" | "in/s" | "in-s" => Units::VelocityIps,
        "mil" => Units::DisplacementMil,
        "lbf" => Units::ForceLbf,
        "V" | "v" | "volt" | "volts" => Units::VoltageV,
        other => Units::Custom(other.to_string()),
    }
}
fn opt_f(v: Option<f64>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}
fn opt_u(v: Option<u32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}
fn parse_u(s: &str) -> Result<u32, TableError> {
    s.trim()
        .parse()
        .map_err(|_| TableError::Parse(format!("bad u32 '{s}'")))
}
fn parse_f(s: &str) -> Result<f64, TableError> {
    s.trim()
        .parse()
        .map_err(|_| TableError::Parse(format!("bad f64 '{s}'")))
}
fn parse_opt_f(s: &str) -> Result<Option<f64>, TableError> {
    if s.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(parse_f(s)?))
    }
}
fn parse_opt_u(s: &str) -> Result<Option<u32>, TableError> {
    if s.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(parse_u(s)?))
    }
}

/// Render an [`AnalysisTier`] to its flat-form cell: `full`, `dec:N`, `rot`.
pub(crate) fn tier_str(t: AnalysisTier) -> String {
    match t {
        AnalysisTier::Full => "full".into(),
        AnalysisTier::Decimated(n) => format!("dec:{n}"),
        AnalysisTier::Rotating => "rot".into(),
    }
}

/// Parse an [`AnalysisTier`] cell. Empty is `Full` (the v1 default, and what an
/// operator leaving the column blank means). An unrecognized spelling is a parse
/// ERROR rather than a silent `Full`: quietly promoting an intended `dec:8` row
/// to full analysis is a 400-channel budget blown with nothing said.
pub(crate) fn tier_from(s: &str) -> Result<AnalysisTier, TableError> {
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
        "bad analysis tier '{s}': expected full | dec:N | rot"
    )))
}

/// Which schema a FLAT (versionless) form's widest row implies. `0` columns
/// means an empty file, which is treated as current rather than as v1: there is
/// nothing in it to migrate.
fn flat_schema_version(widest_row: usize) -> u32 {
    if widest_row == 0 || widest_row > ChannelTable::V1_COLUMNS {
        SCHEMA_VERSION
    } else {
        SCHEMA_VERSION_UNVERSIONED
    }
}

/// True if a delimited line is a header row (first cell resolves to the `idx` field
/// or is literally `idx`). Case-insensitive, space/underscore/`#`-insensitive.
pub(crate) fn is_header_row(line: &str, delim: char) -> bool {
    match line.split(delim).next() {
        Some(first) => matches!(Field::from_header(first), Some(Field::Index)),
        None => false,
    }
}

/// Build a `Channel` from exactly 18 ordered string fields (the fixed TSV/CSV column
/// order). Shared by `from_tsv` and `csv::from_csv`.
pub(crate) fn channel_from_ordered_fields(f: &[&str]) -> Result<Channel, TableError> {
    debug_assert_eq!(f.len(), ChannelTable::FIELD_KEYS.len());
    Ok(Channel {
        index: parse_u(f[0])?,
        enabled: s2b(f[1]),
        save: s2b(f[2]),
        stream: s2b(f[3]),
        interface: iface_from(f[4]),
        iepe_bias_v: parse_opt_f(f[5])?,
        units: units_from(f[6]),
        sensitivity_mv_per_eu: parse_f(f[7])?,
        range_max: parse_f(f[8])?,
        polarity: pol_from(f[9]),
        role: role_from(f[10]),
        control_group: parse_opt_u(f[11])?,
        abort: AbortLimits {
            accel_g: parse_opt_f(f[12])?,
            velocity_ips: parse_opt_f(f[13])?,
            displacement_mil: parse_opt_f(f[14])?,
        },
        description: f[15].to_string(),
        physical: if f[16].trim().is_empty() {
            None
        } else {
            Some(PhysicalChannelRef::parse_resolved(f[16].trim())?)
        },
        analysis_tier: tier_from(f[17])?,
    })
}

/// Render a `Channel` to the 18 ordered string fields (the fixed TSV/CSV column
/// order). Shared by `to_tsv` and `csv::to_csv`.
pub(crate) fn channel_to_ordered_fields(c: &Channel) -> [String; 18] {
    [
        c.index.to_string(),
        b2s(c.enabled).to_string(),
        b2s(c.save).to_string(),
        b2s(c.stream).to_string(),
        iface_str(c.interface).to_string(),
        opt_f(c.iepe_bias_v),
        units_str(&c.units),
        c.sensitivity_mv_per_eu.to_string(),
        c.range_max.to_string(),
        pol_str(c.polarity).to_string(),
        role_str(c.role).to_string(),
        opt_u(c.control_group),
        opt_f(c.abort.accel_g),
        opt_f(c.abort.velocity_ips),
        opt_f(c.abort.displacement_mil),
        c.description.clone(),
        c.physical_name(),
        tier_str(c.analysis_tier),
    ]
}

/// Split one delimited row into owned cells. For tab-delimited data this is a plain
/// split (spreadsheets never quote TSV). For comma-delimited data the CSV module owns
/// RFC-4180 quote handling; here we use the CSV parser so quoted fields survive paste.
fn split_row(line: &str, delim: char) -> Vec<String> {
    if delim == ',' {
        csv::parse_csv_line(line)
    } else {
        line.split(delim).map(|s| s.to_string()).collect()
    }
}

/// The 18 schema fields addressable by a paste header. Used to map arbitrary
/// spreadsheet column names onto the channel fields (case/space/underscore-insensitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Field {
    Index,
    Enabled,
    Save,
    Stream,
    Interface,
    IepeBias,
    Units,
    Sensitivity,
    Range,
    Polarity,
    Role,
    Group,
    AbortG,
    AbortIps,
    AbortMil,
    Description,
    Physical,
    Tier,
}

impl Field {
    fn key(self) -> &'static str {
        match self {
            Field::Index => "idx",
            Field::Enabled => "en",
            Field::Save => "save",
            Field::Stream => "stream",
            Field::Interface => "iface",
            Field::IepeBias => "iepe_v",
            Field::Units => "units",
            Field::Sensitivity => "sens_mv_eu",
            Field::Range => "range",
            Field::Polarity => "pol",
            Field::Role => "role",
            Field::Group => "group",
            Field::AbortG => "abort_g",
            Field::AbortIps => "abort_ips",
            Field::AbortMil => "abort_mil",
            Field::Description => "desc",
            Field::Physical => "physical",
            Field::Tier => "tier",
        }
    }

    /// Resolve a human/short header name to a field. Normalizes by lowercasing and
    /// dropping spaces, underscores, hyphens, `#`, and parenthetical unit suffixes,
    /// then matches a curated alias set. Returns `None` for unrecognized columns.
    fn from_header(raw: &str) -> Option<Field> {
        let n = normalize_header(raw);
        match n.as_str() {
            "idx" | "index" | "ch" | "chan" | "channel" | "channelno" | "chno" | "no" | "num"
            | "number" => Some(Field::Index),
            "en" | "enable" | "enabled" | "on" | "active" => Some(Field::Enabled),
            "save" | "record" | "log" => Some(Field::Save),
            "stream" | "streaming" | "telemetry" => Some(Field::Stream),
            "iface" | "interface" | "coupling" | "input" | "inputtype" | "sensortype" => {
                Some(Field::Interface)
            }
            "iepev" | "iepebias" | "bias" | "biasv" | "iepebiasv" | "iepe" => Some(Field::IepeBias),
            "units" | "unit" | "eu" | "engunit" | "engunits" | "engineeringunit"
            | "engineeringunits" => Some(Field::Units),
            "sensmveu" | "sens" | "sensitivity" | "sensmvg" | "mvg" | "mveu" | "mvpereu"
            | "mvperg" | "cal" | "calibration" => Some(Field::Sensitivity),
            "range" | "rangemax" | "fullscale" | "fs" | "max" | "maxrange" => Some(Field::Range),
            "pol" | "polarity" | "sign" | "direction" => Some(Field::Polarity),
            "role" | "type" | "channelrole" | "function" => Some(Field::Role),
            "group" | "controlgroup" | "ctrlgroup" | "grp" | "cgroup" => Some(Field::Group),
            "abortg" | "abortaccel" | "abortlimitg" | "glimit" | "accelabort" | "limitg" => {
                Some(Field::AbortG)
            }
            "abortips" | "abortvel" | "abortvelocity" | "ipslimit" | "velabort" | "limitips"
            | "abortins" | "insabort" => Some(Field::AbortIps),
            "abortmil" | "abortdisp" | "abortdisplacement" | "millimit" | "dispabort"
            | "limitmil" => Some(Field::AbortMil),
            "desc" | "description" | "label" | "name" | "tag" | "location" | "note" | "notes"
            | "comment" => Some(Field::Description),
            // Schema v2. `name` stays a Description alias (it always was one) and
            // is deliberately NOT claimed here: an operator's "Name" column has
            // meant the human label since v1 and silently rebinding it to a
            // physical channel would move real tables onto wrong hardware.
            "physical" | "physicalchannel" | "physchan" | "phys" | "devchan" | "hwchannel"
            | "hwchan" => Some(Field::Physical),
            "tier" | "analysistier" | "analysis" | "atier" => Some(Field::Tier),
            _ => None,
        }
    }

    /// Apply a raw cell value to the matching field of a channel. Empty is a no-op for
    /// every field except `Description` (which may legitimately be empty).
    fn apply(self, ch: &mut Channel, raw: &str) -> Result<(), TableError> {
        match self {
            Field::Index => ch.index = parse_u(raw)?,
            Field::Enabled => ch.enabled = s2b(raw),
            Field::Save => ch.save = s2b(raw),
            Field::Stream => ch.stream = s2b(raw),
            Field::Interface => ch.interface = iface_from(raw),
            Field::IepeBias => ch.iepe_bias_v = parse_opt_f(raw)?,
            Field::Units => ch.units = units_from(raw),
            Field::Sensitivity => ch.sensitivity_mv_per_eu = parse_f(raw)?,
            Field::Range => ch.range_max = parse_f(raw)?,
            Field::Polarity => ch.polarity = pol_from(raw),
            Field::Role => ch.role = role_from(raw),
            Field::Group => ch.control_group = parse_opt_u(raw)?,
            Field::AbortG => ch.abort.accel_g = parse_opt_f(raw)?,
            Field::AbortIps => ch.abort.velocity_ips = parse_opt_f(raw)?,
            Field::AbortMil => ch.abort.displacement_mil = parse_opt_f(raw)?,
            Field::Description => ch.description = raw.to_string(),
            Field::Physical => {
                ch.physical = if raw.trim().is_empty() {
                    None
                } else {
                    Some(PhysicalChannelRef::parse_resolved(raw.trim())?)
                }
            }
            Field::Tier => ch.analysis_tier = tier_from(raw)?,
        }
        Ok(())
    }
}

/// Normalize a header cell for alias matching: lowercase, drop everything after a `(`
/// (unit suffixes like `Sensitivity (mV/g)`), and strip spaces/underscores/hyphens/`#`.
fn normalize_header(raw: &str) -> String {
    let base = raw.split('(').next().unwrap_or(raw);
    base.chars()
        .filter(|c| !matches!(c, ' ' | '_' | '-' | '#' | '.' | '/'))
        .flat_map(|c| c.to_lowercase())
        .collect()
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

    // --- TSV round-trips + hardening -------------------------------------------

    #[test]
    fn tsv_round_trips() {
        // The non-uniform seed is this round-trip's fixture.
        #[allow(deprecated)]
        let t = ChannelTable::nonuniform_default_8ch();
        let tsv = t.to_tsv();
        let back = ChannelTable::from_tsv(&tsv).unwrap();
        assert_eq!(t.channels, back.channels);
    }

    #[test]
    fn tsv_round_trips_full_field_coverage() {
        // Exercise every non-default field so the flat form is proven lossless.
        let mut t = ChannelTable::new();
        let mut c = Channel::new(7);
        c.enabled = false;
        c.save = false;
        c.stream = true;
        c.interface = Interface::Iepe;
        c.iepe_bias_v = Some(12.5);
        c.units = Units::Custom("psi".into());
        c.sensitivity_mv_per_eu = 33.3;
        c.range_max = 250.0;
        c.polarity = Polarity::Negative;
        c.role = ChannelRole::Reference;
        c.control_group = Some(0);
        c.abort = AbortLimits {
            accel_g: Some(9.0),
            velocity_ips: Some(70.0),
            displacement_mil: Some(2250.0),
        };
        c.description = "Ref X".into();
        t.channels.push(c);
        // add a control so a reference-only table is still coherent
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.control_group = Some(0);
        t.channels.push(ctrl);
        let back = ChannelTable::from_tsv(&t.to_tsv()).unwrap();
        assert_eq!(t.channels, back.channels);
    }

    #[test]
    fn from_tsv_tolerates_crlf_and_trailing_ws_and_no_header() {
        // No header, CRLF endings, trailing spaces after the description, and a
        // trailing blank line. The trailing whitespace is stripped from the row.
        let body = "1\t1\t1\t0\tSE\t\tg\t100\t100\t+\tcontrol\t\t\t\t\tHeadless  \r\n\r\n";
        let t = ChannelTable::from_tsv(body).unwrap();
        assert_eq!(t.channels.len(), 1);
        assert_eq!(t.channels[0].index, 1);
        assert_eq!(t.channels[0].role, ChannelRole::Control);
        // Trailing spaces after "Headless" are stripped along with row trailing ws.
        assert_eq!(t.channels[0].description, "Headless");
    }

    #[test]
    fn from_tsv_pads_short_rows() {
        // Excel drops trailing empty cells on copy: only 11 columns present.
        let row = "3\t1\t1\t0\tIEPE\t2.5\tg\t10\t50\t+\tmonitor";
        let t = ChannelTable::from_tsv(row).unwrap();
        assert_eq!(t.channels.len(), 1);
        assert_eq!(t.channels[0].index, 3);
        assert_eq!(t.channels[0].interface, Interface::Iepe);
        assert_eq!(t.channels[0].iepe_bias_v, Some(2.5));
        assert_eq!(t.channels[0].control_group, None);
        assert_eq!(t.channels[0].abort, AbortLimits::default());
    }

    #[test]
    fn from_tsv_rejects_too_many_columns() {
        // 19 cells: one past the v2 schema width.
        let row = "1\t1\t1\t0\tSE\t\tg\t100\t100\t+\tmonitor\t\t\t\t\tdesc\tDev/ai0\tfull\tEXTRA";
        let err = ChannelTable::from_tsv(row).unwrap_err();
        match err {
            TableError::Parse(ref m) => assert!(
                m.contains("at most 18"),
                "the refusal must name the schema width: {m}"
            ),
            other => panic!("expected a width refusal, got {other:?}"),
        }
    }

    // --- clipboard paste with reordered / renamed columns ----------------------

    #[test]
    fn paste_reordered_renamed_headers() {
        // Human header names, reordered, with unit suffixes and a junk column.
        let pasted = "\
Description\tChannel #\tSensitivity (mV/g)\tEnabled\tType\tScratch
Airbags\t1\t1000.0\tyes\tcontrol\tfoo
Trunions\t2\t10.25\tyes\tmonitor\tbar";
        let t = ChannelTable::paste_from_clipboard_tsv(pasted).unwrap();
        assert_eq!(t.channels.len(), 2);
        assert_eq!(t.channels[0].index, 1);
        assert_eq!(t.channels[0].description, "Airbags");
        assert_eq!(t.channels[0].sensitivity_mv_per_eu, 1000.0);
        assert_eq!(t.channels[0].role, ChannelRole::Control);
        assert_eq!(t.channels[1].index, 2);
        assert_eq!(t.channels[1].sensitivity_mv_per_eu, 10.25);
        // Unmapped fields take Channel::new defaults.
        assert_eq!(t.channels[0].range_max, 100.0);
    }

    #[test]
    fn paste_positional_index_fallback() {
        // No index column at all: index falls back to row position + 1.
        let pasted = "Sensitivity\tDescription\n50\tA\n60\tB";
        let t = ChannelTable::paste_from_clipboard_tsv(pasted).unwrap();
        assert_eq!(t.channels[0].index, 1);
        assert_eq!(t.channels[1].index, 2);
        assert_eq!(t.channels[1].description, "B");
    }

    #[test]
    fn paste_rejects_ambiguous_headers() {
        let pasted = "channel\tch #\tsens\n1\t1\t100";
        assert!(matches!(
            ChannelTable::paste_from_clipboard_tsv(pasted),
            Err(TableError::Parse(_))
        ));
    }

    // --- JSON round-trip (incl. reference spec) --------------------------------

    #[test]
    fn json_round_trips_with_reference_spec() {
        // The non-uniform seed is this round-trip's fixture.
        #[allow(deprecated)]
        let mut t = ChannelTable::nonuniform_default_8ch();
        t.reference = Some(ReferenceSpec {
            n_reference: 0,
            notes: "seeded from run002".into(),
        });
        let json = t.to_json().unwrap();
        assert!(json.contains("run002"));
        let back = ChannelTable::from_json(&json).unwrap();
        assert_eq!(t, back); // full struct equality, reference included
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
        d.sensitivity_mv_per_eu = 0.0; // would be a hard error for an input row
        d.range_max = 0.0;
        t.channels.push(d);
        let report = t.validate_report();
        assert!(
            report.is_ok(),
            "drive rows skip the sensitivity/range hard checks"
        );
        assert!(report
            .warnings
            .contains(&TableWarning::DriveChannelPresent(1)));
    }

    #[test]
    fn reference_count_mismatch_is_warning() {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        let mut r = Channel::new(2);
        r.role = ChannelRole::Reference;
        t.channels.push(ctrl);
        t.channels.push(r);
        t.reference = Some(ReferenceSpec {
            n_reference: 3, // claims 3, only 1 reference-role channel
            notes: String::new(),
        });
        let report = t.validate_report();
        assert!(report.is_ok());
        assert!(report
            .warnings
            .contains(&TableWarning::ReferenceCountMismatch {
                declared: 3,
                found: 1
            }));
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
    /// RED-MUTATION: give `tier_from` a `_ => Ok(AnalysisTier::Full)` arm and
    /// this reds - which is the point: a `dec:8` typo'd as `decim:8` silently
    /// promoted to full analysis blows a 400-channel budget with nothing said.
    #[test]
    fn analysis_tier_parses_its_spellings_and_refuses_the_rest() {
        assert_eq!(tier_from("").unwrap(), AnalysisTier::Full);
        assert_eq!(tier_from(" full ").unwrap(), AnalysisTier::Full);
        assert_eq!(tier_from("ROT").unwrap(), AnalysisTier::Rotating);
        assert_eq!(tier_from("dec:8").unwrap(), AnalysisTier::Decimated(8));
        assert_eq!(
            tier_from("decimated:16").unwrap(),
            AnalysisTier::Decimated(16)
        );
        assert!(tier_from("decim:8").is_err());
        assert!(tier_from("dec:0").is_err(), "0 is not a cadence");
        assert!(tier_from("dec:x").is_err());
        // And every spelling the renderer emits parses back.
        for t in [
            AnalysisTier::Full,
            AnalysisTier::Rotating,
            AnalysisTier::Decimated(4),
        ] {
            assert_eq!(tier_from(&tier_str(t)).unwrap(), t);
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

    /// v2's two new columns survive the flat TSV form in BOTH directions.
    #[test]
    fn tsv_round_trips_the_v2_columns() {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.physical = Some(PhysicalChannelRef::new("cDAQ9178Sim1Mod1", "ai0"));
        ctrl.analysis_tier = AnalysisTier::Full;
        let mut mon = Channel::new(2);
        mon.physical = Some(PhysicalChannelRef::new("cDAQ9178Sim1Mod1", "ai1"));
        mon.analysis_tier = AnalysisTier::Decimated(8);
        let mut unbound = Channel::new(3);
        unbound.analysis_tier = AnalysisTier::Rotating;
        t.channels.extend([ctrl, mon, unbound]);

        let tsv = t.to_tsv();
        assert!(tsv
            .lines()
            .next()
            .unwrap()
            .ends_with("desc\tphysical\ttier"));
        let back = ChannelTable::from_tsv(&tsv).unwrap();
        assert_eq!(t.channels, back.channels);
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert_eq!(back.channels[2].physical, None, "unbound stays unbound");
    }

    /// Same for CSV, including a description that forces quoting so the two new
    /// columns stay on the right side of the quoting logic.
    #[test]
    fn csv_round_trips_the_v2_columns_with_a_quoted_description() {
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.role = ChannelRole::Control;
        c.description = "fixture, \"aft\"".into();
        c.physical = Some(PhysicalChannelRef::new("cDAQ1Mod1", "ai0"));
        c.analysis_tier = AnalysisTier::Decimated(4);
        t.channels.push(c);
        let back = ChannelTable::from_csv(&t.to_csv()).unwrap();
        assert_eq!(t.channels, back.channels);
        assert_eq!(back.channels[0].description, "fixture, \"aft\"");
        assert_eq!(
            back.channels[0].physical.as_ref().unwrap().resolved(),
            "cDAQ1Mod1/ai0"
        );
        assert_eq!(back.channels[0].analysis_tier, AnalysisTier::Decimated(4));
    }

    /// JSON is the lossless format and keeps the two halves of the physical ref
    /// apart (nothing downstream ever has to split the opaque id).
    #[test]
    fn json_round_trips_the_v2_fields_and_the_version() {
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.role = ChannelRole::Control;
        c.physical = Some(PhysicalChannelRef::new("cDAQ1Mod1", "ai0"));
        c.analysis_tier = AnalysisTier::Decimated(2);
        t.channels.push(c);
        let json = t.to_json().unwrap();
        assert!(json.contains("\"schema_version\": 2"), "{json}");
        assert!(json.contains("\"device_id\": \"cDAQ1Mod1\""), "{json}");
        let back = ChannelTable::from_json(&json).unwrap();
        assert_eq!(t, back);
    }

    /// THE MIGRATION: a v1 JSON table (no `schema_version`, no `physical`, no
    /// `analysis_tier`) loads at the current schema with the honest defaults.
    ///
    /// RED-MUTATION: drop the `t.migrate_to_current()?` call from `from_json`
    /// and this reds on the version assertion - which is exactly the state where
    /// a v1 table would reach a module claiming to be v1 forever.
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

    /// A v1 FLAT file (16 columns, no version marker anywhere) dates itself by
    /// its width and goes through the same migration.
    #[test]
    fn a_v1_flat_row_dates_itself_by_width_and_migrates() {
        // Exactly the v1 header + one v1 data row.
        let v1_tsv = "idx\ten\tsave\tstream\tiface\tiepe_v\tunits\tsens_mv_eu\trange\tpol\trole\tgroup\tabort_g\tabort_ips\tabort_mil\tdesc\n\
                      1\t1\t1\t0\tSE\t\tg\t100\t100\t+\tcontrol\t\t\t\t\tCH1";
        let t = ChannelTable::from_tsv(v1_tsv).unwrap();
        assert_eq!(t.schema_version, SCHEMA_VERSION);
        assert_eq!(t.channels[0].physical, None);
        assert_eq!(t.channels[0].analysis_tier, AnalysisTier::Full);
        assert_eq!(t.channels[0].description, "CH1");
        // The width rule itself, stated directly.
        assert_eq!(flat_schema_version(16), SCHEMA_VERSION_UNVERSIONED);
        assert_eq!(flat_schema_version(18), SCHEMA_VERSION);
        assert_eq!(flat_schema_version(0), SCHEMA_VERSION, "nothing to migrate");
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

    /// The header-mapped clipboard path carries the physical column, which is
    /// what makes the 400-row bulk assign a paste rather than 400 dropdowns.
    #[test]
    fn a_header_mapped_paste_carries_physical_and_tier() {
        let pasted = "\
Channel\tPhysical Channel\tAnalysis\tRole
1\tcDAQ1Mod1/ai0\tfull\tcontrol
2\tcDAQ1Mod1/ai1\tdec:4\tmonitor";
        let t = ChannelTable::paste_from_clipboard_tsv(pasted).unwrap();
        assert_eq!(t.schema_version, SCHEMA_VERSION);
        assert_eq!(
            t.channels[0].physical,
            Some(PhysicalChannelRef::new("cDAQ1Mod1", "ai0"))
        );
        assert_eq!(t.channels[1].analysis_tier, AnalysisTier::Decimated(4));
        assert_eq!(t.channels[1].role, ChannelRole::Monitor);
    }

    /// A `Name` column stays the human description. It has meant that since v1,
    /// and rebinding it to the physical channel would move real tables onto
    /// hardware their author never named.
    #[test]
    fn a_name_column_is_still_the_description_not_a_physical_binding() {
        let t = ChannelTable::paste_from_clipboard_tsv("Channel\tName\n1\tAirbags").unwrap();
        assert_eq!(t.channels[0].description, "Airbags");
        assert_eq!(t.channels[0].physical, None);
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
