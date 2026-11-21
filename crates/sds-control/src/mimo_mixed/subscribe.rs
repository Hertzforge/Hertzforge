//! `mimo_mixed.subscribe` (schema/PROTOCOL.md, "MIMO Mixed Mode Frames"): a
//! connection's plots, each a quantity over some controls or parts, sent as
//! `mimo_mixed.data` at the connection's own rate. `{"default": true}` drops the
//! plots (the connection keeps the `mimo_mixed.live` frames every one gets).
//! Mixed Mode's form: `rate_hz` and `plots: [{id, quantity, channels?, parts?}]`.

use super::view::{self, MimoMixedView, ToneQuantity};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use serde_json::{json, Map, Value};

use std::time::Duration;

pub const DEFAULT_RATE_HZ: f64 = 2.0;
pub const MIN_RATE_HZ: f64 = 0.2;
pub const MAX_RATE_HZ: f64 = 10.0;
pub const MAX_PLOTS: usize = 16;

/// The quantities a plot may show.
pub const QUANTITIES: [&str; 7] = [
    "control",
    "residual",
    "cross",
    "drive",
    "tone_amplitude",
    "tone_phase",
    "band_level",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub id: String,
    pub quantity: &'static str,
    /// Control channels; `None` is every control.
    pub channels: Option<Vec<u32>>,
    /// Part ids; `None` is every part the quantity takes.
    pub parts: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Subscription {
    pub rate_hz: f64,
    pub plots: Vec<Plot>,
}

impl Subscription {
    pub fn period(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.rate_hz)
    }
}

fn bad(param: &str, words: impl Into<String>) -> MethodError {
    MethodError::bad_params(param, words)
}

/// Parse the parameters: `None` is the default subscription.
pub fn parse(params: &Map<String, Value>) -> Result<Option<Subscription>, MethodError> {
    if params.get("default") == Some(&Value::Bool(true)) {
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
    let Some(list) = params.get("plots").and_then(Value::as_array) else {
        return Err(bad(
            "plots",
            "a subscription is {\"default\": true} or a list of \"plots\"",
        ));
    };
    if list.is_empty() || list.len() > MAX_PLOTS {
        return Err(bad("plots", format!("from 1 to {MAX_PLOTS} plots")));
    }
    let mut plots = Vec::new();
    for (k, p) in list.iter().enumerate() {
        let o = p
            .as_object()
            .ok_or_else(|| bad("plots", format!("plot {k} is an object")))?;
        let id = o
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("plots", format!("plot {k} has no \"id\"")))?
            .to_owned();
        let q = o.get("quantity").and_then(Value::as_str).unwrap_or("");
        let quantity = QUANTITIES
            .iter()
            .copied()
            .find(|x| *x == q)
            .ok_or_else(|| {
                bad(
                    "plots",
                    format!("plot {k}'s quantity is one of {}", QUANTITIES.join(", ")),
                )
            })?;
        let channels = match o.get("channels") {
            None | Some(Value::Null) => None,
            Some(Value::Array(a)) => Some(
                a.iter()
                    .map(|x| x.as_u64().map(|x| x as u32))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| bad("plots", format!("plot {k}'s channels are indexes")))?,
            ),
            Some(_) => return Err(bad("plots", format!("plot {k}'s channels are a list"))),
        };
        let parts = match o.get("parts") {
            None | Some(Value::Null) => None,
            Some(Value::Array(a)) => Some(
                a.iter()
                    .map(|x| x.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| bad("plots", format!("plot {k}'s parts are ids")))?,
            ),
            Some(_) => return Err(bad("plots", format!("plot {k}'s parts are a list"))),
        };
        plots.push(Plot {
            id,
            quantity,
            channels,
            parts,
        });
    }
    Ok(Some(Subscription { rate_hz, plots }))
}

pub fn result_json(sub: Option<&Subscription>) -> Value {
    match sub {
        None => json!({"default": true, "rate_hz": num(DEFAULT_RATE_HZ), "plots": null,
                       "quantities": QUANTITIES}),
        Some(s) => json!({
            "default": false,
            "rate_hz": num(s.rate_hz),
            "plots": s.plots.iter().map(|p| json!({"id": p.id, "quantity": p.quantity,
                "channels": p.channels, "parts": p.parts})).collect::<Vec<_>>(),
            "quantities": QUANTITIES,
        }),
    }
}

/// One `mimo_mixed.data` frame for `sub`.
pub fn data_json(v: &MimoMixedView, sub: &Subscription, arrays: &mut Arrays) -> Value {
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            let pick: Vec<usize> = v
                .controls
                .iter()
                .enumerate()
                .filter(|(_, c)| p.channels.as_ref().is_none_or(|w| w.contains(c)))
                .map(|(i, _)| i)
                .collect();
            let parts = p.parts.as_deref();
            let view = match (p.quantity, v.spectra.as_deref()) {
                ("control", Some(sp)) => view::control_view(v, sp, &pick, arrays),
                ("residual", Some(sp)) => view::residual_view(v, sp, &pick, arrays),
                ("cross", Some(sp)) => view::cross_view(v, sp, &pick, arrays),
                ("drive", Some(sp)) => view::drive_view(v, sp, arrays),
                ("tone_amplitude", _) => {
                    view::tone_view(v, ToneQuantity::Amplitude, parts, &pick, arrays)
                }
                ("tone_phase", _) => view::tone_view(v, ToneQuantity::Phase, parts, &pick, arrays),
                ("band_level", _) => view::band_view(v, parts, &pick, arrays),
                _ => Value::Null,
            };
            let views = if view.is_null() {
                Vec::new()
            } else {
                vec![view]
            };
            let channels: Vec<u32> = pick.iter().map(|i| v.controls[*i]).collect();
            json!({"id": p.id, "quantity": p.quantity, "channels": channels,
                   "parts": p.parts, "views": views})
        })
        .collect();
    let mut m = view::live_head(v);
    m.insert("plots".into(), Value::Array(plots));
    Value::Object(m)
}
