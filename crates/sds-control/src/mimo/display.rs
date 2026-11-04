//! MIMO Random's display quantities (schema/PROTOCOL.md, "MIMO Random Display"): what
//! a screen may subscribe to, by name and selector, and the `mimo_random.data` frame
//! built for one connection's subscription.
//!
//! Nothing is sent that is not on screen. A plot names one quantity and one selector
//! (a control or drive, a pair, a set of rows for the heatmap, the diagonal, a block
//! of the spectral matrix for the grid), the driver computes only that, and the
//! service decimates it to the plot's width before it goes on the socket: a minimum
//! and maximum envelope per pixel column for spectra, the nearest line per column
//! for heatmap rows and matrix blocks, nothing for numbers. A 72 by 72 matrix is
//! never streamed whole.

use super::driver::{Curve, MimoStatus, PackedBlock, QuantitySelector, Series};
use super::setup::MimoSetup;
use crate::codec::{Array, ArrayData, Arrays};
use crate::protocol::{num, MethodError};
use crate::views::{self, Axis, Role, Row, Scale, Status};
use serde_json::{json, Map, Value};
use std::ops::Range;

/// The quantities MIMO Random offers, in the order the result lists them.
pub const MIMO_QUANTITIES: [&str; 19] = [
    "psd",
    "coh",
    "phase",
    "csd_mag",
    "error_db",
    "drive_psd",
    "drive_coh",
    "frf",
    "sv",
    "cond",
    "mcoh",
    "cap",
    "dof_rms",
    "trace",
    "pred_drive_rms",
    "pred_error_db",
    "law_diag",
    "sdm",
    "readings",
];

/// Frames per second: the default and its bounds (as Random's).
pub const DEFAULT_RATE_HZ: f64 = 5.0;
pub const MIN_RATE_HZ: f64 = 0.5;
pub const MAX_RATE_HZ: f64 = 10.0;
pub const MAX_PLOTS: usize = 64;
pub const MAX_ID_CHARS: usize = 64;
/// A plot's width in points: the default and its bounds.
pub const DEFAULT_POINTS: usize = 1024;
pub const MIN_POINTS: usize = 64;
pub const MAX_POINTS: usize = 8192;
/// The largest block of the spectral matrix one plot may ask for (the grid shows a
/// window of 8 by 8).
pub const MAX_BLOCK: usize = 8;

/// What a selector picks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    None,
    Index(usize),
    Pair(usize, usize),
    /// Heatmap rows: `None` for all of them.
    Rows(Option<Vec<usize>>),
    Diagonal,
    Block(Vec<usize>),
    /// A law diagnostic's key, at a control or drive for a per-channel layout.
    Key(String, Option<usize>),
}

impl Selector {
    fn to_json(&self) -> Value {
        match self {
            Selector::None => json!({}),
            Selector::Index(i) => json!({"index": i}),
            Selector::Pair(i, j) => json!({"pair": [i, j]}),
            Selector::Rows(None) => json!({"rows": "all"}),
            Selector::Rows(Some(r)) => json!({"rows": r}),
            Selector::Diagonal => json!({"diagonal": true}),
            Selector::Block(b) => json!({"block": b}),
            Selector::Key(k, None) => json!({"key": k}),
            Selector::Key(k, Some(i)) => json!({"key": k, "index": i}),
        }
    }
}

/// Which selectors a quantity takes; the first is its default when the plot names
/// none.
fn selectors(q: &str) -> &'static [&'static str] {
    match q {
        "psd" | "drive_psd" | "mcoh" => &["index", "diagonal"],
        "cap" | "pred_drive_rms" => &["diagonal", "index"],
        "coh" | "phase" | "csd_mag" | "drive_coh" | "frf" => &["pair"],
        "error_db" => &["rows"],
        "pred_error_db" => &["index"],
        "sdm" => &["block"],
        "law_diag" => &["key"],
        _ => &["none"],
    }
}

/// One plot of a MIMO subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct MimoPlot {
    pub id: String,
    pub quantity: &'static str,
    pub selector: Selector,
    pub points: usize,
}

/// One connection's MIMO subscription, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct MimoSubscription {
    pub rate_hz: f64,
    pub plots: Vec<MimoPlot>,
}

impl MimoSubscription {
    pub fn period(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / self.rate_hz)
    }
}

fn bad(param: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(param, message)
}

fn position(v: &Value, param: &str) -> Result<usize, MethodError> {
    v.as_u64()
        .map(|n| n as usize)
        .ok_or_else(|| bad(param, "a position is a whole number from 0, counting the setup's controls (or drives) in order"))
}

