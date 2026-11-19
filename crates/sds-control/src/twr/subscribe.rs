//! TWR display subscriptions (schema/PROTOCOL.md, "TWR Display"): what one
//! connection asks to be sent, parsed and checked, and the `twr.data` frame built
//! for it.
//!
//! A subscription narrows the display and nothing else: the run keeps what it keeps
//! for each play, and a frame is only assembled from what a connection's plots
//! show.

use super::view::{
    self, IdentFigures, LawValueSpec, MaskFigures, PlayRecords, PlayRow, Scope, TwrView,
};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
pub use crate::shock::subscribe::{
    DEFAULT_POINTS, DEFAULT_RATE_HZ, MAX_ID_CHARS, MAX_PLOTS, MAX_POINTS, MAX_RATE_HZ, MIN_POINTS,
    MIN_RATE_HZ,
};
use serde_json::{json, Map, Value};

/// The quantities this service offers, in the order the result lists them.
pub const QUANTITIES: [&str; 14] = [
    "target",
    "response",
    "error",
    "drive",
    "envelope",
    "frf",
    "coherence",
    "mask",
    "spectra",
    "convergence",
    "law",
    "scope",
    "readings",
    "plays",
];

/// What a plot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Target,
    Response,
    Error,
    Drive,
    Envelope,
    Frf,
    Coherence,
    Mask,
    Spectra,
    Convergence,
    Law,
    Scope,
    Readings,
    Plays,
}

impl Quantity {
    pub fn parse(name: &str) -> Option<Quantity> {
        Some(match name {
            "target" => Quantity::Target,
            "response" => Quantity::Response,
            "error" => Quantity::Error,
            "drive" => Quantity::Drive,
            "envelope" => Quantity::Envelope,
            "frf" => Quantity::Frf,
            "coherence" => Quantity::Coherence,
            "mask" => Quantity::Mask,
            "spectra" => Quantity::Spectra,
            "convergence" => Quantity::Convergence,
            "law" => Quantity::Law,
            "scope" => Quantity::Scope,
            "readings" => Quantity::Readings,
            "plays" => Quantity::Plays,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Quantity::Target => "target",
            Quantity::Response => "response",
            Quantity::Error => "error",
            Quantity::Drive => "drive",
            Quantity::Envelope => "envelope",
            Quantity::Frf => "frf",
            Quantity::Coherence => "coherence",
            Quantity::Mask => "mask",
            Quantity::Spectra => "spectra",
            Quantity::Convergence => "convergence",
            Quantity::Law => "law",
            Quantity::Scope => "scope",
            Quantity::Readings => "readings",
            Quantity::Plays => "plays",
        }
    }

    /// Whether `points` applies: the records and the scope.
    pub fn takes_points(self) -> bool {
        matches!(
            self,
            Quantity::Target
                | Quantity::Response
                | Quantity::Error
                | Quantity::Drive
                | Quantity::Envelope
                | Quantity::Scope
        )
    }
}

/// One plot of a subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub id: String,
    pub quantity: Quantity,
    /// Set for the quantities that take points only.
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

    /// Whether a plot shows the play in flight.
    pub fn has_scope(&self) -> bool {
        self.plots.iter().any(|p| p.quantity == Quantity::Scope)
    }
}

fn bad(param: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(param, message)
}

/// `points` for a plot: the default for a quantity that takes them.
pub fn parse_points(
    takes: bool,
    v: Option<&Value>,
    param: &str,
) -> Result<Option<usize>, MethodError> {
    match (takes, v) {
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
            "\"points\" applies to the target, response, error, drive, envelope and scope plots",
        )),
    }
}

/// `twr.record`'s `points`: 0 sends every sample.
pub fn parse_record_points(v: Option<&Value>) -> Result<Option<usize>, MethodError> {
    match v {
        None | Some(Value::Null) => Ok(Some(DEFAULT_POINTS)),
        Some(v) => match v.as_u64() {
            Some(0) => Ok(None),
            Some(n) if (MIN_POINTS as u64..=MAX_POINTS as u64).contains(&n) => Ok(Some(n as usize)),
            _ => Err(bad(
                "points",
                format!(
                    "\"points\" is 0 (every sample) or a whole number from {MIN_POINTS} to \
                     {MAX_POINTS}"
                ),
            )),
        },
    }
}

/// `twr.subscribe`'s params; `None` is the default subscription (no `twr.data`).
pub fn parse(params: &Map<String, Value>) -> Result<Option<Subscription>, MethodError> {
    let default = match params.get("default") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("default", "\"default\" is true or false")),
    };
    if let Some(k) = params
        .keys()
        .find(|k| !matches!(k.as_str(), "default" | "rate_hz" | "plots"))
    {
        return Err(bad(
            k,
            format!(
                "a subscription has a rate and its plots, or asks for the default; not \"{k}\""
            ),
        ));
    }
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
                format!("a plot has an id, a quantity and, for a record, points; not \"{k}\""),
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
        let points = parse_points(quantity.takes_points(), o.get("points"), &at("points"))?;
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

/// The `twr.subscribe` result: the subscription with every default filled in.
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

