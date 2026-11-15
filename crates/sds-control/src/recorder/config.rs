//! The Recorder configuration on the wire (schema/PROTOCOL.md, "Recorder
//! Configuration"): parsing (types and structure; a wrong shape, a missing required
//! field or a key the wire does not have is `bad_params` naming the field), the checks
//! (content; every problem a named item located by `field` or `channel`), the derived
//! values, and the core's own `RecorderConfig` built from it with every field supplied.

use crate::explorer::config::{NI_GROUP_DELAY_FIXED_S, NI_GROUP_DELAY_SAMPLES};
use crate::protocol::{num, opt_num, MethodError};
use crate::shock::config::Obj;
use crate::table::{Item, Report};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_recorder::analysis::hop_frames;
use hertzforge_recorder::service::config::default_metadata;
use hertzforge_recorder::trigger::{DEFAULT_PRETRIGGER_S, MAX_PRETRIGGER_S};
use hertzforge_recorder::{
    AcquisitionConfig, AveragingKind, LevelTrigger, MetaField, RecorderConfig, Slope, TriggerMode,
    WindowKind,
};
use serde_json::{json, Map, Value};

/// Accepted requested rates, Hz.
pub const FS_RANGE: (f64, f64) = (1000.0, 204_800.0);
/// The rate asked when the configuration names none, Hz.
pub const DEFAULT_FS_HZ: f64 = 12_800.0;
/// Accepted analysis blocks (powers of two).
pub const BLOCK_RANGE: (u64, u64) = (256, 65_536);
/// Accepted analysis decimations (powers of two).
pub const DECIMATION_RANGE: (u64, u64) = (1, 256);
/// Accepted overlaps, percent.
pub const OVERLAP_RANGE: (f64, f64) = (0.0, 95.0);
/// The least free space a run leaves, GiB.
pub const MIN_KEEP_FREE_GB: f64 = 1.0;
/// Bytes in a GiB.
pub const GIB: f64 = 1_073_741_824.0;
/// Bytes a recorded row takes a frame.
pub const BYTES_PER_SAMPLE: u64 = 4;
/// Blocks in a batch: the most the service reads between two verbs.
pub const BLOCKS_PER_BATCH: usize = 2;
/// The run folder's name when none is given.
pub const DEFAULT_RUN_NAME: &str = "recorder";
/// The overload indicator's level, as a fraction of a row's `range_max`.
pub const OVERLOAD_FRACTION: f64 = 0.9;

/// The station a run would start on, for the derived values before it is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderStation {
    /// The simulator: no group delay.
    Sim,
    /// An NI station: the input path's documented group delay.
    Ni,
}

impl RecorderStation {
    /// The input group delay at `fs_hz`, frames, to 0.01.
    pub fn group_delay_frames(&self, fs_hz: f64) -> f64 {
        match self {
            RecorderStation::Sim => 0.0,
            RecorderStation::Ni => {
                round(NI_GROUP_DELAY_SAMPLES + NI_GROUP_DELAY_FIXED_S * fs_hz, 2)
            }
        }
    }
}

/// What starts a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Start,
    Manual,
    Level,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Start => "start",
            Mode::Manual => "manual",
            Mode::Level => "level",
        }
    }
}

/// The wire's names for the core's windows.
pub const WINDOWS: [(&str, WindowKind); 4] = [
    ("hann", WindowKind::Hann),
    ("flattop", WindowKind::Flattop),
    ("rectangular", WindowKind::Rectangular),
    ("blackman_harris", WindowKind::BlackmanHarris),
];

/// The wire's names for the core's averaging.
pub const AVERAGING: [(&str, AveragingKind); 3] = [
    ("linear", AveragingKind::Linear),
    ("exponential", AveragingKind::Exponential),
    ("peak_hold", AveragingKind::PeakHold),
];

/// The wire's names for the core's slopes.
pub const SLOPES: [(&str, Slope); 2] = [("rising", Slope::Rising), ("falling", Slope::Falling)];

/// The wire's trigger modes.
pub const MODES: [(&str, Mode); 3] = [
    ("start", Mode::Start),
    ("manual", Mode::Manual),
    ("level", Mode::Level),
];