/// `mimo_random.subscribe`'s params; `None` is no subscription (status events only).
pub fn parse(params: &Map<String, Value>) -> Result<Option<MimoSubscription>, MethodError> {
    match params.get("default") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => {
            if params.get("plots").is_some_and(|p| !p.is_null()) {
                return Err(bad(
                    "plots",
                    "the default takes no plots; send either \"default\": true or a list of plots",
                ));
            }
            return Ok(None);
        }
        Some(_) => return Err(bad("default", "\"default\" is true or false")),
    }
    let rate_hz = match params.get("rate_hz") {
        None | Some(Value::Null) => DEFAULT_RATE_HZ,
        Some(v) => match v.as_f64() {
            Some(x) if (MIN_RATE_HZ..=MAX_RATE_HZ).contains(&x) => x,
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
        _ => {
            return Err(bad(
                "plots",
                "a subscription lists its plots (\"plots\", possibly empty), or asks for the default with \"default\": true",
            ))
        }
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
    let mut plots: Vec<MimoPlot> = Vec::with_capacity(list.len());
    for (k, p) in list.iter().enumerate() {
        let at = |f: &str| format!("plots[{k}].{f}");
        let o = p
            .as_object()
            .ok_or_else(|| bad(&format!("plots[{k}]"), "each plot is an object"))?;
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
            .and_then(|q| MIMO_QUANTITIES.iter().find(|n| **n == q).copied())
            .ok_or_else(|| {
                bad(
                    &at("quantity"),
                    format!(
                        "a plot's \"quantity\" is one of {}",
                        MIMO_QUANTITIES.join(", ")
                    ),
                )
            })?;
        let allowed = selectors(quantity);
        let given: Vec<&str> = ["index", "pair", "rows", "diagonal", "block", "key"]
            .into_iter()
            .filter(|k| o.get(*k).is_some_and(|v| !v.is_null()))
            .collect();
        // `index` rides with `key` for a per-channel diagnostic.
        let named: Vec<&str> = if quantity == "law_diag" {
            given.iter().copied().filter(|k| *k != "index").collect()
        } else {
            given.clone()
        };
        if named.len() > 1 {
            return Err(bad(
                &at(named[1]),
                format!(
                    "a plot takes one selector; {} were sent",
                    named.join(" and ")
                ),
            ));
        }
        let which = named.first().copied().unwrap_or(allowed[0]);
        if !allowed.contains(&which) {
            let words = if allowed == ["none"] {
                format!("{quantity} takes no selector")
            } else {
                format!("{quantity} takes {}", allowed.join(" or "))
            };
            return Err(bad(&at(which), words));
        }
        let selector = match which {
            "none" => Selector::None,
            "index" => Selector::Index(position(
                o.get("index")
                    .ok_or_else(|| bad(&at("index"), format!("{quantity} needs an \"index\"")))?,
                &at("index"),
            )?),
            "diagonal" => match o.get("diagonal") {
                None | Some(Value::Bool(true)) => Selector::Diagonal,
                _ => return Err(bad(&at("diagonal"), "\"diagonal\" is true")),
            },
            "pair" => {
                let pair = o
                    .get("pair")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() == 2)
                    .ok_or_else(|| {
                        bad(
                            &at("pair"),
                            format!("{quantity} names a pair of positions, such as [0, 1]"),
                        )
                    })?;
                let (i, j) = (
                    position(&pair[0], &at("pair"))?,
                    position(&pair[1], &at("pair"))?,
                );
                if i == j && matches!(quantity, "coh" | "phase" | "drive_coh") {
                    return Err(bad(
                        &at("pair"),
                        format!("{quantity} is between two different channels"),
                    ));
                }
                Selector::Pair(i, j)
            }
            "rows" => match o.get("rows") {
                None | Some(Value::Null) => Selector::Rows(None),
                Some(Value::String(s)) if s == "all" => Selector::Rows(None),
                Some(Value::Array(a)) if !a.is_empty() => {
                    let mut r = Vec::with_capacity(a.len());
                    for x in a {
                        let i = position(x, &at("rows"))?;
                        if !r.contains(&i) {
                            r.push(i);
                        }
                    }
                    Selector::Rows(Some(r))
                }
                _ => {
                    return Err(bad(
                        &at("rows"),
                        "\"rows\" is \"all\" or a list of control positions",
                    ))
                }
            },
            "block" => {
                let a = o
                    .get("block")
                    .and_then(Value::as_array)
                    .filter(|a| !a.is_empty() && a.len() <= MAX_BLOCK)
                    .ok_or_else(|| {
                        bad(
                            &at("block"),
                            format!("\"block\" lists 1 to {MAX_BLOCK} control positions"),
                        )
                    })?;
                let mut b = Vec::with_capacity(a.len());
                for x in a {
                    let i = position(x, &at("block"))?;
                    if b.contains(&i) {
                        return Err(bad(&at("block"), "a block lists each control once"));
                    }
                    b.push(i);
                }
                Selector::Block(b)
            }
            "key" => {
                let key = o
                    .get("key")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| bad(&at("key"), "a law diagnostic is named by its key"))?;
                let index = match o.get("index") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(position(v, &at("index"))?),
                };
                Selector::Key(key.to_owned(), index)
            }
            _ => Selector::None,
        };
        let points = match o.get("points") {
            None | Some(Value::Null) => DEFAULT_POINTS,
            Some(v) => match v.as_u64() {
                Some(n) if (MIN_POINTS as u64..=MAX_POINTS as u64).contains(&n) => n as usize,
                _ => return Err(bad(&at("points"), format!("\"points\" is the plot's width, a whole number from {MIN_POINTS} to {MAX_POINTS}"))),
            },
        };
        plots.push(MimoPlot {
            id,
            quantity,
            selector,
            points,
        });
    }
    Ok(Some(MimoSubscription { rate_hz, plots }))
}

fn quantities() -> Value {
    Value::from(MIMO_QUANTITIES.to_vec())
}