/// What a `twr.data` frame is assembled from.
pub struct DataContext<'a> {
    pub view: &'a TwrView,
    /// The last judged play's records.
    pub last: Option<&'a PlayRecords>,
    /// The level target now, when no play has been judged at it.
    pub target: Option<&'a PlayRecords>,
    pub identification: Option<&'a IdentFigures>,
    pub mask: Option<&'a MaskFigures>,
    pub rows: &'a [PlayRow],
    pub law_specs: &'a [LawValueSpec],
    pub scope: &'a Scope,
}

/// The views of one plot. A plot of the last play is empty until a play has been
/// judged, `frf`, `coherence` and `mask` until identification has ended, `law`
/// until the law has been called, and `scope` between plays.
pub fn plot_views(p: &Plot, ctx: &DataContext<'_>, arrays: &mut Arrays) -> Vec<Value> {
    let pts = p.points;
    let units = ctx.view.units.as_str();
    let last = || ctx.last;
    match p.quantity {
        Quantity::Target => ctx
            .target
            .or(ctx.last)
            .map_or_else(Vec::new, |f| vec![view::target_view(f, units, pts, arrays)]),
        Quantity::Response => last().map_or_else(Vec::new, |f| {
            vec![view::response_view("Response", f, units, pts, arrays)]
        }),
        Quantity::Error => {
            last().map_or_else(Vec::new, |f| vec![view::error_view(f, units, pts, arrays)])
        }
        Quantity::Drive => last().map_or_else(Vec::new, |f| vec![view::drive_view(f, pts, arrays)]),
        Quantity::Envelope => last().map_or_else(Vec::new, |f| {
            vec![view::envelope_view(f, units, pts, arrays)]
        }),
        Quantity::Frf => ctx
            .identification
            .map_or_else(Vec::new, |f| view::frf_views(f, arrays)),
        Quantity::Coherence => ctx
            .identification
            .map_or_else(Vec::new, |f| vec![view::coherence_view(f, arrays)]),
        Quantity::Mask => ctx
            .mask
            .map_or_else(Vec::new, |f| vec![view::mask_view(f, arrays)]),
        Quantity::Spectra => last()
            .and_then(|f| view::spectra_view(f, units, arrays))
            .map_or_else(Vec::new, |v| vec![v]),
        Quantity::Convergence => {
            if ctx.rows.is_empty() {
                Vec::new()
            } else {
                vec![view::convergence_view(ctx.rows, ctx.view.gate_pct, arrays)]
            }
        }
        Quantity::Law => {
            if ctx.rows.iter().any(|r| r.law.is_some()) {
                let misses = ctx.view.law.as_ref().map_or(0, |l| l.misses);
                view::law_views(ctx.rows, ctx.law_specs, misses, arrays)
            } else {
                Vec::new()
            }
        }
        Quantity::Scope => {
            view::scope_view(ctx.view, ctx.scope, pts.unwrap_or(DEFAULT_POINTS), arrays)
                .map_or_else(Vec::new, |v| vec![v])
        }
        Quantity::Readings => vec![view::readings_view(ctx.view)],
        Quantity::Plays => vec![view::plays_view(ctx.rows)],
    }
}

/// The `twr.data` message data for `sub`: the live fields and each plot.
pub fn data_json(ctx: &DataContext<'_>, sub: &Subscription, arrays: &mut Arrays) -> Value {
    let mut m = view::head_json(ctx.view);
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
    m.insert("plots".into(), Value::Array(plots));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn points_apply_to_the_records_and_the_scope_only() {
        let s = parse(&params(json!({"plots": [
            {"id": "r", "quantity": "response"},
            {"id": "s", "quantity": "scope", "points": 512},
            {"id": "f", "quantity": "frf"},
        ]})))
        .unwrap()
        .unwrap();
        assert_eq!(s.plots[0].points, Some(DEFAULT_POINTS));
        assert_eq!(s.plots[1].points, Some(512));
        assert_eq!(s.plots[2].points, None);
        for bad in [
            json!({"plots": [{"id": "f", "quantity": "frf", "points": 512}]}),
            json!({"plots": [{"id": "r", "quantity": "response", "points": 10}]}),
            json!({"plots": [{"id": "r", "quantity": "pulse"}]}),
            json!({"plots": [{"id": "r", "quantity": "drive"}, {"id": "r", "quantity": "mask"}]}),
            json!({"plots": [{"id": "r", "quantity": "drive", "channels": [1]}]}),
            json!({"rate_hz": 20, "plots": []}),
            json!({"default": true, "plots": []}),
            json!({"plots": [], "speed": 2}),
        ] {
            let e = parse(&params(bad.clone())).unwrap_err();
            assert_eq!(e.reason, Some("bad_params"), "{bad}");
        }
        assert!(parse(&params(json!({"default": true}))).unwrap().is_none());
    }

    #[test]
    fn a_record_takes_every_sample_at_zero_points() {
        assert_eq!(parse_record_points(Some(&json!(0))).unwrap(), None);
        assert_eq!(parse_record_points(None).unwrap(), Some(DEFAULT_POINTS));
        assert!(parse_record_points(Some(&json!(10))).is_err());
    }
}
