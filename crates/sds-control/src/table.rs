//! The channel table (schema/PROTOCOL.md, "Channel Table").
//!
//! One table, held by the control service as the controller's own
//! `hertzforge_channel_table::ChannelTable`, used by every run. Validation is the
//! controller's report (`validate_report`) under the protocol's item names, plus the
//! NI binding rules on an NI station: every enabled row bound to a channel of the
//! selected devices in the right direction, no channel shared, one timing family.
//!
//! IEPE is off unless a client turns it on for a row: the starting table and every
//! row this service creates are single-ended.
//!
//! A row also carries the analysis side's columns (node, direction, the sensor's
//! identity, any other column by name), its analysis tier and its limit mark, all
//! optional: written only when they hold something, read when present, and never
//! used to run anything.

use crate::protocol::{num, opt_num, ErrorCode, MethodError};
use crate::station::{channel_id, Selection};
use hertzforge_channel_table::{
    AbortLimits, AnalysisColumns, AnalysisTier, Channel, ChannelRole, ChannelTable, Interface,
    PhysicalChannelRef, Polarity, TableError, TableWarning, Units,
};
use hertzforge_hal::DiscoveredDevice;
use serde_json::{json, Map, Value};

/// One validation item: `{"reason", [location], "message"}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub reason: &'static str,
    pub message: String,
    pub at: At,
}

/// Where an item points.
#[derive(Debug, Clone, PartialEq)]
pub enum At {
    /// A table row's `index`, or the whole table (`None`).
    Channel(Option<u32>),
    /// A breakpoint row, from 0.
    Row(usize),
    /// A configuration field path.
    Field(String),
}

impl Item {
    pub fn channel(reason: &'static str, index: Option<u32>, message: impl Into<String>) -> Item {
        Item {
            reason,
            message: message.into(),
            at: At::Channel(index),
        }
    }

    pub fn row(reason: &'static str, row: usize, message: impl Into<String>) -> Item {
        Item {
            reason,
            message: message.into(),
            at: At::Row(row),
        }
    }

    pub fn field(
        reason: &'static str,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Item {
        Item {
            reason,
            message: message.into(),
            at: At::Field(field.into()),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("reason".into(), Value::from(self.reason));
        match &self.at {
            At::Channel(i) => {
                m.insert("channel".into(), i.map_or(Value::Null, Value::from));
            }
            At::Row(r) => {
                m.insert("row".into(), Value::from(*r));
            }
            At::Field(f) => {
                m.insert("field".into(), Value::from(f.as_str()));
            }
        }
        m.insert("message".into(), Value::from(self.message.as_str()));
        Value::Object(m)
    }
}

/// Errors and warnings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub errors: Vec<Item>,
    pub warnings: Vec<Item>,
}

impl Report {
    pub fn to_json(&self) -> Value {
        json!({
            "errors": self.errors.iter().map(Item::to_json).collect::<Vec<_>>(),
            "warnings": self.warnings.iter().map(Item::to_json).collect::<Vec<_>>(),
        })
    }

    pub fn extend(&mut self, other: Report) {
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
    }
}

/// The table, its revision, and whether it waits for the operator to apply it.
pub struct Table {
    pub revision: u64,
    pub table: ChannelTable,
    /// Set while the table came from somewhere other than this session's own edits
    /// (the table the service kept, a project, a file) and the operator has not
    /// applied it: no run arms on it until then ([`Table::arm_gate`]).
    pub restored: Option<Restored>,
    /// Keeps the table across restarts (`None`: this service does not keep it).
    pub keeper: Option<std::sync::Arc<crate::table_keep::Keeper>>,
}

/// Where a restored table came from and when (schema/PROTOCOL.md, "Restored Tables").
#[derive(Debug, Clone, PartialEq)]
pub struct Restored {
    /// `kept` (read at start from the data folder), `project` or `file`.
    pub source: &'static str,
    /// The project table's or the file's name; for a kept table, the kept file's name.
    pub name: String,
    /// Unix seconds: when a kept table was last written, otherwise when the service
    /// took the table.
    pub at_unix: f64,
}

impl Restored {
    /// The source in words: "the project table \"Plate\"".
    pub fn words(&self) -> String {
        match self.source {
            "kept" => format!("the table this service kept ({})", self.name),
            "project" => format!("the project table \"{}\"", self.name),
            _ => format!("the file \"{}\"", self.name),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"source": self.source, "name": self.name, "at_unix": num(self.at_unix),
               "at": utc_words(self.at_unix)})
    }
}

/// `unix` as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn utc_words(unix: f64) -> String {
    let s = crate::random::record::utc_stamp(unix);
    format!(
        "{}-{}-{} {}:{}:{} UTC",
        &s[0..4],
        &s[4..6],
        &s[6..8],
        &s[9..11],
        &s[11..13],
        &s[13..15]
    )
}

