//! The test's state machine, run on the run's own thread (never the loop thread): the
//! selected point, the trigger and the hits in impact mode, the frames in shaker mode,
//! accept, reject and undo, and the averages of every point.
//!
//! # States
//!
//! `idle` (a point is selected, nothing taken), `armed` (impact: waiting for a hit),
//! `captured` (impact, manual accept: a hit waits for accept or reject), `measuring`
//! (shaker: the drive is on and frames are averaged), `point_done` (the point's
//! averages are in), and `ended`.
//!
//! # Accept and reject
//!
//! In automatic accept mode an overloaded or double hit is rejected and any other
//! accepted as it lands; in manual mode it waits. Either way, the operator's accept or
//! reject applies to the latest hit and overrides until the next hit lands: rejecting
//! an accepted hit takes its average back out (reopening its point if it was the one
//! that completed it), and accepting a rejected one puts it in. Undo takes the last
//! accepted average of the selected point back out. A force off its target is a signal
//! only and never rejects.

use crate::average::{frame_spectra, Averager, Estimates, FrameSpectra};
use crate::hits::{self, HitReport};
use crate::plan::Plan;
use crate::setup::{AcceptMode, ModalSetup, Mode, SourceKind};
use crate::window::{ExpWindow, ForceWindow, Windows};
use serde_json::{json, Value};
use std::collections::VecDeque;

/// The session's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Armed,
    Captured,
    Measuring,
    PointDone,
    Ended,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Armed => "armed",
            State::Captured => "captured",
            State::Measuring => "measuring",
            State::PointDone => "point_done",
            State::Ended => "ended",
        }
    }
}

/// A hit (or a shaker frame) taken at a point.
#[derive(Debug, Clone, PartialEq)]
pub struct Taken {
    /// From 1 within its point.
    pub n: u32,
    /// From 1 within the test.
    pub seq: u64,
    /// `pending`, `accepted`, `rejected` or `undone`.
    pub status: &'static str,
    /// Why it was rejected: `overload`, `double_hit` or `operator`.
    pub reason: Option<&'static str>,
    /// The input frame its block starts at, and its length.
    pub start_frame: u64,
    pub frames: usize,
    pub report: Option<HitReport>,
    pub overload: bool,
}

impl Taken {
    /// One tile of the per-point list: its status and flags.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "n": self.n,
            "seq": self.seq,
            "status": self.status,
            "reason": self.reason,
            "overload": self.overload,
            "double_hit": self.report.as_ref().is_some_and(|r| r.double_hit.is_some()),
            "start_frame": self.start_frame,
        });
        if let (Some(r), Some(o)) = (&self.report, v.as_object_mut()) {
            o.insert("peak".into(), json!(r.peak));
            o.insert("tip_warning".into(), json!(r.tip_warning));
            o.insert("force_window_cut".into(), json!(r.window_cut.is_some()));
            if let Some(t) = &r.target {
                o.insert("target_verdict".into(), json!(t.verdict));
                o.insert("target_diff_pct".into(), json!(t.diff_pct));
            }
        }
        v
    }
}

/// A point of the plan, as the session holds it.
#[derive(Debug, Clone)]
pub struct PointState {
    pub averager: Averager,
    pub taken: Vec<Taken>,
    pub measured: bool,
    /// The measurement number of its latest finished result.
    pub last_result: Option<u32>,
}

impl PointState {
    pub fn accepted(&self) -> usize {
        self.averager.count()
    }

    pub fn rejected(&self) -> usize {
        self.taken.iter().filter(|t| t.status == "rejected").count()
    }
}

/// What a session's call gave.
#[derive(Debug, Clone)]
pub enum Event {
    /// A hit or frame was taken: its point and its place in the point's list.
    Taken {
        point: usize,
        at: usize,
    },
    /// A taken hit's status changed (by the operator, or by an undo).
    Status {
        point: usize,
        at: usize,
    },
    /// A point's averages are in: its estimates, numbered, and the result it replaces.
    Done {
        point: usize,
        measurement: u32,
        replaces: Option<u32>,
        estimates: Box<Estimates>,
    },
    /// A finished point is open again.
    Reopened {
        point: usize,
    },
    Selected {
        point: usize,
    },
    State(State),
}

