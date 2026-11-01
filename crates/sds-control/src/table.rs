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

use crate::protocol::{num, opt_num, MethodError};
use crate::station::{channel_id, Selection};
use hertzforge_channel_table::{
    AbortLimits, AnalysisTier, Channel, ChannelRole, ChannelTable, Interface, PhysicalChannelRef,
    Polarity, TableError, TableWarning, Units,
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

/// The table and its revision.
pub struct Table {
    pub revision: u64,
    pub table: ChannelTable,
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
        Table { revision: 1, table }
    }

    /// The `channels.get` result.
    pub fn get(&self, ctx: &StationContext<'_>) -> Value {
        json!({
            "revision": self.revision,
            "channels": self.table.channels.iter().map(row_json).collect::<Vec<_>>(),
            "report": validate(&self.table, ctx).to_json(),
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
    match u {
        Units::AccelG => "g".into(),
        Units::VelocityIps => "in/s".into(),
        Units::DisplacementMil => "mil".into(),
        Units::ForceLbf => "lbf".into(),
        Units::VoltageV => "V".into(),
        Units::Custom(s) => s.clone(),
    }
}

fn unit_of(s: &str) -> Units {
    match s {
        "g" => Units::AccelG,
        "in/s" => Units::VelocityIps,
        "mil" => Units::DisplacementMil,
        "lbf" => Units::ForceLbf,
        "V" => Units::VoltageV,
        other => Units::Custom(other.to_owned()),
    }
}

pub fn role_name(r: ChannelRole) -> &'static str {
    match r {
        ChannelRole::Control => "control",
        ChannelRole::Monitor => "monitor",
        ChannelRole::Drive => "drive",
        ChannelRole::Reference => "reference",
    }
}

/// A row on the wire, every field present.
pub fn row_json(c: &Channel) -> Value {
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
    Ok(Channel {
        index,
        enabled,
        save: true,
        stream: false,
        interface,
        iepe_bias_v,
        units: unit_of(units),
        sensitivity_mv_per_eu: num("sensitivity_mv_per_eu")?,
        range_max: num("range_max")?,
        polarity,
        description,
        role,
        control_group,
        abort,
        physical,
        analysis_tier: AnalysisTier::Full,
    })
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
            TableWarning::ReferenceCountMismatch { .. } => {
                Item::channel("reference_count_mismatch", None, message)
            }
            TableWarning::DriveChannelPresent(i) => {
                Item::channel("drive_channel_present", Some(i), message)
            }
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
                     enabled channel is",
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
