//! Sine display subscriptions (schema/PROTOCOL.md, "Sine Display"): what one
//! connection asks to be sent, parsed and checked, and the `sine.data` frame built
//! for it from a [`SineView`]. Random's form: a plot names a quantity, and
//! `channels` and `points` where they apply; a key a quantity does not take is
//! `bad_params` naming it.

use super::view::{self, SineView};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use serde_json::{json, Map, Value};

/// The quantities, in the order the result lists them.
pub const QUANTITIES: [&str; 9] = [
    "control",
    "drive",
    "response",
    "phase",
    "thd",
    "time",
    "identification",
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

/// What a plot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Control,
    Drive,
    Response,
    Phase,
    Thd,
    Time,
    Identification,
    Law,
    Readings,
}

impl Quantity {
    pub fn parse(name: &str) -> Option<Quantity> {
        Some(match name {
            "control" => Quantity::Control,
            "drive" => Quantity::Drive,
            "response" => Quantity::Response,
            "phase" => Quantity::Phase,
            "thd" => Quantity::Thd,
            "time" => Quantity::Time,
            "identification" => Quantity::Identification,
            "law" => Quantity::Law,
            "readings" => Quantity::Readings,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Quantity::Control => "control",
            Quantity::Drive => "drive",
            Quantity::Response => "response",
            Quantity::Phase => "phase",
            Quantity::Thd => "thd",
            Quantity::Time => "time",
            Quantity::Identification => "identification",
            Quantity::Law => "law",
            Quantity::Readings => "readings",
        }
    }

    /// Whether `channels` applies.
    pub fn has_channels(self) -> bool {
        matches!(self, Quantity::Response | Quantity::Phase | Quantity::Time)
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
    pub fn resolve(&self, v: &SineView) -> Vec<usize> {
        let limited = |index: u32| {
            v.figures.as_ref().is_some_and(|f| {
                f.inputs
                    .iter()
                    .any(|i| i.index == index && i.limit.is_some())
            })
        };
        v.channels
            .iter()
            .enumerate()
            .filter(|(_, c)| match self {
                Channels::All => true,
                Channels::Control => c.role == "control",
                Channels::Limited => limited(c.index),
                Channels::Ids(ids) => ids.contains(&c.index),
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

/// `sine.subscribe`'s params; `None` is the default subscription (`sine.live`).
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
            .find(|k| !matches!(k.as_str(), "id" | "quantity" | "channels" | "points"))
        {
            return Err(bad(
                &at(k),
                format!(
                    "a plot takes \"id\", \"quantity\", \"channels\" and \"points\", not \"{k}\""
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
                        "\"channels\" applies to response, phase and time plots, not to {}",
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
            points,
        });
    }
    Ok(Some(Subscription { rate_hz, plots }))
}

fn quantities() -> Value {
    Value::Array(QUANTITIES.iter().map(|q| Value::from(*q)).collect())
}

/// The `sine.subscribe` result: the subscription with every default filled in.
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

/// The `sine.data` frame for one connection's subscription.
pub fn data_json(v: &SineView, sub: &Subscription, arrays: &mut Arrays) -> Value {
    let mut m = view::live_head(v);
    let empty = view::Figures::default();
    let f = v.figures.as_deref().unwrap_or(&empty);
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            let (channels, views) = match p.quantity {
                Quantity::Control => (
                    Value::Null,
                    vec![view::control_view(f, &v.control_unit, arrays)],
                ),
                Quantity::Drive => (Value::Null, vec![view::drive_view(f, arrays)]),
                Quantity::Thd => (Value::Null, vec![view::thd_view(f, arrays)]),
                Quantity::Identification => (
                    Value::Null,
                    view::identification_views(f.identification.as_ref(), arrays),
                ),
                Quantity::Law => (Value::Null, vec![view::law_reading(v)]),
                Quantity::Readings => (Value::Null, vec![view::run_reading(v)]),
                Quantity::Response | Quantity::Phase | Quantity::Time => {
                    let positions = p.channels.as_ref().unwrap_or(&Channels::All).resolve(v);
                    let ids = Value::Array(
                        positions
                            .iter()
                            .map(|i| Value::from(v.channels[*i].index))
                            .collect(),
                    );
                    let views = positions
                        .iter()
                        .map(|pos| {
                            let c = &v.channels[*pos];
                            match p.quantity {
                                Quantity::Time => view::time_view(
                                    f,
                                    c,
                                    p.points.unwrap_or(DEFAULT_POINTS),
                                    arrays,
                                ),
                                Quantity::Phase => view::phase_view(f, c, arrays),
                                _ => view::response_view(f, c, arrays),
                            }
                        })
                        .collect();
                    (ids, views)
                }
            };
            json!({"id": p.id, "quantity": p.quantity.name(), "channels": channels, "views": views})
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
            refused(json!({"plots": [{"id": "a", "quantity": "thd", "channels": [1]}]})),
            "plots[0].channels"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "phase", "points": 128}]})),
            "plots[0].points"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "coherence"}]})),
            "plots[0].quantity"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "law", "rows": 3}]})),
            "plots[0].rows"
        );
        let s = parse(&params(
            json!({"plots": [{"id": "p", "quantity": "phase"}]}),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(s.plots[0].channels, Some(Channels::All));
        assert_eq!(s.plots[0].points, None);
    }
}