/// Why a call was refused.
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub reason: &'static str,
    pub message: String,
}

fn refused(reason: &'static str, message: impl Into<String>) -> Refused {
    Refused {
        reason,
        message: message.into(),
    }
}

/// The session.
pub struct Session {
    pub setup: ModalSetup,
    pub plan: Plan,
    pub windows: Windows,
    lines: std::ops::RangeInclusive<usize>,
    pub points: Vec<PointState>,
    pub point: usize,
    pub state: State,
    measurements: u32,
    seq: u64,
    /// The latest hit: its point, its place and its spectra, until the next lands.
    latest: Option<(usize, usize, FrameSpectra)>,
    // The capture.
    frame: u64,
    hist: Vec<VecDeque<f64>>,
    prev_force: f64,
    trigger_at: Option<u64>,
    holdoff_until: u64,
    // Shaker mode.
    next_seg: Option<u64>,
    seg_step: u64,
    seg_skip: u64,
    seg_taken: u64,
    data_w: Vec<Vec<f64>>,
    analysis_w: Vec<f64>,
}

impl Session {
    pub fn new(setup: ModalSetup, plan: Plan) -> Session {
        let n = setup.block;
        let windows = Windows {
            force: (setup.mode == Mode::Impact && setup.impact.force_window.on).then(|| {
                ForceWindow::new(&setup.impact.force_window, n, setup.pretrigger_frames())
            }),
            exp: (setup.mode == Mode::Impact && setup.impact.exp_window.on)
                .then(|| ExpWindow::new(&setup.impact.exp_window, n, setup.fs_hz)),
            analysis: if setup.mode == Mode::Shaker && setup.shaker.source == SourceKind::Random {
                "hann"
            } else {
                "uniform"
            },
        };
        let lines = setup.lines();
        let df = setup.df_hz();
        let freqs: Vec<f64> = lines.clone().map(|k| k as f64 * df).collect();
        let points = plan
            .measurements
            .iter()
            .map(|_| PointState {
                averager: Averager::new(plan.refs.len(), plan.resps.len(), freqs.clone()),
                taken: Vec::new(),
                measured: false,
                last_result: None,
            })
            .collect();
        let width = plan.refs.len() + plan.resps.len();
        let rw = windows.reference(n);
        let yw = windows.response(n);
        let data_w = (0..width)
            .map(|c| {
                if c < plan.refs.len() {
                    rw.clone()
                } else {
                    yw.clone()
                }
            })
            .collect();
        let analysis_w = windows.analysis_values(n);
        Session {
            lines,
            points,
            point: 0,
            state: State::Idle,
            measurements: 0,
            seq: 0,
            latest: None,
            frame: 0,
            hist: vec![VecDeque::new(); width],
            prev_force: 0.0,
            trigger_at: None,
            holdoff_until: 0,
            next_seg: None,
            seg_step: n as u64,
            seg_skip: 0,
            seg_taken: 0,
            data_w,
            analysis_w,
            windows,
            setup,
            plan,
        }
    }

    pub fn freqs(&self) -> &[f64] {
        &self.points[0].averager.freqs
    }

    pub fn lines(&self) -> std::ops::RangeInclusive<usize> {
        self.lines.clone()
    }

    /// The input frames seen.
    pub fn frames_seen(&self) -> u64 {
        self.frame
    }

    fn pretrigger(&self) -> usize {
        self.setup.pretrigger_frames()
    }

    fn set_state(&mut self, s: State, ev: &mut Vec<Event>) {
        if self.state != s {
            self.state = s;
            ev.push(Event::State(s));
        }
    }

    fn live(&self) -> Result<(), Refused> {
        if self.state == State::Ended {
            return Err(refused("not_live", "The test has ended."));
        }
        Ok(())
    }

    /// Select a point (from 0). Refused while a hit waits or the drive measures.
    pub fn select(&mut self, k: usize) -> Result<Vec<Event>, Refused> {
        self.live()?;
        if k >= self.points.len() {
            return Err(refused(
                "no_such_point",
                format!(
                    "There is no point {k}; the plan has {} (from 0).",
                    self.points.len()
                ),
            ));
        }
        if matches!(self.state, State::Captured | State::Measuring) {
            return Err(refused(
                "busy",
                format!(
                    "Point {} is {}: finish it first.",
                    self.point,
                    self.state.name()
                ),
            ));
        }
        let mut ev = Vec::new();
        self.point = k;
        self.trigger_at = None;
        ev.push(Event::Selected { point: k });
        let s = if self.points[k].measured {
            State::PointDone
        } else {
            State::Idle
        };
        self.state = State::Ended; // force the state event
        self.set_state(s, &mut ev);
        Ok(ev)
    }

