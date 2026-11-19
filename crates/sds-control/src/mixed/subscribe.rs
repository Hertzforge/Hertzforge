//! Mixed Mode display subscriptions (schema/PROTOCOL.md, "Mixed Mode Display"):
//! what one connection asks to be sent, parsed and checked, and the `mixed.data`
//! frame built for it from a [`MixedView`]. Random's form, with `parts` for the
//! per-part histories; a key a quantity does not take is `bad_params` naming it.

use super::view::{self, MixedView};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use crate::views::{self, Axis, Role, Row, Scale, Series, Status};
use hertzforge_mixed::MixedDisplay;
use serde_json::{json, Map, Value};

/// The quantities, in the order the result lists them.
pub const QUANTITIES: [&str; 12] = [
    "control",
    "residual",
    "drive",
    "response",
    "time",
    "coherence",
    "identification",
    "amplitude",
    "frequency",
    "level",
    "law",
    "readings",
];
pub const DEFAULT_RATE_HZ: f64 = 5.0;
pub const MIN_RATE_HZ: f64 = 0.5;
pub const MAX_RATE_HZ: f64 = 10.0;
pub const MAX_PLOTS: usize = 64;
pub const MAX_ID_CHARS: usize = 64;
pub const DEFAULT_POINTS: usize = 1024;
pub const MIN_POINTS: usize = 64;
pub const MAX_POINTS: usize = 8192;
/// The history window of the per-part plots, s, and its most points.
pub const HISTORY_S: f64 = 60.0;
pub const HISTORY_POINTS: usize = 1200;

/// What a plot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Control,
    Residual,
    Drive,
    Response,
    Time,
    Coherence,
    Identification,
    Amplitude,
    Frequency,
    Level,
    Law,
    Readings,
}

impl Quantity {
    pub fn parse(name: &str) -> Option<Quantity> {
        QUANTITIES.iter().position(|q| *q == name).map(|i| {
            [
                Quantity::Control,
                Quantity::Residual,
                Quantity::Drive,
                Quantity::Response,
                Quantity::Time,
                Quantity::Coherence,
                Quantity::Identification,
                Quantity::Amplitude,
                Quantity::Frequency,
                Quantity::Level,
                Quantity::Law,
                Quantity::Readings,
            ][i]
        })
    }

    pub fn name(self) -> &'static str {
        QUANTITIES[self as usize]
    }

    /// Whether `channels` applies.
    pub fn has_channels(self) -> bool {
        matches!(self, Quantity::Response | Quantity::Time)
    }

    /// Whether `parts` applies.
    pub fn has_parts(self) -> bool {
        matches!(
            self,
            Quantity::Amplitude | Quantity::Frequency | Quantity::Level
        )
    }

    /// The part kinds the quantity takes.
    fn takes(self, kind: &str) -> bool {
        match self {
            Quantity::Amplitude => kind == "tone",
            Quantity::Frequency => kind == "tone" || kind == "narrowband",
            Quantity::Level => kind == "narrowband",
            _ => false,
        }
    }
}

/// Which channels a plot shows, resolved against the run's channels on every frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channels {
    All,
    Control,
    Limited,
    Ids(Vec<u32>),
}

impl Channels {
    fn to_json(&self) -> Value {
        match self {
            Channels::All => Value::from("all"),
            Channels::Control => Value::from("control"),
            Channels::Limited => Value::from("limited"),
            Channels::Ids(ids) => Value::Array(ids.iter().map(|i| Value::from(*i)).collect()),
        }
    }

    /// The positions in `v.channels` (table order) this selects.
    pub fn resolve(&self, v: &MixedView, limited: &[u32]) -> Vec<usize> {
        v.channels
            .iter()
            .enumerate()
            .filter(|(_, c)| match self {
                Channels::All => true,
                Channels::Control => c.role == "control",
                Channels::Limited => limited.contains(&c.index),
                Channels::Ids(ids) => ids.contains(&c.index),
            })
            .map(|(i, _)| i)
            .collect()
    }
}

