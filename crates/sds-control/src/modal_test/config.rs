//! A Modal Test configuration on the wire (schema/PROTOCOL.md, "Modal Test"): its
//! shape, and the report a configure returns before anything is armed. The rules a
//! setup is refused by are the crate's ([`hertzforge_modal_test::setup`] and
//! [`hertzforge_modal_test::plan`]); this module reads the shape and turns refusals into
//! the service's items.

use crate::protocol::{num, MethodError};
use crate::shock::config::Obj;
use crate::table::{Item, Report};
use hertzforge_channel_table::ChannelTable;
use hertzforge_modal_test::plan::Plan;
use hertzforge_modal_test::setup::{
    AcceptMode, DriveSet, Estimator, ExpWindowSet, ForceTarget, ForceWindowSet, ImpactSet,
    ModalSetup, Mode, Point, Polarity, Refusal, Roving, ShakerSet, SourceKind, LEVEL_DEFAULT_VRMS,
    STOP_RAMP_DEPTH_DB, STOP_RAMP_S, TARGET_TOLERANCE_PCT,
};
use hertzforge_modal_test::window::{ExpWindow, ForceWindow};
use serde_json::{json, Value};

/// The run folder's name when the setup gives none.
pub const DEFAULT_RUN_NAME: &str = "modal_test";

/// A configuration as accepted.
#[derive(Debug, Clone, PartialEq)]
pub struct WireModal {
    pub raw: Value,
    pub title: String,
    pub operator: String,
    pub article: String,
    pub notes: String,
    pub specification: Option<(String, String)>,
    pub metadata: Vec<(String, String, String)>,
    pub run_name: String,
    pub record_streams: bool,
    pub setup: ModalSetup,
}

fn bad(path: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(path, message)
}

fn only(o: &Obj<'_>, keys: &[&str]) -> Result<(), MethodError> {
    match o.m.keys().find(|k| !keys.contains(&k.as_str())) {
        None => Ok(()),
        Some(k) => Err(bad(
            &o.at(k),
            format!(
                "\"{}\" is not a field of the configuration; {} takes {}",
                o.at(k),
                o.path,
                keys.join(", ")
            ),
        )),
    }
}

fn index(v: &Value, at: &str) -> Result<u32, MethodError> {
    v.as_u64()
        .and_then(|x| u32::try_from(x).ok())
        .ok_or_else(|| bad(at, "a table index"))
}