fn plot_echo(p: &MimoPlot) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), Value::from(p.id.as_str()));
    m.insert("quantity".into(), Value::from(p.quantity));
    if let Value::Object(s) = p.selector.to_json() {
        m.extend(s);
    }
    m.insert("points".into(), Value::from(p.points));
    Value::Object(m)
}

/// The `mimo_random.subscribe` result.
pub fn result_json(sub: Option<&MimoSubscription>) -> Value {
    match sub {
        None => {
            json!({"default": true, "rate_hz": num(DEFAULT_RATE_HZ), "plots": null, "quantities": quantities()})
        }
        Some(s) => json!({
            "default": false, "rate_hz": num(s.rate_hz),
            "plots": s.plots.iter().map(plot_echo).collect::<Vec<_>>(),
            "quantities": quantities(),
        }),
    }
}

// ---- decimation ----------------------------------------------------------------------

/// Split `freqs` (rising) into at most `width` contiguous groups of lines, one per
/// pixel column of a plot `width` wide, on a log or a linear axis. Every line is in
/// exactly one group; empty columns are dropped. With no more lines than columns,
/// each line is its own group.
pub fn columns(freqs: &[f64], width: usize, log: bool) -> Vec<Range<usize>> {
    let n = freqs.len();
    if n <= width.max(1) {
        return (0..n).map(|i| i..i + 1).collect();
    }
    let at = |f: f64| {
        if log {
            f.max(f64::MIN_POSITIVE).ln()
        } else {
            f
        }
    };
    let (lo, hi) = (at(freqs[0]), at(freqs[n - 1]));
    let span = (hi - lo).max(f64::MIN_POSITIVE);
    let mut out: Vec<Range<usize>> = Vec::with_capacity(width);
    let mut start = 0;
    let mut col = 0usize;
    for (i, f) in freqs.iter().enumerate() {
        let c = (((at(*f) - lo) / span) * width as f64)
            .floor()
            .min(width as f64 - 1.0) as usize;
        if c != col && i > start {
            out.push(start..i);
            start = i;
        }
        col = c;
    }
    out.push(start..n);
    out
}

/// The envelope of `y` over `groups`: the largest and the smallest value of each.
pub fn envelope(y: &[f64], groups: &[Range<usize>]) -> (Vec<f64>, Vec<f64>) {
    groups
        .iter()
        .map(|g| {
            y[g.clone()]
                .iter()
                .fold((f64::NEG_INFINITY, f64::INFINITY), |(hi, lo), v| {
                    (hi.max(*v), lo.min(*v))
                })
        })
        .unzip()
}

/// The line nearest each group's center, on the axis's scale.
pub fn nearest(freqs: &[f64], groups: &[Range<usize>], log: bool) -> Vec<usize> {
    groups
        .iter()
        .map(|g| {
            let (a, b) = (freqs[g.start], freqs[g.end - 1]);
            let mid = if log && a > 0.0 {
                (a * b).sqrt()
            } else {
                0.5 * (a + b)
            };
            (g.clone())
                .min_by(|x, y| (freqs[*x] - mid).abs().total_cmp(&(freqs[*y] - mid).abs()))
                .unwrap_or(g.start)
        })
        .collect()
}

/// A curve on the run's lines as a series at most `width` points wide. A density is
/// drawn as steps: undecimated, each line across its own bin; decimated, each
/// column across the union of its lines' bins, its envelope in `y` and `y_min`. A
/// curve that is not a density keeps its lines, or one point per column at the
/// column's center with the envelope.
pub fn decimated_series(
    freqs: &[f64],
    c: &Curve,
    width: usize,
    log: bool,
    density: bool,
) -> views::Series {
    let groups = columns(freqs, width, log);
    let undecimated = groups.len() == freqs.len();
    if undecimated {
        let s = views::Series::new(&c.name, c.role, freqs.to_vec(), c.y.clone());
        return if density { s.stepped() } else { s };
    }
    let (hi, lo) = envelope(&c.y, &groups);
    let edges_all = views::bin_edges(freqs);
    let x: Vec<f64> = groups
        .iter()
        .map(|g| {
            let (a, b) = (freqs[g.start], freqs[g.end - 1]);
            if log && a > 0.0 {
                (a * b).sqrt()
            } else {
                0.5 * (a + b)
            }
        })
        .collect();
    let mut s = views::Series::new(&c.name, c.role, x, hi);
    s.y_min = Some(lo);
    if density {
        let mut e: Vec<f64> = groups.iter().map(|g| edges_all[g.start]).collect();
        e.push(edges_all[freqs.len()]);
        s.edges = Some(e);
    }
    s
}

// ---- building a plot's views ------------------------------------------------------------

/// What a plot's views need to know of the run besides the data.
pub struct PlotContext<'a> {
    pub labels: &'a [String],
    pub drive_labels: &'a [String],
    /// The controlled quantities' unit.
    pub unit: &'a str,
    /// Per control: alarm and abort bands, `[below, above]` dB.
    pub bands: Vec<([f64; 2], [f64; 2])>,
    pub df_hz: f64,
}

fn freq_axis() -> Axis {
    Axis::new("Frequency", Some("Hz"), Scale::Log)
}

