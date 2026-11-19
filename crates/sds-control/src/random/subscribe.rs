//! Live data subscriptions (schema/PROTOCOL.md, "Live Data Subscriptions"): what one
//! connection asks to be sent, parsed and checked, and the `random.data` frame built
//! for it from a [`RunView`].
//!
//! A subscription narrows the display and nothing else: the run computes what it
//! computes for the control loop, and a frame is only assembled from what a
//! connection's plots show. A channel nobody plots is never put into a frame.

use super::view::{self, RunView};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use crate::views::{self, Axis, Role, Scale, Series};
use serde_json::{json, Map, Value};

/// The quantities this service offers, in the order the result lists them.
pub const QUANTITIES: [&str; 6] = [
    "control",
    "drive",
    "response",
    "time",
    "coherence",
    "readings",
];
/// Frames per second when `rate_hz` is not given, and its bounds.
pub const DEFAULT_RATE_HZ: f64 = 5.0;
pub const MIN_RATE_HZ: f64 = 0.5;
pub const MAX_RATE_HZ: f64 = 10.0;
/// The most plots one subscription may hold.
pub const MAX_PLOTS: usize = 64;
/// The longest plot id, in characters.
pub const MAX_ID_CHARS: usize = 64;
/// Points per channel of a `time` plot: the default and the bounds.
pub const DEFAULT_POINTS: usize = 1024;
pub const MIN_POINTS: usize = 64;
pub const MAX_POINTS: usize = 8192;

/// What a plot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Control,
    Drive,
    Response,
    Time,
    Coherence,
    Readings,
}

