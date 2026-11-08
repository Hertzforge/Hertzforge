//! SRS shock display subscriptions (schema/PROTOCOL.md, "SRS Shock Display"): what
//! one connection asks to be sent, parsed and checked, and the `srs.data` frame
//! built for it. The form is classical shock's; the quantities are SRS shock's.

use super::view::{self, IdentFigures, LawReading, PulseFigures, Row, SrsView};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use serde_json::{json, Map, Value};

pub use crate::shock::subscribe::{
    DEFAULT_POINTS, DEFAULT_RATE_HZ, MAX_ID_CHARS, MAX_PLOTS, MAX_POINTS, MAX_RATE_HZ, MIN_POINTS,
    MIN_RATE_HZ,
};

/// The quantities this service offers, in the order the result lists them.
pub const QUANTITIES: [&str; 11] = [
    "srs",
    "pulse",
    "velocity",
    "displacement",
    "identification",
    "components",
    "limit_channels",
    "limit_frf",
    "law",
    "readings",
    "pulses",
];

/// What a plot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Srs,
    Pulse,
    Velocity,
    Displacement,
    Identification,
    Components,
    LimitChannels,
    LimitFrf,
    Law,
    Readings,
    Pulses,
}

impl Quantity {
    pub fn parse(name: &str) -> Option<Quantity> {
        Some(match name {
            "srs" => Quantity::Srs,
            "pulse" => Quantity::Pulse,
            "velocity" => Quantity::Velocity,
            "displacement" => Quantity::Displacement,
            "identification" => Quantity::Identification,
            "components" => Quantity::Components,
            "limit_channels" => Quantity::LimitChannels,
            "limit_frf" => Quantity::LimitFrf,
            "law" => Quantity::Law,
            "readings" => Quantity::Readings,
            "pulses" => Quantity::Pulses,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Quantity::Srs => "srs",
            Quantity::Pulse => "pulse",
            Quantity::Velocity => "velocity",
            Quantity::Displacement => "displacement",
            Quantity::Identification => "identification",
            Quantity::Components => "components",
            Quantity::LimitChannels => "limit_channels",
            Quantity::LimitFrf => "limit_frf",
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

/// `points` for a plot of a quantity: the default for a time history, refused on
/// any other.
pub fn parse_points(
    quantity_time: bool,
    v: Option<&Value>,
    param: &str,
) -> Result<Option<usize>, MethodError> {
    crate::shock::subscribe::parse_points(quantity_time, v, param)
}

/// `srs.subscribe`'s params; `None` is the default subscription (no `srs.data`).
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

/// The `srs.subscribe` result: the subscription with every default filled in.
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

/// What an `srs.data` frame is assembled from: the run's view, the latest pulse's
/// figures, the identification, the law's last values and the rows.
pub struct DataContext<'a> {
    pub view: &'a SrsView,
    pub last: Option<&'a PulseFigures>,
    pub identification: Option<&'a IdentFigures>,
    pub law: &'a [LawReading],
    pub rows: &'a [Row],
}

/// The views of one plot. A plot of the latest pulse is empty until a pulse has
/// fired, `identification` and `limit_frf` until identification has ended, and
/// `limit_channels` without limit channels.
pub fn plot_views(p: &Plot, ctx: &DataContext<'_>, arrays: &mut Arrays) -> Vec<Value> {
    let pts = p.points;
    match p.quantity {
        Quantity::Srs => ctx
            .last
            .and_then(|f| f.srs.as_ref())
            .map_or_else(Vec::new, |s| vec![view::srs_view(s, arrays)]),
        Quantity::Pulse => ctx.last.map_or_else(Vec::new, |f| {
            let mut out = Vec::new();
            if let Some(m) = &f.measured {
                out.push(view::pulse_view(f.fs_hz, m, pts, arrays));
            }
            out.push(crate::shock::view::drive_view(
                0, f.fs_hz, &f.drive, pts, arrays,
            ));
            out
        }),
        Quantity::Velocity => ctx
            .last
            .and_then(|f| f.velocity.as_ref().map(|v| (f.fs_hz, v)))
            .map_or_else(Vec::new, |(fs, v)| {
                vec![view::kinematic_view(true, fs, v, pts, arrays)]
            }),
        Quantity::Displacement => ctx
            .last
            .and_then(|f| f.displacement.as_ref().map(|d| (f.fs_hz, d)))
            .map_or_else(Vec::new, |(fs, d)| {
                vec![view::kinematic_view(false, fs, d, pts, arrays)]
            }),
        Quantity::Identification => ctx
            .identification
            .map_or_else(Vec::new, |i| view::identification_views(i, arrays)),
        Quantity::Components => ctx
            .last
            .and_then(|f| f.components.as_ref())
            .map_or_else(Vec::new, |c| view::component_views(c, arrays)),
        Quantity::LimitChannels => ctx
            .last
            .map_or_else(Vec::new, |f| view::limit_views(&f.limits, arrays)),
        Quantity::LimitFrf => ctx
            .identification
            .map_or_else(Vec::new, |i| view::frf_views(&i.limit_frf, arrays)),
        Quantity::Law => vec![view::law_view(ctx.law)],
        Quantity::Readings => vec![view::readings_view(ctx.view)],
        Quantity::Pulses => vec![view::pulses_view(ctx.rows)],
    }
}

/// The `srs.data` message data for `sub`: the live fields and each plot.
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
            {"id": "c", "quantity": "components"},
        ]})))
        .unwrap()
        .unwrap();
        assert_eq!(s.plots[0].points, Some(DEFAULT_POINTS));
        assert_eq!(s.plots[1].points, None);
        for bad in [
            json!({"plots": [{"id": "s", "quantity": "srs", "points": 512}]}),
            json!({"plots": [{"id": "s", "quantity": "limit_frf", "points": 512}]}),
            json!({"plots": [{"id": "p", "quantity": "spectrum"}]}),
            json!({"plots": [{"id": "p", "quantity": "pulse"}, {"id": "p", "quantity": "srs"}]}),
            json!({"rate_hz": 20, "plots": []}),
        ] {
            let e = parse(&params(bad.clone())).unwrap_err();
            assert_eq!(e.reason, Some("bad_params"), "{bad}");
        }
    }
}