fn label(labels: &[String], i: usize) -> String {
    labels
        .get(i)
        .cloned()
        .unwrap_or_else(|| format!("{}", i + 1))
}

/// The selector resolved for a driver call, or `None` when it names a position the
/// run does not have (left out, never refused: a plot kept across setups shows
/// nothing rather than failing).
pub fn resolve(p: &MimoPlot, controls: usize, drives: usize) -> Vec<QuantitySelector> {
    let c = |i: usize| i < controls;
    let d = |k: usize| k < drives;
    let q = |s: QuantitySelector| vec![s];
    match (p.quantity, &p.selector) {
        ("psd", Selector::Index(i)) if c(*i) => q(QuantitySelector::Psd(*i)),
        ("psd", Selector::Diagonal) => (0..controls).map(QuantitySelector::Psd).collect(),
        ("coh", Selector::Pair(i, j)) if c(*i) && c(*j) => q(QuantitySelector::Coh(*i, *j)),
        ("phase", Selector::Pair(i, j)) if c(*i) && c(*j) => q(QuantitySelector::Phase(*i, *j)),
        ("csd_mag", Selector::Pair(i, j)) if c(*i) && c(*j) => q(QuantitySelector::CsdMag(*i, *j)),
        ("error_db", Selector::Rows(None)) => q(QuantitySelector::ErrorDb((0..controls).collect())),
        ("error_db", Selector::Rows(Some(r))) => {
            let r: Vec<usize> = r.iter().copied().filter(|i| c(*i)).collect();
            if r.is_empty() {
                Vec::new()
            } else {
                q(QuantitySelector::ErrorDb(r))
            }
        }
        ("drive_psd", Selector::Index(k)) if d(*k) => q(QuantitySelector::DrivePsd(*k)),
        ("drive_psd", Selector::Diagonal) => (0..drives).map(QuantitySelector::DrivePsd).collect(),
        ("drive_coh", Selector::Pair(k, l)) if d(*k) && d(*l) => {
            q(QuantitySelector::DriveCoh(*k, *l))
        }
        ("frf", Selector::Pair(i, k)) if c(*i) && d(*k) => q(QuantitySelector::Frf(*i, *k)),
        ("sv", _) => q(QuantitySelector::Sv),
        ("cond", _) => q(QuantitySelector::Cond),
        ("mcoh", Selector::Index(i)) if c(*i) => q(QuantitySelector::Mcoh(*i)),
        ("mcoh", Selector::Diagonal) => (0..controls).map(QuantitySelector::Mcoh).collect(),
        ("dof_rms", _) => (0..controls).map(QuantitySelector::Psd).collect(),
        ("trace", _) => q(QuantitySelector::Trace),
        ("pred_error_db", Selector::Index(i)) if c(*i) => q(QuantitySelector::PredErrorDb(*i)),
        ("sdm", Selector::Block(b)) if b.iter().all(|i| c(*i)) => {
            q(QuantitySelector::SdmBlock(b.clone()))
        }
        ("law_diag", Selector::Key(k, i)) => q(QuantitySelector::LawDiag(k.clone(), *i)),
        _ => Vec::new(),
    }
}

/// The names a resolved plot shows, in its views' order.
pub fn plot_labels(p: &MimoPlot, ctx: &PlotContext<'_>) -> Vec<String> {
    let n = ctx.labels.len();
    let nd = ctx.drive_labels.len();
    match &p.selector {
        Selector::Index(i) if matches!(p.quantity, "drive_psd" | "cap" | "pred_drive_rms") => {
            vec![label(ctx.drive_labels, *i)]
        }
        Selector::Index(i) => vec![label(ctx.labels, *i)],
        Selector::Pair(i, k) if p.quantity == "frf" => {
            vec![label(ctx.labels, *i), label(ctx.drive_labels, *k)]
        }
        Selector::Pair(a, b) if p.quantity == "drive_coh" => {
            vec![label(ctx.drive_labels, *a), label(ctx.drive_labels, *b)]
        }
        Selector::Pair(i, j) => vec![label(ctx.labels, *i), label(ctx.labels, *j)],
        Selector::Rows(None) => ctx.labels.to_vec(),
        Selector::Rows(Some(r)) | Selector::Block(r) => r
            .iter()
            .filter(|i| **i < n)
            .map(|i| label(ctx.labels, *i))
            .collect(),
        Selector::Diagonal if matches!(p.quantity, "drive_psd" | "cap" | "pred_drive_rms") => {
            ctx.drive_labels.to_vec()
        }
        Selector::Diagonal => ctx.labels.to_vec(),
        Selector::None if p.quantity == "dof_rms" => ctx.labels.to_vec(),
        _ => {
            let _ = nd;
            Vec::new()
        }
    }
}

/// One spectrum view of curves on the run's lines, decimated to `width`.
fn spectrum_view(
    arrays: &mut Arrays,
    title: &str,
    y: Axis,
    freqs: &[f64],
    curves: &[Curve],
    width: usize,
    density: bool,
) -> Value {
    let series = curves
        .iter()
        .filter(|c| c.y.len() == freqs.len() && !c.y.is_empty())
        .map(|c| decimated_series(freqs, c, width, true, density))
        .collect();
    views::spectrum(arrays, title, &freq_axis(), &y, series, Some("frequency"))
}