/// Which parts a plot shows, resolved against the run's parts on every frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parts {
    All,
    Tones,
    Bands,
    Ids(Vec<String>),
}

impl Parts {
    fn to_json(&self) -> Value {
        match self {
            Parts::All => Value::from("all"),
            Parts::Tones => Value::from("tones"),
            Parts::Bands => Value::from("bands"),
            Parts::Ids(ids) => Value::Array(ids.iter().map(|i| Value::from(i.as_str())).collect()),
        }
    }

    /// The parts' positions this selects, in the parts' order: a part the
    /// quantity does not take, or that the run does not have, left out.
    pub fn resolve(&self, v: &MixedView, q: Quantity) -> Vec<usize> {
        v.parts
            .iter()
            .enumerate()
            .filter(|(_, p)| q.takes(p.kind))
            .filter(|(_, p)| match self {
                Parts::All => true,
                Parts::Tones => p.kind == "tone",
                Parts::Bands => p.kind == "narrowband",
                Parts::Ids(ids) => ids.contains(&p.id),
            })
            .map(|(i, _)| i)
            .collect()
    }
}

/// One plot of a subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub id: String,
    pub quantity: Quantity,
    pub channels: Option<Channels>,
    pub parts: Option<Parts>,
    pub points: Option<usize>,
}

/// What one connection asked to be sent, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Subscription {
    pub rate_hz: f64,
    pub plots: Vec<Plot>,
}

impl Subscription {
    pub fn period(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / self.rate_hz)
    }
}

fn bad(param: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(param, message)
}

