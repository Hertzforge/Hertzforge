//! `recorder.subscribe` and `recorder.data` (schema/PROTOCOL.md, "Recorder Display"):
//! a connection's plots, each naming a quantity, and the frames built from the run's
//! view and what the pump and the run's thread hand on.

use super::view::{live_head, run_reading, RecorderView};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use crate::views::{self, Axis, Role, Scale, Series};
use serde_json::{json, Map, Value};

pub const QUANTITIES: [&str; 4] = ["time", "spectrum", "frf", "readings"];
pub const DEFAULT_RATE_HZ: f64 = 10.0;
pub const MIN_RATE_HZ: f64 = 0.5;
pub const MAX_RATE_HZ: f64 = 10.0;
pub const MAX_PLOTS: usize = 64;
pub const MAX_ID_CHARS: usize = 64;
pub const DEFAULT_POINTS: u64 = 1024;
pub const POINTS_RANGE: (u64, u64) = (64, 8192);
pub const DEFAULT_SECONDS: f64 = 2.0;
pub const MAX_SECONDS: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Time,
    Spectrum,
    Frf,
    Readings,
}

impl Quantity {
    pub fn parse(name: &str) -> Option<Quantity> {
        Some(match name {
            "time" => Quantity::Time,
            "spectrum" => Quantity::Spectrum,
            "frf" => Quantity::Frf,
            "readings" => Quantity::Readings,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Quantity::Time => "time",
            Quantity::Spectrum => "spectrum",
            Quantity::Frf => "frf",
            Quantity::Readings => "readings",
        }
    }

    /// The keys a plot of this quantity takes beyond `id` and `quantity`.
    fn keys(self) -> &'static [&'static str] {
        match self {
            Quantity::Time => &["channels", "points", "seconds"],
            Quantity::Spectrum => &["channels"],
            Quantity::Frf | Quantity::Readings => &[],
        }
    }
}

/// Which recorded rows a plot draws.
#[derive(Debug, Clone, PartialEq)]
pub enum Channels {
    All,
    List(Vec<u32>),
}

impl Channels {
    pub fn to_json(&self) -> Value {
        match self {
            Channels::All => Value::from("all"),
            Channels::List(l) => json!(l),
        }
    }

    /// The rows it resolves to among `rows`, in table order.
    pub fn resolve(&self, rows: &[u32]) -> Vec<u32> {
        match self {
            Channels::All => rows.to_vec(),
            Channels::List(l) => rows.iter().copied().filter(|r| l.contains(r)).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub id: String,
    pub quantity: Quantity,
    pub channels: Channels,
    pub points: u64,
    pub seconds: f64,
}

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

/// Parse `recorder.subscribe`'s parameters: `None` is the default subscription (back to
/// `recorder.live`).
pub fn parse(params: &Map<String, Value>) -> Result<Option<Subscription>, MethodError> {
    if let Some(k) = params
        .keys()
        .find(|k| !matches!(k.as_str(), "default" | "rate_hz" | "plots"))
    {
        return Err(bad(
            k,
            format!("a subscription takes \"default\", \"rate_hz\" and \"plots\", not \"{k}\""),
        ));
    }
    let default = match params.get("default") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("default", "\"default\" is true or false")),
    };
    if default {
        if params.get("plots").is_some_and(|p| !p.is_null())
            || params.get("rate_hz").is_some_and(|p| !p.is_null())
        {
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
        let quantity = o
            .get("quantity")
            .and_then(Value::as_str)
            .and_then(Quantity::parse)
            .ok_or_else(|| {
                bad(
                    &at("quantity"),
                    format!(
                        "a plot's \"quantity\" is one of {}",
                        QUANTITIES
                            .iter()
                            .map(|q| format!("\"{q}\""))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?;
        if let Some(k) = o.keys().find(|k| {
            !matches!(k.as_str(), "id" | "quantity") && !quantity.keys().contains(&k.as_str())
        }) {
            return Err(bad(
                &at(k),
                format!("a \"{}\" plot does not take \"{k}\"", quantity.name()),
            ));
        }
        let channels = match o.get("channels") {
            None | Some(Value::Null) => Channels::All,
            Some(Value::String(s)) if s == "all" => Channels::All,
            Some(Value::Array(a)) => Channels::List(
                a.iter()
                    .map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok()))
                    .collect::<Option<Vec<u32>>>()
                    .ok_or_else(|| {
                        bad(
                            &at("channels"),
                            "\"channels\" is \"all\" or a list of recorded rows' indexes",
                        )
                    })?,
            ),
            Some(_) => {
                return Err(bad(
                    &at("channels"),
                    "\"channels\" is \"all\" or a list of recorded rows' indexes",
                ))
            }
        };
        let points = match o.get("points") {
            None | Some(Value::Null) => DEFAULT_POINTS,
            Some(v) => v
                .as_u64()
                .filter(|n| (POINTS_RANGE.0..=POINTS_RANGE.1).contains(n))
                .ok_or_else(|| {
                    bad(
                        &at("points"),
                        format!(
                            "\"points\" is a whole number from {} to {}",
                            POINTS_RANGE.0, POINTS_RANGE.1
                        ),
                    )
                })?,
        };
        let seconds = match o.get("seconds") {
            None | Some(Value::Null) => DEFAULT_SECONDS,
            Some(v) => v
                .as_f64()
                .filter(|s| s.is_finite() && *s > 0.0 && *s <= MAX_SECONDS)
                .ok_or_else(|| {
                    bad(
                        &at("seconds"),
                        format!("\"seconds\" is above 0 and at most {}", num(MAX_SECONDS)),
                    )
                })?,
        };
        plots.push(Plot {
            id,
            quantity,
            channels,
            points,
            seconds,
        });
    }
    Ok(Some(Subscription { rate_hz, plots }))
}

/// A plot as the subscription's result echoes it: the keys its quantity takes, every
/// default filled in.
pub fn plot_json(p: &Plot) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), Value::from(p.id.as_str()));
    m.insert("quantity".into(), Value::from(p.quantity.name()));
    match p.quantity {
        Quantity::Time => {
            m.insert("channels".into(), p.channels.to_json());
            m.insert("points".into(), Value::from(p.points));
            m.insert("seconds".into(), num(p.seconds));
        }
        Quantity::Spectrum => {
            m.insert("channels".into(), p.channels.to_json());
        }
        Quantity::Frf | Quantity::Readings => {}
    }
    Value::Object(m)
}