/// A heatmap of rows against frequency, the nearest line to each pixel column.
pub fn heatmap_view(
    arrays: &mut Arrays,
    title: &str,
    row_labels: Vec<String>,
    bands: Vec<([f64; 2], [f64; 2])>,
    freqs: &[f64],
    rows: &[Vec<f64>],
    width: usize,
) -> Value {
    let groups = columns(freqs, width, true);
    let pick = nearest(freqs, &groups, true);
    let x: Vec<f64> = pick.iter().map(|l| freqs[*l]).collect();
    let mut values: Vec<f32> = Vec::with_capacity(rows.len() * pick.len());
    for r in rows {
        values.extend(
            pick.iter()
                .map(|l| r.get(*l).copied().unwrap_or(f64::NAN) as f32),
        );
    }
    let (nr, nc) = (rows.len(), x.len());
    let x_ref = arrays.push_f64(x);
    let v_ref = arrays
        .push(Array::new(vec![nr, nc], ArrayData::F32(values)).expect("shape holds the values"));
    json!({
        "view": "heatmap",
        "title": title,
        "rows": {"label": "Control", "ticks": row_labels},
        "x": json!({"label": "Frequency", "unit": "Hz", "scale": "log"}),
        "x_values": x_ref,
        "values": v_ref,
        "value": {"label": "Error", "unit": "dB"},
        "row_bands": bands.iter().map(|(a, b)| json!({
            "alarm_db": [num(a[0]), num(a[1])], "abort_db": [num(b[0]), num(b[1])],
        })).collect::<Vec<_>>(),
    })
}

/// A block of the response spectral matrix, packed, the nearest line to each pixel
/// column, with the reference's block on the same lines.
pub fn sdm_view(
    arrays: &mut Arrays,
    title: &str,
    channel_labels: Vec<String>,
    unit: &str,
    freqs: &[f64],
    (measured, reference): (&PackedBlock, Option<&PackedBlock>),
    width: usize,
) -> Value {
    let groups = columns(freqs, width, true);
    let pick = nearest(freqs, &groups, true);
    let per = measured.n * (measured.n + 1) / 2;
    let x: Vec<f64> = pick.iter().map(|l| freqs[*l]).collect();
    let x_ref = arrays.push_f64(x);
    let mut block = |b: &PackedBlock| -> Value {
        if b.re.len() != freqs.len() * per || b.im.len() != b.re.len() {
            return Value::Null;
        }
        let take = |v: &[f64]| -> Vec<f64> {
            pick.iter()
                .flat_map(|l| v[l * per..(l + 1) * per].iter().copied())
                .collect()
        };
        let re = arrays
            .push(Array::new(vec![pick.len(), per], ArrayData::F64(take(&b.re))).expect("shape"));
        let im = arrays
            .push(Array::new(vec![pick.len(), per], ArrayData::F64(take(&b.im))).expect("shape"));
        json!({"re": re, "im": im})
    };
    let m = block(measured);
    let r = reference.map_or(Value::Null, &mut block);
    json!({
        "view": "sdm",
        "title": title,
        "channels": channel_labels,
        "packing": "upper",
        "unit": format!("{unit}²/Hz"),
        "x": json!({"label": "Frequency", "unit": "Hz", "scale": "log"}),
        "x_values": x_ref,
        "measured": m,
        "reference": r,
    })
}