    /// Arm the trigger at the selected point (impact).
    pub fn arm(&mut self) -> Result<Vec<Event>, Refused> {
        self.live()?;
        if self.setup.mode != Mode::Impact {
            return Err(refused(
                "not_in_mode",
                "Arm is for impact mode; a shaker test starts its drive.",
            ));
        }
        if self.state == State::Captured {
            return Err(refused("busy", "A hit waits for accept or reject."));
        }
        if self.points[self.point].measured {
            return Err(refused(
                "point_done",
                format!(
                    "Point {} is measured: undo to take it again, or select another.",
                    self.point
                ),
            ));
        }
        let mut ev = Vec::new();
        self.trigger_at = None;
        self.holdoff_until = self.frame;
        self.set_state(State::Armed, &mut ev);
        Ok(ev)
    }

    /// Disarm (impact) without taking anything.
    pub fn disarm(&mut self) -> Vec<Event> {
        let mut ev = Vec::new();
        if self.state == State::Armed {
            self.trigger_at = None;
            self.set_state(State::Idle, &mut ev);
        }
        ev
    }

    /// Begin averaging frames (shaker): `ramp_start` is the input frame the drive's ramp
    /// began at (a source frame's start), `source_frame` that frame's number in the
    /// source, and `ramp_frames` the ramp's length.
    pub fn begin_measuring(
        &mut self,
        ramp_start: u64,
        source_frame: u64,
        ramp_frames: u64,
    ) -> Result<Vec<Event>, Refused> {
        self.live()?;
        if self.setup.mode != Mode::Shaker {
            return Err(refused(
                "not_in_mode",
                "Frames are averaged in shaker mode.",
            ));
        }
        if self.points[self.point].measured {
            return Err(refused(
                "point_done",
                format!(
                    "Point {} is measured: undo to take it again, or select another.",
                    self.point
                ),
            ));
        }
        let n = self.setup.block as u64;
        let s = &self.setup.shaker;
        let d = u64::from(s.delay_frames);
        let first = ramp_start + ramp_frames;
        let (start, step) = match s.source {
            SourceKind::Random => (
                first + d * n,
                ((n as f64) * (1.0 - s.overlap_pct / 100.0))
                    .round()
                    .max(1.0) as u64,
            ),
            SourceKind::BurstRandom | SourceKind::BurstChirp => (first, n),
            SourceKind::PseudoRandom | SourceKind::PeriodicChirp => (first + d * n, n),
            SourceKind::PeriodicRandom => {
                // The measured frame is the last of each group of `d + 1`.
                let j = source_frame + ramp_frames / n;
                let g = d + 1;
                let k = (0..g).find(|k| (j + k) % g == d).unwrap_or(0);
                (first + k * n, g * n)
            }
        };
        self.next_seg = Some(start);
        self.seg_step = step;
        self.seg_skip = 0;
        self.seg_taken = 0;
        let mut ev = Vec::new();
        self.set_state(State::Measuring, &mut ev);
        Ok(ev)
    }

    /// The drive stopped before the averages were in (shaker).
    pub fn stop_measuring(&mut self) -> Vec<Event> {
        let mut ev = Vec::new();
        self.next_seg = None;
        if self.state == State::Measuring {
            let s = if self.points[self.point].measured {
                State::PointDone
            } else {
                State::Idle
            };
            self.set_state(s, &mut ev);
        }
        ev
    }

