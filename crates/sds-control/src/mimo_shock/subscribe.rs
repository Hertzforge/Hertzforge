//! MIMO Shock display subscriptions (schema/PROTOCOL.md, "MIMO Shock Display"):
//! what one connection asks to be sent, parsed and checked, and the
//! `mimo_shock.data` frame built for it.

use super::view::{self, IdentFigures, LawReading, MsView, PulseFigures};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use crate::shock::subscribe::{
    parse_points, DEFAULT_RATE_HZ, MAX_ID_CHARS, MAX_PLOTS, MAX_RATE_HZ, MIN_RATE_HZ,
};
use serde_json::{json, Map, Value};

pub use crate::shock::subscribe::DEFAULT_POINTS;

/// The quantities this service offers, in the order the result lists them.
pub const QUANTITIES: [&str; 10] = [
    "pulse",
    "srs",
    "drives",
    "verdicts",
    "ladder",
    "identification",
    "review",
    "law",
    "readings",
    "pulses",
];

/// One plot of a subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub id: String,
    pub quantity: &'static str,
    /// Set for the time histories only.
    pub points: Option<usize>,
    /// A control's table index, for the per-control quantities.
    pub channel: Option<u32>,
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

fn is_time(q: &str) -> bool {
    matches!(q, "pulse" | "drives")
}

fn takes_channel(q: &str) -> bool {
    matches!(q, "pulse" | "srs" | "identification")
}

/// `mimo_shock.subscribe`'s params; `None` is the default subscription (no
/// `mimo_shock.data`).
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
            format!("a subscription holds at most {MAX_PLOTS} plots"),
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
            .find(|k| !matches!(k.as_str(), "id" | "quantity" | "points" | "channel"))
        {
            return Err(bad(
                &at(k),
                format!(
                    "a plot has an id, a quantity, points for a time history and a channel; \
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
        let quantity = o
            .get("quantity")
            .and_then(Value::as_str)
            .and_then(|s| QUANTITIES.iter().copied().find(|q| *q == s))
            .ok_or_else(|| {
                bad(
                    &at("quantity"),
                    format!("a plot's \"quantity\" is one of {}", QUANTITIES.join(", ")),
                )
            })?;
        let points = parse_points(is_time(quantity), o.get("points"), &at("points"))?;
        let channel = match o.get("channel") {
            None | Some(Value::Null) => None,
            Some(v) if takes_channel(quantity) => Some(
                v.as_u64()
                    .and_then(|c| u32::try_from(c).ok())
                    .ok_or_else(|| bad(&at("channel"), "a channel is a table index"))?,
            ),
            Some(_) => {
                return Err(bad(
                    &at("channel"),
                    "\"channel\" applies to the pulse, srs and identification plots",
                ))
            }
        };
        plots.push(Plot {
            id,
            quantity,
            points,
            channel,
        });
    }
    Ok(Some(Subscription { rate_hz, plots }))
}

fn quantities() -> Value {
    Value::Array(QUANTITIES.iter().map(|q| Value::from(*q)).collect())
}

/// The `mimo_shock.subscribe` result: the subscription with every default filled in.
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
            m.insert("quantity".into(), Value::from(p.quantity));
            if let Some(n) = p.points {
                m.insert("points".into(), Value::from(n));
            }
            if let Some(c) = p.channel {
                m.insert("channel".into(), Value::from(c));
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

/// What a `mimo_shock.data` frame is assembled from.
pub struct DataContext<'a> {
    pub view: &'a MsView,
    pub last: Option<&'a PulseFigures>,
    pub identification: Option<&'a IdentFigures>,
    pub law: &'a [LawReading],
    pub rows: &'a [Value],
}

/// The views of one plot.
pub fn plot_views(p: &Plot, ctx: &DataContext<'_>, arrays: &mut Arrays) -> Vec<Value> {
    let pick = |c: u32| p.channel.is_none_or(|x| x == c);
    match p.quantity {
        "pulse" => ctx.last.map_or_else(Vec::new, |f| {
            f.controls
                .iter()
                .filter(|c| pick(c.channel))
                .map(|c| view::pulse_view(c, f.fs_hz, p.points, arrays))
                .collect()
        }),
        "srs" => ctx.last.map_or_else(Vec::new, |f| {
            f.controls
                .iter()
                .filter(|c| pick(c.channel))
                .filter_map(|c| {
                    c.srs
                        .as_ref()
                        .map(|s| view::srs_view(c.channel, &c.unit, s, arrays))
                })
                .collect()
        }),
        "drives" => ctx.last.map_or_else(Vec::new, |f| {
            vec![view::drives_view(&f.drives, f.fs_hz, p.points, arrays)]
        }),
        "verdicts" => {
            if ctx.view.last_pulse.is_some() {
                vec![view::verdicts_view(ctx.view.last_pulse.as_ref())]
            } else {
                Vec::new()
            }
        }
        "ladder" => vec![view::ladder_view(
            &ctx.view.levels_db,
            ctx.rows,
            ctx.view.level_db,
        )],
        "identification" => ctx.identification.map_or_else(Vec::new, |f| {
            let c = p
                .channel
                .and_then(|ch| f.controls.iter().position(|x| *x == ch))
                .unwrap_or(0);
            view::identification_views(f, c, arrays)
        }),
        "review" => ctx
            .view
            .review
            .as_ref()
            .map_or_else(Vec::new, |r| vec![view::review_view(Some(r))]),
        "law" => vec![view::law_view(ctx.law)],
        "readings" => vec![view::readings_view(ctx.view)],
        "pulses" => vec![view::pulses_view(ctx.rows)],
        _ => Vec::new(),
    }
}

/// The `mimo_shock.data` payload: the live fields and every plot.
pub fn data_json(ctx: &DataContext<'_>, sub: &Subscription, arrays: &mut Arrays) -> Value {
    let mut d = view::live_json(ctx.view);
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "quantity": p.quantity,
                "views": plot_views(p, ctx, arrays),
            })
        })
        .collect();
    if let Value::Object(m) = &mut d {
        m.insert("plots".into(), Value::Array(plots));
    }
    d
}