fn roving(o: &Obj<'_>) -> Result<Roving, MethodError> {
    only(o, &["channels", "points"])?;
    let channels = o
        .array("channels")?
        .ok_or_else(|| bad(&o.at("channels"), "the channels that rove (table indices)"))?
        .iter()
        .enumerate()
        .map(|(i, v)| index(v, &format!("{}[{i}]", o.at("channels"))))
        .collect::<Result<Vec<_>, _>>()?;
    let points = match o.m.get("points") {
        Some(Value::Array(list)) => list
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let q = Obj::of(p, &format!("{}[{i}]", o.at("points")))?;
                only(&q, &["node", "direction"])?;
                let node = q
                    .opt_count("node")?
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| bad(&q.at("node"), "a node number"))?;
                Ok(Point {
                    node,
                    direction: q.opt_str("direction")?.map(str::to_owned),
                })
            })
            .collect::<Result<Vec<_>, MethodError>>()?,
        Some(v @ Value::Object(_)) => {
            let q = Obj::of(v, &o.at("points"))?;
            only(&q, &["start", "step", "count"])?;
            let start = q.opt_count("start")?.unwrap_or(1);
            let step = q.opt_count("step")?.unwrap_or(1);
            let count = q
                .opt_count("count")?
                .ok_or_else(|| bad(&q.at("count"), "how many points"))?;
            if count > 10_000 {
                return Err(bad(&q.at("count"), "at most 10000 points"));
            }
            (0..count)
                .map(|k| {
                    u32::try_from(start + k * step)
                        .map(|node| Point {
                            node,
                            direction: None,
                        })
                        .map_err(|_| bad(&q.at("start"), "a node number"))
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        _ => {
            return Err(bad(
                &o.at("points"),
                "the points: a list of {node, direction} or {start, step, count}",
            ))
        }
    };
    Ok(Roving { channels, points })
}

fn impact(o: &Obj<'_>) -> Result<ImpactSet, MethodError> {
    only(
        o,
        &[
            "trigger_level_pct",
            "polarity",
            "pretrigger_pct",
            "holdoff_blocks",
            "double_hit_pct",
            "tip_warn_db",
            "accept",
            "force_window",
            "exponential_window",
            "auto_advance",
            "target",
        ],
    )?;
    let d = ImpactSet::default();
    let accept = match o.opt_str("accept")? {
        None | Some("auto") => AcceptMode::Auto,
        Some("manual") => AcceptMode::Manual,
        Some(x) => {
            return Err(bad(
                &o.at("accept"),
                format!("\"{x}\" is not an accept mode; it is auto or manual"),
            ))
        }
    };
    let polarity = match o.opt_str("polarity")? {
        None | Some("positive") => Polarity::Positive,
        Some("negative") => Polarity::Negative,
        Some(x) => {
            return Err(bad(
                &o.at("polarity"),
                format!("\"{x}\" is not a polarity; it is positive or negative"),
            ))
        }
    };
    let force_window = match o.opt_obj("force_window")? {
        None => d.force_window,
        Some(f) => {
            only(&f, &["on", "width_pct", "fall_pct"])?;
            let dd = ForceWindowSet::default();
            ForceWindowSet {
                on: f.opt_bool("on")?.unwrap_or(dd.on),
                width_pct: f.f64_or("width_pct", dd.width_pct)?,
                taper_pct: f.f64_or("fall_pct", dd.taper_pct)?,
            }
        }
    };
    let exp_window = match o.opt_obj("exponential_window")? {
        None => d.exp_window,
        Some(f) => {
            only(&f, &["on", "end_pct"])?;
            let dd = ExpWindowSet::default();
            ExpWindowSet {
                on: f.opt_bool("on")?.unwrap_or(dd.on),
                end_pct: f.f64_or("end_pct", dd.end_pct)?,
            }
        }
    };
    let target = match o.m.get("target") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let t = Obj::of(v, &o.at("target"))?;
            only(&t, &["peak", "tolerance_pct"])?;
            Some(ForceTarget {
                peak: t.f64("peak")?,
                tolerance_pct: t.f64_or("tolerance_pct", TARGET_TOLERANCE_PCT)?,
            })
        }
    };
    Ok(ImpactSet {
        trigger_pct: o.f64_or("trigger_level_pct", d.trigger_pct)?,
        polarity,
        pretrigger_pct: o.f64_or("pretrigger_pct", d.pretrigger_pct)?,
        holdoff_blocks: o.f64_or("holdoff_blocks", d.holdoff_blocks)?,
        double_hit_pct: o.f64_or("double_hit_pct", d.double_hit_pct)?,
        tip_warn_db: o.f64_or("tip_warn_db", d.tip_warn_db)?,
        accept,
        force_window,
        exp_window,
        auto_advance: o.opt_bool("auto_advance")?.unwrap_or(d.auto_advance),
        target,
    })
}

fn shaker(o: &Obj<'_>) -> Result<ShakerSet, MethodError> {
    only(
        o,
        &[
            "drives",
            "source",
            "burst_pct",
            "ramp_s",
            "ceiling_v",
            "delay_frames",
            "overlap_pct",
            "seed",
        ],
    )?;
    let d = ShakerSet::default();
    let drives = o
        .array("drives")?
        .ok_or_else(|| bad(&o.at("drives"), "the drive rows and their levels"))?
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let q = Obj::of(v, &format!("{}[{i}]", o.at("drives")))?;
            only(&q, &["row", "level_vrms"])?;
            Ok(DriveSet {
                row: index(q.require("row")?, &q.at("row"))?,
                level_vrms: q.f64_or("level_vrms", LEVEL_DEFAULT_VRMS)?,
            })
        })
        .collect::<Result<Vec<_>, MethodError>>()?;
    let source = match o.opt_str("source")? {
        None => d.source,
        Some(s) => SourceKind::from_name(s).ok_or_else(|| {
            bad(
                &o.at("source"),
                format!(
                    "\"{s}\" is not a source; it is random, burst_random, pseudo_random, \
                     periodic_random, periodic_chirp or burst_chirp"
                ),
            )
        })?,
    };
    let delay = o
        .opt_count("delay_frames")?
        .unwrap_or(u64::from(d.delay_frames));
    Ok(ShakerSet {
        drives,
        source,
        burst_pct: o.f64_or("burst_pct", d.burst_pct)?,
        ramp_s: o.f64_or("ramp_s", d.ramp_s)?,
        ceiling_v: o.f64_or("ceiling_v", d.ceiling_v)?,
        delay_frames: u32::try_from(delay).unwrap_or(u32::MAX),
        overlap_pct: o.f64_or("overlap_pct", d.overlap_pct)?,
        seed: o.opt_count("seed")?.unwrap_or(d.seed),
    })
}