/// A plot's views from the data the driver returned for its resolved selectors (in
/// the same order as [`resolve`]), and the status for the quantities that are
/// numbers of the loop.
pub fn plot_views(
    p: &MimoPlot,
    ctx: &PlotContext<'_>,
    status: &MimoStatus,
    data: &[Option<Series>],
    arrays: &mut Arrays,
) -> Vec<Value> {
    let u = ctx.unit;
    let psd_axis = || Axis::new("PSD", Some(&format!("{u}²/Hz")), Scale::Log);
    let lab = plot_labels(p, ctx);
    let lines_of = |s: &Option<Series>| match s {
        Some(Series::Lines { freqs, curves }) => Some((freqs.clone(), curves.clone())),
        _ => None,
    };
    match p.quantity {
        "cap" | "pred_drive_rms" => {
            let ks: Vec<usize> = match &p.selector {
                Selector::Index(k) if *k < ctx.drive_labels.len() => vec![*k],
                Selector::Diagonal => (0..ctx.drive_labels.len()).collect(),
                _ => Vec::new(),
            };
            if ks.is_empty() {
                return Vec::new();
            }
            let rows = ks
                .iter()
                .map(|k| {
                    let name = label(ctx.drive_labels, *k);
                    if p.quantity == "cap" {
                        match status.caps.get(*k) {
                            Some(c) => Row::number(
                                &name,
                                views::round_to(*c, 3),
                                None,
                                if *c < 1.0 { Status::Warn } else { Status::Info },
                            ),
                            None => Row::text(&name, "not driving", Status::Info),
                        }
                    } else {
                        match status
                            .prediction
                            .as_ref()
                            .and_then(|pr| pr.drive_rms_v.get(*k).zip(pr.ceiling_vrms.get(*k)))
                        {
                            Some((v, c)) => Row::number(
                                &name,
                                views::round_to(*v, 4),
                                Some("V"),
                                if v <= c { Status::Pass } else { Status::Fail },
                            ),
                            None => Row::text(&name, "after identification", Status::Info),
                        }
                    }
                })
                .collect();
            let title = if p.quantity == "cap" {
                "Drive Caps"
            } else {
                "Predicted Drive"
            };
            vec![views::reading(title, rows)]
        }
        "readings" => {
            let mut rows = vec![
                Row::number("Loop", status.loop_index as f64, None, Status::Info),
                match status.level_db {
                    Some(l) => {
                        Row::number("Level", views::round_to(l, 2), Some("dB"), Status::Info)
                    }
                    None => Row::text("Level", "not driving", Status::Info),
                },
            ];
            if let Some(w) = status
                .control_error_db
                .iter()
                .map(|e| e.abs())
                .reduce(f64::max)
            {
                rows.push(Row::number(
                    "Largest control error",
                    views::round_to(w, 2),
                    Some("dB"),
                    if status.alarm {
                        Status::Warn
                    } else {
                        Status::Pass
                    },
                ));
            }
            if let Some(g) = &status.gates {
                rows.push(Row::number(
                    "Smallest multiple coherence",
                    views::round_to(g.coherence_min, 3),
                    None,
                    if g.coherence_min >= g.coherence_floor {
                        Status::Pass
                    } else {
                        Status::Fail
                    },
                ));
                rows.push(Row::number(
                    "Largest condition number",
                    super::check::sig(g.condition_max, 4),
                    None,
                    if g.condition_max <= g.condition_limit {
                        Status::Pass
                    } else {
                        Status::Fail
                    },
                ));
            }
            if let Some(e) = status.elapsed_s {
                rows.push(Row::number(
                    "Elapsed",
                    views::round_to(e, 1),
                    Some("s"),
                    Status::Info,
                ));
            }
            vec![views::reading("Run", rows)]
        }
        "dof_rms" => {
            let rows: Vec<Row> = data
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    let (_, curves) = lines_of(s)?;
                    let m = curves.iter().find(|c| c.role == Role::Measured)?;
                    let rms = (m.y.iter().sum::<f64>() * ctx.df_hz).sqrt();
                    Some(Row::number(
                        &label(ctx.labels, i),
                        super::check::sig(rms, 4),
                        Some(u),
                        Status::Info,
                    ))
                })
                .collect();
            if rows.is_empty() {
                return Vec::new();
            }
            vec![views::reading("Degree-of-Freedom RMS", rows)]
        }
        "error_db" => match data.first() {
            Some(Some(Series::Rows { freqs, rows })) => {
                let idx: Vec<usize> = match &p.selector {
                    Selector::Rows(Some(r)) => r
                        .iter()
                        .copied()
                        .filter(|i| *i < ctx.labels.len())
                        .collect(),
                    _ => (0..ctx.labels.len()).collect(),
                };
                let bands = idx
                    .iter()
                    .map(|i| {
                        ctx.bands
                            .get(*i)
                            .copied()
                            .unwrap_or(([3.0, 3.0], [6.0, 6.0]))
                    })
                    .collect();
                vec![heatmap_view(
                    arrays,
                    "Control Error",
                    lab,
                    bands,
                    freqs,
                    rows,
                    p.points,
                )]
            }
            _ => Vec::new(),
        },
        "sdm" => match data.first() {
            Some(Some(Series::Packed {
                freqs,
                measured,
                reference,
            })) => {
                vec![sdm_view(
                    arrays,
                    "Spectral Matrix",
                    lab,
                    u,
                    freqs,
                    (measured, reference.as_ref()),
                    p.points,
                )]
            }
            _ => Vec::new(),
        },
        "frf" => match data.first().and_then(lines_of) {
            Some((freqs, curves)) if curves.len() == 2 => {
                let t = format!("FRF: {} from {}", lab[0], lab[1]);
                vec![
                    spectrum_view(
                        arrays,
                        &format!("{t}, Magnitude"),
                        Axis::new("Magnitude", Some(&format!("{u}/V")), Scale::Log),
                        &freqs,
                        &curves[..1],
                        p.points,
                        false,
                    ),
                    spectrum_view(
                        arrays,
                        &format!("{t}, Phase"),
                        Axis::new("Phase", Some("deg"), Scale::Linear),
                        &freqs,
                        &curves[1..],
                        p.points,
                        false,
                    ),
                ]
            }
            _ => Vec::new(),
        },
        "law_diag" => match data.first() {
            Some(Some(Series::Lines { freqs, curves })) => {
                let key = match &p.selector {
                    Selector::Key(k, _) => k.as_str(),
                    _ => "",
                };
                vec![spectrum_view(
                    arrays,
                    key,
                    Axis::new(key, None, Scale::Linear),
                    freqs,
                    curves,
                    p.points,
                    false,
                )]
            }
            Some(Some(Series::Scalar(x))) => {
                let key = match &p.selector {
                    Selector::Key(k, _) => k.as_str(),
                    _ => "",
                };
                vec![views::reading(
                    key,
                    vec![Row::number(key, *x, None, Status::Info)],
                )]
            }
            _ => Vec::new(),
        },
        q => {
            let (title, axis, density) = match q {
                "psd" => ("PSD", psd_axis(), true),
                "coh" => (
                    "Coherence",
                    Axis::new("Coherence", None, Scale::Linear),
                    false,
                ),
                "phase" => (
                    "Phase",
                    Axis::new("Phase", Some("deg"), Scale::Linear),
                    false,
                ),
                "csd_mag" => (
                    "Cross Spectrum",
                    Axis::new("CSD magnitude", Some(&format!("{u}²/Hz")), Scale::Log),
                    true,
                ),
                "drive_psd" => (
                    "Drive PSD",
                    Axis::new("PSD", Some("V²/Hz"), Scale::Log),
                    true,
                ),
                "drive_coh" => (
                    "Drive Coherence",
                    Axis::new("Coherence", None, Scale::Linear),
                    false,
                ),
                "sv" => (
                    "Singular Values",
                    Axis::new("Singular value", Some(&format!("{u}/V")), Scale::Log),
                    false,
                ),
                "cond" => (
                    "Condition Number",
                    Axis::new("Condition number", None, Scale::Log),
                    false,
                ),
                "mcoh" => (
                    "Multiple Coherence",
                    Axis::new("Coherence", None, Scale::Linear),
                    false,
                ),
                "trace" => ("Trace", psd_axis(), true),
                "pred_error_db" => (
                    "Predicted Error",
                    Axis::new("Error", Some("dB"), Scale::Linear),
                    false,
                ),
                _ => return Vec::new(),
            };
            let named = |extra: &str| {
                if extra.is_empty() {
                    title.to_owned()
                } else {
                    format!("{title}: {extra}")
                }
            };
            let diagonal = matches!(p.selector, Selector::Diagonal);
            if diagonal {
                // One view, the measured curve of each channel.
                let mut curves = Vec::new();
                let mut freqs = None;
                for (k, s) in data.iter().enumerate() {
                    if let Some((f, cs)) = lines_of(s) {
                        if let Some(c) = cs
                            .iter()
                            .find(|c| matches!(c.role, Role::Measured | Role::Drive))
                        {
                            curves.push(Curve {
                                name: lab.get(k).cloned().unwrap_or_default(),
                                role: c.role,
                                y: c.y.clone(),
                            });
                        }
                        freqs.get_or_insert(f);
                    }
                }
                return match freqs {
                    Some(f) => vec![spectrum_view(
                        arrays,
                        &named(""),
                        axis,
                        &f,
                        &curves,
                        p.points,
                        density,
                    )],
                    None => Vec::new(),
                };
            }
            match data.first().and_then(lines_of) {
                Some((freqs, curves)) => vec![spectrum_view(
                    arrays,
                    &named(&lab.join(", ")),
                    axis,
                    &freqs,
                    &curves,
                    p.points,
                    density,
                )],
                None => Vec::new(),
            }
        }
    }
}

