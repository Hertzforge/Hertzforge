//! Sine profiles on the wire (schema/PROTOCOL.md, "Sine Configuration", "Profiles"):
//! the profile a configuration takes, its checks as named items, its facts (where it
//! changes quantity, its peaks over a band), the three views a screen draws it with,
//! and `sine.profile.import` and `sine.profile.validate`.
//!
//! The profile's grammar is the controller's own (`hertzforge_sine::profile`): this
//! module reads and writes the wire form and names the controller's findings; it
//! repeats none of its rules.

use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use crate::shock::config::Obj;
use crate::table::{Item, Report};
use crate::views::{self, Axis, Role, Scale, Series};
use hertzforge_shaker_db::{MovingMass, ShakerRecord};
use hertzforge_sine::{
    accel_g_pk_of, assess_sine, crossover_hz, displacement_in_pp_from_accel, import_with_report,
    velocity_ips_pk_from_accel, ProfileError, ProfileRow, ProfileWarning, Quantity, SineProfile,
    DEFAULT_ABORT_DB, DEFAULT_ALARM_DB,
};
use serde_json::{json, Map, Value};
use std::path::Path;

/// The largest file `sine.profile.import` reads, bytes.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Points a profile view is drawn on: this many log-spaced across the band, and
/// every row frequency inside it.
pub const VIEW_POINTS: usize = 256;

/// A row's quantity on the wire.
pub fn quantity_name(q: Quantity) -> &'static str {
    match q {
        Quantity::AccelGPk => "accel_g",
        Quantity::VelocityIpsPk => "velocity_ips",
        Quantity::DisplacementInPp => "disp_pp_in",
    }
}

const QUANTITIES: [(&str, Quantity); 3] = [
    ("accel_g", Quantity::AccelGPk),
    ("velocity_ips", Quantity::VelocityIpsPk),
    ("disp_pp_in", Quantity::DisplacementInPp),
];

/// Round to `sig` significant digits, for facts a person reads.
pub fn sig(x: f64, sig: i32) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let p = 10f64.powi(sig - 1 - x.abs().log10().floor() as i32);
    (x * p).round() / p
}

fn band_json(b: (f64, f64)) -> Value {
    json!([num(b.0), num(b.1)])
}

/// `[below, above]` in positive dB.
pub(crate) fn db_pair(o: &Obj<'_>, k: &str) -> Result<Option<(f64, f64)>, MethodError> {
    let Some(a) = o.array(k)? else {
        return Ok(None);
    };
    let bad = || {
        MethodError::bad_params(
            &o.at(k),
            format!("\"{}\" is [below, above] in positive dB", o.at(k)),
        )
    };
    if a.len() != 2 {
        return Err(bad());
    }
    let below = a[0]
        .as_f64()
        .filter(|x| x.is_finite() && *x >= 0.0)
        .ok_or_else(bad)?;
    let above = a[1]
        .as_f64()
        .filter(|x| x.is_finite() && *x >= 0.0)
        .ok_or_else(bad)?;
    Ok(Some((below, above)))
}

/// Parse a profile at `path` (`config.profile`, or a limit channel's bound). Types
/// and structure only: what the controller's grammar refuses comes back as items
/// from [`check_profile`].
pub fn parse_profile(v: &Value, path: &str) -> Result<SineProfile, MethodError> {
    let o = Obj::of(v, path)?;
    let label = o.opt_str("label")?.unwrap_or("").to_owned();
    let rows_v = o.array("rows")?.ok_or_else(|| {
        MethodError::bad_params(
            &o.at("rows"),
            format!("\"{}\" is a list of rows", o.at("rows")),
        )
    })?;
    let mut rows = Vec::with_capacity(rows_v.len());
    for (i, r) in rows_v.iter().enumerate() {
        let ro = Obj::of(r, &format!("{path}.rows[{i}]"))?;
        rows.push(ProfileRow {
            freq_hz: ro.f64("freq_hz")?,
            quantity: ro.choice("quantity", &QUANTITIES, None)?,
            value: ro.f64("value")?,
            alarm_db: db_pair(&ro, "alarm_db")?,
            abort_db: db_pair(&ro, "abort_db")?,
        });
    }
    Ok(SineProfile {
        label,
        rows,
        global_alarm_db: db_pair(&o, "alarm_db")?.unwrap_or(DEFAULT_ALARM_DB),
        global_abort_db: db_pair(&o, "abort_db")?.unwrap_or(DEFAULT_ABORT_DB),
    })
}