/// Parse `config`: its shape only. A key it does not have is refused `bad_params`.
pub fn parse(config: &Value) -> Result<WireModal, MethodError> {
    let c = Obj::of(config, "config")?;
    only(
        &c,
        &[
            "title",
            "operator",
            "article",
            "notes",
            "specification",
            "capture",
            "record",
            "metadata",
            "mode",
            "fs_hz",
            "block",
            "band",
            "estimator",
            "averages",
            "decay_warn_pct",
            "roving",
            "impact",
            "shaker",
        ],
    )?;
    let mode = match c.opt_str("mode")? {
        Some("impact") => Mode::Impact,
        Some("shaker") => Mode::Shaker,
        Some(x) => {
            return Err(bad(
                "config.mode",
                format!("\"{x}\" is not a mode; it is impact or shaker"),
            ))
        }
        None => return Err(bad("config.mode", "the mode is needed: impact or shaker")),
    };
    let fs_hz = c.f64("fs_hz")?;
    let block = c.opt_count("block")?.unwrap_or(2048) as usize;
    let mut s = ModalSetup::new(mode, fs_hz, block);
    if let Some(b) = c.opt_obj("band")? {
        only(&b, &["lo_hz", "hi_hz"])?;
        s.band = (b.f64_or("lo_hz", s.band.0)?, b.f64_or("hi_hz", s.band.1)?);
    }
    s.estimator = match c.opt_str("estimator")? {
        None | Some("H1") => Estimator::H1,
        Some("H2") => Estimator::H2,
        Some("Hv") => Estimator::Hv,
        Some(x) => {
            return Err(bad(
                "config.estimator",
                format!("\"{x}\" is not an estimator; it is H1, H2 or Hv"),
            ))
        }
    };
    if let Some(a) = c.opt_count("averages")? {
        s.averages = u32::try_from(a).unwrap_or(u32::MAX);
    }
    s.decay_warn_pct = c.f64_or("decay_warn_pct", s.decay_warn_pct)?;
    if let Some(r) = c.opt_obj("roving")? {
        s.roving = Some(roving(&r)?);
    }
    if let Some(i) = c.opt_obj("impact")? {
        s.impact = impact(&i)?;
    }
    match (mode, c.opt_obj("shaker")?) {
        (_, Some(o)) => s.shaker = shaker(&o)?,
        (Mode::Shaker, None) => {
            return Err(bad(
                "config.shaker",
                "shaker mode needs its drives: config.shaker.drives",
            ))
        }
        _ => {}
    }
    let mut run_name = DEFAULT_RUN_NAME.to_owned();
    if let Some(o) = c.opt_obj("capture")? {
        only(&o, &["run_name"])?;
        if let Some(n) = o.opt_str("run_name")? {
            run_name = n.to_owned();
        }
    }
    let metadata = match c.array("metadata")? {
        None => Vec::new(),
        Some(list) => {
            let mut out = Vec::with_capacity(list.len());
            for (i, m) in list.iter().enumerate() {
                let o = Obj::of(m, &format!("config.metadata[{i}]"))?;
                only(&o, &["key", "label", "value"])?;
                let key = o
                    .opt_str("key")?
                    .ok_or_else(|| bad(&o.at("key"), "\"key\" is text"))?
                    .to_owned();
                let label = o.opt_str("label")?.unwrap_or(&key).to_owned();
                let value = o
                    .opt_str("value")?
                    .ok_or_else(|| bad(&o.at("value"), "\"value\" is text"))?
                    .to_owned();
                out.push((key, label, value));
            }
            out
        }
    };
    let specification = match c.m.get("specification") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let o = Obj::of(v, "config.specification")?;
            only(&o, &["document", "clause"])?;
            Some((
                o.opt_str("document")?.unwrap_or("").to_owned(),
                o.opt_str("clause")?.unwrap_or("").to_owned(),
            ))
        }
    };
    let record_streams = match c.opt_obj("record")? {
        None => true,
        Some(o) => {
            only(&o, &["streams"])?;
            o.opt_bool("streams")?.unwrap_or(true)
        }
    };
    let words =
        |k: &str| -> Result<String, MethodError> { Ok(c.opt_str(k)?.unwrap_or("").to_owned()) };
    Ok(WireModal {
        raw: config.clone(),
        title: words("title")?,
        operator: words("operator")?,
        article: words("article")?,
        notes: words("notes")?,
        specification,
        metadata,
        run_name,
        record_streams,
        setup: s,
    })
}