/// What a client may name as a table's source in `channels.set`: `project` or
/// `file`, with a name. `None` when the request names none (a table typed or edited
/// in this session).
pub fn parse_source(
    params: &Map<String, Value>,
) -> Result<Option<(&'static str, String)>, MethodError> {
    let Some(v) = params.get("source") else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let bad = || {
        MethodError::bad_params(
            "source",
            "\"source\" is {\"kind\": \"project\" or \"file\", \"name\": the table's or \
             the file's name}.",
        )
    };
    let o = v.as_object().ok_or_else(bad)?;
    let kind = match o.get("kind").and_then(Value::as_str) {
        Some("project") => "project",
        Some("file") => "file",
        _ => return Err(bad()),
    };
    let name = o
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(bad)?;
    Ok(Some((kind, name.to_owned())))
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

impl Table {
    /// The starting table: one control input and one drive output, both unbound and
    /// single-ended.
    pub fn new() -> Table {
        let mut control = new_row(1);
        control.description = "Control".into();
        control.role = ChannelRole::Control;
        let mut drive = new_row(2);
        drive.description = "Drive".into();
        drive.role = ChannelRole::Drive;
        drive.units = Units::VoltageV;
        drive.sensitivity_mv_per_eu = 1000.0;
        drive.range_max = 10.0;
        let mut table = ChannelTable::new();
        table.channels = vec![control, drive];
        Table {
            revision: 1,
            table,
            restored: None,
            keeper: None,
        }
    }

    /// The `channels.get` result. `restored` is written only while the table waits
    /// to be applied.
    pub fn get(&self, ctx: &StationContext<'_>) -> Value {
        let mut v = json!({
            "revision": self.revision,
            "channels": self.table.channels.iter().map(row_json).collect::<Vec<_>>(),
            "report": validate(&self.table, ctx).to_json(),
        });
        if let Some(r) = &self.restored {
            v["restored"] = r.to_json();
        }
        if let Some(p) = self.keeper.as_ref().and_then(|k| k.problem()) {
            v["kept_problem"] = Value::from(p);
        }
        v
    }

    /// Hand the table as it is now to the keeper, if this service keeps it.
    pub fn keep(&self, now_unix: f64) {
        if let Some(k) = &self.keeper {
            k.keep(&self.table, self.revision, now_unix);
        }
    }

    /// The one check every test module's arm makes, beside its own checks of the
    /// table: a restored table the operator has not applied does not arm.
    pub fn arm_gate(&self) -> Result<(), MethodError> {
        let Some(r) = &self.restored else {
            return Ok(());
        };
        Err(MethodError::named(
            ErrorCode::Refused,
            "table_not_applied",
            format!(
                "The channel table (revision {}) was restored from {} at {} and has not been \
                 applied; review it and apply it before arming.",
                self.revision,
                r.words(),
                utc_words(r.at_unix)
            ),
        )
        .with_detail(json!({"revision": self.revision, "restored": r.to_json()})))
    }

    /// The operator applies the restored table they were shown, at `revision`.
    /// Answers where it had come from.
    pub fn apply(&mut self, revision: u64) -> Result<Restored, MethodError> {
        if revision != self.revision {
            let current = self.revision;
            return Err(MethodError::named(
                ErrorCode::Refused,
                "stale_revision",
                format!(
                    "The table changed since revision {revision} (it is now {current}); read it \
                     again and review it before applying."
                ),
            )
            .with_detail(json!({"current": current, "provided": revision})));
        }
        self.restored.take().ok_or_else(|| {
            MethodError::named(
                ErrorCode::Refused,
                "nothing_to_apply",
                format!(
                    "The channel table (revision {revision}) is not a restored table; it is \
                     already applied."
                ),
            )
        })
    }
}

/// A new row: enabled, monitor, single-ended, 100 mV per g, range 100.
pub fn new_row(index: u32) -> Channel {
    Channel::new(index)
}

/// What the table is validated against.
pub struct StationContext<'a> {
    pub selection: &'a Selection,
    pub scan: &'a [DiscoveredDevice],
}

/// The protocol name of a unit.
pub fn unit_name(u: &Units) -> String {
    u.name()
}

pub fn role_name(r: ChannelRole) -> &'static str {
    match r {
        ChannelRole::Control => "control",
        ChannelRole::Monitor => "monitor",
        ChannelRole::Drive => "drive",
        ChannelRole::Reference => "reference",
    }
}

/// The optional members of a row that hold text, in the order they are written.
const ANALYSIS_TEXT: [&str; 8] = [
    "direction",
    "channel_type",
    "analysis_role",
    "serial_number",
    "make",
    "model",
    "triax_dof",
    "expiration",
];

fn analysis_text<'a>(a: &'a AnalysisColumns, key: &str) -> &'a str {
    match key {
        "direction" => &a.direction,
        "channel_type" => &a.channel_type,
        "analysis_role" => &a.role,
        "serial_number" => &a.serial_number,
        "make" => &a.make,
        "model" => &a.model,
        "triax_dof" => &a.triax_dof,
        "expiration" => &a.expiration,
        _ => "",
    }
}

fn analysis_text_mut<'a>(a: &'a mut AnalysisColumns, key: &str) -> Option<&'a mut String> {
    Some(match key {
        "direction" => &mut a.direction,
        "channel_type" => &mut a.channel_type,
        "analysis_role" => &mut a.role,
        "serial_number" => &mut a.serial_number,
        "make" => &mut a.make,
        "model" => &mut a.model,
        "triax_dof" => &mut a.triax_dof,
        "expiration" => &mut a.expiration,
        _ => return None,
    })
}

