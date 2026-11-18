//! MIMO Sine display subscriptions (schema/PROTOCOL.md, "MIMO Sine Display"):
//! what one connection asks to be sent, parsed and checked, and the
//! `mimo_sine.data` frame built for it from an [`MsView`]. Sine's form: a plot
//! names a quantity, and `channels` where it applies; a key a quantity does
//! not take is `bad_params` naming it.

use super::view::{self, Figures, MsView};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use crate::views::{self, Axis, Role, Row, Scale, Series, Status};
use serde_json::{json, Map, Value};

/// The quantities, in the order the result lists them.
pub const QUANTITIES: [&str; 9] = [
    "control",
    "phase",
    "error",
    "limit",
    "drive",
    "drive_phase",
    "readings",
    "identification",
    "law",
];
pub const DEFAULT_RATE_HZ: f64 = 5.0;
pub const MIN_RATE_HZ: f64 = 0.5;
pub const MAX_RATE_HZ: f64 = 10.0;
pub const MAX_PLOTS: usize = 64;
pub const MAX_ID_CHARS: usize = 64;

/// What a plot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Control,
    Phase,
    Error,
    Limit,
    Drive,
    DrivePhase,
    Readings,
    Identification,
    Law,
}

impl Quantity {
    pub fn parse(name: &str) -> Option<Quantity> {
        Some(match name {
            "control" => Quantity::Control,
            "phase" => Quantity::Phase,
            "error" => Quantity::Error,
            "limit" => Quantity::Limit,
            "drive" => Quantity::Drive,
            "drive_phase" => Quantity::DrivePhase,
            "readings" => Quantity::Readings,
            "identification" => Quantity::Identification,
            "law" => Quantity::Law,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Quantity::Control => "control",
            Quantity::Phase => "phase",
            Quantity::Error => "error",
            Quantity::Limit => "limit",
            Quantity::Drive => "drive",
            Quantity::DrivePhase => "drive_phase",
            Quantity::Readings => "readings",
            Quantity::Identification => "identification",
            Quantity::Law => "law",
        }
    }

    /// Whether `channels` applies.
    pub fn has_channels(self) -> bool {
        matches!(
            self,
            Quantity::Control | Quantity::Phase | Quantity::Limit | Quantity::Identification
        )
    }
}

/// Which channels a plot shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channels {
    All,
    Ids(Vec<u32>),
}

impl Channels {
    fn to_json(&self) -> Value {
        match self {
            Channels::All => Value::from("all"),
            Channels::Ids(ids) => Value::Array(ids.iter().map(|i| Value::from(*i)).collect()),
        }
    }

    fn picks(&self, ch: u32) -> bool {
        match self {
            Channels::All => true,
            Channels::Ids(ids) => ids.contains(&ch),
        }
    }
}

/// One plot of a subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub id: String,
    pub quantity: Quantity,
    pub channels: Option<Channels>,
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

/// `mimo_sine.subscribe`'s params; `None` is the default subscription
/// (`mimo_sine.live`).
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
            .find(|k| !matches!(k.as_str(), "id" | "quantity" | "channels"))
        {
            return Err(bad(
                &at(k),
                format!("a plot takes \"id\", \"quantity\" and \"channels\", not \"{k}\""),
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
        let words = "\"channels\" is a list of channel indexes, or \"all\"";
        let channels = match (quantity.has_channels(), o.get("channels")) {
            (true, None | Some(Value::Null)) => Some(Channels::All),
            (true, Some(Value::String(s))) if s == "all" => Some(Channels::All),
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
                        "\"channels\" applies to control, phase, limit and identification \
                         plots, not to {}",
                        quantity.name()
                    ),
                ))
            }
        };
        plots.push(Plot {
            id,
            quantity,
            channels,
        });
    }
    Ok(Some(Subscription { rate_hz, plots }))
}

fn quantities() -> Value {
    Value::Array(QUANTITIES.iter().map(|q| Value::from(*q)).collect())
}

/// The `mimo_sine.subscribe` result: the subscription with every default
/// filled in.
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
            Value::Object(m)
        })
        .collect();
    json!({"default": false, "rate_hz": num(sub.rate_hz), "plots": plots,
           "quantities": quantities()})
}

