//! The setup's checks against a checked file (D3.2 to D3.5), and the one way to a
//! [`CheckedDrive`], the only thing a run arms with.
//!
//! A file reaches a run only through [`CheckedDrive::new`], which refuses unless the
//! file's own check passed on the hash the run will re-check at arm, every file
//! channel goes to a distinct enabled drive row, no more channels than the station
//! drives, every channel's peak at the level is within the ceiling, the ceiling is
//! within each mapped row's range, and the file's rate is a rate the station runs at.
//! Each refusal names the channel and the numbers.

use crate::check::FileCheck;
use crate::file::{DriveFile, FileError};
use serde_json::{json, Value};

/// The level's range, dB.
pub const LEVEL_MIN_DB: f64 = -40.0;
pub const LEVEL_MAX_DB: f64 = 0.0;
/// The pre-roll's and the post-roll's range, seconds.
pub const ROLL_MAX_S: f64 = 60.0;
/// The controlled stop: this far down, over [`STOP_RAMP_S`], then zero. The same
/// stop the Random and MIMO Random modules ramp by default.
pub const STOP_RAMP_DEPTH_DB: f64 = 40.0;
pub const STOP_RAMP_S: f64 = 3.0;

/// What the run plays at: the setup's numbers the checks need.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaySetup {
    /// The level, dB (0 plays the file as it is).
    pub level_db: f64,
    /// The drive ceiling, volts.
    pub ceiling_v: f64,
    pub pre_roll_s: f64,
    pub post_roll_s: f64,
    /// The drive row (table index) each file channel goes to, in file order.
    pub map: Vec<u32>,
}

impl PlaySetup {
    pub fn gain(&self) -> f64 {
        10f64.powf(self.level_db / 20.0)
    }
}

/// A drive row the run may play: its table index and its range, volts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriveRow {
    pub index: u32,
    pub range_max: f64,
}

/// What a station offers the run.
#[derive(Debug, Clone, PartialEq)]
pub struct StationOffer {
    /// The rate the station adopts for the file's rate, when it is known (the
    /// simulator runs at any rate it is asked; hardware says at arm).
    pub rate_hz: Option<f64>,
    /// The rates the station offers nearest the file's, for a refusal's words.
    pub nearest_hz: Vec<f64>,
    /// The drives the station has.
    pub outputs: usize,
}

/// The refusal of a rate the station does not run the file at.
pub fn rate_refusal(file_hz: f64, station_hz: f64, nearest: &[f64]) -> FileError {
    let list: Vec<String> = nearest.iter().map(|r| format!("{r} Hz")).collect();
    FileError::new(
        "rate_mismatch",
        format!(
            "The drive file is at {file_hz} Hz and the station runs at {station_hz} Hz; a file \
             is played at its own rate and never resampled. The nearest rates the station \
             offers: {}. Convert the file at one of them.",
            if list.is_empty() {
                format!("{station_hz} Hz")
            } else {
                list.join(", ")
            }
        ),
        json!({"file_hz": file_hz, "station_hz": station_hz,
               "nearest_hz": if nearest.is_empty() { vec![station_hz] } else { nearest.to_vec() }}),
    )
}

/// Every refusal of `setup` with `file` and its check on the drive rows and station
/// given, in a fixed order: the ends, the map, the rate, the peaks.
pub fn refusals(
    file: &DriveFile,
    check: &FileCheck,
    setup: &PlaySetup,
    rows: &[DriveRow],
    station: &StationOffer,
) -> Vec<FileError> {
    let mut out = Vec::new();
    let failing: Vec<String> = check
        .channels
        .iter()
        .filter(|c| !c.pass())
        .map(|c| c.words())
        .collect();
    if !failing.is_empty() {
        out.push(FileError::new(
            "ends_not_zero",
            format!(
                "The drive does not start and end at zero: {}. Fix its ends into a new file, \
                 or make it again.",
                failing.join("; ")
            ),
            json!({"channels": check.channels.iter().filter(|c| !c.pass())
                .map(|c| c.to_json()).collect::<Vec<_>>()}),
        ));
    }
    let n = file.channels.len();
    if setup.map.len() != n {
        out.push(FileError::new(
            "map_length",
            format!(
                "The drive file has {n} channels and the map names {} drive rows; each file \
                 channel needs its own drive row.",
                setup.map.len()
            ),
            json!({"channels": n, "map": setup.map}),
        ));
    }
    if n > station.outputs {
        out.push(FileError::new(
            "too_many_drives",
            format!(
                "The drive file has {n} channels and the station drives {}.",
                station.outputs
            ),
            json!({"channels": n, "outputs": station.outputs}),
        ));
    }
    for (k, row) in setup.map.iter().enumerate() {
        if !rows.iter().any(|r| r.index == *row) {
            out.push(FileError::new(
                "map_missing_row",
                format!(
                    "Drive file channel {k} goes to row {row}, which is not an enabled drive row \
                     of the channel table."
                ),
                json!({"channel": k, "row": row,
                       "drive_rows": rows.iter().map(|r| r.index).collect::<Vec<_>>()}),
            ));
        } else if setup.map[..k].contains(row) {
            let first = setup.map.iter().position(|r| r == row).unwrap_or(0);
            out.push(FileError::new(
                "map_repeated_row",
                format!(
                    "Drive file channels {first} and {k} both go to drive row {row}; each \
                     channel needs its own drive row."
                ),
                json!({"channels": [first, k], "row": row}),
            ));
        }
    }
    if let Some(rate) = station.rate_hz {
        if rate != file.rate_hz {
            out.push(rate_refusal(file.rate_hz, rate, &station.nearest_hz));
        }
    }
    let gain = setup.gain();
    for c in &check.channels {
        // The drive path plays f32: the peak as played is the file's peak rounded to f32,
        // which may be above it. Judged on the larger, configure never passes a file
        // whose first played peak trips the ceiling.
        let played = c.peak_v.max(f64::from(c.peak_v as f32)) * gain;
        if played > setup.ceiling_v {
            out.push(FileError::new(
                "peak_over_ceiling",
                format!(
                    "Drive file channel {} ({}) peaks at {:.6} V, {:.6} V at {} dB, over the \
                     drive ceiling of {} V.",
                    c.channel, c.name, c.peak_v, played, setup.level_db, setup.ceiling_v
                ),
                json!({"channel": c.channel, "peak_v": c.peak_v, "level_db": setup.level_db,
                       "played_peak_v": played, "ceiling_v": setup.ceiling_v}),
            ));
        }
    }
    for (k, row) in setup.map.iter().enumerate() {
        if let Some(r) = rows.iter().find(|r| r.index == *row) {
            if setup.ceiling_v > r.range_max {
                out.push(FileError::new(
                    "ceiling_over_range",
                    format!(
                        "The drive ceiling of {} V is over drive row {row}'s range of {} V \
                         (file channel {k}).",
                        setup.ceiling_v, r.range_max
                    ),
                    json!({"channel": k, "row": row, "ceiling_v": setup.ceiling_v,
                           "range_max": r.range_max}),
                ));
            }
        }
    }
    out
}