/// A profile in its wire form, every field written.
pub fn profile_json(p: &SineProfile) -> Value {
    let rows: Vec<Value> = p
        .rows
        .iter()
        .map(|r| {
            let mut m = Map::new();
            m.insert("freq_hz".into(), num(r.freq_hz));
            m.insert("quantity".into(), Value::from(quantity_name(r.quantity)));
            m.insert("value".into(), num(r.value));
            if let Some(b) = r.alarm_db {
                m.insert("alarm_db".into(), band_json(b));
            }
            if let Some(b) = r.abort_db {
                m.insert("abort_db".into(), band_json(b));
            }
            Value::Object(m)
        })
        .collect();
    json!({
        "label": p.label,
        "rows": rows,
        "alarm_db": band_json(p.global_alarm_db),
        "abort_db": band_json(p.global_abort_db),
    })
}

/// Where a profile's items point: the control profile's rows by `row`, any other
/// profile's (a limit channel's bound) by field.
#[derive(Debug, Clone, Copy)]
pub enum Locate<'a> {
    /// The control profile: rows by index, the rest at `profile`.
    Control,
    /// Another profile, at this field path.
    Field(&'a str),
}

impl Locate<'_> {
    fn row(&self, reason: &'static str, row: usize, message: String) -> Item {
        match self {
            Locate::Control => Item::row(reason, row, message),
            Locate::Field(f) => Item::field(reason, format!("{f}.rows[{row}]"), message),
        }
    }

    fn whole(&self, reason: &'static str, key: &str, message: String) -> Item {
        match self {
            Locate::Control => Item::field(reason, format!("profile.{key}"), message),
            Locate::Field(f) => Item::field(reason, format!("{f}.{key}"), message),
        }
    }
}

/// The controller's grammar on `p`, its findings as named items.
pub fn check_profile(p: &SineProfile, at: Locate<'_>) -> Report {
    let mut r = Report::default();
    let report = p.validate_report();
    for e in &report.errors {
        r.errors.push(match e {
            ProfileError::TooFewRows(n) => at.whole(
                "profile_too_short",
                "rows",
                format!("A profile needs at least two rows; this one has {n}."),
            ),
            ProfileError::NonPositiveFreq { row, freq_hz } => at.row(
                "frequency_not_positive",
                *row,
                format!(
                    "Row {row}'s frequency is {}; a frequency is above 0 Hz.",
                    num(*freq_hz)
                ),
            ),
            ProfileError::NonMonotonicFreq { row, freq_hz } => at.row(
                "frequency_not_ascending",
                *row,
                format!(
                    "Row {row}'s frequency of {} Hz is not above the row before it; rows run \
                     in ascending frequency.",
                    num(*freq_hz)
                ),
            ),
            ProfileError::NonPositiveValue { row, value } => at.row(
                "value_not_positive",
                *row,
                format!("Row {row}'s value is {}; a value is above 0.", num(*value)),
            ),
            ProfileError::MultipleQuantities { row, columns } => at.row(
                "quantity_ambiguous",
                *row,
                format!(
                    "Row {row} gives more than one quantity ({}); give exactly one.",
                    columns.join(", ")
                ),
            ),
            ProfileError::Parse(s) => at.whole("profile_file", "rows", s.clone()),
        });
    }
    for w in &report.warnings {
        r.warnings.push(match w {
            ProfileWarning::AlarmWiderThanAbort { row } => at.row(
                "alarm_wider_than_abort",
                *row,
                format!(
                    "Row {row}'s alarm band is wider than its abort band on one side, so it \
                     would abort without first alarming."
                ),
            ),
            ProfileWarning::GlobalAlarmWiderThanAbort => at.whole(
                "alarm_wider_than_abort",
                "alarm_db",
                "The alarm band is wider than the abort band on one side, so the run would \
                 abort without first alarming."
                    .into(),
            ),
            ProfileWarning::JoinDiscontinuity { row, delta_db } => at.row(
                "join_discontinuity",
                *row,
                format!(
                    "The profile jumps {} dB where row {row} changes quantity; moving row \
                     {row} to the crossover frequency removes the step.",
                    num(sig(*delta_db, 3))
                ),
            ),
            ProfileWarning::UnitsColumnIgnored => at.whole(
                "units_column_ignored",
                "rows",
                "The file's units column was read and ignored: each row's quantity is the \
                 column its value is in."
                    .into(),
            ),
        });
    }
    r
}