/// `mixed.subscribe`'s params; `None` is the default subscription (`mixed.live`).
pub fn parse(params: &Map<String, Value>) -> Result<Option<Subscription>, MethodError> {
    let default = match params.get("default") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("default", "\"default\" is true or false")),
    };
    if default {
        if params.get("plots").is_some_and(|p| !p.is_null()) {
            return Err(bad(
                "plots",
                "the default subscription takes no plots; send either \"default\": true or a \
                 list of plots",
            ));
        }
        return Ok(None);
    }
    let rate_hz = match params.get("rate_hz") {
        None | Some(Value::Null) => DEFAULT_RATE_HZ,
        Some(v) => match v.as_f64() {
            Some(x) if x.is_finite() && (MIN_RATE_HZ..=MAX_RATE_HZ).contains(&x) => x,
            _ => {
                return Err(bad(
                    "rate_hz",
                    format!(
                        "\"rate_hz\" is a number of frames per second from {} to {}",
                        num(MIN_RATE_HZ),
                        num(MAX_RATE_HZ)
                    ),
                ))
            }
        },
    };
    let list = match params.get("plots") {
        Some(Value::Array(a)) => a,
        None | Some(Value::Null) => {
            return Err(bad(
                "plots",
                "a subscription lists its plots (\"plots\", possibly empty), or asks for the \
                 default with \"default\": true",
            ))
        }
        Some(_) => return Err(bad("plots", "\"plots\" is a list")),
    };
    if list.len() > MAX_PLOTS {
        return Err(bad(
            "plots",
            format!(
                "a subscription holds at most {MAX_PLOTS} plots; {} were sent",
                list.len()
            ),
        ));
    }
    let mut plots: Vec<Plot> = Vec::with_capacity(list.len());
    for (i, p) in list.iter().enumerate() {
        let at = |field: &str| format!("plots[{i}].{field}");
        let o = p
            .as_object()
            .ok_or_else(|| bad(&format!("plots[{i}]"), "each plot is an object"))?;
        if let Some(k) = o.keys().find(|k| {
            !matches!(
                k.as_str(),
                "id" | "quantity" | "channels" | "parts" | "points"
            )
        }) {
            return Err(bad(
                &at(k),
                format!(
                    "a plot takes \"id\", \"quantity\", \"channels\", \"parts\" and \"points\", \
                     not \"{k}\""
                ),
            ));
        }
        let id = match o.get("id") {
            Some(Value::String(s)) if !s.is_empty() && s.chars().count() <= MAX_ID_CHARS => {
                s.clone()
            }
            _ => {
                return Err(bad(
                    &at("id"),
                    format!("a plot's \"id\" is text of 1 to {MAX_ID_CHARS} characters"),
                ))
            }
        };
        if plots.iter().any(|q| q.id == id) {
            return Err(bad(
                &at("id"),
                format!("the plot id \"{id}\" is used twice; each plot's id is its own"),
            ));
        }
        let quantity = match o.get("quantity") {
            Some(Value::String(s)) => Quantity::parse(s),
            _ => None,
        }
        .ok_or_else(|| {
            bad(
                &at("quantity"),
                format!("a plot's \"quantity\" is one of {}", QUANTITIES.join(", ")),
            )
        })?;
        let words = "\"channels\" is a list of channel indexes, or \"all\", \"control\" or \
                     \"limited\"";
        let channels = match (quantity.has_channels(), o.get("channels")) {
            (true, None | Some(Value::Null)) => Some(Channels::All),
            (true, Some(Value::String(s))) => Some(match s.as_str() {
                "all" => Channels::All,
                "control" => Channels::Control,
                "limited" => Channels::Limited,
                _ => return Err(bad(&at("channels"), words)),
            }),
            (true, Some(Value::Array(a))) => {
                let mut ids: Vec<u32> = Vec::with_capacity(a.len());
                for x in a {
                    let id = x
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| {
                            bad(&at("channels"), "a channel index is a whole number from 0")
                        })?;
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                Some(Channels::Ids(ids))
            }
            (true, Some(_)) => return Err(bad(&at("channels"), words)),
            (false, None | Some(Value::Null)) => None,
            (false, Some(_)) => {
                return Err(bad(
                    &at("channels"),
                    format!(
                        "\"channels\" applies to response and time plots, not to {}",
                        quantity.name()
                    ),
                ))
            }
        };
        let pwords = "\"parts\" is a list of part ids, or \"all\", \"tones\" or \"bands\"";
        let parts = match (quantity.has_parts(), o.get("parts")) {
            (true, None | Some(Value::Null)) => Some(Parts::All),
            (true, Some(Value::String(s))) => Some(match s.as_str() {
                "all" => Parts::All,
                "tones" => Parts::Tones,
                "bands" => Parts::Bands,
                _ => return Err(bad(&at("parts"), pwords)),
            }),
            (true, Some(Value::Array(a))) => {
                let mut ids: Vec<String> = Vec::with_capacity(a.len());
                for x in a {
                    let id = x
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| bad(&at("parts"), "a part is named by its id"))?;
                    if !ids.iter().any(|i| i == id) {
                        ids.push(id.to_owned());
                    }
                }
                Some(Parts::Ids(ids))
            }
            (true, Some(_)) => return Err(bad(&at("parts"), pwords)),
            (false, None | Some(Value::Null)) => None,
            (false, Some(_)) => {
                return Err(bad(
                    &at("parts"),
                    format!(
                        "\"parts\" applies to amplitude, frequency and level plots, not to {}",
                        quantity.name()
                    ),
                ))
            }
        };
        let points = match (quantity, o.get("points")) {
            (Quantity::Time, None | Some(Value::Null)) => Some(DEFAULT_POINTS),
            (Quantity::Time, Some(v)) => match v.as_u64() {
                Some(n) if (MIN_POINTS as u64..=MAX_POINTS as u64).contains(&n) => Some(n as usize),
                _ => {
                    return Err(bad(
                        &at("points"),
                        format!("\"points\" is a whole number from {MIN_POINTS} to {MAX_POINTS}"),
                    ))
                }
            },
            (_, None | Some(Value::Null)) => None,
            (_, Some(_)) => {
                return Err(bad(
                    &at("points"),
                    format!(
                        "\"points\" applies to time plots, not to {}",
                        quantity.name()
                    ),
                ))
            }
        };
        plots.push(Plot {
            id,
            quantity,
            channels,
            parts,
            points,
        });
    }
    Ok(Some(Subscription { rate_hz, plots }))
}