/// The check report (D3): the file, the tolerances, each channel's numbers with the
/// row it goes to and its peak at the level, whether it passes and the refusals.
pub fn report_json(
    file: &DriveFile,
    check: &FileCheck,
    setup: &PlaySetup,
    refusals: &[FileError],
) -> Value {
    let gain = setup.gain();
    json!({
        "pass": refusals.is_empty(),
        "sha256": check.sha256,
        "rate_hz": file.rate_hz,
        "samples": file.samples,
        "duration_s": file.duration_s(),
        "level_db": setup.level_db,
        "ceiling_v": setup.ceiling_v,
        "tolerances": FileCheck::tolerances_json(),
        "channels": check.channels.iter().map(|c| {
            let mut v = c.to_json();
            if let Value::Object(m) = &mut v {
                m.insert("drive_row".into(), setup.map.get(c.channel).copied().map_or(Value::Null, Value::from));
                m.insert("played_peak_v".into(), Value::from(c.peak_v * gain));
            }
            v
        }).collect::<Vec<_>>(),
        "refusals": refusals.iter().map(|r| json!({"reason": r.reason, "message": r.message,
            "detail": r.detail})).collect::<Vec<_>>(),
        "limits": "The check takes the drive signal as an acceleration. It cannot know the \
                   table's motion without a model of the shaker and its amplifier, so stroke \
                   is not checked.",
    })
}

/// A file checked and judged with its setup: the only thing a run arms with.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedDrive {
    pub file: DriveFile,
    pub check: FileCheck,
    pub setup: PlaySetup,
    /// The output slot (position among the station's drive rows, in table order)
    /// each file channel plays on.
    pub slots: Vec<usize>,
    /// The drive rows in the order the station plays them.
    pub rows: Vec<DriveRow>,
}

impl CheckedDrive {
    /// Judge `setup` with `file` and its `check`: `Ok` only when nothing is refused and
    /// the check was taken on the hash the sidecar states.
    pub fn new(
        file: DriveFile,
        check: FileCheck,
        setup: PlaySetup,
        rows: &[DriveRow],
        station: &StationOffer,
    ) -> Result<CheckedDrive, Vec<FileError>> {
        let mut r = Vec::new();
        if check.sha256 != file.sha256
            || check.samples != file.samples
            || check.channels.len() != file.channels.len()
        {
            r.push(FileError::new(
                "drive_unchecked",
                "The drive file was not checked as it is now; check it again before arming.",
                json!({"checked": check.sha256, "file": file.sha256}),
            ));
        }
        if !(setup.level_db.is_finite()
            && (LEVEL_MIN_DB..=LEVEL_MAX_DB).contains(&setup.level_db)
            && setup.ceiling_v.is_finite()
            && setup.ceiling_v > 0.0
            && (0.0..=ROLL_MAX_S).contains(&setup.pre_roll_s)
            && (0.0..=ROLL_MAX_S).contains(&setup.post_roll_s))
        {
            r.push(FileError::new(
                "out_of_range",
                format!(
                    "The level must be {LEVEL_MIN_DB} to {LEVEL_MAX_DB} dB, the ceiling above 0 V, \
                     and the rolls 0 to {ROLL_MAX_S} s."
                ),
                json!({"level_db": setup.level_db, "ceiling_v": setup.ceiling_v,
                       "pre_roll_s": setup.pre_roll_s, "post_roll_s": setup.post_roll_s}),
            ));
        }
        r.extend(refusals(&file, &check, &setup, rows, station));
        if !r.is_empty() {
            return Err(r);
        }
        let slots = setup
            .map
            .iter()
            .map(|row| rows.iter().position(|r| r.index == *row).unwrap_or(0))
            .collect();
        Ok(CheckedDrive {
            file,
            check,
            setup,
            slots,
            rows: rows.to_vec(),
        })
    }

    pub fn report_json(&self) -> Value {
        report_json(&self.file, &self.check, &self.setup, &[])
    }
}