    /// One captured input frame (the station's inputs in table order; anything after
    /// them is ignored).
    pub fn feed(&mut self, frame: &[f32]) -> Vec<Event> {
        let mut ev = Vec::new();
        if self.state == State::Ended {
            return ev;
        }
        let n = self.setup.block;
        let keep = 2 * n + 1;
        let slots: Vec<usize> = self
            .plan
            .refs
            .iter()
            .chain(&self.plan.resps)
            .map(|r| r.slot)
            .collect();
        for (c, s) in slots.iter().enumerate() {
            let v = frame.get(*s).copied().unwrap_or(0.0) as f64;
            let h = &mut self.hist[c];
            h.push_back(v);
            if h.len() > keep {
                h.pop_front();
            }
        }
        let i = self.frame;
        self.frame += 1;
        match self.setup.mode {
            Mode::Impact => {
                if self.state == State::Armed
                    && self.trigger_at.is_none()
                    && i >= self.holdoff_until
                {
                    let level = self.setup.impact.trigger_pct / 100.0 * self.plan.refs[0].range;
                    let f = *self.hist[0].back().unwrap_or(&0.0);
                    if self.prev_force < level && f >= level {
                        self.trigger_at = Some(i);
                    }
                    self.prev_force = f;
                } else if let Some(&f) = self.hist[0].back() {
                    self.prev_force = f;
                }
                if let Some(t) = self.trigger_at {
                    let pre = self.pretrigger() as u64;
                    let start = t.saturating_sub(pre);
                    if self.frame >= start + n as u64 {
                        self.trigger_at = None;
                        let b = self.block_from(start);
                        self.holdoff_until = start
                            + n as u64
                            + (self.setup.impact.holdoff_blocks * n as f64).round() as u64;
                        ev.extend(self.take_hit(start, (t - start) as usize, b));
                    }
                }
            }
            Mode::Shaker => {
                if self.state == State::Measuring {
                    if let Some(at) = self.next_seg {
                        if self.frame >= at + n as u64 {
                            self.next_seg = Some(at + self.seg_step);
                            let b = self.block_from(at);
                            ev.extend(self.take_frame(at, b));
                        }
                    }
                }
            }
        }
        ev
    }

    /// The block of every measured channel from input frame `start`.
    fn block_from(&self, start: u64) -> Vec<Vec<f64>> {
        let n = self.setup.block;
        self.hist
            .iter()
            .map(|h| {
                let first = self.frame - h.len() as u64;
                let off = start.saturating_sub(first) as usize;
                h.iter().skip(off).take(n).copied().collect()
            })
            .collect()
    }

    fn spectra(&self, b: &[Vec<f64>]) -> FrameSpectra {
        let ch: Vec<&[f64]> = b.iter().map(|v| v.as_slice()).collect();
        let w: Vec<&[f64]> = self.data_w.iter().map(|v| v.as_slice()).collect();
        frame_spectra(
            &ch,
            &w,
            &self.analysis_w,
            self.setup.fs_hz,
            self.lines.clone(),
        )
    }

    fn take_hit(&mut self, start: u64, pre: usize, b: Vec<Vec<f64>>) -> Vec<Event> {
        let mut ev = Vec::new();
        let r = self.plan.refs.len();
        let resp: Vec<(&[f64], f64)> = self
            .plan
            .resps
            .iter()
            .enumerate()
            .map(|(k, row)| (b[r + k].as_slice(), row.range))
            .collect();
        let report = hits::check(&hits::HitInput {
            force: &b[0],
            force_range: self.plan.refs[0].range,
            responses: &resp,
            pretrigger: pre,
            fs_hz: self.setup.fs_hz,
            band_top_line: *self.lines.end(),
            double_hit_pct: self.setup.impact.double_hit_pct,
            tip_warn_db: self.setup.impact.tip_warn_db,
            target: self.setup.impact.target,
            force_window_end: self.windows.force.as_ref().map(ForceWindow::flat_end),
        });
        let spectra = self.spectra(&b);
        let p = self.point;
        self.seq += 1;
        let overload = !report.overloaded.is_empty();
        let auto = self.setup.impact.accept == AcceptMode::Auto;
        let reason = report.rejected();
        let t = Taken {
            n: self.points[p].taken.len() as u32 + 1,
            seq: self.seq,
            status: if !auto {
                "pending"
            } else if reason.is_some() {
                "rejected"
            } else {
                "accepted"
            },
            reason: if auto { reason } else { None },
            start_frame: start,
            frames: self.setup.block,
            report: Some(report),
            overload,
        };
        let accepted = t.status == "accepted";
        self.points[p].taken.push(t);
        let at = self.points[p].taken.len() - 1;
        ev.push(Event::Taken { point: p, at });
        if accepted {
            self.points[p].averager.push(spectra.clone());
        }
        self.latest = Some((p, at, spectra));
        if !auto {
            self.set_state(State::Captured, &mut ev);
        } else if accepted {
            ev.extend(self.maybe_done(p));
        }
        ev
    }

