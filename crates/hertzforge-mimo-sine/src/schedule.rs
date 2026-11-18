//! The schedule: where the tone is and at what level, update by update.
//!
//! Sweeps, dwells, stepped sines on a grid and the level ladder are
//! single-shaker Sine's schedulers, unchanged. What is added here: a stepped
//! sine on a list, a settle window after each step (the step's dwell clock
//! waits until the trackers have settled), the operator's hold, resume and
//! manual frequency, and a law's request to hold or slow the sweep, granted
//! only when the setup allows it and within its bounds.

use crate::setup::{LevelMode, MimoSineSetup, RequestSettings, StepPlan, TestPlan};
use crate::tick::Request;
use hertzforge_sine::scheduler::{BidirSweep, DwellPlan, DwellRun, LadderPlan, StepSine};

/// The test's own scheduler.
#[derive(Debug, Clone)]
enum Plan {
    Sweep(BidirSweep),
    Dwell(DwellRun),
    Grid(StepSine),
    List {
        freqs: Vec<f64>,
        idx: usize,
        run: DwellRun,
        plan: hertzforge_sine::scheduler::DwellFor,
        done: bool,
    },
}

/// What a law's request did this update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestOutcome {
    /// No request.
    None,
    /// Granted as asked.
    Granted,
    /// Granted within the setup's bounds (a slow factor raised to the floor).
    Bounded,
    /// Recorded and not acted on: requests are not honoured in this run.
    NotHonoured,
    /// Refused: a hold has lasted the longest the setup allows.
    HoldTooLong,
    /// Not applicable: the test is not sweeping (a dwell, a step, a hold).
    NotSweeping,
}

impl RequestOutcome {
    /// The stable record name.
    pub fn as_str(self) -> &'static str {
        match self {
            RequestOutcome::None => "none",
            RequestOutcome::Granted => "granted",
            RequestOutcome::Bounded => "bounded",
            RequestOutcome::NotHonoured => "not_honoured",
            RequestOutcome::HoldTooLong => "hold_too_long",
            RequestOutcome::NotSweeping => "not_sweeping",
        }
    }
}

/// One update's schedule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduleStep {
    /// The tone's frequency for the next interval, Hz.
    pub frequency_hz: f64,
    /// The scheduled level, dB against full level.
    pub level_db: f64,
    /// The sweep rate actually run this update, Hz per second, signed (0 at
    /// a dwell, a step, or while held).
    pub sweep_hz_per_s: f64,
    /// The same rate in octaves per minute, signed.
    pub sweep_oct_per_min: f64,
    /// The frequency jumped (a new step, a manual frequency): the trackers
    /// restart their settle and the settle window opens.
    pub jumped: bool,
    /// The level changed this update.
    pub level_changed: bool,
    /// The test is complete.
    pub done: bool,
    /// The schedule is held (by the operator, or by a granted law request).
    pub held: bool,
    /// What the law's request did.
    pub request: RequestOutcome,
}

/// The schedule of one run.
#[derive(Debug, Clone)]
pub struct Scheduler {
    plan: Plan,
    ladder: LadderPlan,
    requests: RequestSettings,
    operator_hold: bool,
    law_hold_s: f64,
    settle_left_s: f64,
    last_level_db: f64,
    last_sweeps: u32,
    pending_jump: bool,
}

impl Scheduler {
    /// The schedule of `setup` (already validated).
    pub fn new(setup: &MimoSineSetup) -> Result<Scheduler, String> {
        let plan = match &setup.test {
            TestPlan::Sweep(c) => Plan::Sweep(BidirSweep::new(*c).map_err(|e| e.to_string())?),
            TestPlan::Dwell(d) => Plan::Dwell(DwellRun::new(*d).map_err(|e| e.to_string())?),
            TestPlan::Stepped(StepPlan::Grid(g)) => {
                Plan::Grid(StepSine::new(*g).map_err(|e| e.to_string())?)
            }
            TestPlan::Stepped(StepPlan::List {
                frequencies_hz,
                dwell,
            }) => Plan::List {
                freqs: frequencies_hz.clone(),
                idx: 0,
                run: DwellRun::new(DwellPlan {
                    freq_hz: frequencies_hz[0],
                    for_: *dwell,
                })
                .map_err(|e| e.to_string())?,
                plan: *dwell,
                done: false,
            },
        };
        let mut ladder =
            LadderPlan::new(setup.schedule.ladder.clone()).map_err(|e| e.to_string())?;
        ladder.set_manual(setup.schedule.mode == LevelMode::Manual);
        let last_level_db = ladder.level_db();
        Ok(Scheduler {
            plan,
            ladder,
            requests: setup.requests,
            operator_hold: false,
            law_hold_s: 0.0,
            settle_left_s: 0.0,
            last_level_db,
            last_sweeps: 0,
            pending_jump: false,
        })
    }

    /// The frequency the tone starts at, Hz.
    pub fn frequency_hz(&self) -> f64 {
        match &self.plan {
            Plan::Sweep(s) => s.freq_hz(),
            Plan::Dwell(d) => d.freq_hz(),
            Plan::Grid(g) => g.freq_hz(),
            Plan::List { freqs, idx, .. } => freqs[*idx],
        }
    }

    /// The level now, dB against full level.
    pub fn level_db(&self) -> f64 {
        self.ladder.level_db()
    }