/// Whether `p` covers `band`. Checked only on a profile without errors.
pub fn covers(p: &SineProfile, band: (f64, f64)) -> bool {
    match (p.rows.first(), p.rows.last()) {
        (Some(a), Some(b)) => a.freq_hz <= band.0 && band.1 <= b.freq_hz,
        _ => false,
    }
}

/// Where a profile changes quantity: each pair of neighbouring rows of different
/// quantities, at the frequency where they give the same acceleration.
#[derive(Debug, Clone, PartialEq)]
pub struct Crossover {
    pub freq_hz: f64,
    pub from: Quantity,
    pub to: Quantity,
}

pub fn crossovers(p: &SineProfile) -> Vec<Crossover> {
    p.rows
        .windows(2)
        .filter(|w| w[0].quantity != w[1].quantity)
        .filter_map(|w| {
            crossover_hz(&w[0], &w[1]).map(|f| Crossover {
                freq_hz: f,
                from: w[0].quantity,
                to: w[1].quantity,
            })
        })
        .collect()
}

pub fn crossovers_json(c: &[Crossover]) -> Value {
    Value::Array(
        c.iter()
            .map(|x| {
                json!({
                    "freq_hz": num(sig(x.freq_hz, 4)),
                    "from": quantity_name(x.from),
                    "to": quantity_name(x.to),
                })
            })
            .collect(),
    )
}

/// A profile's peaks over a band: g pk, in/s pk, in pk-pk.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peaks {
    pub accel_g: f64,
    pub velocity_ips: f64,
    pub disp_pp_in: f64,
}

/// The peaks of `p` over `band`, from each segment's ends (every quantity is a
/// power of frequency inside a segment, so its largest value is at an end). `None`
/// when no segment meets the band.
pub fn peaks(p: &SineProfile, band: (f64, f64)) -> Option<Peaks> {
    let mut out: Option<Peaks> = None;
    for w in p.rows.windows(2) {
        let (l, r) = (&w[0], &w[1]);
        let a = l.freq_hz.max(band.0);
        let b = r.freq_hz.min(band.1);
        if !(a <= b && a > 0.0) {
            continue;
        }
        for f in [a, b] {
            // The segment's own value at f (its left row governs it up to the next).
            let v = if l.quantity == r.quantity {
                let slope = (r.value / l.value).ln() / (r.freq_hz / l.freq_hz).ln();
                l.value * (f / l.freq_hz).powf(slope)
            } else {
                l.value
            };
            let accel = accel_g_pk_of(l.quantity, v, f);
            let vel = velocity_ips_pk_from_accel(accel, f);
            let disp = displacement_in_pp_from_accel(accel, f);
            if !(accel.is_finite() && vel.is_finite() && disp.is_finite()) {
                continue;
            }
            let o = out.get_or_insert(Peaks {
                accel_g: accel,
                velocity_ips: vel,
                disp_pp_in: disp,
            });
            o.accel_g = o.accel_g.max(accel);
            o.velocity_ips = o.velocity_ips.max(vel);
            o.disp_pp_in = o.disp_pp_in.max(disp);
        }
    }
    out
}

