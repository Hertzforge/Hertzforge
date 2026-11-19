//! A Playback configuration on the wire (schema/PROTOCOL.md, "Playback Configuration"):
//! its shape, its fields' checks against the table, and the drive file's check, which
//! runs off the service's lock on a worker thread.

use crate::protocol::{num, ErrorCode, MethodError};
use crate::shock::config::Obj;
use crate::table::{Item, Report};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_playback::plan::{
    self, LEVEL_MAX_DB, LEVEL_MIN_DB, ROLL_MAX_S, STOP_RAMP_DEPTH_DB, STOP_RAMP_S,
};
use hertzforge_playback::{
    check_file, DriveFile, DriveRow, FileCheck, FileError, PlaySetup, StationOffer,
};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// The run folder's name when the setup gives none.
pub const DEFAULT_RUN_NAME: &str = "playback";
/// The rates a station is asked for, Hz.
pub const RATE_MIN_HZ: f64 = 1000.0;
pub const RATE_MAX_HZ: f64 = 204_800.0;
/// The ceiling's top, volts.
pub const CEILING_MAX_V: f64 = 10.0;

/// A configuration as accepted.
#[derive(Debug, Clone, PartialEq)]
pub struct WirePlayback {
    pub raw: Value,
    pub title: String,
    pub folder: PathBuf,
    pub sha256: String,
    pub map: Vec<u32>,
    pub level_db: f64,
    pub ceiling_v: f64,
    pub pre_roll_s: f64,
    pub post_roll_s: f64,
    pub run_name: String,
    pub metadata: Vec<(String, String, String)>,
    pub operator: String,
    pub article: String,
    pub notes: String,
    pub specification: Option<(String, String)>,
    pub record_streams: bool,
}

fn bad(path: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(path, message)
}

fn only(o: &Obj<'_>, keys: &[&str]) -> Result<(), MethodError> {
    match o.m.keys().find(|k| !keys.contains(&k.as_str())) {
        None => Ok(()),
        Some(k) => Err(bad(
            &o.at(k),
            format!(
                "\"{}\" is not a field of the configuration; {} takes {}",
                o.at(k),
                o.path,
                keys.join(", ")
            ),
        )),
    }
}

/// Parse `config`: its shape only. A key it does not have is refused `bad_params`.
pub fn parse(config: &Value) -> Result<WirePlayback, MethodError> {
    let c = Obj::of(config, "config")?;
    only(
        &c,
        &[
            "title",
            "drive",
            "map",
            "level_db",
            "ceiling_v",
            "pre_roll_s",
            "post_roll_s",
            "capture",
            "metadata",
            "operator",
            "article",
            "notes",
            "specification",
            "record",
        ],
    )?;
    let d = c.obj("drive")?;
    only(&d, &["folder", "sha256"])?;
    let folder = d
        .opt_str("folder")?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("config.drive.folder", "the drive file's folder is needed"))?;
    let sha256 = d
        .opt_str("sha256")?
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| {
            bad(
                "config.drive.sha256",
                "the drive file's SHA-256 (64 hexadecimal digits) is needed, so a file changed \
                 since it was chosen is refused",
            )
        })?
        .to_ascii_lowercase();
    let map = c
        .array("map")?
        .ok_or_else(|| {
            bad(
                "config.map",
                "the map of drive file channels to drive rows is needed",
            )
        })?
        .iter()
        .enumerate()
        .map(|(i, v)| {
            v.as_u64()
                .and_then(|x| u32::try_from(x).ok())
                .ok_or_else(|| bad(&format!("config.map[{i}]"), "a drive row's table index"))
        })
        .collect::<Result<Vec<u32>, _>>()?;
    let level_db = c.f64_or("level_db", -12.0)?;
    let ceiling_v = c.f64("ceiling_v")?;
    let pre_roll_s = c.f64_or("pre_roll_s", 0.5)?;
    let post_roll_s = c.f64_or("post_roll_s", 2.0)?;
    let mut run_name = DEFAULT_RUN_NAME.to_owned();
    if let Some(o) = c.opt_obj("capture")? {
        only(&o, &["run_name"])?;
        if let Some(n) = o.opt_str("run_name")? {
            run_name = n.to_owned();
        }
    }
    let metadata = match c.array("metadata")? {
        None => Vec::new(),
        Some(list) => {
            let mut out = Vec::with_capacity(list.len());
            for (i, m) in list.iter().enumerate() {
                let o = Obj::of(m, &format!("config.metadata[{i}]"))?;
                only(&o, &["key", "label", "value"])?;
                let key = o
                    .opt_str("key")?
                    .ok_or_else(|| bad(&o.at("key"), "\"key\" is text"))?
                    .to_owned();
                let label = o.opt_str("label")?.unwrap_or(&key).to_owned();
                let value = o
                    .opt_str("value")?
                    .ok_or_else(|| bad(&o.at("value"), "\"value\" is text"))?
                    .to_owned();
                out.push((key, label, value));
            }
            out
        }
    };
    let words =
        |k: &str| -> Result<String, MethodError> { Ok(c.opt_str(k)?.unwrap_or("").to_owned()) };
    let specification = match c.m.get("specification") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let o = Obj::of(v, "config.specification")?;
            only(&o, &["document", "clause"])?;
            Some((
                o.opt_str("document")?.unwrap_or("").to_owned(),
                o.opt_str("clause")?.unwrap_or("").to_owned(),
            ))
        }
    };
    let record_streams = match c.opt_obj("record")? {
        None => true,
        Some(o) => {
            only(&o, &["streams"])?;
            o.opt_bool("streams")?.unwrap_or(true)
        }
    };
    Ok(WirePlayback {
        raw: config.clone(),
        title: words("title")?,
        folder: PathBuf::from(folder),
        sha256,
        map,
        level_db,
        ceiling_v,
        pre_roll_s,
        post_roll_s,
        run_name,
        metadata,
        operator: words("operator")?,
        article: words("article")?,
        notes: words("notes")?,
        specification,
        record_streams,
    })
}

