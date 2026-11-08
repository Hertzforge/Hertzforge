//! Classical shock display subscriptions (schema/PROTOCOL.md, "Classical Shock
//! Display"): what one connection asks to be sent, parsed and checked, and the
//! `shock.data` frame built for it.
//!
//! A subscription narrows the display and nothing else: the run computes what it
//! computes for each pulse, and a frame is only assembled from what a connection's
//! plots show.

use super::view::{self, IdentFigures, LawReading, PulseFigures, Row, ShockView};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use serde_json::{json, Map, Value};

/// The quantities this service offers, in the order the result lists them.
pub const QUANTITIES: [&str; 9] = [
    "pulse",
    "velocity",
    "displacement",
    "srs",
    "spectrum",
    "identification",
    "law",
    "readings",
    "pulses",
];
/// Frames per second at most, when `rate_hz` is not given, and its bounds.
pub const DEFAULT_RATE_HZ: f64 = 5.0;
pub const MIN_RATE_HZ: f64 = 0.5;
pub const MAX_RATE_HZ: f64 = 10.0;
/// The most plots one subscription may hold.
pub const MAX_PLOTS: usize = 64;
/// The longest plot id, in characters.
pub const MAX_ID_CHARS: usize = 64;
/// Points per time history: the default and the bounds.
pub const DEFAULT_POINTS: usize = 1024;
pub const MIN_POINTS: usize = 64;
pub const MAX_POINTS: usize = 8192;

/// What a plot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Pulse,
    Velocity,
    Displacement,
    Srs,
    Spectrum,
    Identification,
    Law,
    Readings,
    Pulses,
}

impl Quantity {
    pub fn parse(name: &str) -> Option<Quantity> {
        Some(match name {
            "pulse" => Quantity::Pulse,
            "velocity" => Quantity::Velocity,
            "displacement" => Quantity::Displacement,
            "srs" => Quantity::Srs,
            "spectrum" => Quantity::Spectrum,
            "identification" => Quantity::Identification,
            "law" => Quantity::Law,
            "readings" => Quantity::Readings,
            "pulses" => Quantity::Pulses,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Quantity::Pulse => "pulse",
            Quantity::Velocity => "velocity",
            Quantity::Displacement => "displacement",
            Quantity::Srs => "srs",
            Quantity::Spectrum => "spectrum",
            Quantity::Identification => "identification",
            Quantity::Law => "law",
            Quantity::Readings => "readings",
            Quantity::Pulses => "pulses",
        }
    }

    /// Whether `points` applies: the time histories.
    pub fn is_time(self) -> bool {
        matches!(
            self,
            Quantity::Pulse | Quantity::Velocity | Quantity::Displacement
        )
    }
}

/// One plot of a subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub id: String,
    pub quantity: Quantity,
    /// Set for the time histories only.
    pub points: Option<usize>,
}

/// What one connection asked to be sent, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Subscription {
    pub rate_hz: f64,
    pub plots: Vec<Plot>,
}

impl Subscription {
    /// The least time between two frames.
    pub fn period(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / self.rate_hz)
    }
}

fn bad(param: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(param, message)
}

/// `points` for a plot of `quantity`: the default for a time history.
pub fn parse_points(
    quantity_time: bool,
    v: Option<&Value>,
    param: &str,
) -> Result<Option<usize>, MethodError> {
    match (quantity_time, v) {
        (true, None | Some(Value::Null)) => Ok(Some(DEFAULT_POINTS)),
        (true, Some(v)) => match v.as_u64() {
            Some(n) if (MIN_POINTS as u64..=MAX_POINTS as u64).contains(&n) => Ok(Some(n as usize)),
            _ => Err(bad(
                param,
                format!("\"points\" is a whole number from {MIN_POINTS} to {MAX_POINTS}"),
            )),
        },
        (false, None | Some(Value::Null)) => Ok(None),
        (false, Some(_)) => Err(bad(
            param,
            "\"points\" applies to the pulse, velocity and displacement plots",
        )),
    }
}

/// `shock.subscribe`'s params; `None` is the default subscription (no `shock.data`).
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
        if let Some(k) = o
            .keys()
            .find(|k| !matches!(k.as_str(), "id" | "quantity" | "points"))
        {
            return Err(bad(
                &at(k),
                format!(
                    "a plot has an id, a quantity and, for a time history, points; not \"{k}\""
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
        let points = parse_points(quantity.is_time(), o.get("points"), &at("points"))?;
        plots.push(Plot {
            id,
            quantity,
            points,
        });
    }
    Ok(Some(Subscription { rate_hz, plots }))
}

fn quantities() -> Value {
    Value::Array(QUANTITIES.iter().map(|q| Value::from(*q)).collect())
}

/// The `shock.subscribe` result: the subscription with every default filled in.
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
            if let Some(n) = p.points {
                m.insert("points".into(), Value::from(n));
            }
            Value::Object(m)
        })
        .collect();
    json!({
        "default": false,
        "rate_hz": num(sub.rate_hz),
        "plots": plots,
        "quantities": quantities(),
    })
}