/// The profile's facts in `derived`: its crossovers and its peaks.
pub fn facts_json(p: &SineProfile, band: (f64, f64)) -> Value {
    let pk = peaks(p, band);
    json!({
        "crossovers": crossovers_json(&crossovers(p)),
        "peak_accel_g": pk.map_or(Value::Null, |k| num(sig(k.accel_g, 4))),
        "peak_velocity_ips": pk.map_or(Value::Null, |k| num(sig(k.velocity_ips, 4))),
        "peak_disp_pp_in": pk.map_or(Value::Null, |k| num(sig(k.disp_pp_in, 4))),
    })
}

/// The feasibility verdict of `p` against a shaker, in words.
pub fn feasibility_verdict(
    p: &SineProfile,
    shaker: Option<(&ShakerRecord, &MovingMass)>,
) -> (&'static str, String) {
    let Some((record, mass)) = shaker else {
        return (
            "not_evaluated",
            "No shaker was given, so the profile was not checked against shaker ratings.".into(),
        );
    };
    let report = assess_sine(record, mass, p);
    let (verdict, words) = crate::shock::config::verdict(&report);
    // The shock words speak of a pulse; this is a profile.
    (verdict, words.replace("The pulse", "The profile"))
}

/// A profile's three views, as figures: the profile and its bands at full level,
/// and the profile as velocity and displacement, all on the same points.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileFigures {
    pub freqs_hz: Vec<f64>,
    pub target: Vec<f64>,
    pub alarm_hi: Vec<f64>,
    pub alarm_lo: Vec<f64>,
    pub abort_hi: Vec<f64>,
    pub abort_lo: Vec<f64>,
    pub velocity: Vec<f64>,
    pub displacement: Vec<f64>,
}

/// The points a profile is drawn on over `band`: [`VIEW_POINTS`] log-spaced and
/// every row frequency inside, ascending.
pub fn view_points(p: &SineProfile, band: (f64, f64)) -> Vec<f64> {
    let (lo, hi) = band;
    let mut f: Vec<f64> = if hi > lo {
        (0..VIEW_POINTS)
            .map(|k| lo * (hi / lo).powf(k as f64 / (VIEW_POINTS - 1) as f64))
            .collect()
    } else {
        vec![lo]
    };
    f.extend(
        p.rows
            .iter()
            .map(|r| r.freq_hz)
            .filter(|x| *x > lo && *x < hi),
    );
    f.sort_by(f64::total_cmp);
    f.dedup();
    f
}

/// The figures of `p` over `band`; `None` where the profile does not reach.
pub fn figures(p: &SineProfile, band: (f64, f64)) -> Option<ProfileFigures> {
    let freqs = view_points(p, band);
    let mut fig = ProfileFigures {
        freqs_hz: Vec::with_capacity(freqs.len()),
        target: Vec::new(),
        alarm_hi: Vec::new(),
        alarm_lo: Vec::new(),
        abort_hi: Vec::new(),
        abort_lo: Vec::new(),
        velocity: Vec::new(),
        displacement: Vec::new(),
    };
    for f in freqs {
        let t = p.target_accel_g_pk(f)?;
        let (al, ah) = p.alarm_bounds(f)?;
        let (bl, bh) = p.abort_bounds(f)?;
        fig.freqs_hz.push(f);
        fig.target.push(t);
        fig.alarm_hi.push(ah);
        fig.alarm_lo.push(al);
        fig.abort_hi.push(bh);
        fig.abort_lo.push(bl);
        fig.velocity.push(velocity_ips_pk_from_accel(t, f));
        fig.displacement.push(displacement_in_pp_from_accel(t, f));
    }
    Some(fig)
}

pub fn freq_axis() -> Axis {
    Axis::new("Frequency", Some("Hz"), Scale::Log)
}