/// `recorder.subscribe`'s result.
pub fn result_json(sub: Option<&Subscription>) -> Value {
    match sub {
        None => json!({
            "default": true, "rate_hz": DEFAULT_RATE_HZ, "plots": null,
            "quantities": QUANTITIES,
        }),
        Some(s) => json!({
            "default": false,
            "rate_hz": num(s.rate_hz),
            "plots": s.plots.iter().map(plot_json).collect::<Vec<_>>(),
            "quantities": QUANTITIES,
        }),
    }
}

/// A row's spectrum: its `index`, description, unit, the line spacing and the PSD.
pub struct RowSpectrum {
    pub index: u32,
    pub description: String,
    pub unit: String,
    pub df_hz: f64,
    pub psd: Vec<f64>,
}

/// The FRF for the configured pair.
pub struct FrfData {
    pub unit: String,
    pub df_hz: f64,
    pub mag: Vec<f64>,
    pub phase_deg: Vec<f64>,
    pub coherence: Vec<f64>,
    pub valid: bool,
}

/// Everything a `recorder.data` frame draws from.
pub struct DataContext<'a> {
    pub view: &'a RecorderView,
    /// The last frames of every recorded row: the station frame of the first, and one
    /// column a row in table order.
    pub time: Option<(u64, &'a [Vec<f32>])>,
    pub spectra: &'a [RowSpectrum],
    pub frf: Option<&'a FrfData>,
}

