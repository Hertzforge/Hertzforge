//! `signals.subscribe` (schema/PROTOCOL.md, "Signals"): what one connection asks to be
//! sent, parsed and checked, and the `signals.data` frame built for it from a run's tap.
//!
//! A frame is assembled only from the channels a connection's plots show: a channel
//! nobody plots is never copied out of the ring.

use super::{Kind, SignalChannel, TapRun, MAX_WINDOW_S};
use crate::codec::Arrays;
use crate::protocol::{num, MethodError};
use crate::views::{self, Axis, Role, Scale, Series};
use serde_json::{json, Map, Value};

/// Frames per second when `rate_hz` is not given, and its bounds.
pub const DEFAULT_RATE_HZ: f64 = 10.0;
pub const MIN_RATE_HZ: f64 = 0.5;
pub const MAX_RATE_HZ: f64 = 10.0;
/// The most plots one subscription may hold.
pub const MAX_PLOTS: usize = 64;
/// The longest plot id, in characters.
pub const MAX_ID_CHARS: usize = 64;
/// The window a plot shows when it does not say, seconds.
pub const DEFAULT_WINDOW_S: f64 = 2.0;
/// Points per channel: the default and the bounds.
pub const DEFAULT_POINTS: usize = 1024;
pub const MIN_POINTS: usize = 64;
pub const MAX_POINTS: usize = 8192;
/// The channel selectors by name.
pub const SELECTORS: [&str; 4] = ["all", "inputs", "drives", "control"];

/// Which channels a plot shows, resolved against the run's channels on every frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channels {
    /// Every input and every drive.
    All,
    Inputs,
    Drives,
    /// The control rows.
    Control,
    /// These table rows; one the run does not have is left out.
    Ids(Vec<u32>),
}

impl Channels {
    fn to_json(&self) -> Value {
        match self {
            Channels::All => Value::from("all"),
            Channels::Inputs => Value::from("inputs"),
            Channels::Drives => Value::from("drives"),
            Channels::Control => Value::from("control"),
            Channels::Ids(ids) => Value::Array(ids.iter().map(|i| Value::from(*i)).collect()),
        }
    }

    /// The channels of `all` (table order) this selects, in table order.
    pub fn resolve<'a>(&self, all: &'a [SignalChannel]) -> Vec<&'a SignalChannel> {
        all.iter()
            .filter(|c| match self {
                Channels::All => true,
                Channels::Inputs => c.kind == Kind::Input,
                Channels::Drives => c.kind == Kind::Drive,
                Channels::Control => c.role == "control",
                Channels::Ids(ids) => ids.contains(&c.index),
            })
            .collect()
    }
}

/// How a plot's channels are laid out in its views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrange {
    /// One view for each unit, the channels in it overlaid.
    Overlay,
    /// One view for each channel.
    Split,
}

impl Arrange {
    fn name(self) -> &'static str {
        match self {
            Arrange::Overlay => "overlay",
            Arrange::Split => "split",
        }
    }
}

/// One plot of a subscription, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub id: String,
    pub channels: Channels,
    pub window_s: f64,
    pub points: usize,
    pub arrange: Arrange,
}

/// What one connection asked to be sent.
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

fn channels_param(v: Option<&Value>, at: &str) -> Result<Channels, MethodError> {
    let words = "\"channels\" is a list of channel indexes, or \"all\", \"inputs\", \"drives\" \
                 or \"control\"";
    match v {
        None | Some(Value::Null) => Ok(Channels::All),
        Some(Value::String(s)) => match s.as_str() {
            "all" => Ok(Channels::All),
            "inputs" => Ok(Channels::Inputs),
            "drives" => Ok(Channels::Drives),
            "control" => Ok(Channels::Control),
            _ => Err(bad(at, words)),
        },
        Some(Value::Array(a)) => {
            let mut ids: Vec<u32> = Vec::with_capacity(a.len());
            for x in a {
                let id = x
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| bad(at, "a channel index is a whole number from 0"))?;
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            Ok(Channels::Ids(ids))
        }
        Some(_) => Err(bad(at, words)),
    }
}