/// The crate's refusals as the service's items.
pub fn items(r: &[Refusal]) -> Vec<Item> {
    r.iter()
        .map(|x| {
            if x.field == "channels" {
                Item::channel(x.reason, x.channel, x.message.clone())
            } else {
                Item::field(x.reason, format!("config.{}", x.field), x.message.clone())
            }
        })
        .collect()
}

/// The plan, or the refusals as a report.
pub fn check(w: &WireModal, table: &ChannelTable) -> (Report, Option<Plan>) {
    match Plan::build(table, &w.setup) {
        Ok(p) => (Report::default(), Some(p)),
        Err(r) => (
            Report {
                errors: items(&r),
                ..Report::default()
            },
            None,
        ),
    }
}

/// The warnings a configure gives with a plan that passes.
pub fn warnings(w: &WireModal, plan: &Plan) -> Vec<Item> {
    let s = &w.setup;
    let mut out = Vec::new();
    if s.mode == Mode::Impact {
        if s.impact.pretrigger_pct == 0.0 {
            out.push(Item::field(
                "no_pretrigger",
                "config.impact.pretrigger_pct",
                "With no pre-trigger, the no-response check is not judged.",
            ));
        }
        if s.impact.exp_window.on {
            let e = ExpWindow::new(&s.impact.exp_window, s.block, s.fs_hz);
            out.push(Item::field(
                "exponential_window_damping",
                "config.impact.exponential_window",
                format!(
                    "The exponential window (tau {:.4} s) adds 1/(2 pi f_n tau) to every measured \
                     damping ratio: {:.4} at {} Hz. The results record tau so it can be taken out.",
                    e.tau_s,
                    e.added_damping(s.band.1.max(1.0) / 2.0),
                    num(s.band.1 / 2.0)
                ),
            ));
        }
        if s.averages == 1 {
            out.push(Item::field(
                "one_average",
                "config.averages",
                "With one average the coherence is 1 at every line and says nothing.",
            ));
        }
    }
    if plan.measurements.len() == 1 && s.roving.is_none() {
        out.push(Item::field(
            "no_roving",
            "config.roving",
            "No roving plan: the test measures the table's DOFs once.",
        ));
    }
    out
}