fn quantities() -> Value {
    Value::Array(QUANTITIES.iter().map(|q| Value::from(*q)).collect())
}

/// The `mixed.subscribe` result: the subscription with every default filled in.
pub fn result_json(sub: Option<&Subscription>) -> Value {
    let Some(sub) = sub else {
        return json!({
            "default": true, "rate_hz": num(DEFAULT_RATE_HZ), "plots": null,
            "quantities": quantities(),
        });
    };
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            let mut m = Map::new();
            m.insert("id".into(), Value::from(p.id.as_str()));
            m.insert("quantity".into(), Value::from(p.quantity.name()));
            if let Some(c) = &p.channels {
                m.insert("channels".into(), c.to_json());
            }
            if let Some(c) = &p.parts {
                m.insert("parts".into(), c.to_json());
            }
            if let Some(n) = p.points {
                m.insert("points".into(), Value::from(n));
            }
            Value::Object(m)
        })
        .collect();
    let mut m = Map::new();
    m.insert("default".into(), Value::from(false));
    m.insert("rate_hz".into(), num(sub.rate_hz));
    m.insert("plots".into(), Value::Array(plots));
    m.insert("quantities".into(), quantities());
    Value::Object(m)
}

fn centers(d: &MixedDisplay) -> Vec<f64> {
    (0..d.grid.lines)
        .map(|k| d.grid.f0_hz + k as f64 * d.grid.df_hz)
        .collect()
}

fn f64s(y: &[f32]) -> Vec<f64> {
    y.iter().map(|x| f64::from(*x)).collect()
}

fn stepped(name: &str, role: Role, d: &MixedDisplay, y: &[f32]) -> Series {
    let mut s = Series::new(name, role, centers(d), f64s(y));
    s.edges = Some(d.grid.edges_hz());
    s
}

/// A limit channel's rows, `(frequency Hz, ceiling)`, as configured.
pub type LimitRows = (u32, Vec<(f64, f64)>);

/// A limited channel's ceiling on the display's lines its rows cover (Random's
/// construction, log-log between rows), stepped on those lines' own edges, as
/// Random's response view draws it; `None` when no line is covered.
fn limit_series(d: &MixedDisplay, rows: &[(f64, f64)]) -> Option<Series> {
    let g = &d.grid;
    if g.lines == 0 || g.df_hz <= 0.0 {
        return None;
    }
    let k0 = (g.f0_hz / g.df_hz).round() as usize;
    let ceiling = crate::random::config::ceiling_on_grid(rows, k0 + g.lines, g.df_hz);
    let keep: Vec<usize> = (0..g.lines)
        .filter(|i| ceiling.get(k0 + i).is_some_and(|c| c.is_finite()))
        .collect();
    let (first, last) = (*keep.first()?, *keep.last()?);
    let x: Vec<f64> = (first..=last)
        .map(|i| g.f0_hz + i as f64 * g.df_hz)
        .collect();
    let y: Vec<f64> = (first..=last)
        .map(|i| ceiling[k0 + i])
        .map(|c| if c.is_finite() { c } else { f64::NAN })
        .collect();
    let edges = g.edges_hz()[first..=last + 1].to_vec();
    let mut s = Series::new("Limit", Role::Limit, x, y);
    s.edges = Some(edges);
    Some(s)
}