/// Profile, Velocity and Displacement, in that order.
pub fn profile_views(fig: &ProfileFigures, arrays: &mut Arrays) -> Vec<Value> {
    let x = || fig.freqs_hz.clone();
    let profile = views::spectrum(
        arrays,
        "Profile",
        &freq_axis(),
        &Axis::new("Acceleration", Some("g"), Scale::Log),
        vec![
            Series::new("Profile", Role::Reference, x(), fig.target.clone()),
            Series::new("Alarm high", Role::Tolerance, x(), fig.alarm_hi.clone()),
            Series::new("Alarm low", Role::Tolerance, x(), fig.alarm_lo.clone()),
            Series::new("Abort high", Role::Abort, x(), fig.abort_hi.clone()),
            Series::new("Abort low", Role::Abort, x(), fig.abort_lo.clone()),
        ],
        Some("frequency"),
    );
    let velocity = views::spectrum(
        arrays,
        "Velocity",
        &freq_axis(),
        &Axis::new("Velocity", Some("in/s"), Scale::Log),
        vec![Series::new(
            "Profile",
            Role::Reference,
            x(),
            fig.velocity.clone(),
        )],
        Some("frequency"),
    );
    let displacement = views::spectrum(
        arrays,
        "Displacement",
        &freq_axis(),
        &Axis::new("Displacement", Some("in pk-pk"), Scale::Log),
        vec![Series::new(
            "Profile",
            Role::Reference,
            x(),
            fig.displacement.clone(),
        )],
        Some("frequency"),
    );
    vec![profile, velocity, displacement]
}

/// The kinematics summary `sine.profile.validate` gives.
#[derive(Debug, Clone, PartialEq)]
pub struct Kinematics {
    pub peaks: Peaks,
    pub verdict: &'static str,
    pub message: String,
}

impl Kinematics {
    pub fn to_json(&self) -> Value {
        json!({
            "peak_accel_g": num(sig(self.peaks.accel_g, 4)),
            "peak_velocity_ips": num(sig(self.peaks.velocity_ips, 4)),
            "peak_disp_pp_in": num(sig(self.peaks.disp_pp_in, 4)),
            "verdict": self.verdict,
            "message": self.message,
        })
    }
}

/// The `sine.profile.validate` result from its parts.
pub fn validate_frame(
    report: &Report,
    crossovers: &[Crossover],
    kinematics: Option<&Kinematics>,
    fig: Option<&ProfileFigures>,
    arrays: &mut Arrays,
) -> Value {
    let views = match (report.errors.is_empty(), fig) {
        (true, Some(f)) => profile_views(f, arrays),
        _ => Vec::new(),
    };
    json!({
        "errors": report.errors.iter().map(Item::to_json).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(Item::to_json).collect::<Vec<_>>(),
        "crossovers": crossovers_json(crossovers),
        "kinematics": kinematics.map_or(Value::Null, Kinematics::to_json),
        "views": views,
    })
}

/// `sine.profile.validate {profile, band_hz, feasibility}`.
pub fn validate(params: &Map<String, Value>, arrays: &mut Arrays) -> Result<Value, MethodError> {
    let o = Obj {
        m: params,
        path: "params".into(),
    };
    let v = o.require("profile").map_err(|_| {
        MethodError::bad_params("profile", "sine.profile.validate needs a \"profile\"")
    })?;
    let p = parse_profile(v, "profile")?;
    let band = o.band("band_hz")?;
    let feasibility = match o.opt_obj("feasibility")? {
        None => None,
        Some(f) => Some(super::config::parse_feasibility(&f)?),
    };
    let mut report = check_profile(&p, Locate::Field("profile"));
    let span = match (p.rows.first(), p.rows.last()) {
        (Some(a), Some(b)) => (a.freq_hz, b.freq_hz),
        _ => (0.0, 0.0),
    };
    let band = match band {
        Some(b) if report.errors.is_empty() => {
            if !(b.0 > 0.0 && b.1 >= b.0) {
                report.errors.push(Item::field(
                    "out_of_range",
                    "band_hz",
                    format!(
                        "The band is {} to {} Hz; it runs from above 0 Hz upward.",
                        num(b.0),
                        num(b.1)
                    ),
                ));
            } else if !covers(&p, b) {
                report.errors.push(not_covering("band_hz", &p, b));
            }
            b
        }
        Some(b) => b,
        None => span,
    };
    let crossovers = crossovers(&p);
    if !report.errors.is_empty() {
        return Ok(validate_frame(&report, &crossovers, None, None, arrays));
    }
    let kinematics = peaks(&p, band).map(|pk| {
        let (verdict, message) = feasibility_verdict(
            &p,
            feasibility.as_ref().map(|f| (&f.shaker, &f.moving_mass)),
        );
        Kinematics {
            peaks: pk,
            verdict,
            message,
        }
    });
    let fig = figures(&p, band);
    Ok(validate_frame(
        &report,
        &crossovers,
        kinematics.as_ref(),
        fig.as_ref(),
        arrays,
    ))
}