    fn take_frame(&mut self, start: u64, b: Vec<Vec<f64>>) -> Vec<Event> {
        let mut ev = Vec::new();
        let frac = hertzforge_hal::INPUT_OVERLOAD_FRACTION;
        let rows: Vec<f64> = self
            .plan
            .refs
            .iter()
            .chain(&self.plan.resps)
            .map(|r| r.range)
            .collect();
        let overload = b
            .iter()
            .zip(&rows)
            .any(|(x, r)| *r > 0.0 && x.iter().any(|v| v.abs() >= frac * r));
        let p = self.point;
        self.seq += 1;
        self.seg_taken += 1;
        let t = Taken {
            n: self.points[p].taken.len() as u32 + 1,
            seq: self.seq,
            status: if overload { "rejected" } else { "accepted" },
            reason: overload.then_some("overload"),
            start_frame: start,
            frames: self.setup.block,
            report: None,
            overload,
        };
        self.points[p].taken.push(t);
        let at = self.points[p].taken.len() - 1;
        ev.push(Event::Taken { point: p, at });
        if !overload {
            let s = self.spectra(&b);
            self.points[p].averager.push(s.clone());
            self.latest = Some((p, at, s));
            ev.extend(self.maybe_done(p));
        }
        let _ = self.seg_skip;
        ev
    }

    fn maybe_done(&mut self, p: usize) -> Vec<Event> {
        let mut ev = Vec::new();
        if self.points[p].measured || self.points[p].averager.count() < self.setup.averages as usize
        {
            if self.state == State::Captured {
                self.set_state(State::Armed, &mut ev);
            }
            return ev;
        }
        let Some(est) = self.points[p].averager.estimate(self.setup.estimator) else {
            return ev;
        };
        self.measurements += 1;
        let m = self.measurements;
        let replaces = self.points[p].last_result.replace(m);
        self.points[p].measured = true;
        self.next_seg = None;
        ev.push(Event::Done {
            point: p,
            measurement: m,
            replaces,
            estimates: Box::new(est),
        });
        self.set_state(State::PointDone, &mut ev);
        if self.setup.mode == Mode::Impact && self.setup.impact.auto_advance {
            if let Some(next) = (p + 1..self.points.len())
                .chain(0..p)
                .find(|k| !self.points[*k].measured)
            {
                self.point = next;
                self.trigger_at = None;
                ev.push(Event::Selected { point: next });
                self.set_state(State::Armed, &mut ev);
            }
        }
        ev
    }

    /// Accept the latest hit.
    pub fn accept(&mut self) -> Result<Vec<Event>, Refused> {
        self.live()?;
        let Some((p, at, s)) = self.latest.clone() else {
            return Err(refused("no_hit", "There is no hit to accept."));
        };
        let st = self.points[p].taken[at].status;
        if st == "accepted" {
            return Err(refused("already", "The latest hit is accepted already."));
        }
        if st == "undone" {
            return Err(refused(
                "no_hit",
                "The latest hit was undone; take another.",
            ));
        }
        if self.points[p].measured {
            return Err(refused(
                "point_done",
                format!("Point {p} is measured already."),
            ));
        }
        let mut ev = Vec::new();
        let t = &mut self.points[p].taken[at];
        t.status = "accepted";
        t.reason = None;
        self.points[p].averager.push(s);
        ev.push(Event::Status { point: p, at });
        if self.state == State::Captured {
            self.state = State::Armed;
            ev.push(Event::State(State::Armed));
        }
        ev.extend(self.maybe_done(p));
        Ok(ev)
    }