/// A row on the wire: every field, then the optional members that hold
/// something.
pub fn row_json(c: &Channel) -> Value {
    let mut row = required_row_json(c);
    let o = row.as_object_mut().expect("a row is an object");
    let a = &c.analysis;
    if let Some(node) = a.node {
        o.insert("node".into(), Value::from(node));
    }
    for key in ANALYSIS_TEXT.iter() {
        let text = analysis_text(a, key);
        if !text.is_empty() {
            o.insert((*key).into(), Value::from(text));
        }
    }
    if let Some(v) = a.daq_range_v {
        o.insert("daq_range_v".into(), num(v));
    }
    if let Some(v) = a.sensor_range_v {
        o.insert("sensor_range_v".into(), num(v));
    }
    if c.analysis_tier != AnalysisTier::Full {
        o.insert("analysis_tier".into(), Value::from(c.analysis_tier.name()));
    }
    if c.limit {
        o.insert("limit".into(), Value::Bool(true));
    }
    if !a.extra.is_empty() {
        o.insert(
            "extra".into(),
            Value::Object(
                a.extra
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
                    .collect(),
            ),
        );
    }
    row
}

fn required_row_json(c: &Channel) -> Value {
    json!({
        "index": c.index,
        "enabled": c.enabled,
        "description": c.description,
        "role": role_name(c.role),
        "physical": c.physical.as_ref().map(|p| json!({"device": p.device_id, "channel": p.channel})),
        "interface": match c.interface { Interface::SingleEnded => "single_ended", Interface::Iepe => "iepe" },
        "iepe_bias_v": opt_num(c.iepe_bias_v),
        "units": unit_name(&c.units),
        "sensitivity_mv_per_eu": num(c.sensitivity_mv_per_eu),
        "range_max": num(c.range_max),
        "polarity": match c.polarity { Polarity::Positive => "positive", Polarity::Negative => "negative" },
        "control_group": c.control_group,
        "abort": {
            "accel_g": opt_num(c.abort.accel_g),
            "velocity_ips": opt_num(c.abort.velocity_ips),
            "displacement_mil": opt_num(c.abort.displacement_mil),
        },
    })
}

/// Parse `channels` (every row carries every field). `bad_params` names the field.
pub fn parse_rows(value: &Value) -> Result<ChannelTable, MethodError> {
    let rows = value.as_array().ok_or_else(|| {
        MethodError::bad_params("channels", "\"channels\" must be a list of rows")
    })?;
    let mut table = ChannelTable::new();
    for (i, row) in rows.iter().enumerate() {
        table.channels.push(parse_row(row, i)?);
    }
    Ok(table)
}

fn parse_row(v: &Value, i: usize) -> Result<Channel, MethodError> {
    let at = |k: &str| format!("channels[{i}].{k}");
    let o = v.as_object().ok_or_else(|| {
        MethodError::bad_params(&format!("channels[{i}]"), "a row must be an object")
    })?;
    let field = |k: &str| -> Result<&Value, MethodError> {
        o.get(k).ok_or_else(|| {
            MethodError::bad_params(
                &at(k),
                format!("row {i} has no \"{k}\"; every row carries every field"),
            )
        })
    };
    let num = |k: &str| -> Result<f64, MethodError> {
        field(k)?.as_f64().filter(|x| x.is_finite()).ok_or_else(|| {
            MethodError::bad_params(&at(k), format!("row {i}: \"{k}\" must be a number"))
        })
    };
    let maybe_num = |k: &str, val: &Value| -> Result<Option<f64>, MethodError> {
        match val {
            Value::Null => Ok(None),
            v => v
                .as_f64()
                .filter(|x| x.is_finite())
                .map(Some)
                .ok_or_else(|| {
                    MethodError::bad_params(
                        &at(k),
                        format!("row {i}: \"{k}\" must be a number or null"),
                    )
                }),
        }
    };
    let text = |k: &str| -> Result<&str, MethodError> {
        field(k)?.as_str().ok_or_else(|| {
            MethodError::bad_params(&at(k), format!("row {i}: \"{k}\" must be a string"))
        })
    };
    let one_of = |k: &str, options: &[&str]| -> Result<String, MethodError> {
        let s = text(k)?;
        if options.contains(&s) {
            Ok(s.to_owned())
        } else {
            Err(MethodError::bad_params(
                &at(k),
                format!(
                    "row {i}: \"{k}\" is one of {}, not \"{s}\"",
                    options.join(", ")
                ),
            ))
        }
    };

    let index = field("index")?
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| {
            MethodError::bad_params(
                &at("index"),
                format!("row {i}: \"index\" must be a whole number from 0"),
            )
        })?;
    let enabled = field("enabled")?.as_bool().ok_or_else(|| {
        MethodError::bad_params(
            &at("enabled"),
            format!("row {i}: \"enabled\" must be true or false"),
        )
    })?;
    let description = text("description")?.to_owned();
    let role = match one_of("role", &["control", "monitor", "drive", "reference"])?.as_str() {
        "control" => ChannelRole::Control,
        "drive" => ChannelRole::Drive,
        "reference" => ChannelRole::Reference,
        _ => ChannelRole::Monitor,
    };
    let physical = match field("physical")? {
        Value::Null => None,
        Value::Object(p) => {
            let part = |k: &str| -> Result<String, MethodError> {
                p.get(k)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        MethodError::bad_params(
                            &format!("channels[{i}].physical.{k}"),
                            format!("row {i}: \"physical\" is {{\"device\",\"channel\"}} from the scan, or null"),
                        )
                    })
            };
            Some(PhysicalChannelRef::new(part("device")?, part("channel")?))
        }
        _ => {
            return Err(MethodError::bad_params(
                &at("physical"),
                format!(
                    "row {i}: \"physical\" is {{\"device\",\"channel\"}} from the scan, or null"
                ),
            ))
        }
    };
    let interface = match one_of("interface", &["single_ended", "iepe"])?.as_str() {
        "iepe" => Interface::Iepe,
        _ => Interface::SingleEnded,
    };
    let iepe_bias_v = maybe_num("iepe_bias_v", field("iepe_bias_v")?)?;
    let units = text("units")?;
    if units.is_empty() {
        return Err(MethodError::bad_params(
            &at("units"),
            format!("row {i}: \"units\" must name a unit"),
        ));
    }
    let polarity = match one_of("polarity", &["positive", "negative"])?.as_str() {
        "negative" => Polarity::Negative,
        _ => Polarity::Positive,
    };
    let control_group = match field("control_group")? {
        Value::Null => None,
        v => Some(
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    MethodError::bad_params(
                        &at("control_group"),
                        format!(
                            "row {i}: \"control_group\" must be a whole number from 0, or null"
                        ),
                    )
                })?,
        ),
    };
    let abort = field("abort")?.as_object().ok_or_else(|| {
        MethodError::bad_params(
            &at("abort"),
            format!("row {i}: \"abort\" is {{\"accel_g\",\"velocity_ips\",\"displacement_mil\"}}"),
        )
    })?;
    let limit = |k: &str| -> Result<Option<f64>, MethodError> {
        let v = abort.get(k).ok_or_else(|| {
            MethodError::bad_params(
                &format!("channels[{i}].abort.{k}"),
                format!("row {i}: \"abort\" has no \"{k}\""),
            )
        })?;
        let x = maybe_num(&format!("abort.{k}"), v)?;
        if x.is_some_and(|x| x <= 0.0) {
            return Err(MethodError::bad_params(
                &format!("channels[{i}].abort.{k}"),
                format!("row {i}: an abort limit is above 0, or null for none"),
            ));
        }
        Ok(x)
    };
    let abort = AbortLimits {
        accel_g: limit("accel_g")?,
        velocity_ips: limit("velocity_ips")?,
        displacement_mil: limit("displacement_mil")?,
    };
    let (analysis, analysis_tier, limit_mark) = parse_optional(o, i)?;
    Ok(Channel {
        index,
        enabled,
        interface,
        iepe_bias_v,
        units: Units::from_name(units),
        sensitivity_mv_per_eu: num("sensitivity_mv_per_eu")?,
        range_max: num("range_max")?,
        polarity,
        description,
        role,
        control_group,
        abort,
        physical,
        analysis_tier,
        limit: limit_mark,
        analysis,
    })
}