/// The item for a profile that does not cover `band`, at `field`.
pub fn not_covering(field: &str, p: &SineProfile, band: (f64, f64)) -> Item {
    let (a, b) = match (p.rows.first(), p.rows.last()) {
        (Some(a), Some(b)) => (a.freq_hz, b.freq_hz),
        _ => (0.0, 0.0),
    };
    Item::field(
        "profile_not_covering",
        field,
        format!(
            "The profile runs from {} to {} Hz and the test's band from {} to {} Hz; the \
             profile must cover the band.",
            num(a),
            num(b),
            num(band.0),
            num(band.1)
        ),
    )
}

// ---------------------------------------------------------------------------------
// `sine.profile.import`
// ---------------------------------------------------------------------------------

/// Read `path`: absolute, a file, at most [`MAX_FILE_BYTES`]. A relative path is
/// `bad_params`; a file that cannot be read is a `profile_file` item at `path`.
fn read(path: &str) -> Result<Result<String, Item>, MethodError> {
    if path.trim().is_empty() {
        return Err(MethodError::bad_params(
            "path",
            "An import needs the path of its file.",
        ));
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(MethodError::bad_params(
            "path",
            "Give the file's full path on this computer.",
        ));
    }
    let unreadable = |why: String| {
        Item::field(
            "profile_file",
            "path",
            format!(
                "{} could not be read ({why}). Check the path and that the file is readable.",
                p.display()
            ),
        )
    };
    match std::fs::metadata(p) {
        Err(e) => return Ok(Err(unreadable(e.to_string()))),
        Ok(m) if !m.is_file() => return Ok(Err(unreadable("it is not a file".into()))),
        Ok(m) if m.len() > MAX_FILE_BYTES => {
            return Ok(Err(unreadable(format!(
                "it is {} bytes, over the {} bytes an import reads",
                m.len(),
                MAX_FILE_BYTES
            ))))
        }
        Ok(_) => {}
    }
    Ok(std::fs::read_to_string(p).map_err(|e| unreadable(e.to_string())))
}

/// The data row a parse error names (the controller's words begin "row N:"), and
/// the words without it.
fn parse_row(s: &str) -> Option<(usize, &str)> {
    let rest = s.strip_prefix("row ")?;
    let (n, words) = rest.split_once(':')?;
    Some((n.trim().parse().ok()?, words.trim()))
}