impl Quantity {
    pub fn parse(name: &str) -> Option<Quantity> {
        Some(match name {
            "control" => Quantity::Control,
            "drive" => Quantity::Drive,
            "response" => Quantity::Response,
            "time" => Quantity::Time,
            "coherence" => Quantity::Coherence,
            "readings" => Quantity::Readings,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Quantity::Control => "control",
            Quantity::Drive => "drive",
            Quantity::Response => "response",
            Quantity::Time => "time",
            Quantity::Coherence => "coherence",
            Quantity::Readings => "readings",
        }
    }

    /// Whether `channels` applies.
    pub fn has_channels(self) -> bool {
        matches!(self, Quantity::Response | Quantity::Time)
    }
}

/// Which channels a `response` or `time` plot shows. Resolved by the service against
/// the run's table on every frame, so the table is the one authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channels {
    /// Every enabled input.
    All,
    /// The control rows.
    Control,
    /// The rows the configuration limits.
    Limited,
    /// These channel indexes; one the run does not have is left out.
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
    pub fn resolve(&self, v: &RunView) -> Vec<usize> {
        v.channels
            .iter()
            .enumerate()
            .filter(|(_, c)| match self {
                Channels::All => true,
                Channels::Control => c.role == "control",
                Channels::Limited => v.limits.iter().any(|l| l.channel == c.index),
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
    /// Set for `response` and `time` only.
    pub channels: Option<Channels>,
    /// Set for `time` only.
    pub points: Option<usize>,
}

/// What one connection asked to be sent, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Subscription {
    pub rate_hz: f64,
    pub plots: Vec<Plot>,
}

impl Subscription {
    /// The time between two frames.
    pub fn period(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / self.rate_hz)
    }
}

fn bad(param: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(param, message)
}

/// `random.subscribe`'s params; `None` is the default subscription (`random.live`).
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
        let channels =
            match (quantity.has_channels(), o.get("channels")) {
                (true, None | Some(Value::Null)) => Some(Channels::All),
                (true, Some(Value::String(s))) => Some(match s.as_str() {
                    "all" => Channels::All,
                    "control" => Channels::Control,
                    "limited" => Channels::Limited,
                    _ => return Err(bad(
                        &at("channels"),
                        "\"channels\" is a list of channel indexes, or \"all\", \"control\" or \
                         \"limited\"",
                    )),
                }),
                (true, Some(Value::Array(a))) => {
                    let mut ids: Vec<u32> = Vec::with_capacity(a.len());
                    for x in a {
                        let id =
                            x.as_u64()
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
                (true, Some(_)) => {
                    return Err(bad(
                        &at("channels"),
                        "\"channels\" is a list of channel indexes, or \"all\", \"control\" or \
                     \"limited\"",
                    ))
                }
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

/// The `random.subscribe` result: the subscription with every default filled in.
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

/// `Channel <index>: <description>`, the role when there is no description.
fn channel_title(c: &view::LiveChannel) -> String {
    let what = if c.description.trim().is_empty() {
        c.role
    } else {
        c.description.trim()
    };
    format!("Channel {}: {what}", c.index)
}

/// One channel's PSD over the band against its limit, stepped on the run's lines.
fn response_view(v: &RunView, pos: usize, arrays: &mut Arrays) -> Value {
    let c = &v.channels[pos];
    let freqs = v
        .lines
        .as_deref()
        .map_or_else(Vec::new, |l| l.freqs.clone());
    let mut series = Vec::with_capacity(2);
    let psd = v
        .inputs
        .as_deref()
        .and_then(|i| i.psd.get(pos))
        .and_then(|p| p.as_ref());
    if let Some(p) = psd.filter(|p| p.len() == freqs.len() && !p.is_empty()) {
        series.push(
            Series::new(
                &format!("Channel {}", c.index),
                Role::Measured,
                freqs.clone(),
                p.clone(),
            )
            .stepped(),
        );
    }
    if let Some(l) = v
        .limits
        .iter()
        .find(|l| l.channel == c.index && !l.freqs.is_empty())
    {
        series.push(limit_series(l, &freqs));
    }
    views::spectrum(
        arrays,
        &channel_title(c),
        &view::frequency_axis(),
        &Axis::new("PSD", Some(&format!("{}²/Hz", c.unit)), Scale::Log),
        series,
        Some("frequency"),
    )
}

/// The limit on the lines its rows cover, each drawn across the same bin as the
/// measured line it bounds (the edges are the run's own, so a one-line limit still
/// has its two edges).
fn limit_series(l: &view::LimitLines, freqs: &[f64]) -> Series {
    let all = views::bin_edges(freqs);
    let first = freqs
        .iter()
        .position(|f| (f - l.freqs[0]).abs() < 1e-9)
        .unwrap_or(0);
    let n = l.freqs.len();
    let edges = if freqs.len() >= 2 && first + n < all.len() {
        all[first..=first + n].to_vec()
    } else {
        views::bin_edges(&l.freqs)
    };
    let mut s = Series::new("Limit", Role::Limit, l.freqs.clone(), l.ceiling.clone());
    s.edges = Some(edges);
    s
}

/// `samples` at `fs_hz` as at most `points` (time, value) pairs: every sample when
/// they fit, otherwise the minimum and the maximum of each interval in time order,
/// so no peak is lost to the decimation.
pub fn decimate(samples: &[f32], fs_hz: f64, points: usize) -> (Vec<f64>, Vec<f64>) {
    let n = samples.len();
    let dt = if fs_hz > 0.0 { 1.0 / fs_hz } else { 0.0 };
    if n <= points.max(2) {
        let t = (0..n).map(|k| k as f64 * dt).collect();
        let y = samples.iter().map(|x| f64::from(*x)).collect();
        return (t, y);
    }
    let buckets = (points / 2).max(1);
    let mut t = Vec::with_capacity(buckets * 2);
    let mut y = Vec::with_capacity(buckets * 2);
    for b in 0..buckets {
        let lo = b * n / buckets;
        let hi = ((b + 1) * n / buckets).max(lo + 1).min(n);
        let (mut imin, mut imax) = (lo, lo);
        for k in lo..hi {
            if samples[k] < samples[imin] {
                imin = k;
            }
            if samples[k] > samples[imax] {
                imax = k;
            }
        }
        let (a, z) = if imin <= imax {
            (imin, imax)
        } else {
            (imax, imin)
        };
        t.push(a as f64 * dt);
        y.push(f64::from(samples[a]));
        if z != a {
            t.push(z as f64 * dt);
            y.push(f64::from(samples[z]));
        }
    }
    (t, y)
}

/// One channel's last acquired samples.
fn time_view(v: &RunView, pos: usize, points: usize, arrays: &mut Arrays) -> Value {
    let c = &v.channels[pos];
    let mut series = Vec::with_capacity(1);
    if let Some(inputs) = v.inputs.as_deref() {
        if let Some(samples) = inputs.time.get(pos).filter(|s| !s.is_empty()) {
            let (t, y) = decimate(samples, inputs.fs_hz, points);
            series.push(Series::new(
                &format!("Channel {}", c.index),
                Role::Measured,
                t,
                y,
            ));
        }
    }
    views::spectrum(
        arrays,
        &channel_title(c),
        &Axis::new("Time", Some("s"), Scale::Linear),
        &Axis::new("Amplitude", Some(&c.unit), Scale::Linear),
        series,
        None,
    )
}

/// The system identification's coherence on the band's lines.
fn coherence_view(v: &RunView, arrays: &mut Arrays) -> Value {
    let freqs = v
        .lines
        .as_deref()
        .map_or_else(Vec::new, |l| l.freqs.clone());
    let series = v
        .coherence
        .as_deref()
        .filter(|c| c.len() == freqs.len() && !c.is_empty())
        .map(|c| vec![Series::new("Coherence", Role::Measured, freqs, c.clone())])
        .unwrap_or_default();
    views::spectrum(
        arrays,
        "Coherence",
        &view::frequency_axis(),
        &Axis::new("Coherence", None, Scale::Linear),
        series,
        Some("frequency"),
    )
}

/// The `random.data` frame for one connection's subscription.
pub fn data_json(v: &RunView, sub: &Subscription, arrays: &mut Arrays) -> Value {
    let mut m = view::live_head(v);
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            let (channels, views) = match p.quantity {
                Quantity::Control => (Value::Null, vec![view::control_view(v, arrays)]),
                Quantity::Drive => (Value::Null, vec![view::drive_view(v, arrays)]),
                Quantity::Readings => (Value::Null, vec![view::run_reading(v)]),
                Quantity::Coherence => (Value::Null, vec![coherence_view(v, arrays)]),
                Quantity::Response | Quantity::Time => {
                    let positions = p.channels.as_ref().unwrap_or(&Channels::All).resolve(v);
                    let ids = Value::Array(
                        positions
                            .iter()
                            .map(|i| Value::from(v.channels[*i].index))
                            .collect(),
                    );
                    let views = positions
                        .iter()
                        .map(|pos| match p.quantity {
                            Quantity::Time => {
                                time_view(v, *pos, p.points.unwrap_or(DEFAULT_POINTS), arrays)
                            }
                            _ => response_view(v, *pos, arrays),
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
    fn the_default_and_a_list_of_plots_parse_with_defaults_filled() {
        assert_eq!(parse(&params(json!({"default": true}))).unwrap(), None);
        let s = parse(&params(json!({"plots": [
            {"id": "a", "quantity": "control"},
            {"id": "b", "quantity": "response"},
            {"id": "c", "quantity": "time", "channels": [3, 2, 3]},
        ]})))
        .unwrap()
        .unwrap();
        assert_eq!(s.rate_hz, DEFAULT_RATE_HZ);
        assert_eq!(s.plots[0].channels, None);
        assert_eq!(s.plots[1].channels, Some(Channels::All));
        assert_eq!(s.plots[1].points, None);
        assert_eq!(s.plots[2].channels, Some(Channels::Ids(vec![3, 2])));
        assert_eq!(s.plots[2].points, Some(DEFAULT_POINTS));
        let r = result_json(Some(&s));
        assert_eq!(r["default"], false);
        assert_eq!(r["rate_hz"], 5);
        assert_eq!(r["plots"][0], json!({"id": "a", "quantity": "control"}));
        assert_eq!(
            r["plots"][1],
            json!({"id": "b", "quantity": "response", "channels": "all"})
        );
        assert_eq!(
            r["plots"][2],
            json!({"id": "c", "quantity": "time", "channels": [3, 2], "points": 1024})
        );
        assert_eq!(r["quantities"].as_array().unwrap().len(), 6);
        let d = result_json(None);
        assert_eq!(d["default"], true);
        assert_eq!(d["plots"], Value::Null);
    }

    #[test]
    fn an_empty_list_is_a_subscription_to_the_numbers_only() {
        let s = parse(&params(json!({"plots": [], "rate_hz": 0.5})))
            .unwrap()
            .unwrap();
        assert!(s.plots.is_empty());
        assert_eq!(s.rate_hz, 0.5);
    }

    #[test]
    fn every_bad_field_is_refused_by_its_path() {
        assert_eq!(refused(json!({})), "plots");
        assert_eq!(refused(json!({"default": 1})), "default");
        assert_eq!(refused(json!({"default": true, "plots": []})), "plots");
        assert_eq!(refused(json!({"plots": [], "rate_hz": 0.4})), "rate_hz");
        assert_eq!(refused(json!({"plots": [], "rate_hz": 10.5})), "rate_hz");
        assert_eq!(refused(json!({"plots": [7]})), "plots[0]");
        assert_eq!(
            refused(json!({"plots": [{"quantity": "control"}]})),
            "plots[0].id"
        );
        let long = "x".repeat(MAX_ID_CHARS + 1);
        assert_eq!(
            refused(json!({"plots": [{"id": long, "quantity": "control"}]})),
            "plots[0].id"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "control"},
                                     {"id": "a", "quantity": "drive"}]})),
            "plots[1].id"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "control"},
                                     {"id": "b", "quantity": "nonsense"}]})),
            "plots[1].quantity"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "control", "channels": [1]}]})),
            "plots[0].channels"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "response", "channels": "some"}]})),
            "plots[0].channels"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "response", "channels": [-1]}]})),
            "plots[0].channels"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "response", "points": 100}]})),
            "plots[0].points"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "time", "points": 63}]})),
            "plots[0].points"
        );
        let many: Vec<Value> = (0..=MAX_PLOTS)
            .map(|i| json!({"id": format!("p{i}"), "quantity": "control"}))
            .collect();
        assert_eq!(refused(json!({ "plots": many })), "plots");
    }

    #[test]
    fn decimation_keeps_every_peak_in_time_order() {
        // 10 000 samples of a slow ramp with one spike up and one down.
        let mut s: Vec<f32> = (0..10_000).map(|k| (k as f32) * 1e-4).collect();
        s[4321] = 50.0;
        s[7777] = -50.0;
        let (t, y) = decimate(&s, 1000.0, 256);
        assert!(t.len() <= 256, "{} points", t.len());
        assert!(t.windows(2).all(|w| w[1] > w[0]), "in time order");
        assert!(y.contains(&50.0), "the spike up is kept");
        assert!(y.contains(&-50.0), "the spike down is kept");
        let at = t[y.iter().position(|v| *v == 50.0).unwrap()];
        assert!((at - 4.321).abs() < 1e-9, "at its own time, {at}");
        // Short blocks pass whole.
        let (t, y) = decimate(&s[..100], 1000.0, 256);
        assert_eq!(t.len(), 100);
        assert_eq!(y[99], f64::from(s[99]));
    }
}
