//! Explorer display subscriptions (schema/PROTOCOL.md, "Explorer Display"): what one
//! connection asks to be sent, parsed and checked. Random's form: a plot names a
//! quantity, and `channels` and `points` where they apply; a key a quantity does not
//! take is `bad_params` naming it.

use crate::protocol::{num, MethodError};
use serde_json::{Map, Value};

/// The quantities, in the order the result lists them.
pub const QUANTITIES: [&str; 4] = ["survey", "spectrum", "time", "readings"];
pub const DEFAULT_RATE_HZ: f64 = 10.0;
pub const MIN_RATE_HZ: f64 = 0.5;
pub const MAX_RATE_HZ: f64 = 10.0;
pub const MAX_PLOTS: usize = 64;
pub const MAX_ID_CHARS: usize = 64;
pub const DEFAULT_TIME_POINTS: usize = 1024;
pub const MIN_POINTS: usize = 64;
pub const MAX_POINTS: usize = 8192;

/// What a plot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Survey,
    Spectrum,
    Time,
    Readings,
}

impl Quantity {
    pub fn parse(name: &str) -> Option<Quantity> {
        Some(match name {
            "survey" => Quantity::Survey,
            "spectrum" => Quantity::Spectrum,
            "time" => Quantity::Time,
            "readings" => Quantity::Readings,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Quantity::Survey => "survey",
            Quantity::Spectrum => "spectrum",
            Quantity::Time => "time",
            Quantity::Readings => "readings",
        }
    }
}

/// Which channels a spectrum plot shows, resolved against the run's channels on every
/// frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channels {
    All,
    Control,
    Ids(Vec<u32>),
}

impl Channels {
    pub fn to_json(&self) -> Value {
        match self {
            Channels::All => Value::from("all"),
            Channels::Control => Value::from("control"),
            Channels::Ids(ids) => Value::Array(ids.iter().map(|i| Value::from(*i)).collect()),
        }
    }
}

/// How many points a plot is drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Points {
    /// The quantity takes no `points`.
    None,
    /// The survey on its own points.
    Own,
    /// Drawn on this many points.
    N(usize),
}

/// One plot of a subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub id: String,
    pub quantity: Quantity,
    /// `spectrum` only.
    pub channels: Option<Channels>,
    pub points: Points,
}

/// What one connection asked to be sent, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Subscription {
    pub rate_hz: f64,
    pub plots: Vec<Plot>,
}

fn bad(param: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(param, message)
}