/// The table's enabled drive rows, in the order the station plays them.
pub fn drive_rows(table: &ChannelTable) -> Vec<DriveRow> {
    table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role == ChannelRole::Drive)
        .map(|c| DriveRow {
            index: c.index,
            range_max: c.range_max,
        })
        .collect()
}

impl WirePlayback {
    pub fn setup(&self) -> PlaySetup {
        PlaySetup {
            level_db: self.level_db,
            ceiling_v: self.ceiling_v,
            pre_roll_s: self.pre_roll_s,
            post_roll_s: self.post_roll_s,
            map: self.map.clone(),
        }
    }

    /// The fields' checks that need no file: ranges, the run name, an input to record.
    pub fn check_fields(&self, table: &ChannelTable) -> Report {
        let mut r = Report::default();
        if !(LEVEL_MIN_DB..=LEVEL_MAX_DB).contains(&self.level_db) {
            r.errors.push(Item::field(
                "out_of_range",
                "level_db",
                format!(
                    "The level is {} dB; it is {LEVEL_MIN_DB} to {LEVEL_MAX_DB} dB.",
                    self.level_db
                ),
            ));
        }
        if !(self.ceiling_v > 0.0 && self.ceiling_v <= CEILING_MAX_V) {
            r.errors.push(Item::field(
                "out_of_range",
                "ceiling_v",
                format!(
                    "The drive ceiling is {} V; it is above 0 and at most {CEILING_MAX_V} V.",
                    self.ceiling_v
                ),
            ));
        }
        for (k, v) in [
            ("pre_roll_s", self.pre_roll_s),
            ("post_roll_s", self.post_roll_s),
        ] {
            if !(0.0..=ROLL_MAX_S).contains(&v) {
                r.errors.push(Item::field(
                    "out_of_range",
                    k,
                    format!("{k} is {v} s; it is 0 to {ROLL_MAX_S} s."),
                ));
            }
        }
        if self.run_name.is_empty()
            || self.run_name.contains(['/', '\\'])
            || self.run_name == "."
            || self.run_name == ".."
        {
            r.errors.push(Item::field(
                "out_of_range",
                "capture.run_name",
                "The run folder's name is a plain name with no path in it.",
            ));
        }
        if !table
            .channels
            .iter()
            .any(|c| c.enabled && c.role != ChannelRole::Drive)
        {
            r.errors.push(Item::channel(
                "no_input_channel",
                None,
                "The channel table has no enabled input, so nothing would be recorded.",
            ));
        }
        if drive_rows(table).is_empty() {
            r.errors.push(Item::channel(
                "no_drive_channel",
                None,
                "The channel table has no enabled drive row to play the file on.",
            ));
        }
        r
    }
}