/// The trigger part of a configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct WireTrigger {
    pub mode: Mode,
    /// The level row's `index` (level mode).
    pub row: Option<u32>,
    pub level: Option<f64>,
    pub slope: Slope,
    /// Null: 1% of the row's `range_max`.
    pub hysteresis: Option<f64>,
    pub pre_s: f64,
    /// Null: until cut or the run's end.
    pub record_s: Option<f64>,
    /// Null: until the run ends.
    pub count: Option<u64>,
    pub holdoff_s: f64,
}

/// The analysis part of a configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct WireAnalysis {
    pub block: u64,
    pub window: WindowKind,
    /// Null: the window's own.
    pub overlap_pct: Option<f64>,
    pub averaging: AveragingKind,
    pub averages: u64,
    pub decimation: u64,
    /// `(reference, response)` rows by `index`.
    pub frf: Option<(u32, u32)>,
}

/// A parsed Recorder configuration, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct WireRecorder {
    pub title: String,
    pub fs_hz: f64,
    pub trigger: WireTrigger,
    pub analysis: WireAnalysis,
    pub run_name: String,
    pub keep_free_gb: f64,
    pub metadata: Vec<MetaField>,
    /// The operator's words for the run's record, as Random's: who ran it, on what,
    /// notes, and the specification it was run to.
    pub operator: String,
    pub article: String,
    pub notes: String,
    pub specification: Option<(String, String)>,
    /// The configuration as sent, for `recorder.config` and `recorder.reset`.
    pub raw: Value,
}

/// Everything a configuration's checks are made against.
pub struct CheckContext<'a> {
    /// The channel table the run would start with.
    pub table: &'a ChannelTable,
    /// The station selected.
    pub station: RecorderStation,
    /// The data volume's free bytes now, if it can be read.
    pub free_bytes: Option<u64>,
}

/// What configure computes from a configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub fs_hz: f64,
    pub block_frames: usize,
    pub batch_frames: usize,
    pub group_delay_frames: f64,
    /// True once the run has started and the figures are the station's.
    pub actual: bool,
    /// The recorded rows' `index`es, in column order.
    pub rows: Vec<u32>,
    pub analysis_fs_hz: f64,
    pub df_hz: f64,
    pub lines: usize,
    pub span_hz: f64,
    pub block_s: f64,
    pub overlap_pct: f64,
    pub averages_s: f64,
    pub pre_frames: u64,
    pub record_frames: Option<u64>,
    pub holdoff_frames: u64,
    pub bytes_per_s: u64,
    pub floor_bytes: u64,
    pub free_bytes: Option<u64>,
}

impl Derived {
    /// Whole seconds of recording that fit above the floor now, if the free space
    /// is known.
    pub fn seconds_above_floor(&self) -> Option<u64> {
        seconds_above_floor(self.free_bytes, self.floor_bytes, self.bytes_per_s)
    }

    pub fn grid_json(&self) -> Value {
        json!({
            "fs_hz": num(self.fs_hz),
            "block_frames": self.block_frames,
            "batch_frames": self.batch_frames,
            "group_delay_frames": num(self.group_delay_frames),
            "actual": self.actual,
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "grid": self.grid_json(),
            "outputs": "none",
            "rows": self.rows,
            "analysis": {
                "fs_hz": num(round(self.analysis_fs_hz, 6)),
                "df_hz": num(round(self.df_hz, 6)),
                "lines": self.lines,
                "span_hz": num(round(self.span_hz, 6)),
                "block_s": num(round(self.block_s, 6)),
                "overlap_pct": num(self.overlap_pct),
                "averages_s": num(round(self.averages_s, 6)),
            },
            "trigger": {
                "pre_frames": self.pre_frames,
                "record_frames": self.record_frames,
                "holdoff_frames": self.holdoff_frames,
            },
            "disk": {
                "bytes_per_s": self.bytes_per_s,
                "floor_bytes": self.floor_bytes,
                "free_bytes": self.free_bytes,
                "seconds_above_floor": self.seconds_above_floor(),
            },
        })
    }
}

pub fn round(x: f64, digits: i32) -> f64 {
    let p = 10f64.powi(digits);
    (x * p).round() / p
}

/// Whole seconds of recording above `floor_bytes` in `free_bytes`, at `bytes_per_s`.
pub fn seconds_above_floor(
    free_bytes: Option<u64>,
    floor_bytes: u64,
    bytes_per_s: u64,
) -> Option<u64> {
    let free = free_bytes?;
    Some(free.saturating_sub(floor_bytes) / bytes_per_s.max(1))
}