/// The import of `text` (from `field`, `text` or `path`).
pub fn import_text(text: &str, field: &'static str) -> Value {
    let mut report = Report::default();
    let profile = match import_with_report(text, None) {
        Err(ProfileError::MultipleQuantities { row, columns }) => {
            report.errors.push(Item::row(
                "quantity_ambiguous",
                row,
                format!(
                    "Data row {row} (from 0, after the header and comments) gives more than \
                     one quantity ({}); give exactly one.",
                    columns.join(", ")
                ),
            ));
            None
        }
        Err(ProfileError::Parse(s)) => {
            report.errors.push(match parse_row(&s) {
                Some((row, words)) => Item::row(
                    "profile_file",
                    row,
                    format!("Data row {row} (from 0, after the header and comments): {words}."),
                ),
                None => Item::field(
                    "profile_file",
                    field,
                    format!("The rows could not be read: {s}."),
                ),
            });
            None
        }
        Err(other) => {
            report
                .errors
                .push(Item::field("profile_file", field, other.to_string()));
            None
        }
        Ok((p, _)) => {
            report.extend(check_profile(&p, Locate::Control));
            Some(p)
        }
    };
    let profile = profile.filter(|_| report.errors.is_empty());
    json!({
        "profile": profile.as_ref().map_or(Value::Null, profile_json),
        "errors": report.errors.iter().map(Item::to_json).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(Item::to_json).collect::<Vec<_>>(),
    })
}

/// `sine.profile.import {text}` or `{path}`.
pub fn import(params: &Map<String, Value>) -> Result<Value, MethodError> {
    let text = crate::protocol::named_str(params, "text")?;
    let path = crate::protocol::named_str(params, "path")?;
    match (text, path) {
        (Some(t), None) => Ok(import_text(t, "text")),
        (None, Some(p)) => Ok(match read(p)? {
            Ok(t) => import_text(&t, "path"),
            Err(item) => json!({
                "profile": Value::Null,
                "errors": [item.to_json()],
                "warnings": [],
            }),
        }),
        _ => Err(MethodError::bad_params(
            "text",
            "sine.profile.import takes the rows as \"text\" or the file's \"path\", one of the \
             two",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn golden_profile() -> SineProfile {
        SineProfile {
            label: "1 g".into(),
            rows: vec![
                ProfileRow::displacement(20.0, 0.0097),
                ProfileRow::accel(45.0, 1.0)
                    .with_alarm_db(1.0, 1.0)
                    .with_abort_db(2.0, 2.0),
                ProfileRow::accel(2000.0, 1.0),
            ],
            global_alarm_db: (1.5, 1.5),
            global_abort_db: (3.0, 3.0),
        }
    }

    #[test]
    fn the_peaks_come_from_the_segment_ends_inside_the_band() {
        let p = golden_profile();
        let pk = peaks(&p, (20.0, 2000.0)).unwrap();
        assert_eq!(sig(pk.accel_g, 4), 1.004, "the displacement leg at 45 Hz");
        assert_eq!(sig(pk.disp_pp_in, 4), 0.0097);
        // Inside the accel plateau only: 1 g, and the displacement at 45 Hz.
        let pk = peaks(&p, (100.0, 2000.0)).unwrap();
        assert_eq!(pk.accel_g, 1.0);
        assert!(pk.disp_pp_in < 0.0097);
    }

    #[test]
    fn a_crossover_is_where_neighbouring_quantities_meet() {
        let c = crossovers(&golden_profile());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].from, Quantity::DisplacementInPp);
        assert!((c[0].freq_hz - 44.9).abs() < 0.1, "{}", c[0].freq_hz);
    }

    #[test]
    fn an_import_names_the_row_it_could_not_read() {
        let v = import_text("freq_hz,accel\n20,1\nnope,1\n", "text");
        assert_eq!(v["profile"], Value::Null);
        assert_eq!(v["errors"][0]["reason"], "profile_file");
        assert_eq!(v["errors"][0]["row"], 1);
        let v = import_text("freq_hz,accel,velocity\n20,1,\n40,1,2\n", "text");
        assert_eq!(v["errors"][0]["reason"], "quantity_ambiguous");
        assert_eq!(v["errors"][0]["row"], 1);
    }

    #[test]
    fn a_view_point_set_holds_every_row_inside_the_band() {
        let f = view_points(&golden_profile(), (20.0, 2000.0));
        assert!(f.contains(&45.0));
        assert_eq!((f[0], *f.last().unwrap()), (20.0, 2000.0));
        assert!(f.windows(2).all(|w| w[0] < w[1]));
    }
}