/// What a `shock.data` frame is assembled from: the run's view, the latest pulse's
/// figures, the identification, the law's last values and the rows.
pub struct DataContext<'a> {
    pub view: &'a ShockView,
    pub last: Option<&'a PulseFigures>,
    pub identification: Option<&'a IdentFigures>,
    pub law: &'a [LawReading],
    pub rows: &'a [Row],
}

/// The views of one plot. A plot of the latest pulse is empty until a pulse has
/// fired, `identification` until identification has ended.
pub fn plot_views(p: &Plot, ctx: &DataContext<'_>, arrays: &mut Arrays) -> Vec<Value> {
    let pts = p.points;
    match p.quantity {
        Quantity::Pulse => ctx.last.map_or_else(Vec::new, |f| {
            vec![
                view::pulse_view(&f.time, pts, arrays),
                view::drive_view(f.time.start, f.time.fs_hz, &f.drive, pts, arrays),
            ]
        }),
        Quantity::Velocity | Quantity::Displacement => {
            let which = if p.quantity == Quantity::Velocity {
                view::Kinematic::Velocity
            } else {
                view::Kinematic::Displacement
            };
            let series = ctx.last.and_then(|f| {
                let s = if which == view::Kinematic::Velocity {
                    f.velocity.as_ref()
                } else {
                    f.displacement.as_ref()
                }?;
                Some((f.time.start, f.time.fs_hz, s))
            });
            series.map_or_else(Vec::new, |(start, fs, (reference, measured))| {
                vec![view::kinematic_view(
                    which,
                    start,
                    fs,
                    reference,
                    Some(measured),
                    pts,
                    arrays,
                )]
            })
        }
        Quantity::Srs => ctx
            .last
            .and_then(|f| f.srs.as_ref())
            .map_or_else(Vec::new, |s| vec![view::srs_view(s, arrays)]),
        Quantity::Spectrum => ctx
            .last
            .and_then(|f| f.spectrum.as_ref())
            .map_or_else(Vec::new, |(f, r, y)| {
                vec![view::spectrum_view(f, r, Some(y), arrays)]
            }),
        Quantity::Identification => ctx
            .identification
            .map_or_else(Vec::new, |i| view::identification_views(i, arrays)),
        Quantity::Law => vec![view::law_view(ctx.law)],
        Quantity::Readings => vec![view::readings_view(ctx.view)],
        Quantity::Pulses => vec![view::pulses_view(ctx.rows)],
    }
}

/// The `shock.data` message data for `sub`: the live fields and each plot.
pub fn data_json(ctx: &DataContext<'_>, sub: &Subscription, arrays: &mut Arrays) -> Value {
    let mut data = view::live_json(ctx.view);
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "quantity": p.quantity.name(),
                "views": plot_views(p, ctx, arrays),
            })
        })
        .collect();
    if let Value::Object(m) = &mut data {
        m.insert("plots".into(), Value::Array(plots));
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn points_apply_to_the_time_histories_only() {
        let s = parse(&params(json!({"plots": [
            {"id": "p", "quantity": "pulse"},
            {"id": "s", "quantity": "srs"},
        ]})))
        .unwrap()
        .unwrap();
        assert_eq!(s.plots[0].points, Some(DEFAULT_POINTS));
        assert_eq!(s.plots[1].points, None);
        for bad in [
            json!({"plots": [{"id": "s", "quantity": "srs", "points": 512}]}),
            json!({"plots": [{"id": "p", "quantity": "pulse", "points": 10}]}),
            json!({"plots": [{"id": "p", "quantity": "control"}]}),
            json!({"plots": [{"id": "p", "quantity": "pulse"}, {"id": "p", "quantity": "srs"}]}),
            json!({"plots": [{"id": "p", "quantity": "pulse", "channels": "all"}]}),
            json!({"rate_hz": 20, "plots": []}),
            json!({"default": true, "plots": []}),
        ] {
            let e = parse(&params(bad.clone())).unwrap_err();
            assert_eq!(e.reason, Some("bad_params"), "{bad}");
        }
        assert!(parse(&params(json!({"default": true}))).unwrap().is_none());
    }
}