/// One channel's response spectrum (tones in), and its limit when it is a
/// limit channel (a control channel too).
fn response_view(
    v: &MixedView,
    d: &MixedDisplay,
    ch: u32,
    limits: &[LimitRows],
    arrays: &mut Arrays,
) -> Value {
    let c = v.channels.iter().find(|c| c.index == ch);
    let title = c.map_or(format!("Channel {ch}"), |c| {
        if c.description.is_empty() {
            format!("Channel {ch}")
        } else {
            format!("Channel {ch}: {}", c.description)
        }
    });
    let unit = c.map_or("g".to_owned(), |c| c.unit.clone());
    let series = d
        .channels
        .iter()
        .find(|x| x.channel_index == ch)
        .filter(|x| !x.control.is_empty())
        .map(|x| vec![stepped("Response", Role::Measured, d, &x.control)])
        .unwrap_or_default();
    let mut series = series;
    series.extend(
        limits
            .iter()
            .filter(|(c, _)| *c == ch)
            .filter_map(|(_, rows)| limit_series(d, rows)),
    );
    views::spectrum(
        arrays,
        &title,
        &Axis::new("Frequency", Some("Hz"), Scale::Log),
        &Axis::new("PSD", Some(&format!("{unit}²/Hz")), Scale::Log),
        series,
        Some("frequency"),
    )
}

/// The identification's coherence on the band's lines.
fn coherence_view(d: &MixedDisplay, arrays: &mut Arrays) -> Vec<Value> {
    let Some(id) = d
        .identification
        .as_ref()
        .filter(|i| !i.coherence.is_empty())
    else {
        return Vec::new();
    };
    vec![views::spectrum(
        arrays,
        "Coherence",
        &Axis::new("Frequency", Some("Hz"), Scale::Log),
        &Axis::new("Coherence", None, Scale::Linear),
        vec![stepped("Coherence", Role::Measured, d, &id.coherence)],
        Some("frequency"),
    )]
}

/// The identification's gain, and where the tones may trust it.
fn identification_view(d: &MixedDisplay, arrays: &mut Arrays) -> Vec<Value> {
    let Some(id) = d.identification.as_ref().filter(|i| !i.h_mag2.is_empty()) else {
        return Vec::new();
    };
    let gain: Vec<f32> = id.h_mag2.iter().map(|x| x.max(0.0).sqrt()).collect();
    let mut series = vec![stepped("Gain", Role::Measured, d, &gain)];
    if !id.trusted.is_empty() {
        let trusted: Vec<f32> = gain
            .iter()
            .zip(&id.trusted)
            .map(|(g, t)| if *t { *g } else { f32::NAN })
            .collect();
        series.push(stepped("Trusted", Role::Reference, d, &trusted));
    }
    vec![views::spectrum(
        arrays,
        "Identification Gain",
        &Axis::new("Frequency", Some("Hz"), Scale::Log),
        &Axis::new("Gain", None, Scale::Log),
        series,
        Some("frequency"),
    )]
}

/// The last [`HISTORY_S`] of a history, at most [`HISTORY_POINTS`] points.
fn window(t: &[f64]) -> std::ops::Range<usize> {
    let Some(last) = t.last() else {
        return 0..0;
    };
    let from = t.partition_point(|x| *x < last - HISTORY_S);
    from.max(t.len().saturating_sub(HISTORY_POINTS))..t.len()
}

fn time_axis() -> Axis {
    Axis::new("Time", Some("s"), Scale::Linear)
}

fn line(name: &str, role: Role, t: &[f64], y: &[f32]) -> Series {
    Series::new(name, role, t.to_vec(), f64s(y))
}

fn scaled(y: &[f32], db: f64) -> Vec<f32> {
    let k = 10f64.powf(db / 20.0) as f32;
    y.iter().map(|x| x * k).collect()
}

fn amplitude_view(
    name: &str,
    h: &hertzforge_mixed::ToneHistory,
    bands: ((f64, f64), (f64, f64)),
    arrays: &mut Arrays,
) -> Value {
    let r = window(&h.t_s);
    let t = &h.t_s[r.clone()];
    let target = &h.target_g[r.clone()];
    let ((al, ah), (bl, bh)) = bands;
    let series = vec![
        line("Measured", Role::Measured, t, &h.measured_g[r.clone()]),
        line("Target", Role::Reference, t, target),
        line("Alarm high", Role::Tolerance, t, &scaled(target, ah)),
        line("Alarm low", Role::Tolerance, t, &scaled(target, -al)),
        line("Abort high", Role::Abort, t, &scaled(target, bh)),
        line("Abort low", Role::Abort, t, &scaled(target, -bl)),
    ];
    views::spectrum(
        arrays,
        name,
        &time_axis(),
        &Axis::new("Amplitude", Some("g"), Scale::Log),
        series,
        Some("time"),
    )
}