fn identification_views(v: &MsView, ch: &Channels, arrays: &mut Arrays) -> Vec<Value> {
    let Some(f) = v.identification_figures.as_deref() else {
        return Vec::new();
    };
    let rows: Vec<(usize, u32)> = v
        .controls
        .iter()
        .chain(&v.limits)
        .enumerate()
        .map(|(i, c)| (i, c.channel))
        .collect();
    let picked: Vec<(usize, u32)> = match ch {
        Channels::All => rows.first().copied().into_iter().collect(),
        Channels::Ids(_) => rows.iter().copied().filter(|(_, c)| ch.picks(*c)).collect(),
    };
    let x = Axis::new("Frequency", Some("Hz"), Scale::Linear);
    let mut out = Vec::new();
    for (r, chn) in picked {
        let series = |which: &Vec<Vec<Vec<f64>>>| -> Vec<Series> {
            v.drives
                .iter()
                .enumerate()
                .map(|(d, dc)| {
                    Series::new(
                        &format!("Drive {}", dc.channel),
                        Role::Measured,
                        f.freqs_hz.clone(),
                        which[r][d].clone(),
                    )
                })
                .collect()
        };
        out.push(views::spectrum(
            arrays,
            &format!("Identification Gain {chn}"),
            &x,
            &Axis::new("Gain", None, Scale::Log),
            series(&f.gain),
            Some("frequency"),
        ));
        out.push(views::spectrum(
            arrays,
            &format!("Identification Phase {chn}"),
            &x,
            &Axis::new("Phase", Some("deg"), Scale::Linear),
            series(&f.phase),
            Some("frequency"),
        ));
    }
    let mut coh: Vec<Series> = rows
        .iter()
        .map(|(r, c)| {
            let name = if *r < v.controls.len() {
                format!("Control {c}")
            } else {
                format!("Limit {c}")
            };
            Series::new(&name, Role::Measured, f.freqs_hz.clone(), f.coherence[*r].clone())
        })
        .collect();
    coh.push(Series::new(
        "Trust coherence",
        Role::Tolerance,
        f.freqs_hz.clone(),
        vec![f.trust_coherence; f.freqs_hz.len()],
    ));
    out.push(views::spectrum(
        arrays,
        "Coherence",
        &x,
        &Axis::new("Coherence", None, Scale::Linear),
        coh,
        Some("frequency"),
    ));
    out
}

fn law_reading(v: &MsView) -> Value {
    let live = v.live.as_deref();
    let mut rows: Vec<Row> = v
        .law_values
        .iter()
        .enumerate()
        .map(|(k, (_, label, unit))| {
            Row::number(
                label,
                view::sig(live.and_then(|l| l.values.get(k).copied()).unwrap_or(f64::NAN)),
                (!unit.is_empty()).then_some(unit.as_str()),
                Status::Info,
            )
        })
        .collect();
    rows.push(Row::number(
        "Unusable calls in a row",
        f64::from(v.counts.misses),
        None,
        if v.counts.misses > 0 {
            Status::Warn
        } else {
            Status::Info
        },
    ));
    rows.push(Row::number(
        "Ticks a cap bound",
        v.counts.capped_ticks as f64,
        None,
        Status::Info,
    ));
    rows.push(Row::number(
        "Largest rotation",
        crate::sine::config::round(v.counts.rotation_max_deg, 2),
        Some("deg"),
        Status::Info,
    ));
    rows.push(Row::text(
        "Requests",
        &format!("{} made, {} granted", v.counts.asked, v.counts.granted),
        Status::Info,
    ));
    views::reading("Law", rows)
}