/// `signals.subscribe`'s params.
pub fn parse(params: &Map<String, Value>) -> Result<Subscription, MethodError> {
    if let Some(k) = params
        .keys()
        .find(|k| !["rate_hz", "plots"].contains(&k.as_str()))
    {
        return Err(bad(
            k,
            format!("signals.subscribe takes \"rate_hz\" and \"plots\", not \"{k}\""),
        ));
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
                "a subscription lists its plots (\"plots\", possibly empty)",
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
            .find(|k| !["id", "channels", "window_s", "points", "arrange"].contains(&k.as_str()))
        {
            return Err(bad(
                &at(k),
                format!("a plot has an id, channels, window_s, points and arrange; not \"{k}\""),
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
        let channels = channels_param(o.get("channels"), &at("channels"))?;
        let window_s = match o.get("window_s") {
            None | Some(Value::Null) => DEFAULT_WINDOW_S,
            Some(v) => match v.as_f64() {
                Some(x) if x.is_finite() && x > 0.0 && x <= MAX_WINDOW_S => x,
                _ => {
                    return Err(bad(
                        &at("window_s"),
                        format!(
                            "\"window_s\" is a number of seconds above 0 and at most {}",
                            num(MAX_WINDOW_S)
                        ),
                    ))
                }
            },
        };
        let points = match o.get("points") {
            None | Some(Value::Null) => DEFAULT_POINTS,
            Some(v) => match v.as_u64() {
                Some(n) if (MIN_POINTS as u64..=MAX_POINTS as u64).contains(&n) => n as usize,
                _ => {
                    return Err(bad(
                        &at("points"),
                        format!("\"points\" is a whole number from {MIN_POINTS} to {MAX_POINTS}"),
                    ))
                }
            },
        };
        let arrange = match o.get("arrange") {
            None | Some(Value::Null) => Arrange::Overlay,
            Some(Value::String(s)) if s == "overlay" => Arrange::Overlay,
            Some(Value::String(s)) if s == "split" => Arrange::Split,
            Some(_) => {
                return Err(bad(
                    &at("arrange"),
                    "\"arrange\" is \"overlay\" (one view for each unit) or \"split\" (one \
                     view for each channel)",
                ))
            }
        };
        plots.push(Plot {
            id,
            channels,
            window_s,
            points,
            arrange,
        });
    }
    Ok(Subscription { rate_hz, plots })
}

fn plot_json(p: &Plot) -> Value {
    json!({
        "id": p.id,
        "channels": p.channels.to_json(),
        "window_s": num(p.window_s),
        "points": p.points,
        "arrange": p.arrange.name(),
    })
}

/// The run's channels as the wire lists them.
pub fn channels_json(tap: Option<&TapRun>) -> Value {
    Value::Array(
        tap.map(|t| t.channels())
            .unwrap_or_default()
            .iter()
            .map(|c| json!({"index": c.index, "name": c.name, "unit": c.unit, "kind": c.kind.name()}))
            .collect(),
    )
}

/// The `signals.subscribe` result: whether a run is acquiring and its channels, and the
/// subscription with every default filled in.
pub fn result_json(sub: &Subscription, tap: Option<&TapRun>) -> Value {
    let live = tap.filter(|t| t.is_live());
    json!({
        "available": live.is_some(),
        "fs_hz": live.map_or(Value::Null, |t| num(t.fs_hz())),
        "max_window_s": live.map_or(Value::Null, |t| num(t.max_window_s())),
        "rate_hz": num(sub.rate_hz),
        "channels": channels_json(live),
        "plots": sub.plots.iter().map(plot_json).collect::<Vec<_>>(),
    })
}

/// `Channel <index>: <name>` for one channel, `Channels <i>, <j>` for several.
fn view_title(chs: &[&SignalChannel]) -> String {
    match chs {
        [c] => format!("Channel {}: {}", c.index, c.name),
        _ => format!(
            "Channels {}",
            chs.iter()
                .map(|c| c.index.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// One channel's series over `[lo, hi)`: every sample when they fit in `points`, else
/// the envelope at `points`. `None` when the ring holds none of it yet.
fn series(
    tap: &TapRun,
    ch: &SignalChannel,
    lo: u64,
    hi: u64,
    points: usize,
    buf: &mut Vec<f32>,
) -> Option<Series> {
    let first = tap.window(ch, lo, hi, buf);
    if buf.is_empty() {
        return None;
    }
    let fs = tap.fs_hz();
    let role = match ch.kind {
        Kind::Input => Role::Measured,
        Kind::Drive => Role::Drive,
    };
    let name = format!("Channel {}", ch.index);
    if buf.len() <= points {
        let x = (0..buf.len())
            .map(|k| (first + k as u64) as f64 / fs)
            .collect();
        let y = buf.iter().map(|v| f64::from(*v)).collect();
        return Some(Series::new(&name, role, x, y));
    }
    let span = views::envelope_span(buf.len(), points);
    let (hi_y, lo_y) = views::envelope(buf, span);
    let x = (0..hi_y.len())
        .map(|k| (first + (k * span) as u64) as f64 / fs)
        .collect();
    let mut s = Series::new(&name, role, x, hi_y);
    s.y_min = Some(lo_y);
    Some(s)
}

fn view(
    tap: &TapRun,
    chs: &[&SignalChannel],
    lo: u64,
    hi: u64,
    points: usize,
    arrays: &mut Arrays,
    buf: &mut Vec<f32>,
) -> Value {
    let series: Vec<Series> = chs
        .iter()
        .filter_map(|c| series(tap, c, lo, hi, points, buf))
        .collect();
    views::spectrum(
        arrays,
        &view_title(chs),
        &Axis::new("Time", Some("s"), Scale::Linear),
        &Axis::new("Amplitude", Some(&chs[0].unit), Scale::Linear),
        series,
        None,
    )
}

/// The `signals.data` frame of `tap` for one connection's subscription. The windows end
/// at the run's newest input frame.
pub fn data_json(tap: &TapRun, sub: &Subscription, arrays: &mut Arrays) -> Value {
    let now = tap.now();
    let fs = tap.fs_hz();
    let mut buf = Vec::new();
    let plots: Vec<Value> = sub
        .plots
        .iter()
        .map(|p| {
            let chs = p.channels.resolve(tap.channels());
            let frames = ((p.window_s * fs).round() as u64).clamp(1, tap.window_frames());
            let lo = now.saturating_sub(frames);
            let views: Vec<Value> = match p.arrange {
                Arrange::Split => chs
                    .iter()
                    .map(|c| view(tap, &[c], lo, now, p.points, arrays, &mut buf))
                    .collect(),
                Arrange::Overlay => {
                    let mut units: Vec<&str> = Vec::new();
                    for c in &chs {
                        if !units.contains(&c.unit.as_str()) {
                            units.push(&c.unit);
                        }
                    }
                    units
                        .iter()
                        .map(|u| {
                            let group: Vec<&SignalChannel> =
                                chs.iter().copied().filter(|c| c.unit == *u).collect();
                            view(tap, &group, lo, now, p.points, arrays, &mut buf)
                        })
                        .collect()
                }
            };
            json!({
                "id": p.id,
                "channels": chs.iter().map(|c| c.index).collect::<Vec<_>>(),
                "window_s": num(frames as f64 / fs),
                "first_frame": lo,
                "views": views,
            })
        })
        .collect();
    json!({
        "available": tap.is_live(),
        "fs_hz": num(fs),
        "t_s": num(now as f64 / fs),
        "frames": now,
        "max_window_s": num(tap.max_window_s()),
        "lost_frames": tap.lost_frames(),
        "channels": channels_json(Some(tap)),
        "plots": plots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{Kind, SignalChannel, TapRun};

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

    fn ch(index: u32, kind: Kind, unit: &str, role: &'static str, col: usize) -> SignalChannel {
        SignalChannel {
            index,
            name: format!("Row {index}"),
            unit: unit.into(),
            kind,
            role,
            col,
        }
    }

    /// Rows 1 (control, g), 2 (drive), 3 (monitor, lbf) at 1024 Hz, 4096 frames of each
    /// input and of the drive: input col 0 reads its frame, col 1 minus its frame, the
    /// drive its frame over a thousand.
    fn tap() -> TapRun {
        let run = TapRun::new(
            7,
            1024.0,
            vec![
                ch(1, Kind::Input, "g", "control", 0),
                ch(2, Kind::Drive, "V", "drive", 0),
                ch(3, Kind::Input, "lbf", "monitor", 1),
            ],
        );
        let ins: Vec<f32> = (0..4096).flat_map(|f| [f as f32, -(f as f32)]).collect();
        run.put_inputs(&ins, 4096);
        let drv: Vec<f32> = (0..4096).map(|f| f as f32 / 1000.0).collect();
        run.put_drives(&drv, 4096);
        run
    }

    #[test]
    fn a_subscription_parses_with_every_default_filled() {
        let s = parse(&params(json!({"plots": [
            {"id": "a"},
            {"id": "b", "channels": [3, 2, 3], "window_s": 0.5, "points": 64, "arrange": "split"},
            {"id": "c", "channels": "drives"},
        ]})))
        .unwrap();
        assert_eq!(s.rate_hz, DEFAULT_RATE_HZ);
        assert_eq!(s.plots[0].channels, Channels::All);
        assert_eq!(s.plots[0].window_s, DEFAULT_WINDOW_S);
        assert_eq!(s.plots[0].points, DEFAULT_POINTS);
        assert_eq!(s.plots[0].arrange, Arrange::Overlay);
        assert_eq!(s.plots[1].channels, Channels::Ids(vec![3, 2]));
        let r = result_json(&s, None);
        assert_eq!(r["available"], false);
        assert_eq!(r["fs_hz"], Value::Null);
        assert_eq!(r["channels"], json!([]));
        assert_eq!(
            r["plots"][1],
            json!({"id": "b", "channels": [3, 2], "window_s": 0.5, "points": 64,
                   "arrange": "split"})
        );
        assert_eq!(r["plots"][2]["channels"], "drives");
        let t = tap();
        let r = result_json(&s, Some(&t));
        assert_eq!(r["available"], true);
        assert_eq!(r["fs_hz"], 1024);
        assert_eq!(r["max_window_s"], 10);
        assert_eq!(
            r["channels"][1],
            json!({"index": 2, "name": "Row 2", "unit": "V", "kind": "drive"})
        );
        t.end();
        assert_eq!(
            result_json(&s, Some(&t))["available"],
            false,
            "an ended run is not offered"
        );
    }

    #[test]
    fn refusals_name_the_field() {
        assert_eq!(refused(json!({})), "plots");
        assert_eq!(refused(json!({"plots": [], "rate_hz": 20})), "rate_hz");
        assert_eq!(refused(json!({"plots": [], "channels": [1]})), "channels");
        assert_eq!(refused(json!({"plots": [{"id": ""}]})), "plots[0].id");
        assert_eq!(
            refused(json!({"plots": [{"id": "a"}, {"id": "a"}]})),
            "plots[1].id"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "quantity": "time"}]})),
            "plots[0].quantity"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "channels": "limited"}]})),
            "plots[0].channels"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "channels": [-1]}]})),
            "plots[0].channels"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "window_s": 0}]})),
            "plots[0].window_s"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "window_s": 10.5}]})),
            "plots[0].window_s"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "points": 63}]})),
            "plots[0].points"
        );
        assert_eq!(
            refused(json!({"plots": [{"id": "a", "arrange": "stacked"}]})),
            "plots[0].arrange"
        );
        let many: Vec<Value> = (0..65).map(|i| json!({"id": format!("p{i}")})).collect();
        assert_eq!(refused(json!({ "plots": many })), "plots");
    }

    #[test]
    fn selectors_resolve_in_table_order_and_leave_out_rows_the_run_lacks() {
        let t = tap();
        let idx =
            |c: Channels| -> Vec<u32> { c.resolve(t.channels()).iter().map(|c| c.index).collect() };
        assert_eq!(idx(Channels::All), [1, 2, 3]);
        assert_eq!(idx(Channels::Inputs), [1, 3]);
        assert_eq!(idx(Channels::Drives), [2]);
        assert_eq!(idx(Channels::Control), [1]);
        assert_eq!(idx(Channels::Ids(vec![3, 99, 1])), [1, 3]);
    }

    /// Every sample when the window fits the points, on the station's time base.
    #[test]
    fn a_short_window_sends_every_sample_at_its_station_time() {
        let t = tap();
        let s = parse(&params(json!({"plots": [
            {"id": "a", "channels": [3], "window_s": 0.0625, "points": 64},
        ]})))
        .unwrap();
        let mut arrays = Arrays::new();
        let d = data_json(&t, &s, &mut arrays);
        assert_eq!(d["t_s"], 4);
        assert_eq!(d["frames"], 4096);
        let p = &d["plots"][0];
        assert_eq!(p["channels"], json!([3]));
        assert_eq!(p["first_frame"], 4032);
        assert_eq!(p["window_s"], 0.0625);
        let v = &p["views"][0];
        assert_eq!(v["title"], "Channel 3: Row 3");
        assert_eq!(v["y"]["unit"], "lbf");
        assert!(v["series"][0].get("y_min").is_none());
        let a: Vec<_> = arrays.iter().collect();
        let (x, y) = (a[0].1.to_f64(), a[1].1.to_f64());
        assert_eq!(x.len(), 64);
        assert_eq!(x[0], 4032.0 / 1024.0);
        for w in x.windows(2) {
            assert_eq!(w[1] - w[0], 1.0 / 1024.0);
        }
        assert_eq!(y[0], -4032.0);
        assert_eq!(y[63], -4095.0);
    }

    /// A window with more samples than points is the envelope at the points, the drive
    /// in its own view in volts when the plot overlays channels.
    #[test]
    fn a_long_window_is_the_envelope_and_units_are_never_overlaid() {
        let t = tap();
        let s = parse(&params(json!({"plots": [
            {"id": "b", "window_s": 1, "points": 64},
            {"id": "c", "window_s": 1, "points": 64, "arrange": "split"},
        ]})))
        .unwrap();
        let mut arrays = Arrays::new();
        let d = data_json(&t, &s, &mut arrays);
        let b = &d["plots"][0];
        assert_eq!(b["channels"], json!([1, 2, 3]));
        let titles: Vec<&str> = b["views"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            ["Channel 1: Row 1", "Channel 2: Row 2", "Channel 3: Row 3"]
        );
        assert_eq!(b["views"][1]["series"][0]["role"], "drive");
        assert_eq!(b["views"][1]["y"]["unit"], "V");
        assert_eq!(d["plots"][1]["views"].as_array().unwrap().len(), 3);
        // The first series: 1024 samples at 64 points, each the largest and smallest of 16.
        let a: Vec<_> = arrays.iter().collect();
        let (x, hi, lo) = (a[0].1.to_f64(), a[1].1.to_f64(), a[2].1.to_f64());
        assert_eq!(x.len(), 64);
        assert_eq!(x[1] - x[0], 16.0 / 1024.0);
        assert_eq!((lo[0], hi[0]), (3072.0, 3087.0));
        assert_eq!((lo[63], hi[63]), (4080.0, 4095.0));
    }

    #[test]
    fn two_inputs_of_one_unit_share_a_view() {
        let run = TapRun::new(
            1,
            100.0,
            vec![
                ch(1, Kind::Input, "g", "control", 0),
                ch(4, Kind::Input, "g", "monitor", 1),
                ch(5, Kind::Drive, "V", "drive", 0),
            ],
        );
        run.put_inputs(&[1.0, 2.0, 3.0, 4.0], 2);
        let s = parse(&params(json!({"plots": [{"id": "a", "window_s": 1}]}))).unwrap();
        let mut arrays = Arrays::new();
        let d = data_json(&run, &s, &mut arrays);
        let views = d["plots"][0]["views"].as_array().unwrap();
        assert_eq!(views.len(), 2);
        assert_eq!(views[0]["title"], "Channels 1, 4");
        assert_eq!(views[0]["series"].as_array().unwrap().len(), 2);
        // The drive has written nothing: its view is there, with no series yet.
        assert_eq!(views[1]["series"], json!([]));
        assert_eq!(d["plots"][0]["first_frame"], 0);
    }
}