/// A row's optional members: each absent or `null` is blank, and one of the
/// wrong type is `bad_params` naming it.
fn parse_optional(
    o: &Map<String, Value>,
    i: usize,
) -> Result<(AnalysisColumns, AnalysisTier, bool), MethodError> {
    let at = |k: &str| format!("channels[{i}].{k}");
    let bad = |k: &str, what: &str| {
        MethodError::bad_params(&at(k), format!("row {i}: \"{k}\" is {what}"))
    };
    let present = |k: &str| o.get(k).filter(|v| !v.is_null());
    let mut a = AnalysisColumns::default();
    if let Some(v) = present("node") {
        a.node = Some(
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| bad("node", "a whole number from 0, or null"))?,
        );
    }
    for key in ANALYSIS_TEXT.iter() {
        if let Some(v) = present(key) {
            let text = v.as_str().ok_or_else(|| bad(key, "text"))?;
            *analysis_text_mut(&mut a, key).expect("a text member") = text.to_owned();
        }
    }
    let range = |k: &str| -> Result<Option<f64>, MethodError> {
        match present(k) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|x| x.is_finite() && *x > 0.0)
                .map(Some)
                .ok_or_else(|| bad(k, "a number of volts above 0, or null")),
        }
    };
    a.daq_range_v = range("daq_range_v")?;
    a.sensor_range_v = range("sensor_range_v")?;
    if let Some(v) = present("extra") {
        let map = v
            .as_object()
            .ok_or_else(|| bad("extra", "an object of text by column name"))?;
        for (k, v) in map {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => return Err(bad("extra", "an object of text by column name")),
            };
            a.extra.insert(k.clone(), text);
        }
    }
    let tier = match present("analysis_tier") {
        None => AnalysisTier::Full,
        Some(v) => {
            let text = v
                .as_str()
                .ok_or_else(|| bad("analysis_tier", "full, rotating or decimated:N"))?;
            AnalysisTier::from_name(text)
                .map_err(|_| bad("analysis_tier", "full, rotating or decimated:N"))?
        }
    };
    let limit = match present("limit") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad("limit", "true or false"))?,
    };
    Ok((a, tier, limit))
}