/// Frames at `fs_hz` for `seconds`: `round(seconds x fs_hz)`.
pub fn frames_for(seconds: f64, fs_hz: f64) -> u64 {
    (seconds * fs_hz).round().max(0.0) as u64
}

/// The free-space floor: the larger of `keep_free_gb` and twice one recording's bytes
/// (its pre-window included) when recordings are timed.
pub fn floor_bytes(keep_free_gb: f64, recording_frames: Option<u64>, n_rows: usize) -> u64 {
    let keep = (keep_free_gb * GIB).round().max(0.0) as u64;
    let twice = recording_frames.map_or(0, |f| 2 * f * n_rows as u64 * BYTES_PER_SAMPLE);
    keep.max(twice)
}

/// The recorded rows: every enabled input row, in table order.
pub fn recorded_rows(table: &ChannelTable) -> Vec<&Channel> {
    table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .collect()
}

// ---------------------------------------------------------------------------------
// Parsing: types and structure. Content problems are items, not parse errors.
// ---------------------------------------------------------------------------------

fn bad(path: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(path, message)
}

/// Refuse any key of `o` not in `keys`, by its path.
fn only(o: &Obj<'_>, keys: &[&str]) -> Result<(), MethodError> {
    match o.m.keys().find(|k| !keys.contains(&k.as_str())) {
        None => Ok(()),
        Some(k) => Err(bad(
            &o.at(k),
            format!(
                "\"{}\" is not a field of the configuration; {} takes {}",
                o.at(k),
                if o.path == "config" {
                    "it".to_owned()
                } else {
                    format!("\"{}\"", o.path)
                },
                keys.iter()
                    .map(|k| format!("\"{k}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// A row's `index`: a whole number from 0.
fn opt_index(o: &Obj<'_>, k: &str) -> Result<Option<u32>, MethodError> {
    match o.opt_count(k)? {
        None => Ok(None),
        Some(n) => u32::try_from(n)
            .map(Some)
            .map_err(|_| bad(&o.at(k), format!("\"{}\" is a row's index", o.at(k)))),
    }
}

fn parse_trigger(c: &Obj<'_>) -> Result<WireTrigger, MethodError> {
    let mut t = WireTrigger {
        mode: Mode::Manual,
        row: None,
        level: None,
        slope: Slope::Rising,
        hysteresis: None,
        pre_s: DEFAULT_PRETRIGGER_S,
        record_s: None,
        count: Some(1),
        holdoff_s: 0.0,
    };
    let Some(o) = c.opt_obj("trigger")? else {
        return Ok(t);
    };
    only(
        &o,
        &[
            "mode",
            "row",
            "level",
            "slope",
            "hysteresis",
            "pre_s",
            "record_s",
            "count",
            "holdoff_s",
        ],
    )?;
    t.mode = o.choice("mode", &MODES, Some(Mode::Manual))?;
    t.row = opt_index(&o, "row")?;
    t.level = o.opt_f64("level")?;
    t.slope = o.choice("slope", &SLOPES, Some(Slope::Rising))?;
    t.hysteresis = o.opt_f64("hysteresis")?;
    t.pre_s = o.f64_or("pre_s", DEFAULT_PRETRIGGER_S)?;
    t.record_s = o.opt_f64("record_s")?;
    t.count = match o.m.get("count") {
        None => Some(1),
        Some(Value::Null) => None,
        Some(_) => o.opt_count("count")?,
    };
    t.holdoff_s = o.f64_or("holdoff_s", 0.0)?;
    if t.mode == Mode::Level {
        // A level trigger needs its row, its level and its length: missing, they are
        // the request's shape, not its content.
        for (k, missing) in [
            ("row", t.row.is_none()),
            ("level", t.level.is_none()),
            ("record_s", t.record_s.is_none()),
        ] {
            if missing {
                return Err(bad(
                    &o.at(k),
                    format!(
                        "a level trigger needs \"{}\": {}",
                        o.at(k),
                        match k {
                            "row" => "the row the level is read on",
                            "level" => "the level, in the row's unit",
                            _ => "each recording's length, so the trigger can re-arm",
                        }
                    ),
                ));
            }
        }
    }
    Ok(t)
}

fn parse_analysis(c: &Obj<'_>) -> Result<WireAnalysis, MethodError> {
    let core = AcquisitionConfig::default();
    let mut a = WireAnalysis {
        block: core.block_size as u64,
        window: core.window,
        overlap_pct: None,
        averaging: core.averaging,
        averages: core.average_count as u64,
        decimation: u64::from(core.analysis_decimation),
        frf: None,
    };
    let Some(o) = c.opt_obj("analysis")? else {
        return Ok(a);
    };
    only(
        &o,
        &[
            "block",
            "window",
            "overlap_pct",
            "averaging",
            "averages",
            "decimation",
            "frf",
        ],
    )?;
    a.block = o.opt_count("block")?.unwrap_or(a.block);
    a.window = o.choice("window", &WINDOWS, Some(a.window))?;
    a.overlap_pct = o.opt_f64("overlap_pct")?;
    a.averaging = o.choice("averaging", &AVERAGING, Some(a.averaging))?;
    a.averages = o.opt_count("averages")?.unwrap_or(a.averages);
    a.decimation = o.opt_count("decimation")?.unwrap_or(a.decimation);
    if let Some(f) = o.opt_obj("frf")? {
        only(&f, &["reference", "response"])?;
        let need = |k: &str| -> Result<u32, MethodError> {
            opt_index(&f, k)?.ok_or_else(|| {
                bad(
                    &f.at(k),
                    format!("an FRF pair needs \"{}\", a row's index", f.at(k)),
                )
            })
        };
        a.frf = Some((need("reference")?, need("response")?));
    }
    Ok(a)
}

pub fn parse(config: &Value) -> Result<WireRecorder, MethodError> {
    let c = Obj::of(config, "config")?;
    only(
        &c,
        &[
            "title",
            "acquisition",
            "trigger",
            "analysis",
            "capture",
            "metadata",
            "operator",
            "article",
            "notes",
            "specification",
        ],
    )?;
    let title = c.opt_str("title")?.unwrap_or("").to_owned();

    let fs_hz = match c.opt_obj("acquisition")? {
        None => DEFAULT_FS_HZ,
        Some(a) => {
            only(&a, &["fs_hz"])?;
            a.f64_or("fs_hz", DEFAULT_FS_HZ)?
        }
    };
    let trigger = parse_trigger(&c)?;
    let analysis = parse_analysis(&c)?;

    let mut run_name = DEFAULT_RUN_NAME.to_owned();
    let mut keep_free_gb = MIN_KEEP_FREE_GB;
    if let Some(o) = c.opt_obj("capture")? {
        only(&o, &["run_name", "keep_free_gb"])?;
        if let Some(n) = o.opt_str("run_name")? {
            run_name = n.to_owned();
        }
        keep_free_gb = o.f64_or("keep_free_gb", keep_free_gb)?;
    }

    let metadata = match c.array("metadata")? {
        None => default_metadata(),
        Some(list) => {
            let mut out = Vec::with_capacity(list.len());
            for (i, m) in list.iter().enumerate() {
                let o = Obj::of(m, &format!("config.metadata[{i}]"))?;
                only(&o, &["key", "label", "value"])?;
                let text = |k: &str| -> Result<String, MethodError> {
                    o.opt_str(k)?
                        .map(str::to_owned)
                        .ok_or_else(|| bad(&o.at(k), format!("\"{}\" is text", o.at(k))))
                };
                let key = text("key")?;
                let label = o.opt_str("label")?.unwrap_or(&key).to_owned();
                out.push(MetaField {
                    key,
                    label,
                    value: text("value")?,
                });
            }
            out
        }
    };

    let words =
        |k: &str| -> Result<String, MethodError> { Ok(c.opt_str(k)?.unwrap_or("").to_owned()) };
    let (operator, article, notes) = (words("operator")?, words("article")?, words("notes")?);
    let specification = match c.opt_obj("specification")? {
        None => None,
        Some(o) => {
            only(&o, &["document", "clause"])?;
            Some((
                o.opt_str("document")?.unwrap_or("").to_owned(),
                o.opt_str("clause")?.unwrap_or("").to_owned(),
            ))
        }
    };

    Ok(WireRecorder {
        title,
        fs_hz,
        trigger,
        analysis,
        run_name,
        keep_free_gb,
        metadata,
        operator,
        article,
        notes,
        specification,
        raw: config.clone(),
    })
}

// ---------------------------------------------------------------------------------
// Checking: every problem is a named item.
// ---------------------------------------------------------------------------------

/// A configuration field in words, for a message a person reads.
pub fn field_words(field: &str) -> String {
    let words = match field {
        "acquisition.fs_hz" => "The sample rate",
        "trigger.level" => "The trigger level",
        "trigger.hysteresis" => "The trigger's hysteresis",
        "trigger.pre_s" => "The pre-window",
        "trigger.record_s" => "The recording length",
        "trigger.count" => "The count",
        "trigger.holdoff_s" => "The holdoff",
        "analysis.block" => "The analysis block",
        "analysis.overlap_pct" => "The overlap",
        "analysis.averages" => "The averages",
        "analysis.decimation" => "The decimation",
        "capture.run_name" => "The run folder's name",
        "capture.keep_free_gb" => "The free space to keep",
        _ => return format!("\"{field}\""),
    };
    words.to_owned()
}

fn out_of_range(r: &mut Report, field: &str, value: f64, ok: bool, text: &str) {
    if !ok {
        r.errors.push(Item::field(
            "out_of_range",
            field,
            format!("{} is {}; {text}", field_words(field), num(value)),
        ));
    }
}

fn power_of_two(r: &mut Report, field: &str, value: u64, range: (u64, u64)) {
    if !value.is_power_of_two() {
        r.errors.push(Item::field(
            "not_power_of_two",
            field,
            format!(
                "{} is {value}; it is a power of two from {} to {}.",
                field_words(field),
                range.0,
                range.1
            ),
        ));
    } else {
        out_of_range(
            r,
            field,
            value as f64,
            (range.0..=range.1).contains(&value),
            &format!("it is a power of two from {} to {}.", range.0, range.1),
        );
    }
}

impl WireRecorder {
    /// The overlap in force: the one asked, or the window's own.
    pub fn overlap_pct(&self) -> f64 {
        self.analysis
            .overlap_pct
            .unwrap_or_else(|| self.analysis.window.default_overlap_pct())
    }

    /// The pre-window in force, seconds: none in `start` mode.
    pub fn pre_s(&self) -> f64 {
        match self.trigger.mode {
            Mode::Start => 0.0,
            _ => self.trigger.pre_s,
        }
    }

    /// The table's refusals and warnings, as items. Checked at configure and again at
    /// start.
    pub fn check_table(table: &ChannelTable, r: &mut Report) {
        if recorded_rows(table).is_empty() {
            r.errors.push(Item::channel(
                "no_input_channel",
                None,
                "The channel table has no enabled input row; a Recorder run records the \
                 enabled input rows.",
            ));
        }
        for c in table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
        {
            r.warnings.push(Item::channel(
                "drive_channel_present",
                Some(c.index),
                format!(
                    "Row {} is a drive row; a Recorder run does not open or record it.",
                    c.index
                ),
            ));
        }
    }

    /// The core's configuration, every field supplied, for `table`. The rows the
    /// trigger and the FRF name are mapped to their columns; a row that is not
    /// recorded has been refused by [`WireRecorder::check`] first.
    pub fn build(&self, table: &ChannelTable) -> RecorderConfig {
        let rows = recorded_rows(table);
        let column = |index: u32| rows.iter().position(|c| c.index == index);
        let acq = AcquisitionConfig {
            native_rate_hz: self.fs_hz,
            analysis_decimation: self.analysis.decimation as u32,
            block_size: self.analysis.block as usize,
            window: self.analysis.window,
            overlap_pct: self.overlap_pct(),
            averaging: self.analysis.averaging,
            average_count: self.analysis.averages as usize,
        };
        let t = &self.trigger;
        let trigger_mode = match t.mode {
            Mode::Start | Mode::Manual => TriggerMode::Manual,
            Mode::Level => {
                let row = t.row.unwrap_or(0);
                let slot = column(row).unwrap_or(0);
                let range = rows.get(slot).map_or(1.0, |c| c.range_max);
                TriggerMode::Level(LevelTrigger {
                    channel_index: slot as u32,
                    threshold_eu: t.level.unwrap_or(0.0),
                    slope: t.slope,
                    hysteresis_eu: t
                        .hysteresis
                        .unwrap_or_else(|| LevelTrigger::default_hysteresis_eu(range)),
                    holdoff_s: t.holdoff_s,
                })
            }
        };
        // `count` recordings after an arm; the core counts the re-arms after the
        // first. Null (until the run ends) is the largest count the core holds.
        let auto_rearm = match (t.mode, t.count) {
            (Mode::Level, Some(n)) => n.saturating_sub(1).min(u64::from(u32::MAX)) as u32,
            (Mode::Level, None) => u32::MAX,
            _ => 0,
        };
        RecorderConfig {
            acq,
            trigger_mode,
            auto_rearm,
            pretrigger_s: Some(self.pre_s()),
            timed_record_s: t.record_s,
            overload_fraction: OVERLOAD_FRACTION,
            frf_pair: self
                .analysis
                .frf
                .and_then(|(x, y)| Some((column(x)?, column(y)?))),
            metadata: self.metadata.clone(),
            // The floor asked, which the core keeps while the run records (its own least,
            // 1 GiB, otherwise; the derived floor is the larger of this and twice a timed
            // recording, as the core's is).
            keep_free_bytes: Some((self.keep_free_gb * GIB).round().max(0.0) as u64),
            ..RecorderConfig::default()
        }
    }

    /// Check against `ctx`: the items, and the derived values when there are no errors.
    pub fn check(&self, ctx: &CheckContext<'_>) -> (Report, Option<Derived>) {
        let mut r = Report::default();
        let fs = self.fs_hz;
        out_of_range(
            &mut r,
            "acquisition.fs_hz",
            fs,
            (FS_RANGE.0..=FS_RANGE.1).contains(&fs),
            &format!(
                "it runs from {} to {} Hz.",
                num(FS_RANGE.0),
                num(FS_RANGE.1)
            ),
        );

        let rows = recorded_rows(ctx.table);
        let row = |index: u32| rows.iter().find(|c| c.index == index).copied();
        let not_recorded = |r: &mut Report, field: &str, index: u32| {
            r.errors.push(Item::field(
                "row_not_recorded",
                field,
                format!(
                    "Row {index} is not an enabled input row of the channel table; only those \
                     are recorded."
                ),
            ))
        };

        let t = &self.trigger;
        if t.mode == Mode::Level {
            if let (Some(index), Some(level)) = (t.row, t.level) {
                match row(index) {
                    None => not_recorded(&mut r, "trigger.row", index),
                    Some(c) => {
                        out_of_range(
                            &mut r,
                            "trigger.level",
                            level,
                            level.abs() <= c.range_max,
                            &format!(
                                "it is within row {index}'s range, {} either side of zero.",
                                num(c.range_max)
                            ),
                        );
                        if let Some(h) = t.hysteresis {
                            out_of_range(
                                &mut r,
                                "trigger.hysteresis",
                                h,
                                h > 0.0 && h <= c.range_max,
                                &format!(
                                    "it is above 0 and at most row {index}'s range, {}.",
                                    num(c.range_max)
                                ),
                            );
                        }
                    }
                }
            }
            if let Some(n) = t.count {
                out_of_range(
                    &mut r,
                    "trigger.count",
                    n as f64,
                    n >= 1,
                    "it is at least 1, or null for until the run ends.",
                );
            }
            out_of_range(
                &mut r,
                "trigger.holdoff_s",
                t.holdoff_s,
                t.holdoff_s >= 0.0,
                "it is at least 0 s.",
            );
        }
        out_of_range(
            &mut r,
            "trigger.pre_s",
            t.pre_s,
            (0.0..=MAX_PRETRIGGER_S).contains(&t.pre_s),
            &format!("it runs from 0 to {} s.", num(MAX_PRETRIGGER_S)),
        );
        if let Some(s) = t.record_s {
            out_of_range(&mut r, "trigger.record_s", s, s > 0.0, "it is above 0 s.");
        }

        let a = &self.analysis;
        power_of_two(&mut r, "analysis.block", a.block, BLOCK_RANGE);
        if let Some(o) = a.overlap_pct {
            out_of_range(
                &mut r,
                "analysis.overlap_pct",
                o,
                (OVERLAP_RANGE.0..=OVERLAP_RANGE.1).contains(&o),
                &format!(
                    "it runs from {} to {} percent.",
                    num(OVERLAP_RANGE.0),
                    num(OVERLAP_RANGE.1)
                ),
            );
        }
        out_of_range(
            &mut r,
            "analysis.averages",
            a.averages as f64,
            a.averages >= 1,
            "it is at least 1.",
        );
        power_of_two(
            &mut r,
            "analysis.decimation",
            a.decimation,
            DECIMATION_RANGE,
        );
        if let Some((x, y)) = a.frf {
            for index in [x, y] {
                if row(index).is_none() {
                    not_recorded(&mut r, "analysis.frf", index);
                }
            }
            if x == y {
                r.errors.push(Item::field(
                    "frf_same_row",
                    "analysis.frf",
                    format!(
                        "The FRF's reference and response are both row {x}; they are two \
                         different rows."
                    ),
                ));
            }
        }

        let name = &self.run_name;
        let plain =
            !name.is_empty() && !name.contains(['/', '\\', ':']) && name != "." && name != "..";
        if !plain {
            r.errors.push(Item::field(
                "out_of_range",
                "capture.run_name",
                format!(
                    "The run folder's name is \"{name}\"; it is a plain folder name, with no \
                     path in it."
                ),
            ));
        }
        out_of_range(
            &mut r,
            "capture.keep_free_gb",
            self.keep_free_gb,
            self.keep_free_gb >= MIN_KEEP_FREE_GB,
            &format!("it is at least {} GiB.", num(MIN_KEEP_FREE_GB)),
        );

        Self::check_table(ctx.table, &mut r);

        if !r.errors.is_empty() {
            return (r, None);
        }
        (
            r,
            Some(self.derive(ctx.table, ctx.station, fs, ctx.free_bytes)),
        )
    }

    /// The derived values at `fs_hz` (the rate asked before start, the station's after).
    pub fn derive(
        &self,
        table: &ChannelTable,
        station: RecorderStation,
        fs_hz: f64,
        free_bytes: Option<u64>,
    ) -> Derived {
        let rows: Vec<u32> = recorded_rows(table).iter().map(|c| c.index).collect();
        let block_frames = hertzforge_hal::default_block_frames(fs_hz);
        let a = &self.analysis;
        let analysis_fs = fs_hz / a.decimation as f64;
        let block = a.block as usize;
        let overlap = self.overlap_pct();
        let hop = hop_frames(block, overlap);
        let averages_frames = block + (a.averages as usize).saturating_sub(1) * hop;
        let pre_frames = frames_for(self.pre_s(), fs_hz);
        let record_frames = self.trigger.record_s.map(|s| frames_for(s, fs_hz));
        let holdoff_frames = match self.trigger.mode {
            Mode::Level => frames_for(self.trigger.holdoff_s, fs_hz),
            _ => 0,
        };
        let bytes_per_s = (rows.len() as f64 * BYTES_PER_SAMPLE as f64 * fs_hz).round() as u64;
        let floor = floor_bytes(
            self.keep_free_gb,
            record_frames.map(|f| f + pre_frames),
            rows.len(),
        );
        Derived {
            fs_hz,
            block_frames,
            batch_frames: BLOCKS_PER_BATCH * block_frames,
            group_delay_frames: station.group_delay_frames(fs_hz),
            actual: false,
            rows,
            analysis_fs_hz: analysis_fs,
            df_hz: analysis_fs / block as f64,
            lines: (block as f64 / 2.56).floor() as usize,
            span_hz: analysis_fs / 2.56,
            block_s: block as f64 / analysis_fs,
            overlap_pct: overlap,
            averages_s: averages_frames as f64 / analysis_fs,
            pre_frames,
            record_frames,
            holdoff_frames,
            bytes_per_s,
            floor_bytes: floor,
            free_bytes,
        }
    }
}

/// The refusal of a configuration with errors.
pub fn config_invalid(report: &Report) -> MethodError {
    crate::shock::config::config_invalid(report)
}

/// `recorder.config`'s answer for a configured run.
pub fn config_result(run_id: &str, w: &WireRecorder, derived: &Derived) -> Value {
    let mut m = Map::new();
    m.insert("run_id".into(), Value::from(run_id));
    m.insert("config".into(), w.raw.clone());
    m.insert("derived".into(), derived.to_json());
    Value::Object(m)
}

/// A number or null, rounded to `digits`.
pub fn opt_round(x: Option<f64>, digits: i32) -> Value {
    opt_num(x.map(|v| round(v, digits)))
}