    /// Reject the latest hit.
    pub fn reject(&mut self) -> Result<Vec<Event>, Refused> {
        self.live()?;
        let Some((p, at, _)) = self.latest.clone() else {
            return Err(refused("no_hit", "There is no hit to reject."));
        };
        let st = self.points[p].taken[at].status;
        if st == "rejected" {
            return Err(refused("already", "The latest hit is rejected already."));
        }
        if st == "undone" {
            return Err(refused(
                "no_hit",
                "The latest hit was undone; take another.",
            ));
        }
        if matches!(self.state, State::Measuring) {
            return Err(refused(
                "busy",
                "Frames are being averaged: stop the drive first.",
            ));
        }
        let mut ev = Vec::new();
        let was_accepted = st == "accepted";
        let t = &mut self.points[p].taken[at];
        t.status = "rejected";
        t.reason = Some("operator");
        ev.push(Event::Status { point: p, at });
        if was_accepted {
            self.points[p].averager.undo();
            ev.extend(self.reopen(p));
        } else if self.state == State::Captured {
            self.set_state(State::Armed, &mut ev);
        }
        Ok(ev)
    }

    /// A point no longer complete: open again, selected and (impact) armed.
    fn reopen(&mut self, p: usize) -> Vec<Event> {
        let mut ev = Vec::new();
        if self.points[p].measured {
            self.points[p].measured = false;
            ev.push(Event::Reopened { point: p });
        }
        if self.point != p {
            self.point = p;
            self.trigger_at = None;
            ev.push(Event::Selected { point: p });
        }
        if self.state != State::Measuring {
            let s = if self.setup.mode == Mode::Impact && self.state != State::Idle {
                State::Armed
            } else {
                State::Idle
            };
            self.set_state(s, &mut ev);
        }
        ev
    }

    /// Undo the selected point's last accepted average. A shaker point's averages are
    /// all taken out: its frames belong together.
    pub fn undo(&mut self) -> Result<Vec<Event>, Refused> {
        self.live()?;
        if matches!(self.state, State::Measuring | State::Captured) {
            return Err(refused(
                "busy",
                format!(
                    "Point {} is {}: finish that first.",
                    self.point,
                    self.state.name()
                ),
            ));
        }
        let p = self.point;
        if self.points[p].averager.count() == 0 {
            return Err(refused(
                "nothing_to_undo",
                format!("Point {p} has no accepted average."),
            ));
        }
        let mut ev = Vec::new();
        let all = self.setup.mode == Mode::Shaker;
        loop {
            self.points[p].averager.undo();
            if let Some(at) = self.points[p]
                .taken
                .iter()
                .rposition(|t| t.status == "accepted")
            {
                self.points[p].taken[at].status = "undone";
                ev.push(Event::Status { point: p, at });
                if self.latest.as_ref().is_some_and(|l| l.0 == p && l.1 == at) {
                    self.latest = None;
                }
            }
            if !all || self.points[p].averager.count() == 0 {
                break;
            }
        }
        let was_done = self.points[p].measured;
        ev.extend(self.reopen(p));
        if was_done && self.state == State::PointDone {
            self.set_state(State::Idle, &mut ev);
        }
        Ok(ev)
    }

    pub fn end(&mut self) -> Vec<Event> {
        let mut ev = Vec::new();
        self.next_seg = None;
        self.trigger_at = None;
        self.set_state(State::Ended, &mut ev);
        ev
    }

    /// The counts the shell shows, kept current after every take, accept, reject and
    /// undo. `unit` is `hits` in impact mode and `frames` in shaker mode.
    pub fn counts_json(&self) -> Value {
        let p = &self.points[self.point];
        let points: Vec<Value> = self
            .points
            .iter()
            .enumerate()
            .map(|(k, s)| json!({"index": k, "accepted": s.accepted(), "rejected": s.rejected(), "taken": s.taken.len(), "measured": s.measured}))
            .collect();
        json!({
            "unit": if self.setup.mode == Mode::Impact { "hits" } else { "frames" },
            "point": {"index": self.point, "accepted": p.accepted(), "rejected": p.rejected(), "asked": self.setup.averages},
            "points": points,
            "totals": self.totals_json(),
        })
    }

    /// The whole test's totals.
    pub fn totals_json(&self) -> Value {
        json!({
            "accepted": self.points.iter().map(|p| p.accepted()).sum::<usize>(),
            "taken": self.points.iter().map(|p| p.taken.len()).sum::<usize>(),
            "points_measured": self.points.iter().filter(|p| p.measured).count(),
            "points_planned": self.points.len(),
        })
    }