/// Validate a table against the selected station.
pub fn validate(table: &ChannelTable, ctx: &StationContext<'_>) -> Report {
    let mut report = Report::default();
    let r = table.validate_report();
    for e in r.errors {
        let message = e.to_string();
        report.errors.push(match e {
            TableError::DuplicateIndex(i) => Item::channel("duplicate_index", Some(i), message),
            TableError::EmptyEnabled => Item::channel("no_channel_enabled", None, message),
            TableError::ZeroSensitivity(i) => Item::channel("zero_sensitivity", Some(i), message),
            TableError::NonPositiveRange(i) => {
                Item::channel("non_positive_range", Some(i), message)
            }
            TableError::NonContiguousControlGroup(_) => {
                Item::channel("control_group_gap", None, message)
            }
            TableError::ReferenceWithoutControl => {
                Item::channel("reference_without_control", None, message)
            }
            TableError::UnsupportedSchemaVersion { .. } | TableError::Parse(_) => {
                Item::channel("unsupported_schema", None, message)
            }
        });
    }
    for w in r.warnings {
        let message = w.to_string();
        report.warnings.push(match w {
            TableWarning::IepeMissingBias(i) => {
                Item::channel("iepe_without_bias", Some(i), message)
            }
            TableWarning::BiasOnSingleEnded(i) => {
                Item::channel("bias_on_single_ended", Some(i), message)
            }
            TableWarning::ReferenceWithoutControl => {
                Item::channel("reference_without_control", None, message)
            }
            TableWarning::DriveChannelPresent(i) => {
                Item::channel("drive_channel_present", Some(i), message)
            }
            TableWarning::UnitUnknown(i) => Item::channel("unit_unknown", Some(i), message),
            TableWarning::DriveNotInVolts(i) => {
                Item::channel("drive_not_in_volts", Some(i), message)
            }
            TableWarning::AbortNotInG(i) => Item::channel("abort_not_in_g", Some(i), message),
        });
    }
    if let Selection::Ni { devices } = ctx.selection {
        report
            .errors
            .extend(ni_binding_errors(table, devices, ctx.scan));
    }
    report
}