/// One plot's entry in a `mimo_random.data` frame.
pub fn plot_json(p: &MimoPlot, labels: Vec<String>, views: Vec<Value>) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), Value::from(p.id.as_str()));
    m.insert("quantity".into(), Value::from(p.quantity));
    m.insert("selector".into(), p.selector.to_json());
    m.insert("labels".into(), Value::from(labels));
    m.insert("views".into(), Value::Array(views));
    Value::Object(m)
}

/// The plot context of a setup.
/// The names, unit and bands a setup's plots are drawn with.
pub struct SetupDisplay {
    pub labels: Vec<String>,
    pub drive_labels: Vec<String>,
    pub unit: String,
    pub bands: Vec<([f64; 2], [f64; 2])>,
}

pub fn context(setup: &MimoSetup) -> SetupDisplay {
    let labels = setup.controlled_labels();
    let drive_labels = setup.drives.iter().map(|d| d.label.clone()).collect();
    let unit = setup
        .controls
        .first()
        .map_or_else(|| "g".to_owned(), |c| c.unit.clone());
    let bands = match &setup.space {
        super::setup::ControlSpace::Physical => setup
            .controls
            .iter()
            .map(|c| (c.alarm_db, c.abort_db))
            .collect(),
        super::setup::ControlSpace::Virtual { dofs, .. } => {
            vec![([3.0, 3.0], [6.0, 6.0]); dofs.len()]
        }
    };
    SetupDisplay {
        labels,
        drive_labels,
        unit,
        bands,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn refused(v: Value) -> String {
        let e = parse(&p(v)).unwrap_err();
        assert_eq!(e.reason, Some("bad_params"));
        e.detail.unwrap()["param"].as_str().unwrap().to_owned()
    }

    #[test]
    fn selectors_parse_with_defaults_and_are_refused_by_their_path() {
        let s = parse(&p(json!({"plots": [
            {"id": "a", "quantity": "psd", "index": 3},
            {"id": "b", "quantity": "coh", "pair": [0, 5]},
            {"id": "c", "quantity": "error_db"},
            {"id": "d", "quantity": "psd", "diagonal": true, "points": 400},
            {"id": "e", "quantity": "sdm", "block": [0, 1, 2]},
            {"id": "f", "quantity": "cap"},
            {"id": "g", "quantity": "law_diag", "key": "condition"},
            {"id": "h", "quantity": "trace"},
        ]})))
        .unwrap()
        .unwrap();
        assert_eq!(s.plots[0].selector, Selector::Index(3));
        assert_eq!(s.plots[1].selector, Selector::Pair(0, 5));
        assert_eq!(s.plots[2].selector, Selector::Rows(None));
        assert_eq!(
            (s.plots[3].selector.clone(), s.plots[3].points),
            (Selector::Diagonal, 400)
        );
        assert_eq!(s.plots[5].selector, Selector::Diagonal);
        let r = result_json(Some(&s));
        assert_eq!(
            r["plots"][1],
            json!({"id": "b", "quantity": "coh", "pair": [0, 5], "points": 1024})
        );
        assert_eq!(r["plots"][2]["rows"], "all");
        assert_eq!(
            r["quantities"].as_array().unwrap().len(),
            MIMO_QUANTITIES.len()
        );
        assert_eq!(parse(&p(json!({"default": true}))).unwrap(), None);

        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "coh", "pair": [2, 2]}]})),
            "plots[0].pair"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "coh", "index": 2}]})),
            "plots[0].index"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "trace", "index": 2}]})),
            "plots[0].index"
        );
        assert_eq!(
            refused(
                json!({"plots": [{"id": "a", "quantity": "psd", "index": 1, "diagonal": true}]})
            ),
            "plots[0].diagonal"
        );
        assert_eq!(
            refused(
                json!({"plots": [{"id": "a", "quantity": "sdm", "block": [0,1,2,3,4,5,6,7,8]}]})
            ),
            "plots[0].block"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "sdm", "block": [1, 1]}]})),
            "plots[0].block"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "psd", "index": 0, "points": 10}]})),
            "plots[0].points"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "control"}]})),
            "plots[0].quantity"
        );
        assert_eq!(
            refused(
                json!({"plots": [{"id": "a", "quantity": "psd", "index": 0}, {"id": "a", "quantity": "trace"}]})
            ),
            "plots[1].id"
        );
        assert_eq!(refused(json!({"plots": [], "rate_hz": 20})), "rate_hz");
    }

    #[test]
    fn columns_cover_every_line_once_on_either_axis() {
        let freqs: Vec<f64> = (1..=1600).map(|k| k as f64 * 2.0).collect();
        for log in [true, false] {
            let g = columns(&freqs, 300, log);
            assert!(g.len() <= 300 && g.len() > 100, "{} columns", g.len());
            assert_eq!(g[0].start, 0);
            assert_eq!(g.last().unwrap().end, freqs.len());
            assert!(g
                .windows(2)
                .all(|w| w[0].end == w[1].start && !w[0].is_empty()));
        }
        // Few lines: one column each, untouched.
        let g = columns(&freqs[..50], 300, true);
        assert_eq!(g.len(), 50);
    }

    /// The decimated envelope holds every original point: each line's value lies
    /// between its column's minimum and maximum, and its frequency inside the
    /// column's edges.
    #[test]
    fn the_envelope_contains_every_original_point() {
        let freqs: Vec<f64> = (10..3210).map(|k| k as f64 * 0.5).collect();
        let y: Vec<f64> = freqs
            .iter()
            .enumerate()
            .map(|(k, f)| {
                1e-3 * (1.0 + (f / 37.0).sin().powi(2)) * if k % 97 == 0 { 50.0 } else { 1.0 }
            })
            .collect();
        let c = Curve {
            name: "Control".into(),
            role: Role::Measured,
            y: y.clone(),
        };
        for (width, log) in [(256usize, true), (500, false), (64, true)] {
            let s = decimated_series(&freqs, &c, width, log, true);
            let (edges, lo) = (s.edges.as_ref().unwrap(), s.y_min.as_ref().unwrap());
            assert!(s.y.len() <= width && s.y.len() == lo.len() && edges.len() == s.y.len() + 1);
            let mut b = 0;
            for (l, (f, v)) in freqs.iter().zip(&y).enumerate() {
                while *f > edges[b + 1] {
                    b += 1;
                }
                assert!(
                    edges[b] <= *f && *f <= edges[b + 1],
                    "line {l} outside its column"
                );
                assert!(
                    lo[b] <= *v && *v <= s.y[b],
                    "line {l}: {v} outside [{}, {}]",
                    lo[b],
                    s.y[b]
                );
            }
            // The largest value survives as a column's maximum.
            assert!(
                s.y.iter().cloned().fold(0.0, f64::max) == y.iter().cloned().fold(0.0, f64::max)
            );
        }
        // Undecimated: the lines themselves, each across its own bin.
        let s = decimated_series(
            &freqs[..40],
            &Curve {
                y: y[..40].to_vec(),
                ..c.clone()
            },
            256,
            true,
            true,
        );
        assert_eq!(s.x, freqs[..40].to_vec());
        assert!(s.y_min.is_none());
        assert_eq!(s.edges.unwrap(), views::bin_edges(&freqs[..40]));
    }

    #[test]
    fn heatmap_columns_take_the_nearest_line() {
        let freqs: Vec<f64> = (1..=1000).map(|k| k as f64).collect();
        let g = columns(&freqs, 100, true);
        let pick = nearest(&freqs, &g, true);
        for (gr, l) in g.iter().zip(&pick) {
            assert!(gr.contains(l));
            let mid = (freqs[gr.start] * freqs[gr.end - 1]).sqrt();
            assert!(gr
                .clone()
                .all(|k| (freqs[*l] - mid).abs() <= (freqs[k] - mid).abs()));
        }
    }
}