fn frequency_view(name: &str, t_all: &[f64], f_all: &[f32], arrays: &mut Arrays) -> Value {
    let r = window(t_all);
    views::spectrum(
        arrays,
        name,
        &time_axis(),
        &Axis::new("Frequency", Some("Hz"), Scale::Linear),
        vec![line(
            "Frequency",
            Role::Reference,
            &t_all[r.clone()],
            &f_all[r],
        )],
        Some("time"),
    )
}

fn level_view(name: &str, h: &hertzforge_mixed::BandHistory, arrays: &mut Arrays) -> Value {
    let r = window(&h.t_s);
    let t = &h.t_s[r.clone()];
    views::spectrum(
        arrays,
        name,
        &time_axis(),
        &Axis::new("Level", Some("g²/Hz"), Scale::Log),
        vec![
            line("Measured", Role::Measured, t, &h.measured[r.clone()]),
            line("Reference", Role::Reference, t, &h.reference[r]),
        ],
        Some("time"),
    )
}

/// The `law` readings: the floor law's, then each tone instance's.
fn law_readings(v: &MixedView) -> Vec<Value> {
    let Some(l) = v.law.as_ref().filter(|l| l.built) else {
        return Vec::new();
    };
    let status = |n: u32| if n == 0 { Status::Pass } else { Status::Warn };
    let mut out = Vec::new();
    let mut rows: Vec<Row> = l
        .floor_values
        .iter()
        .map(|(k, x)| Row::number(k, *x, None, Status::Info))
        .collect();
    rows.push(Row::number(
        "Unusable calls in a row",
        f64::from(l.floor.misses),
        None,
        status(l.floor.misses),
    ));
    rows.push(Row::number(
        "Caps bound",
        l.floor.capped as f64,
        None,
        Status::Info,
    ));
    out.push(views::reading("Floor law", rows));
    for f in v.parts.iter().filter(|f| f.kind == "tone") {
        let row = v
            .status
            .as_ref()
            .and_then(|s| s.parts.iter().find(|r| r.id == f.id));
        let vals = l
            .tone_values
            .iter()
            .find(|(id, _)| id == &f.id)
            .map(|(_, x)| x.clone())
            .unwrap_or_default();
        let misses = row.map_or(0, |r| r.misses);
        let mut rows: Vec<Row> = vals
            .iter()
            .map(|(k, x)| Row::number(k, *x, None, Status::Info))
            .collect();
        rows.push(Row::number(
            "Unusable calls in a row",
            f64::from(misses),
            None,
            status(misses),
        ));
        rows.push(Row::number(
            "Caps bound",
            row.map_or(0, |r| r.capped) as f64,
            None,
            Status::Info,
        ));
        out.push(views::reading(&format!("Tone law: {}", f.name), rows));
    }
    out
}

/// A tone's `(alarm, abort)` bands, dB, by part id.
pub type ToneBands<'a> = dyn Fn(&str) -> ((f64, f64), (f64, f64)) + 'a;