/// The NI binding rules: every enabled row bound to a channel of the selected
/// devices in the right direction, no channel shared, one timing family.
pub fn ni_binding_errors(
    table: &ChannelTable,
    selected: &[String],
    scan: &[DiscoveredDevice],
) -> Vec<Item> {
    let mut errors = Vec::new();
    let mut used: Vec<(String, u32)> = Vec::new();
    let families = hertzforge_station::timing_families(scan);
    let mut row_families: Vec<String> = Vec::new();
    for c in table.channels.iter().filter(|c| c.enabled) {
        let Some(p) = &c.physical else {
            errors.push(Item::channel(
                "channel_unbound",
                Some(c.index),
                format!(
                    "channel {} is not bound to a physical channel; on NI hardware every \
                     enabled channel must be bound to one",
                    c.index
                ),
            ));
            continue;
        };
        let key = p.resolved();
        let device = scan
            .iter()
            .find(|d| d.id == p.device_id && selected.contains(&d.id));
        let has = |list: &[hertzforge_hal::DiscoveredChannel]| {
            list.iter()
                .any(|ch| channel_id(&p.device_id, &ch.id) == p.channel)
        };
        let is_input = device.is_some_and(|d| has(&d.inputs));
        let is_output = device.is_some_and(|d| has(&d.outputs));
        if !is_input && !is_output {
            errors.push(Item::channel(
                "physical_channel_unknown",
                Some(c.index),
                format!(
                    "channel {} is bound to {key}, which the selected devices do not have",
                    c.index
                ),
            ));
        } else if (c.role == ChannelRole::Drive) != is_output {
            errors.push(Item::channel(
                "physical_channel_wrong_direction",
                Some(c.index),
                format!(
                    "channel {} is {} bound to {key}, which is {}",
                    c.index,
                    if c.role == ChannelRole::Drive {
                        "a drive output"
                    } else {
                        "an input"
                    },
                    if is_output { "an output" } else { "an input" }
                ),
            ));
        }
        if let Some((_, other)) = used.iter().find(|(k, _)| k == &key) {
            errors.push(Item::channel(
                "physical_channel_shared",
                Some(c.index),
                format!(
                    "channel {} is bound to {key}, which channel {other} already uses; \
                     channels are never shared",
                    c.index
                ),
            ));
        } else {
            used.push((key, c.index));
        }
        if let Some((_, f)) = families.iter().find(|(d, _)| d == &p.device_id) {
            if !row_families.contains(f) {
                row_families.push(f.clone());
            }
        }
    }
    if row_families.len() > 1 {
        errors.push(Item::channel(
            "mixed_timing_families",
            None,
            format!(
                "the table's channels span timing families {}, which cannot share one sample \
                 clock; bind every channel in one chassis or one device",
                row_families.join(", ")
            ),
        ));
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::fixtures::{card, rig};

    fn bound(table: &mut ChannelTable, index: u32, device: &str, channel: &str) {
        let c = table
            .channels
            .iter_mut()
            .find(|c| c.index == index)
            .unwrap();
        c.physical = Some(PhysicalChannelRef::new(device, channel));
    }

    fn reasons(items: &[Item]) -> Vec<&'static str> {
        items.iter().map(|i| i.reason).collect()
    }

    fn ni_ctx<'a>(sel: &'a Selection, scan: &'a [DiscoveredDevice]) -> StationContext<'a> {
        StationContext {
            selection: sel,
            scan,
        }
    }

    #[test]
    fn the_starting_table_is_unbound_single_ended_and_valid_on_the_simulator() {
        let t = Table::new();
        assert!(t
            .table
            .channels
            .iter()
            .all(|c| c.interface == Interface::SingleEnded && c.physical.is_none()));
        let r = validate(
            &t.table,
            &StationContext {
                selection: &Selection::Sim,
                scan: &[],
            },
        );
        assert!(r.errors.is_empty(), "{r:?}");
        assert_eq!(reasons(&r.warnings), ["drive_channel_present"]);
    }

    #[test]
    fn the_wire_row_round_trips() {
        let golden: Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../schema/golden/response_channels_get.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let rows = &golden["result"]["channels"];
        let table = parse_rows(rows).unwrap();
        let back: Vec<Value> = table.channels.iter().map(row_json).collect();
        assert_eq!(
            serde_json::to_string(&back).unwrap(),
            serde_json::to_string(rows).unwrap()
        );
    }

    /// Every optional member a row may carry, each holding something.
    fn every_optional_member() -> Value {
        json!({
            "node": 101, "direction": "Z+", "channel_type": "acceleration",
            "analysis_role": "response", "daq_range_v": 5, "sensor_range_v": 4.5,
            "serial_number": "0042", "make": "Maker", "model": "M-1", "triax_dof": "Z",
            "expiration": "2027-03-01", "analysis_tier": "decimated:8", "limit": true,
            "extra": {"bay": "3", "cable": "C-12"}
        })
    }

    fn with_every_optional_member(rows: &Value) -> Value {
        Value::Array(
            rows.as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    let mut r = r.clone();
                    let o = r.as_object_mut().unwrap();
                    for (k, v) in every_optional_member().as_object().unwrap() {
                        o.insert(k.clone(), v.clone());
                    }
                    r
                })
                .collect(),
        )
    }

    fn golden_rows() -> Value {
        let golden: Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../schema/golden/response_channels_get.json"
            ))
            .unwrap(),
        )
        .unwrap();
        golden["result"]["channels"].clone()
    }

    /// A row with every optional member comes back from `channels.get` with every
    /// one, unchanged.
    ///
    /// Falsified by leaving `extra` out of `row_json`: the comparison fails on the
    /// first row.
    #[test]
    fn channel_table_wire_row_keeps_every_optional_member() {
        let rows = with_every_optional_member(&golden_rows());
        let table = parse_rows(&rows).unwrap();
        let back = Value::Array(table.channels.iter().map(row_json).collect());
        assert_eq!(back, rows);
    }

    /// A member of the wrong type is refused by name; an absent or null one is blank.
    #[test]
    fn channel_table_wire_row_refuses_a_wrong_optional_member_by_name() {
        let base = golden_rows()[0].clone();
        for (k, bad) in [
            ("node", json!(-1)),
            ("direction", json!(3)),
            ("daq_range_v", json!(0)),
            ("analysis_tier", json!("decimated:0")),
            ("limit", json!("yes")),
            ("extra", json!(["bay"])),
        ] {
            let mut row = base.clone();
            row.as_object_mut().unwrap().insert(k.into(), bad);
            let e = parse_rows(&json!([row])).unwrap_err();
            assert_eq!(
                e.detail.unwrap()["param"],
                format!("channels[0].{k}"),
                "{k}"
            );
        }
        let mut row = base.clone();
        row.as_object_mut()
            .unwrap()
            .insert("node".into(), Value::Null);
        let table = parse_rows(&json!([row])).unwrap();
        assert_eq!(row_json(&table.channels[0]), base);
    }

    /// What every module arms with, row by row: the numbers a test reads from the
    /// table. Pinned to the values today's protocol rows give, so a change to how a
    /// row is read that moves any of them fails here.
    fn arm_numbers(t: &ChannelTable) -> Vec<String> {
        t.channels
            .iter()
            .map(|c| {
                format!(
                    "{} {} {:?} sens={} eu/V={} units={} range={} {:?} {:?} bias={:?} abort={:?}/{:?}/{:?} group={:?} at={}",
                    c.index,
                    c.enabled,
                    c.role,
                    c.sensitivity_mv_per_eu,
                    c.eu_per_volt(),
                    c.units.name(),
                    c.range_max,
                    c.polarity,
                    c.interface,
                    c.iepe_bias_v,
                    c.abort.accel_g,
                    c.abort.velocity_ips,
                    c.abort.displacement_mil,
                    c.control_group,
                    c.physical_name(),
                )
            })
            .collect()
    }

    /// The analysis columns, the tier and the limit mark change nothing a test arms
    /// with: the same rows read with and without them give the same sensitivities,
    /// unit, range, polarity, interface, bias, abort limits, role and binding, and
    /// those are the values pinned below.
    ///
    /// Falsified by letting `parse_optional` set `units` from `channel_type` (or any
    /// other member): the projection of the second table differs.
    #[test]
    fn channel_table_arm_numbers_are_unchanged_by_the_optional_members() {
        let plain = parse_rows(&golden_rows()).unwrap();
        let carried = parse_rows(&with_every_optional_member(&golden_rows())).unwrap();
        assert_eq!(arm_numbers(&plain), arm_numbers(&carried));
        assert_eq!(
            arm_numbers(&plain),
            [
                "1 true Control sens=100 eu/V=10 units=g range=50 Positive SingleEnded bias=None abort=Some(40.0)/None/None group=None at=cDAQ1Mod1/ai0",
                "2 true Monitor sens=100 eu/V=10 units=g range=50 Positive SingleEnded bias=None abort=None/None/None group=None at=cDAQ1Mod1/ai1",
                "3 true Drive sens=1000 eu/V=1 units=V range=1 Positive SingleEnded bias=None abort=None/None/None group=None at=cDAQ1Mod2/ao0",
            ]
        );
        // And the validation a run's arm consults is the same for both.
        let sim = StationContext {
            selection: &Selection::Sim,
            scan: &[],
        };
        assert_eq!(validate(&plain, &sim), validate(&carried, &sim));
        // Kept across a restart, restored and then applied, the table arms with the
        // same numbers: neither the kept file nor the gate changes a value.
        let dir = std::env::temp_dir().join(format!(
            "sds-control-arm-numbers-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            crate::table_keep::path(&dir),
            crate::table_keep::contents(&carried, 5, 1_794_240_000.25),
        )
        .unwrap();
        let kept = crate::table_keep::read(&dir, &sim).unwrap().unwrap();
        assert_eq!(arm_numbers(&kept.table), arm_numbers(&plain));
        let mut t = Table {
            revision: kept.revision,
            table: kept.table,
            restored: Some(Restored {
                source: "kept",
                name: crate::table_keep::FILE.into(),
                at_unix: kept.saved_unix,
            }),
            keeper: None,
        };
        t.apply(5).unwrap();
        assert_eq!(arm_numbers(&t.table), arm_numbers(&plain));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn golden_frame(name: &str) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/../../schema/golden/{name}.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        )
        .unwrap()
    }

    /// A table taken from a project is shown as restored, refused at arm in the
    /// words of the golden refusal, and arms once applied at the revision shown.
    ///
    /// Falsified by making `arm_gate` answer `Ok(())` always: the first assertion
    /// fails with "a restored table armed".
    #[test]
    fn channel_table_gate_refuses_a_restored_table_until_it_is_applied() {
        let set = golden_frame("request_channels_set_restored");
        let mut t = Table {
            revision: 4,
            table: parse_rows(&set["params"]["channels"]).unwrap(),
            restored: Some(Restored {
                source: "project",
                name: "Bracket table".into(),
                at_unix: 1_794_240_000.25,
            }),
            keeper: None,
        };
        let e = t.arm_gate().expect_err("a restored table armed");
        let want = golden_frame("response_error_table_not_applied")["error"].clone();
        assert_eq!(e.code, ErrorCode::Refused);
        assert_eq!(Some(e.reason.unwrap()), want["reason"].as_str());
        assert_eq!(e.message, want["message"].as_str().unwrap());
        assert_eq!(*e.detail.unwrap(), want["detail"]);
        let sim = StationContext {
            selection: &Selection::Sim,
            scan: &[],
        };
        let got = t.get(&sim);
        let shown = golden_frame("response_channels_get_restored")["result"].clone();
        assert_eq!(got["restored"], shown["restored"]);
        assert_eq!(got["revision"], shown["revision"]);
        // Applying names the revision the operator was shown.
        assert_eq!(t.apply(3).unwrap_err().reason, Some("stale_revision"));
        assert!(t.arm_gate().is_err(), "a stale apply applied");
        let was = t.apply(4).unwrap();
        assert_eq!(
            was.to_json(),
            golden_frame("response_channels_apply")["result"]["applied"]
        );
        t.arm_gate().expect("an applied table arms");
        assert!(t.get(&sim).get("restored").is_none());
        assert_eq!(t.apply(4).unwrap_err().reason, Some("nothing_to_apply"));
    }

    /// A unit outside the vocabulary, a drive not in volts and an abort limit on a
    /// row not in g come through as warnings by name, and never as errors.
    #[test]
    fn channel_table_unit_warnings_come_through_by_name() {
        let mut t = Table::new().table;
        t.channels[0].units = Units::Custom("A".into());
        t.channels[0].abort.accel_g = Some(5.0);
        t.channels[1].units = Units::AccelG;
        let r = validate(
            &t,
            &StationContext {
                selection: &Selection::Sim,
                scan: &[],
            },
        );
        assert!(r.errors.is_empty(), "{r:?}");
        let got = reasons(&r.warnings);
        for want in ["unit_unknown", "abort_not_in_g", "drive_not_in_volts"] {
            assert!(got.contains(&want), "{want} in {got:?}");
        }
    }

    /// The round trip's control leg: the rows of the golden `tables.save`
    /// request (every optional member, on every row) read as the service reads
    /// `channels.set` and written as `channels.get` writes them are the same
    /// rows. The shell's and the analysis side's tests read the same file, so a
    /// row goes from this table through the wire, the shell and a `.hforge` file
    /// and back unchanged.
    #[test]
    fn channel_table_round_trip_golden_rows_are_the_services_own() {
        let golden: Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../schema/golden/request_tables_save_channels.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let rows = &golden["params"]["channels"];
        let table = parse_rows(rows).unwrap();
        let back = Value::Array(table.channels.iter().map(row_json).collect());
        assert_eq!(&back, rows);
        let sim = StationContext {
            selection: &Selection::Sim,
            scan: &[],
        };
        assert!(validate(&table, &sim).errors.is_empty());
    }

    /// The one rule set, as the golden table file holds it: what the control
    /// service answers each case with, and that every vocabulary unit is known.
    ///
    /// Falsified by letting a disabled row's 0 sensitivity count (dropping the
    /// `ch.enabled` gate in `validate_report`): the case "sensitivity 0 on a
    /// disabled row" answers `table_invalid`.
    #[test]
    fn channel_table_rules_golden_holds_in_rust() {
        let golden: Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../schema/golden/channel_table_rules.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let sim = StationContext {
            selection: &Selection::Sim,
            scan: &[],
        };
        for case in golden["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let control = case["control"].as_str().unwrap();
            let table = match parse_rows(&case["rows"]) {
                Err(e) => {
                    assert_eq!(e.reason, Some("bad_params"), "{name}");
                    assert_eq!(control, "bad_params", "{name}");
                    continue;
                }
                Ok(t) => t,
            };
            let r = validate(&table, &sim);
            let errors = reasons(&r.errors);
            let want: Vec<&str> = case["errors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert_eq!(errors, want, "{name}");
            let answer = if errors.is_empty() {
                "ok"
            } else {
                "table_invalid"
            };
            assert_eq!(answer, control, "{name}");
            let warnings = reasons(&r.warnings);
            for w in case["warnings"].as_array().unwrap() {
                let w = w.as_str().unwrap();
                assert!(warnings.contains(&w), "{name}: {w} in {warnings:?}");
            }
        }
        let vocabulary: Vec<&str> = golden["unit_vocabulary"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(vocabulary, hertzforge_channel_table::UNIT_VOCABULARY);
        for unit in vocabulary {
            let mut row = new_row(1);
            row.units = Units::from_name(unit);
            let mut t = Table::new().table;
            t.channels.push(Channel { index: 3, ..row });
            let r = validate(&t, &sim);
            assert!(!reasons(&r.warnings).contains(&"unit_unknown"), "{unit}");
        }
    }

    #[test]
    fn a_missing_field_names_the_parameter() {
        let mut row = row_json(&new_row(1));
        row.as_object_mut().unwrap().remove("polarity");
        let e = parse_rows(&json!([row])).unwrap_err();
        assert_eq!(e.reason, Some("bad_params"));
        assert_eq!(e.detail.unwrap()["param"], "channels[0].polarity");
    }

    #[test]
    fn the_controllers_table_errors_come_through_by_name() {
        let mut t = Table::new().table;
        t.channels[0].sensitivity_mv_per_eu = 0.0;
        t.channels.push(new_row(1));
        let r = validate(
            &t,
            &StationContext {
                selection: &Selection::Sim,
                scan: &[],
            },
        );
        let got = reasons(&r.errors);
        assert!(
            got.contains(&"duplicate_index") && got.contains(&"zero_sensitivity"),
            "{got:?}"
        );
    }

    #[test]
    fn on_ni_every_row_binds_to_a_selected_channel_in_its_direction_once() {
        let scan = rig(false);
        let sel = Selection::Ni {
            devices: vec!["cDAQ2Mod1".into(), "cDAQ2Mod2".into()],
        };
        let mut t = Table::new().table;
        // Unbound rows are refused on NI.
        let r = validate(&t, &ni_ctx(&sel, &scan));
        assert_eq!(reasons(&r.errors), ["channel_unbound", "channel_unbound"]);

        // Correctly bound: control on ai0, drive on ao0.
        bound(&mut t, 1, "cDAQ2Mod1", "ai0");
        bound(&mut t, 2, "cDAQ2Mod2", "ao0");
        let r = validate(&t, &ni_ctx(&sel, &scan));
        assert!(r.errors.is_empty(), "{:?}", r.errors);

        // The drive on an input is the wrong direction.
        bound(&mut t, 2, "cDAQ2Mod1", "ai1");
        let r = validate(&t, &ni_ctx(&sel, &scan));
        assert_eq!(reasons(&r.errors), ["physical_channel_wrong_direction"]);

        // A channel the selected devices do not have.
        bound(&mut t, 2, "cDAQ2Mod2", "ao7");
        let r = validate(&t, &ni_ctx(&sel, &scan));
        assert_eq!(reasons(&r.errors), ["physical_channel_unknown"]);

        // Two rows on one channel.
        bound(&mut t, 2, "cDAQ2Mod2", "ao0");
        let mut monitor = new_row(3);
        monitor.physical = Some(PhysicalChannelRef::new("cDAQ2Mod1", "ai0"));
        t.channels.push(monitor);
        let r = validate(&t, &ni_ctx(&sel, &scan));
        assert_eq!(reasons(&r.errors), ["physical_channel_shared"]);
    }

    #[test]
    fn rows_across_two_timing_families_are_refused() {
        let mut scan = rig(false);
        scan.push(card(false));
        let sel = Selection::Ni {
            devices: vec!["cDAQ2Mod1".into(), "Dev1".into()],
        };
        let mut t = Table::new().table;
        bound(&mut t, 1, "cDAQ2Mod1", "ai0");
        bound(&mut t, 2, "Dev1", "ao0");
        let r = validate(&t, &ni_ctx(&sel, &scan));
        assert_eq!(reasons(&r.errors), ["mixed_timing_families"]);
    }

    #[test]
    fn the_simulator_keeps_bindings_but_does_not_check_them() {
        let mut t = Table::new().table;
        bound(&mut t, 1, "nowhere", "ai9");
        let r = validate(
            &t,
            &StationContext {
                selection: &Selection::Sim,
                scan: &[],
            },
        );
        assert!(r.errors.is_empty());
    }
}