    /// The selected point's hits (or frames), one tile each.
    pub fn taken_json(&self, p: usize) -> Vec<Value> {
        self.points
            .get(p)
            .map_or_else(Vec::new, |s| s.taken.iter().map(Taken::to_json).collect())
    }

    /// The latest hit's report.
    pub fn latest(&self) -> Option<(usize, &Taken)> {
        self.latest
            .as_ref()
            .map(|(p, at, _)| (*p, &self.points[*p].taken[*at]))
    }

    /// The current estimate of point `p`.
    pub fn estimate(&self, p: usize) -> Option<Estimates> {
        self.points.get(p)?.averager.estimate(self.setup.estimator)
    }

    /// Set the force target from the latest hit's peak ("use this hit as the target").
    pub fn target_from_latest(&mut self) -> Result<f64, Refused> {
        let Some(peak) = self
            .latest()
            .and_then(|(_, t)| t.report.as_ref().map(|r| r.peak))
        else {
            return Err(refused(
                "no_hit",
                "There is no hit to take the target from.",
            ));
        };
        let tol = self
            .setup
            .impact
            .target
            .map_or(crate::setup::TARGET_TOLERANCE_PCT, |t| t.tolerance_pct);
        self.setup.impact.target = Some(crate::setup::ForceTarget {
            peak,
            tolerance_pct: tol,
        });
        Ok(peak)
    }