/// The file opened and checked: the slow part, run on a worker thread.
pub fn check_drive(w: &WirePlayback) -> Result<(DriveFile, FileCheck), FileError> {
    let file = DriveFile::open(&w.folder)?;
    let check = check_file(&file, Some(&w.sha256))?;
    Ok((file, check))
}

/// The field a check's refusal points at.
fn field_of(reason: &str) -> &'static str {
    match reason {
        "map_length" | "map_missing_row" | "map_repeated_row" | "too_many_drives" => "map",
        "peak_over_ceiling" => "level_db",
        "ceiling_over_range" => "ceiling_v",
        "out_of_range" => "level_db",
        _ => "drive",
    }
}

/// The refusals as report items, by the field they point at.
pub fn items(refusals: &[FileError]) -> Vec<Item> {
    refusals
        .iter()
        .map(|r| Item::field(r.reason, field_of(r.reason), r.message.clone()))
        .collect()
}

/// What the station offers at configure: any rate on the simulator, the rate checked
/// again at start on hardware; as many drives as the table has drive rows.
pub fn offer(table: &ChannelTable) -> StationOffer {
    StationOffer {
        rate_hz: None,
        nearest_hz: Vec::new(),
        outputs: drive_rows(table).len(),
    }
}

/// The file's rate against the rates a station is asked for.
pub fn rate_item(file: &DriveFile) -> Option<Item> {
    (!(RATE_MIN_HZ..=RATE_MAX_HZ).contains(&file.rate_hz)).then(|| {
        Item::field(
            "rate_mismatch",
            "drive",
            format!(
                "The drive file is at {} Hz; a station runs {RATE_MIN_HZ} to {RATE_MAX_HZ} Hz, \
                 and a file is never resampled.",
                file.rate_hz
            ),
        )
    })
}

/// The check report with each channel's preview envelopes.
pub fn report_with_previews(
    file: &DriveFile,
    check: &FileCheck,
    setup: &PlaySetup,
    refusals: &[FileError],
) -> Value {
    let mut r = plan::report_json(file, check, setup, refusals);
    if let Value::Object(m) = &mut r {
        m.insert(
            "previews".into(),
            Value::Array(
                check
                    .channels
                    .iter()
                    .map(|c| c.envelopes_json(file.rate_hz))
                    .collect(),
            ),
        );
    }
    r
}

/// The derived values: the grid, the run's length and the stop.
pub fn derived(w: &WirePlayback, file: &DriveFile) -> Value {
    let fs = file.rate_hz;
    json!({
        "fs_hz": num(fs),
        "block_frames": hertzforge_hal::default_block_frames(fs),
        "samples": file.samples,
        "channels": file.channels.len(),
        "duration_s": num(file.duration_s()),
        "pre_roll_frames": (w.pre_roll_s * fs).round() as u64,
        "post_roll_frames": (w.post_roll_s * fs).round() as u64,
        "total_s": num(w.pre_roll_s + file.duration_s() + w.post_roll_s),
        "gain": num(w.setup().gain()),
        "stop": {"depth_db": num(STOP_RAMP_DEPTH_DB), "ramp_s": num(STOP_RAMP_S)},
        "read_ahead_s": num(2.0),
    })
}

/// The refusal of a configuration with errors: the items, and the check report when
/// the file was checked.
pub fn config_invalid(report: &Report, check: Option<&Value>) -> MethodError {
    let n = report.errors.len();
    let mut detail = report.to_json();
    if let (Value::Object(m), Some(c)) = (&mut detail, check) {
        m.insert("check".into(), c.clone());
    }
    MethodError::named(
        ErrorCode::BadRequest,
        "config_invalid",
        format!(
            "The configuration has {n} error{}: {}",
            if n == 1 { "" } else { "s" },
            report
                .errors
                .iter()
                .map(|i| i.message.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        ),
    )
    .with_detail(detail)
}

/// The sidecar's members a run's record keeps beside the check.
pub fn sidecar_json(file: &DriveFile) -> Value {
    let mut m = Map::new();
    m.insert(
        "folder".into(),
        Value::from(file.folder.display().to_string()),
    );
    m.insert("sidecar".into(), file.sidecar.clone());
    Value::Object(m)
}