/// Parse `explorer.subscribe`'s parameters: `None` is the default subscription (back to
/// `explorer.live`).
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
        let words = "\"channels\" is a list of channel indexes, or \"all\" or \"control\"";
        let channels = match (quantity, o.get("channels")) {
            (Quantity::Spectrum, None | Some(Value::Null)) => Some(Channels::All),
            (Quantity::Spectrum, Some(Value::String(s))) => Some(match s.as_str() {
                "all" => Channels::All,
                "control" => Channels::Control,
                _ => return Err(bad(&at("channels"), words)),
            }),
            (Quantity::Spectrum, Some(Value::Array(a))) => {
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
            (Quantity::Spectrum, Some(_)) => return Err(bad(&at("channels"), words)),
            (_, None | Some(Value::Null)) => None,
            (q, Some(_)) => {
                return Err(bad(
                    &at("channels"),
                    format!(
                        "\"channels\" applies to spectrum plots, not to {}",
                        q.name()
                    ),
                ))
            }
        };
        let count = |v: &Value| -> Result<usize, MethodError> {
            match v.as_u64() {
                Some(n) if (MIN_POINTS as u64..=MAX_POINTS as u64).contains(&n) => Ok(n as usize),
                _ => Err(bad(
                    &at("points"),
                    format!("\"points\" is a whole number from {MIN_POINTS} to {MAX_POINTS}"),
                )),
            }
        };
        let points = match (quantity, o.get("points")) {
            (Quantity::Survey, None | Some(Value::Null)) => Points::Own,
            (Quantity::Survey, Some(v)) => Points::N(count(v)?),
            (Quantity::Time, None | Some(Value::Null)) => Points::N(DEFAULT_TIME_POINTS),
            (Quantity::Time, Some(v)) => Points::N(count(v)?),
            (_, None | Some(Value::Null)) => Points::None,
            (q, Some(_)) => {
                return Err(bad(
                    &at("points"),
                    format!(
                        "\"points\" applies to survey and time plots, not to {}",
                        q.name()
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

/// One plot as the result states it: `channels` and `points` where they apply.
pub fn plot_json(p: &Plot) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), Value::from(p.id.as_str()));
    m.insert("quantity".into(), Value::from(p.quantity.name()));
    if let Some(c) = &p.channels {
        m.insert("channels".into(), c.to_json());
    }
    match p.points {
        Points::None => {}
        Points::Own => {
            m.insert("points".into(), Value::Null);
        }
        Points::N(n) => {
            m.insert("points".into(), Value::from(n));
        }
    }
    Value::Object(m)
}

/// The `explorer.subscribe` result: the subscription with every default filled in.
pub fn result_json(sub: Option<&Subscription>) -> Value {
    let mut m = Map::new();
    match sub {
        None => {
            m.insert("default".into(), Value::from(true));
            m.insert("rate_hz".into(), num(DEFAULT_RATE_HZ));
            m.insert("plots".into(), Value::Null);
        }
        Some(sub) => {
            m.insert("default".into(), Value::from(false));
            m.insert("rate_hz".into(), num(sub.rate_hz));
            m.insert(
                "plots".into(),
                Value::Array(sub.plots.iter().map(plot_json).collect()),
            );
        }
    }
    m.insert("quantities".into(), quantities());
    Value::Object(m)
}

impl Subscription {
    /// The time between two frames.
    pub fn period(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / self.rate_hz)
    }
}

/// What a subscribed frame is built from, beside the view.
pub struct DataContext<'a> {
    pub view: &'a super::view::ExplorerView,
    /// The survey line's own points, `(hz, level)`.
    pub survey: &'a [(f64, f64)],
    /// The latest display window per acquired channel: `(index, df_hz, magnitudes)`.
    pub spectra: &'a [(u32, f64, Vec<f64>)],
    /// The control row's most recent samples: `(first input frame, rate, samples)`.
    pub time: Option<(u64, f64, &'a [f64])>,
}

/// The `explorer.data` frame for one connection's subscription.
pub fn data_json(
    ctx: &DataContext<'_>,
    sub: &Subscription,
    arrays: &mut crate::codec::Arrays,
) -> Value {
    use super::view;
    let v = ctx.view;
    let mut m = view::live_head(v);
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            let mut channels = Value::Null;
            let mut first_frame = Value::Null;
            let views: Vec<Value> = match p.quantity {
                Quantity::Survey => {
                    if ctx.survey.is_empty() {
                        Vec::new()
                    } else {
                        let pts = match p.points {
                            Points::N(n) => view::draw_survey(ctx.survey, v.band_hz, n),
                            _ => ctx.survey.to_vec(),
                        };
                        vec![view::survey_view(&pts, &v.control_unit, arrays)]
                    }
                }
                Quantity::Spectrum => {
                    let control = v.control.as_ref().map(|c| c.index);
                    let picked: Vec<&super::view::LiveChannel> = v
                        .channels
                        .iter()
                        .filter(|c| match &p.channels {
                            Some(Channels::Control) => Some(c.index) == control,
                            Some(Channels::Ids(ids)) => ids.contains(&c.index),
                            _ => true,
                        })
                        .collect();
                    channels = Value::Array(picked.iter().map(|c| Value::from(c.index)).collect());
                    picked
                        .iter()
                        .filter_map(|c| {
                            let (_, df, mags) = ctx.spectra.iter().find(|s| s.0 == c.index)?;
                            let name = if c.description.is_empty() {
                                c.role
                            } else {
                                c.description.as_str()
                            };
                            Some(view::channel_view(
                                c.index, name, &c.unit, *df, mags, arrays,
                            ))
                        })
                        .collect()
                }
                Quantity::Time => match (ctx.time, v.control.as_ref()) {
                    (Some((first, fs, samples)), Some(c)) if !samples.is_empty() => {
                        let n = match p.points {
                            Points::N(n) => n,
                            _ => DEFAULT_TIME_POINTS,
                        };
                        let skip = samples.len().saturating_sub(n);
                        first_frame = Value::from(first + skip as u64);
                        vec![view::time_view(
                            c.index,
                            &v.control_unit,
                            fs,
                            first + skip as u64,
                            &samples[skip..],
                            arrays,
                        )]
                    }
                    _ => Vec::new(),
                },
                Quantity::Readings => vec![view::run_reading(v)],
            };
            let mut e = Map::new();
            e.insert("id".into(), Value::from(p.id.as_str()));
            e.insert("quantity".into(), Value::from(p.quantity.name()));
            e.insert("channels".into(), channels);
            e.insert("first_frame".into(), first_frame);
            e.insert("views".into(), Value::Array(views));
            Value::Object(e)
        })
        .collect();
    m.insert("plots".into(), Value::Array(plots));
    Value::Object(m)
}