    /// The last captured block's samples, for the display: per measured channel.
    pub fn recent(&self, frames: usize) -> Vec<Vec<f64>> {
        self.hist
            .iter()
            .map(|h| {
                h.iter()
                    .skip(h.len().saturating_sub(frames))
                    .copied()
                    .collect()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::{Point, Roving};
    use crate::structure::{self, Dof, HitKind};

    fn session(accept: AcceptMode, averages: u32) -> Session {
        let mut s = ModalSetup::new(Mode::Impact, 1024.0, 1024);
        s.band = (0.0, 250.0);
        s.averages = averages;
        s.impact.accept = accept;
        s.roving = Some(Roving {
            channels: vec![0],
            points: (1..=3)
                .map(|n| Point {
                    node: n,
                    direction: None,
                })
                .collect(),
        });
        let plan = Plan::build(&crate::plan::tests::table(false), &s).unwrap();
        Session::new(s, plan)
    }

    /// Feed a hit of `kind` at the selected point, then quiet until it is taken.
    fn hit(s: &mut Session, kind: HitKind) -> Vec<Event> {
        let fs = s.setup.fs_hz;
        let n = 3 * s.setup.block;
        let m = &s.plan.measurements[s.point];
        let f = structure::hammer(kind, fs, n, 100, structure::PULSE_PEAK_LBF, 100.0);
        let h = structure::impulse_response(&m.resps[0], &m.refs[0], fs, 1024);
        let y = structure::convolve(&f, &h, n);
        let mut ev = Vec::new();
        for i in 0..n {
            ev.extend(s.feed(&[f[i] as f32, y[i] as f32, 0.0]));
        }
        ev
    }

    #[test]
    fn modal_test_roving_hammer_session_reads_the_structure() {
        let mut s = session(AcceptMode::Auto, 2);
        s.arm().unwrap();
        let mut done = Vec::new();
        for _ in 0..6 {
            for e in hit(&mut s, HitKind::Clean) {
                if let Event::Done {
                    point, estimates, ..
                } = e
                {
                    done.push((point, estimates));
                }
            }
        }
        assert_eq!(done.len(), 3);
        assert_eq!(s.totals_json()["points_measured"], 3);
        assert_eq!(s.totals_json()["accepted"], 6);
        for (p, est) in done {
            let m = &s.plan.measurements[p];
            for (fm, _) in structure::MODES {
                let k = fm.round() as usize;
                let want = structure::accelerance(&m.resps[0], &m.refs[0], k as f64);
                let got = est.frf[0][0][k];
                assert!(
                    (got - want).norm() < 0.03 * want.norm(),
                    "point {p} line {k}: {got} vs {want}"
                );
            }
        }
    }

    #[test]
    fn modal_test_auto_rejects_double_and_overload_and_operator_overrides() {
        let mut s = session(AcceptMode::Auto, 3);
        s.arm().unwrap();
        hit(&mut s, HitKind::Double);
        assert_eq!(s.points[0].taken[0].status, "rejected");
        assert_eq!(s.points[0].taken[0].reason, Some("double_hit"));
        hit(&mut s, HitKind::Overload);
        assert_eq!(s.points[0].taken[1].reason, Some("overload"));
        assert_eq!(s.points[0].accepted(), 0);
        // The operator accepts the latest (overloaded) hit anyway.
        s.accept().unwrap();
        assert_eq!(s.points[0].accepted(), 1);
        // A clean hit is accepted, then the operator rejects it.
        hit(&mut s, HitKind::Clean);
        assert_eq!(s.points[0].accepted(), 2);
        s.reject().unwrap();
        assert_eq!(s.points[0].accepted(), 1);
        assert_eq!(s.points[0].taken[2].reason, Some("operator"));
        // Only the latest: the earlier one is not reachable now.
        assert_eq!(s.reject().unwrap_err().reason, "already");
        let c = s.counts_json();
        assert_eq!(c["point"]["accepted"], 1);
        assert_eq!(c["point"]["rejected"], 2);
        assert_eq!(c["point"]["asked"], 3);
        assert_eq!(c["totals"]["taken"], 3);
        assert_eq!(s.taken_json(0).len(), 3);
        assert_eq!(s.taken_json(0)[0]["double_hit"], true);
    }

    #[test]
    fn modal_test_rejecting_the_hit_that_completed_a_point_reopens_it() {
        let mut s = session(AcceptMode::Auto, 1);
        s.arm().unwrap();
        let ev = hit(&mut s, HitKind::Clean);
        assert!(ev.iter().any(|e| matches!(
            e,
            Event::Done {
                point: 0,
                measurement: 1,
                replaces: None,
                ..
            }
        )));
        assert_eq!(s.point, 1, "auto advance");
        let ev = s.reject().unwrap();
        assert!(ev.iter().any(|e| matches!(e, Event::Reopened { point: 0 })));
        assert_eq!(s.point, 0);
        assert!(!s.points[0].measured);
        assert_eq!(s.state, State::Armed);
        let ev = hit(&mut s, HitKind::Clean);
        assert!(ev.iter().any(|e| matches!(
            e,
            Event::Done {
                point: 0,
                measurement: 2,
                replaces: Some(1),
                ..
            }
        )));
    }

    #[test]
    fn modal_test_manual_accept_waits_and_undo_takes_back() {
        let mut s = session(AcceptMode::Manual, 2);
        s.arm().unwrap();
        hit(&mut s, HitKind::Clean);
        assert_eq!(s.state, State::Captured);
        assert_eq!(s.points[0].taken[0].status, "pending");
        assert_eq!(s.points[0].accepted(), 0);
        // No new hit is taken while one waits.
        hit(&mut s, HitKind::Clean);
        assert_eq!(s.points[0].taken.len(), 1);
        s.accept().unwrap();
        assert_eq!(s.state, State::Armed);
        hit(&mut s, HitKind::Clean);
        s.accept().unwrap();
        assert!(s.points[0].measured);
        // Undo reopens the point and takes one average out.
        s.select(0).unwrap();
        s.undo().unwrap();
        assert!(!s.points[0].measured);
        assert_eq!(s.points[0].accepted(), 1);
        assert_eq!(s.points[0].taken[1].status, "undone");
        assert_eq!(s.counts_json()["points"][0]["measured"], false);
    }

    #[test]
    fn modal_test_target_from_latest_hit() {
        let mut s = session(AcceptMode::Auto, 5);
        s.arm().unwrap();
        hit(&mut s, HitKind::Clean);
        let peak = s.target_from_latest().unwrap();
        assert!((peak - structure::PULSE_PEAK_LBF).abs() < 2.0);
        assert_eq!(s.setup.impact.target.unwrap().tolerance_pct, 20.0);
        hit(&mut s, HitKind::Clean);
        let t = s.points[0].taken[1]
            .report
            .as_ref()
            .unwrap()
            .target
            .clone()
            .unwrap();
        assert_eq!(t.verdict, "on_target");
        let _ = Dof::new(1, "Z+");
    }
}