    /// Whether the test is a sweep.
    pub fn is_sweep(&self) -> bool {
        matches!(self.plan, Plan::Sweep(_))
    }

    /// Whether the operator holds the schedule.
    pub fn operator_held(&self) -> bool {
        self.operator_hold
    }

    /// Open a settle window of `s` seconds: a step's dwell clock waits until
    /// it closes.
    pub fn settle_for(&mut self, s: f64) {
        self.settle_left_s = self.settle_left_s.max(s);
    }

    /// Whether a settle window is open.
    pub fn settling(&self) -> bool {
        self.settle_left_s > 0.0
    }

    /// The operator's hold: the frequency and the level clocks stop.
    pub fn hold(&mut self) {
        self.operator_hold = true;
    }

    /// The operator's resume.
    pub fn resume(&mut self) {
        self.operator_hold = false;
    }

    /// Manual frequency: put the tone at `hz`, clamped to the test's band.
    /// Held, the tone stays there; resumed, the sweep goes on from there. A
    /// dwell and a stepped sine refuse (`None`): their frequencies are the
    /// test.
    pub fn go_to(&mut self, hz: f64) -> Option<f64> {
        match &mut self.plan {
            Plan::Sweep(s) => {
                let f = s.go_to(hz);
                self.pending_jump = true;
                Some(f)
            }
            _ => None,
        }
    }

    /// Move the level by `delta_db` within the ladder's table (manual level).
    pub fn step_level_db(&mut self, delta_db: f64) -> f64 {
        self.ladder.step_db(delta_db)
    }

    fn sweep_rate(&self) -> (f64, f64) {
        match &self.plan {
            Plan::Sweep(s) => (
                s.rate_hz_per_s(),
                s.rate_oct_per_min().unwrap_or_else(|| {
                    let f = s.freq_hz().max(1e-9);
                    s.rate_hz_per_s() / f / std::f64::consts::LN_2 * 60.0
                }),
            ),
            _ => (0.0, 0.0),
        }
    }

    /// Advance the schedule by `dt` seconds with the law's `request`.
    pub fn step(&mut self, dt: f64, request: Option<Request>) -> ScheduleStep {
        let sweeping = self.is_sweep() && !self.operator_hold;
        // The law's request.
        let mut factor = 1.0;
        let mut law_hold = false;
        let outcome = match request {
            None => RequestOutcome::None,
            Some(_) if !self.requests.honour => RequestOutcome::NotHonoured,
            Some(_) if !sweeping => RequestOutcome::NotSweeping,
            Some(Request::Hold) => {
                if self.law_hold_s + dt > self.requests.max_hold_s + 1e-12 {
                    RequestOutcome::HoldTooLong
                } else {
                    law_hold = true;
                    RequestOutcome::Granted
                }
            }
            Some(Request::Slow(f)) => {
                if f < self.requests.min_slow_factor {
                    factor = self.requests.min_slow_factor;
                    RequestOutcome::Bounded
                } else {
                    factor = f;
                    RequestOutcome::Granted
                }
            }
        };
        if law_hold {
            self.law_hold_s += dt;
        } else {
            self.law_hold_s = 0.0;
        }
        let held = self.operator_hold || law_hold;
        let settling = self.settle_left_s > 0.0;
        self.settle_left_s = (self.settle_left_s - dt).max(0.0);
        // The test clock runs unless held; a step's clock waits for its settle.
        let run_dt = if held { 0.0 } else { dt };
        let before = self.frequency_hz();
        let mut jumped = std::mem::take(&mut self.pending_jump);
        let mut done;
        match &mut self.plan {
            Plan::Sweep(s) => {
                s.tick(run_dt * factor);
                let n = s.sweeps_done();
                while self.last_sweeps < n {
                    self.last_sweeps += 1;
                    self.ladder.note_sweep_completed();
                }
                done = s.done();
            }
            Plan::Dwell(d) => {
                if !settling {
                    d.tick(run_dt);
                }
                done = d.done();
            }
            Plan::Grid(g) => {
                if !settling {
                    g.tick(run_dt);
                }
                done = g.done();
            }
            Plan::List {
                freqs,
                idx,
                run,
                plan,
                done: d,
            } => {
                if !settling && !*d {
                    run.tick(run_dt);
                    if run.done() {
                        if *idx + 1 >= freqs.len() {
                            *d = true;
                        } else {
                            *idx += 1;
                            *run = DwellRun::new(DwellPlan {
                                freq_hz: freqs[*idx],
                                for_: *plan,
                            })
                            .expect("checked at setup");
                        }
                    }
                }
                done = *d;
            }
        }
        let after = self.frequency_hz();
        if !self.is_sweep() && after != before {
            jumped = true;
        }
        let level_db = if held || settling {
            self.ladder.level_db()
        } else {
            self.ladder.tick(run_dt)
        };
        // The ladder ends a test whose last rung is complete.
        if self.ladder.done() {
            done = true;
        }
        let level_changed = level_db != self.last_level_db;
        self.last_level_db = level_db;
        let (r_hz, r_oct) = if held || !self.is_sweep() {
            (0.0, 0.0)
        } else {
            let (a, b) = self.sweep_rate();
            (a * factor, b * factor)
        };
        ScheduleStep {
            frequency_hz: after,
            level_db,
            sweep_hz_per_s: r_hz,
            sweep_oct_per_min: r_oct,
            jumped,
            level_changed,
            done,
            held,
            request: outcome,
        }
    }
}