/// The report a configure returns, shown before anything is armed.
pub fn report(w: &WireModal, plan: &Plan, source: &str) -> Value {
    let s = &w.setup;
    let lines = s.lines();
    let rows = |rows: &[hertzforge_modal_test::plan::Row]| -> Vec<Value> {
        rows.iter()
            .map(|r| {
                json!({"index": r.index, "unit": r.unit, "range": num(r.range),
                       "node": r.dof.as_ref().map(|d| d.node),
                       "direction": r.dof.as_ref().map(|d| d.direction.clone()),
                       "roves": r.roves})
            })
            .collect()
    };
    let points: Vec<Value> = plan
        .measurements
        .iter()
        .enumerate()
        .map(|(k, m)| {
            let (refs, resps) = plan.rows_json(m);
            json!({"index": k, "references": refs, "responses": resps})
        })
        .collect();
    let mut v = json!({
        "source": source,
        "mode": s.mode.name(),
        "fs_hz": num(s.fs_hz),
        "block": s.block,
        "frame_s": num(s.block as f64 / s.fs_hz),
        "df_hz": num(s.df_hz()),
        "band": {"lo_hz": num(s.band.0), "hi_hz": num(s.band.1),
                 "first_line": lines.start(), "last_line": lines.end(),
                 "lines": lines.clone().count()},
        "estimator": if plan.refs.len() > 1 { "matrix H1" } else { s.estimator.name() },
        "averages": s.averages,
        "references": rows(&plan.refs),
        "responses": rows(&plan.resps),
        "points": points,
    });
    let o = v.as_object_mut().expect("an object");
    match s.mode {
        Mode::Impact => {
            let i = &s.impact;
            let hammer = &plan.refs[0];
            let fw = i
                .force_window
                .on
                .then(|| ForceWindow::new(&i.force_window, s.block, s.pretrigger_frames()));
            let ew = i
                .exp_window
                .on
                .then(|| ExpWindow::new(&i.exp_window, s.block, s.fs_hz));
            o.insert(
                "impact".into(),
                json!({
                    "drives": "none: nothing is driven in impact mode",
                    "trigger": {"level": num(i.polarity.sign() * i.trigger_pct / 100.0 * hammer.range), "unit": hammer.unit,
                                "polarity": i.polarity.name(),
                                "watches": format!("the force rising through {} {} ({} hammer); live once {} pre-trigger frames are in",
                                                   num(i.polarity.sign() * i.trigger_pct / 100.0 * hammer.range), hammer.unit,
                                                   i.polarity.name(), s.pretrigger_frames()),
                                "pretrigger_frames": s.pretrigger_frames()},
                    "holdoff_blocks": num(i.holdoff_blocks),
                    "accept": i.accept.name(),
                    "auto_advance": i.auto_advance,
                    "checks": {
                        "overload": format!("{} of each row's range rejects", num(hertzforge_hal::INPUT_OVERLOAD_FRACTION)),
                        "double_hit": format!("a second impact of the hammer's polarity over {} percent of the peak, or six times the pre-trigger RMS if larger, after the pulse falls below 5 percent of it, rejects", num(i.double_hit_pct)),
                        "tip_warn_db": num(i.tip_warn_db),
                        "target": i.target.map(|t| json!({"peak": num(t.peak), "tolerance_pct": num(t.tolerance_pct),
                                                          "unit": hammer.unit, "rejects": false})),
                    },
                    "force_window": fw.map(|f| json!({"pre_frames": f.pre, "width_frames": f.width, "fall_frames": f.fall})),
                    "exponential_window": ew.map(|e| json!({"tau_s": num(e.tau_s), "inv_tau_per_s": num(1.0 / e.tau_s), "end": num(e.end),
                                                            "added_damping": "1/(2*pi*f_n*tau_s)"})),
                }),
            );
        }
        Mode::Shaker => {
            let sh = &s.shaker;
            o.insert(
                "shaker".into(),
                json!({
                    "source": sh.source.name(),
                    "crest": num(sh.source.crest()),
                    "drives": plan.drives.iter().map(|d| json!({
                        "row": d.index, "level_vrms": num(d.level_vrms),
                        "peak_v": num(d.level_vrms * sh.source.crest()),
                        "range_v": num(d.range_v)})).collect::<Vec<_>>(),
                    "ceiling_v": num(sh.ceiling_v),
                    "burst_pct": sh.source.is_burst().then(|| num(sh.burst_pct)),
                    "ramp_s": num(s.ramp_frames() as f64 / s.fs_hz),
                    "delay_frames": sh.delay_frames,
                    "overlap_pct": (sh.source == SourceKind::Random).then(|| num(sh.overlap_pct)),
                    "seed": sh.seed,
                    "window": if sh.source == SourceKind::Random { "hann" } else { "uniform" },
                    "stop": {"depth_db": num(STOP_RAMP_DEPTH_DB), "ramp_s": num(STOP_RAMP_S), "then": "zero"},
                    "seconds_per_point": num((s.ramp_frames() as f64 + (f64::from(s.averages)
                        * if sh.source == SourceKind::PeriodicRandom { f64::from(sh.delay_frames) + 1.0 } else { 1.0 }
                        + f64::from(sh.delay_frames)) * s.block as f64) / s.fs_hz),
                }),
            );
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_test_config_parses_both_modes_and_refuses_unknown_keys() {
        let w = parse(&json!({"mode": "impact", "fs_hz": 1024, "block": 512,
            "band": {"hi_hz": 200}, "roving": {"channels": [0], "points": {"start": 1, "count": 3}},
            "impact": {"target": {"peak": 20}}}))
        .unwrap();
        assert_eq!(w.setup.mode, Mode::Impact);
        assert_eq!(w.setup.roving.as_ref().unwrap().points.len(), 3);
        assert_eq!(w.setup.impact.target.unwrap().tolerance_pct, 20.0);
        let s = parse(
            &json!({"mode": "shaker", "fs_hz": 1024, "shaker": {"drives": [{"row": 4}],
            "source": "periodic_chirp"}}),
        )
        .unwrap();
        assert_eq!(s.setup.shaker.drives[0].level_vrms, LEVEL_DEFAULT_VRMS);
        assert!(parse(&json!({"mode": "impact", "fs_hz": 1024, "impact": {"target": {"peak": 20, "reject": true}}})).is_err());
        assert!(parse(&json!({"mode": "impact", "fs_hz": 1024, "bogus": 1})).is_err());
        assert!(parse(&json!({"mode": "shaker", "fs_hz": 1024})).is_err());
    }
}