/// The `mimo_sine.data` frame for one connection's subscription.
pub fn data_json(v: &MsView, sub: &Subscription, arrays: &mut Arrays) -> Value {
    let mut m = view::live_head(v);
    let empty = Figures::new((1.0, 1.0), None, true, 0, 0, 0);
    let f = v.figures.as_deref();
    let engaged = f.is_some() && v.live.is_some();
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            let ch = p.channels.clone().unwrap_or(Channels::All);
            let resolved: Vec<u32> = match p.quantity {
                Quantity::Control | Quantity::Phase => v
                    .controls
                    .iter()
                    .map(|c| c.channel)
                    .filter(|c| ch.picks(*c))
                    .collect(),
                Quantity::Limit => v
                    .limits
                    .iter()
                    .map(|c| c.channel)
                    .filter(|c| ch.picks(*c))
                    .collect(),
                _ => Vec::new(),
            };
            let views: Vec<Value> = match p.quantity {
                Quantity::Identification => identification_views(v, &ch, arrays),
                Quantity::Law if v.law_built => vec![law_reading(v)],
                _ if !engaged => Vec::new(),
                q => {
                    let f = f.unwrap_or(&empty);
                    let wanted: Vec<String> = match q {
                        Quantity::Control => {
                            resolved.iter().map(|c| format!("Control {c}")).collect()
                        }
                        Quantity::Phase => resolved.iter().map(|c| format!("Phase {c}")).collect(),
                        Quantity::Limit => resolved.iter().map(|c| format!("Limit {c}")).collect(),
                        Quantity::Drive => vec!["Drives".into()],
                        Quantity::DrivePhase => vec!["Drive phase".into()],
                        Quantity::Readings => vec!["Readings".into()],
                        _ => Vec::new(),
                    };
                    let mut out = if q == Quantity::Error {
                        vec![error_view(v, f, arrays)]
                    } else {
                        view::pass_views_where(v, f, arrays, &|t| wanted.iter().any(|w| w == t))
                    };
                    if q == Quantity::Readings {
                        out.push(view::run_reading(v));
                    }
                    out
                }
            };
            let channels = if p.quantity.has_channels() {
                match p.quantity {
                    Quantity::Identification => match &ch {
                        Channels::All => json!(v.controls.first().map(|c| c.channel)),
                        Channels::Ids(ids) => json!(ids),
                    },
                    _ => json!(resolved),
                }
            } else {
                Value::Null
            };
            json!({"id": p.id, "quantity": p.quantity.name(), "channels": channels, "views": views})
        })
        .collect();
    m.insert("plots".into(), Value::Array(plots));
    Value::Object(m)
}

/// **Error**: every control's error, dB, and its notch depth while one is
/// taken.
fn error_view(v: &MsView, f: &Figures, arrays: &mut Arrays) -> Value {
    let (x, link) = if f.against_time {
        (Axis::new("Time", Some("s"), Scale::Linear), "time")
    } else {
        (Axis::new("Frequency", Some("Hz"), Scale::Log), "frequency")
    };
    let mut series = Vec::new();
    for (c, pts) in v.controls.iter().zip(&f.control) {
        let (xs, ys): (Vec<f64>, Vec<f64>) = f
            .x
            .iter()
            .zip(pts.iter().map(|p| p[4]))
            .filter(|(_, y)| y.is_finite())
            .map(|(x, y)| (*x, y))
            .unzip();
        series.push(Series::new(
            &format!("Control {}", c.channel),
            Role::Measured,
            xs,
            ys,
        ));
        let (xs, ys): (Vec<f64>, Vec<f64>) = f
            .x
            .iter()
            .zip(pts.iter().map(|p| p[5]))
            .filter(|(_, y)| y.is_finite() && *y > 0.0)
            .map(|(x, y)| (*x, y))
            .unzip();
        if !xs.is_empty() {
            series.push(Series::new(
                &format!("Notch depth {}", c.channel),
                Role::Limit,
                xs,
                ys,
            ));
        }
    }
    views::spectrum(
        arrays,
        "Error",
        &x,
        &Axis::new("Error", Some("dB"), Scale::Linear),
        series,
        Some(link),
    )
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
    fn mimo_sine_every_key_a_quantity_does_not_take_is_refused_by_its_path() {
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "drive", "channels": [1]}]})),
            "plots[0].channels"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "time"}]})),
            "plots[0].quantity"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "law", "points": 3}]})),
            "plots[0].points"
        );
        let s = parse(&params(json!({"plots": [{"id": "p", "quantity": "phase"}]})))
            .unwrap()
            .unwrap();
        assert_eq!(s.plots[0].channels, Some(Channels::All));
    }
}