/// One plot's views.
fn plot_views(ctx: &DataContext<'_>, p: &Plot, arrays: &mut Arrays) -> (Value, Value, Value) {
    let rows = &ctx.view.rows;
    let fs = ctx.view.grid.fs_hz;
    match p.quantity {
        Quantity::Time => {
            let want = p.channels.resolve(rows);
            let Some((first, cols)) = ctx
                .time
                .filter(|(_, c)| c.first().is_some_and(|c| !c.is_empty()))
            else {
                return (json!(want), Value::Null, json!([]));
            };
            let have = cols[0].len();
            let keep = ((p.seconds * fs).round() as usize).clamp(1, have);
            let skip = have - keep;
            let start = first + skip as u64;
            let points = (p.points as usize).min(keep);
            let span = keep.div_ceil(points);
            let n_points = keep.div_ceil(span);
            let x: Vec<f64> = (0..n_points)
                .map(|i| (start + (i * span) as u64) as f64 / fs)
                .collect();
            let unit = ctx
                .view
                .live_rows
                .iter()
                .find(|r| want.contains(&r.index))
                .map_or("g".to_owned(), |r| r.unit.clone());
            let series = want
                .iter()
                .filter_map(|index| {
                    let col = rows.iter().position(|r| r == index)?;
                    let samples = &cols.get(col)?[skip..];
                    let mut hi = Vec::with_capacity(n_points);
                    let mut lo = Vec::with_capacity(n_points);
                    for chunk in samples.chunks(span) {
                        let (mut a, mut b) = (f64::INFINITY, f64::NEG_INFINITY);
                        for v in chunk {
                            a = a.min(f64::from(*v));
                            b = b.max(f64::from(*v));
                        }
                        hi.push(b);
                        lo.push(a);
                    }
                    let mut s =
                        Series::new(&format!("Channel {index}"), Role::Measured, x.clone(), hi);
                    s.y_min = Some(lo);
                    Some(s)
                })
                .collect();
            let view = views::spectrum(
                arrays,
                "Time",
                &Axis::new("Time", Some("s"), Scale::Linear),
                &Axis::new("Amplitude", Some(&unit), Scale::Linear),
                series,
                None,
            );
            (json!(want), json!(start), json!([view]))
        }
        Quantity::Spectrum => {
            let want = p.channels.resolve(rows);
            let views: Vec<Value> = want
                .iter()
                .filter_map(|index| ctx.spectra.iter().find(|s| s.index == *index))
                .filter(|s| !s.psd.is_empty())
                .map(|s| {
                    let x: Vec<f64> = (0..s.psd.len()).map(|k| k as f64 * s.df_hz).collect();
                    let title = if s.description.is_empty() {
                        format!("Channel {}", s.index)
                    } else {
                        format!("Channel {}: {}", s.index, s.description)
                    };
                    views::spectrum(
                        arrays,
                        &title,
                        &Axis::new("Frequency", Some("Hz"), Scale::Linear),
                        &Axis::new("PSD", Some(&format!("{}²/Hz", s.unit)), Scale::Log),
                        vec![Series::new(
                            &format!("Channel {}", s.index),
                            Role::Measured,
                            x,
                            s.psd.clone(),
                        )],
                        Some("frequency"),
                    )
                })
                .collect();
            (json!(want), Value::Null, Value::Array(views))
        }
        Quantity::Frf => {
            let Some(f) = ctx.frf.filter(|f| !f.mag.is_empty()) else {
                return (Value::Null, Value::Null, json!([]));
            };
            let x: Vec<f64> = (0..f.mag.len()).map(|k| k as f64 * f.df_hz).collect();
            let fx = Axis::new("Frequency", Some("Hz"), Scale::Linear);
            let views = vec![
                views::spectrum(
                    arrays,
                    "FRF",
                    &fx,
                    &Axis::new("Magnitude", Some(&f.unit), Scale::Log),
                    vec![Series::new("H1", Role::Measured, x.clone(), f.mag.clone())],
                    Some("frequency"),
                ),
                views::spectrum(
                    arrays,
                    "Phase",
                    &fx,
                    &Axis::new("Phase", Some("deg"), Scale::Linear),
                    vec![Series::new(
                        "H1",
                        Role::Measured,
                        x.clone(),
                        f.phase_deg.clone(),
                    )],
                    Some("frequency"),
                ),
                views::spectrum(
                    arrays,
                    "Coherence",
                    &fx,
                    &Axis::new("Coherence", None, Scale::Linear),
                    vec![Series::new(
                        "Coherence",
                        Role::Measured,
                        x,
                        if f.valid {
                            f.coherence.clone()
                        } else {
                            Vec::new()
                        },
                    )],
                    Some("frequency"),
                ),
            ];
            (Value::Null, Value::Null, Value::Array(views))
        }
        Quantity::Readings => (Value::Null, Value::Null, json!([run_reading(ctx.view)])),
    }
}

/// `recorder.data`: the head and each plot's views.
pub fn data_json(ctx: &DataContext<'_>, sub: &Subscription, arrays: &mut Arrays) -> Value {
    let mut m = live_head(ctx.view);
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            let (channels, first_frame, views) = plot_views(ctx, p, arrays);
            json!({
                "id": p.id,
                "quantity": p.quantity.name(),
                "channels": channels,
                "first_frame": first_frame,
                "views": views,
            })
        })
        .collect();
    m.insert("plots".into(), Value::Array(plots));
    Value::Object(m)
}