/// The `mixed.data` frame for one connection's subscription. `bands_db` gives
/// each tone's `(alarm, abort)` bands by part id, for its amplitude plot.
pub fn data_json(
    v: &MixedView,
    sub: &Subscription,
    tone_bands: &ToneBands<'_>,
    limits: &[LimitRows],
    arrays: &mut Arrays,
) -> Value {
    let limited: Vec<u32> = limits.iter().map(|(c, _)| *c).collect();
    let mut m = view::live_head(v);
    let engaged = v.status.is_some();
    let d = v.display.as_deref();
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            let mut channels = Value::Null;
            let mut parts = Value::Null;
            let views: Vec<Value> = match (p.quantity, d) {
                (Quantity::Control, Some(d)) if engaged => vec![view::control_view(v, d, arrays)],
                (Quantity::Residual, Some(d)) if engaged => {
                    vec![view::residual_view(v, d, arrays)]
                }
                (Quantity::Drive, Some(d)) if engaged => vec![view::drive_view(d, arrays)],
                (Quantity::Coherence, Some(d)) => coherence_view(d, arrays),
                (Quantity::Identification, Some(d)) => identification_view(d, arrays),
                (Quantity::Law, _) => law_readings(v),
                (Quantity::Readings, _) => vec![view::run_reading(v)],
                (Quantity::Response | Quantity::Time, _) => {
                    let pos = p
                        .channels
                        .as_ref()
                        .unwrap_or(&Channels::All)
                        .resolve(v, &limited);
                    channels = Value::Array(pos.iter().map(|i| Value::from(v.channels[*i].index)).collect());
                    match (p.quantity, d) {
                        (Quantity::Response, Some(d)) if engaged => pos
                            .iter()
                            .map(|i| response_view(v, d, v.channels[*i].index, limits, arrays))
                            .collect(),
                        // The acquired samples reach the frame from the run thread's
                        // tap; until a run has them there is nothing to draw.
                        _ => Vec::new(),
                    }
                }
                (Quantity::Amplitude | Quantity::Frequency | Quantity::Level, _) => {
                    let pos = p.parts.as_ref().unwrap_or(&Parts::All).resolve(v, p.quantity);
                    parts = Value::Array(pos.iter().map(|i| Value::from(v.parts[*i].id.as_str())).collect());
                    let mut out = Vec::new();
                    if let Some(d) = d {
                        for i in pos {
                            let f = &v.parts[i];
                            let tone = d.tones.iter().find(|h| h.part == f.id && !h.t_s.is_empty());
                            let band = d.bands.iter().find(|h| h.part == f.id && !h.t_s.is_empty());
                            match (p.quantity, tone, band) {
                                (Quantity::Amplitude, Some(h), _) => {
                                    out.push(amplitude_view(&f.name, h, tone_bands(&f.id), arrays))
                                }
                                (Quantity::Frequency, Some(h), _) => {
                                    out.push(frequency_view(&f.name, &h.t_s, &h.freq_hz, arrays))
                                }
                                (Quantity::Frequency, None, Some(h)) => {
                                    out.push(frequency_view(&f.name, &h.t_s, &h.center_hz, arrays))
                                }
                                (Quantity::Level, _, Some(h)) => {
                                    out.push(level_view(&f.name, h, arrays))
                                }
                                _ => {}
                            }
                        }
                    }
                    out
                }
                _ => Vec::new(),
            };
            json!({"id": p.id, "quantity": p.quantity.name(), "channels": channels, "parts": parts, "views": views})
        })
        .collect();
    m.insert("plots".into(), Value::Array(plots));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn refused(v: Value) -> String {
        let e = parse(&params(v)).unwrap_err();
        assert_eq!(e.reason, Some("bad_params"), "{e:?}");
        e.detail.as_ref().unwrap()["param"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn every_key_a_quantity_does_not_take_is_refused_by_its_path() {
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "control", "channels": [1]}]})),
            "plots[0].channels"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "response", "parts": "all"}]})),
            "plots[0].parts"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "amplitude", "points": 128}]})),
            "plots[0].points"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "phase"}]})),
            "plots[0].quantity"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "level", "parts": "floor"}]})),
            "plots[0].parts"
        );
        let s = parse(&params(json!({"plots": [
            {"id": "p", "quantity": "amplitude"},
            {"id": "t", "quantity": "time"}
        ]})))
        .unwrap()
        .unwrap();
        assert_eq!(s.plots[0].parts, Some(Parts::All));
        assert_eq!(s.plots[0].channels, None);
        assert_eq!(s.plots[1].points, Some(DEFAULT_POINTS));
        assert_eq!(s.plots[1].channels, Some(Channels::All));
    }

    #[test]
    fn every_quantity_round_trips_its_name() {
        for q in QUANTITIES {
            assert_eq!(Quantity::parse(q).unwrap().name(), q);
        }
    }
}
