//! The run's real-time core: one phase-continuous tone on every drive, a
//! complex tracker on every control and limit channel, and the update every
//! control tick, in the design's order: the trackers (fed sample by sample),
//! judge, schedule, law, guards, drives, record.
//!
//! # The tone and its convention
//!
//! One oscillator runs for the whole run. Drive `i` plays `Re{D_i exp(j phi)}`
//! with its own complex amplitude `D_i`, moved from one update's value to the
//! next by a per-sample ramp in amplitude and in phase (the shorter way round),
//! so no drive ever steps. Every input is demodulated against the oscillator's
//! own phase retarded by the drive-to-response lag the identification measured,
//! so a channel's tracked amplitude `C` reads `Re{C exp(j phi)}` for the tone
//! that produced it and a linear structure in steady state gives `C = H D`.
//!
//! # Stops
//!
//! A trip (an abort, a law fault, divergence) and an operator stop both latch
//! and walk the drive down at single-shaker Sine's ramp, 120 dB a second to
//! -120 dB and then zero: never a cliff. The law is not called again once a
//! stop has begun. The cliff (the station's emergency stop) is the service's,
//! for a run thread that failed.

use crate::guards::{self, BackstopNotch, DivergenceWatch};
use crate::law::{LawIdentification, LawValues, MimoSineLaw};
use crate::limits::{MAX_CONTROLS, MAX_DRIVES};
use crate::records::{Event, EventKind, Outcome, RecordSink, TickRecord};
use crate::schedule::{RequestOutcome, Scheduler};
use crate::setup::{MimoSineSetup, PhasePoint};
use crate::tick::{check_outputs, Hold, LawFault, OutputRule, Request, TickInputs, TickOutputs};
use hertzforge_dsp::sweep::SweepOscillator;
use hertzforge_dsp::tone::{ToneTracker, SETTLE_WIDTHS};
use hertzforge_mimo_sine_law_math::linalg::reg_pinv;
use hertzforge_mimo_sine_law_math::readings::{largest_share, opposition_ratio};
use hertzforge_mimo_sine_law_math::setup::{interp_stack, GroupDelay};
use hertzforge_sdm::C64;
use hertzforge_sine::law::LawClock;
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

const ZERO: C64 = C64::new(0.0, 0.0);

/// The stop ramp, dB a second: single-shaker Sine's.
pub const RAMP_DB_PER_S: f64 = 120.0;

/// The drive scale a stop ramp snaps to zero at: -120 dB.
pub const RAMP_FLOOR_SCALE: f64 = 1e-6;

/// The settle window after engage and after a step, in tracker settle times.
pub const SETTLE_TRACKER_TIMES: f64 = 2.0;

/// The settle window after engage and after a step, in the structure's group
/// delays near the tone (its own response time).
pub const SETTLE_STRUCTURE_TIMES: f64 = 3.0;

/// The regularization of the inverse the first drive's bound reads.
const FIRST_DRIVE_EPS: f64 = 1e-3;

/// What the operator (or the service) asks of the running core, through its
/// mailbox.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// The controlled stop.
    Stop,
    /// Hold the schedule (frequency and level clocks).
    Hold,
    /// Resume the schedule.
    Resume,
    /// Put the tone at this frequency (a sweep only).
    GoTo(f64),
    /// Move the level by this many dB within the ladder.
    StepLevel(f64),
}

/// A shaker's ratings, judged on the control that reads its motion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShakerRating {
    /// The control (its position in the setup's controls) that reads this
    /// shaker's motion.
    pub control: usize,
    /// Displacement, mm peak; `None` unrated.
    pub max_displacement_mm_pk: Option<f64>,
    /// Velocity, m/s peak; `None` unrated.
    pub max_velocity_m_s_pk: Option<f64>,
    /// Acceleration, g peak; `None` unrated.
    pub max_acceleration_g_pk: Option<f64>,
}

/// Where each role sits in the station's frames.
#[derive(Debug, Clone, PartialEq)]
pub struct Slots {
    /// Values in one input frame.
    pub in_channels: usize,
    /// Values in one drive frame.
    pub out_channels: usize,
    /// Each control's position in an input frame.
    pub controls: Vec<usize>,
    /// Each limit channel's position in an input frame.
    pub limits: Vec<usize>,
    /// Each drive's position in a drive frame.
    pub drives: Vec<usize>,
}

/// The run's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CoreState {
    /// Running the schedule.
    Running,
    /// Walking the drive down toward zero, for this reason.
    Stopping(Outcome),
    /// The drive is at zero and the run is over.
    Ended(Outcome),
}

/// One drive's ramp from one update's amplitude to the next.
#[derive(Debug, Clone, Copy)]
struct DriveRamp {
    mag: f64,
    mag_step: f64,
    ph: f64,
    ph_step: f64,
    target: C64,
}

impl DriveRamp {
    fn zero() -> Self {
        DriveRamp {
            mag: 0.0,
            mag_step: 0.0,
            ph: 0.0,
            ph_step: 0.0,
            target: ZERO,
        }
    }

    /// Go from where it is to `to` over `frames` samples.
    fn go(&mut self, to: C64, frames: usize) {
        let n = frames.max(1) as f64;
        let (m1, p1) = (to.norm(), to.arg());
        // A drive that starts or ends at zero keeps the other end's phase.
        if self.mag <= 0.0 {
            self.ph = p1;
        }
        let mut dp = if m1 > 0.0 { p1 - self.ph } else { 0.0 };
        dp = (dp + PI).rem_euclid(TAU) - PI;
        self.mag_step = (m1 - self.mag) / n;
        self.ph_step = dp / n;
        self.target = to;
    }

    #[inline]
    fn next(&mut self) -> (f64, f64) {
        self.mag += self.mag_step;
        self.ph += self.ph_step;
        (self.mag, self.ph)
    }

    /// Land exactly on the target at the end of the update.
    fn land(&mut self) {
        self.mag = self.target.norm();
        if self.mag > 0.0 {
            self.ph = self.target.arg();
        }
        self.mag_step = 0.0;
        self.ph_step = 0.0;
    }
}

/// The reference phase against frequency, linear in the logarithm of
/// frequency, held beyond the ends.
fn phase_at(points: &[PhasePoint], f: f64) -> f64 {
    match points.len() {
        0 => 0.0,
        _ if f <= points[0].freq_hz => points[0].phase_deg.to_radians(),
        n if f >= points[n - 1].freq_hz => points[n - 1].phase_deg.to_radians(),
        _ => {
            let k = points.partition_point(|p| p.freq_hz < f);
            let (a, b) = (points[k - 1], points[k]);
            let t = (f.ln() - a.freq_hz.ln()) / (b.freq_hz.ln() - a.freq_hz.ln());
            (a.phase_deg + t * (b.phase_deg - a.phase_deg)).to_radians()
        }
    }
}

/// Every control's full-level reference at `f_hz` (amplitude from its
/// profile, phase from its phase points when the phases are specified, else
/// in phase), EU pk.
pub fn reference_at(setup: &MimoSineSetup, f_hz: f64, out: &mut [C64]) {
    for (o, c) in out.iter_mut().zip(&setup.controls) {
        let a = c.profile.target_accel_g_pk(f_hz).unwrap_or(0.0);
        let ph = if setup.phase_specified {
            phase_at(&c.phase, f_hz)
        } else {
            0.0
        };
        *o = C64::from_polar(a, ph);
    }
}

/// Everything the core is built with besides the setup.
pub struct CoreParts {
    pub slots: Slots,
    /// The identification the run was armed on, and its lag, samples.
    pub identification: LawIdentification,
    pub lag_frames: usize,
    /// The law, built off the real-time path.
    pub law: Box<dyn MimoSineLaw + Send>,
    /// Its declared values' count.
    pub law_values: usize,
    /// The clock the law's calls are timed against.
    pub clock: Arc<dyn LawClock>,
    /// The deadline of every law call in place of the setup's, s; `None`
    /// keeps the setup's (`guards.law_budget` of the update).
    pub law_budget_s: Option<f64>,
    /// Shaker ratings for the kinematic check (empty: none).
    pub ratings: Vec<ShakerRating>,
    pub records: RecordSink,
    pub commands: Option<Receiver<Command>>,
}

/// The real-time core of one run.
pub struct MimoSineCore {
    setup: MimoSineSetup,
    n: usize,
    m: usize,
    p: usize,
    tick_frames: usize,
    tick_s: f64,
    slots: Slots,
    // The tone.
    osc: SweepOscillator,
    phase_ring: Vec<f64>,
    lag_s: f64,
    ring_head: usize,
    ctl_trackers: Vec<ToneTracker>,
    lim_trackers: Vec<ToneTracker>,
    ramps: Vec<DriveRamp>,
    frame_in_tick: usize,
    stop_scale: f64,
    ramp_per_frame: f64,
    // The update.
    sched: Scheduler,
    law: Box<dyn MimoSineLaw + Send>,
    law_out: TickOutputs,
    law_values: LawValues,
    clock: Arc<dyn LawClock>,
    budget_s: f64,
    setup_budget_s: f64,
    misses: u32,
    ident: LawIdentification,
    group_delay: GroupDelay,
    notch: BackstopNotch,
    divergence: DivergenceWatch,
    ratings: Vec<ShakerRating>,
    records: RecordSink,
    commands: Option<Receiver<Command>>,
    state: CoreState,
    index: u64,
    engaged: bool,
    engage_settle: bool,
    /// The level the reference, the judgement and the drive are at now, dB:
    /// it follows the scheduled level at the change caps' pace.
    level_db: f64,
    /// The scheduled level, dB.
    level_target_db: f64,
    request: Option<Request>,
    outside_abort: [u32; MAX_CONTROLS],
    in_alarm: [bool; MAX_CONTROLS],
    notch_binding: bool,
    over_ceiling: bool,
    over_ceiling_count: u64,
    abort_vpk: Vec<f64>,
    ceiling_vpk: Vec<f64>,
    // Per-update buffers, sized once.
    h: Vec<C64>,
    t: Vec<C64>,
    reference: Vec<C64>,
    judged: Vec<C64>,
    controls: Vec<C64>,
    limits: Vec<C64>,
    bounds: Vec<Option<f64>>,
    in_force: Vec<C64>,
    proposed: Vec<C64>,
    predicted: Vec<C64>,
    next: Vec<C64>,
    played: Vec<C64>,
    law_notch: Vec<f64>,
    record: TickRecord,
    worst_played: f64,
}

impl MimoSineCore {
    /// The core for `setup` (validated) at `fs_hz`, its parts built off the
    /// real-time path.
    pub fn new(setup: MimoSineSetup, fs_hz: f64, parts: CoreParts) -> Result<Self, String> {
        let (n, m, p) = (setup.controls.len(), setup.drives.len(), setup.limits.len());
        let tick_frames = ((setup.tick_s * fs_hz).round() as usize).max(1);
        let tick_s = tick_frames as f64 / fs_hz;
        let sched = Scheduler::new(&setup)?;
        let f0 = sched.frequency_hz();
        if !(f0 > 0.0 && f0 < fs_hz / 2.0) {
            return Err(format!(
                "the start frequency {f0} Hz cannot be played at {fs_hz} Hz"
            ));
        }
        let bw = setup.tracking.bandwidth_hz(f0);
        let lag = parts.lag_frames;
        let level_db = sched.level_db();
        let group_delay = GroupDelay::of(&hertzforge_mimo_sine_law_math::setup::Identification {
            grid_hz: parts.identification.frequency_hz.clone(),
            response: parts.identification.response.clone(),
            limit_response: parts.identification.limit_response.clone(),
            coherence: parts.identification.coherence.clone(),
            limit_coherence: parts.identification.limit_coherence.clone(),
            trusted: parts.identification.trusted.clone(),
        });
        let setup_budget_s = setup.guards.law_budget * tick_s;
        let budget_s = parts.law_budget_s.unwrap_or(setup_budget_s);
        Ok(MimoSineCore {
            n,
            m,
            p,
            tick_frames,
            tick_s,
            osc: SweepOscillator::new(fs_hz, f0),
            phase_ring: vec![0.0; lag + 1],
            lag_s: lag as f64 / fs_hz,
            ring_head: 0,
            ctl_trackers: vec![ToneTracker::new(fs_hz, bw); n],
            lim_trackers: vec![ToneTracker::new(fs_hz, bw); p],
            ramps: vec![DriveRamp::zero(); m],
            frame_in_tick: 0,
            stop_scale: 1.0,
            ramp_per_frame: 10f64.powf(-RAMP_DB_PER_S / (20.0 * fs_hz)),
            sched,
            law: parts.law,
            law_out: TickOutputs::new(m, n),
            law_values: LawValues::new(parts.law_values),
            clock: parts.clock,
            budget_s,
            setup_budget_s,
            misses: 0,
            ident: parts.identification,
            group_delay,
            notch: BackstopNotch::new(
                setup.guards.notch.smoothing_ticks,
                setup.guards.notch.release_ramp_db_per_s,
                setup.guards.notch.release_hysteresis_db,
            ),
            divergence: DivergenceWatch::new(setup.guards.divergence_s),
            ratings: parts.ratings,
            records: parts.records,
            commands: parts.commands,
            state: CoreState::Running,
            index: 0,
            engaged: false,
            engage_settle: true,
            level_db,
            level_target_db: level_db,
            request: None,
            outside_abort: [0; MAX_CONTROLS],
            in_alarm: [false; MAX_CONTROLS],
            notch_binding: false,
            over_ceiling: false,
            over_ceiling_count: 0,
            abort_vpk: setup.drives.iter().map(|d| d.abort_vpk).collect(),
            ceiling_vpk: setup.drives.iter().map(|d| d.ceiling_vpk).collect(),
            h: vec![ZERO; n * m],
            t: vec![ZERO; p * m],
            reference: vec![ZERO; n],
            judged: vec![ZERO; n],
            controls: vec![ZERO; n],
            limits: vec![ZERO; p],
            bounds: vec![None; p],
            in_force: vec![ZERO; m],
            proposed: vec![ZERO; m],
            predicted: vec![ZERO; p],
            next: vec![ZERO; m],
            played: vec![ZERO; m],
            law_notch: vec![1.0; n],
            record: TickRecord::empty(n, m, p),
            worst_played: 0.0,
            slots: parts.slots,
            setup,
        })
    }

    /// The run's state.
    pub fn state(&self) -> CoreState {
        self.state
    }

    /// The control update interval actually used, s (whole samples).
    pub fn tick_s(&self) -> f64 {
        self.tick_s
    }

    /// The frequency now, Hz.
    pub fn frequency_hz(&self) -> f64 {
        self.osc.frequency()
    }

    /// The level now, dB.
    pub fn level_db(&self) -> f64 {
        self.level_db
    }

    /// The drive in force (the last update's guarded drive), volts peak.
    pub fn drive_in_force(&self) -> &[C64] {
        &self.in_force
    }

    /// The deadline of a law call in force, s.
    pub fn law_budget_s(&self) -> f64 {
        self.budget_s
    }

    /// The setup's deadline of a law call, s (`guards.law_budget` of the
    /// update): the deadline on hardware, whatever a harness relaxed.
    pub fn setup_law_budget_s(&self) -> f64 {
        self.setup_budget_s
    }

    /// Updates where a drive over its ceiling scaled the vector.
    pub fn over_ceiling_count(&self) -> u64 {
        self.over_ceiling_count
    }

    /// The largest drive sample played so far, volts.
    pub fn largest_sample_v(&self) -> f64 {
        self.worst_played
    }

    /// Whether every drive is at zero (nothing engaged, or the stop ramp is
    /// complete).
    pub fn drive_is_zero(&self) -> bool {
        !self.engaged || self.stop_scale == 0.0
    }

    /// Records and events dropped.
    pub fn records_dropped(&self) -> u64 {
        self.records.dropped()
    }

    /// Begin the stop ramp for `outcome` (the first reason wins).
    pub fn stop(&mut self, outcome: Outcome) {
        if self.state == CoreState::Running {
            self.state = CoreState::Stopping(outcome);
            self.event(EventKind::Stopping { outcome });
        }
    }

    fn event(&self, kind: EventKind) {
        self.event_at(kind, self.level_db);
    }

    fn event_at(&self, kind: EventKind, level_db: f64) {
        self.records.event(Event {
            time_s: self.index as f64 * self.tick_s,
            index: self.index,
            frequency_hz: self.osc.frequency(),
            level_db,
            kind,
        });
    }

    /// The level carry: while the level in force is not the scheduled one,
    /// move it toward the schedule by at most the change caps allow in one
    /// update (the rise cap on the way up) and scale the drive in force by
    /// the same ratio, so the drive follows the level and keeps what the
    /// loop has learned (exactly right on a linear structure). The settle
    /// window opens with every move.
    fn carry_level(&mut self, f: f64, dt: f64) {
        if self.level_db == self.level_target_db {
            return;
        }
        let g = &self.setup.guards;
        let up = g.max_step_db.min(g.max_rise_db_per_s * dt);
        let down = g.max_step_db;
        let want = self.level_target_db - self.level_db;
        let step = want.clamp(-down, up);
        let before = self.level();
        self.level_db = if step == want {
            self.level_target_db
        } else {
            self.level_db + step
        };
        let ratio = self.level() / before;
        guards::scale(&mut self.in_force, ratio);
        self.sched.settle_for(self.settle_s(f));
        // The readings answer the drive before the move until the window
        // closes: the law is told to hold, as after an engage, so it does not
        // correct on them.
        self.engage_settle = true;
    }

    fn level(&self) -> f64 {
        10f64.powf(self.level_db / 20.0)
    }

    fn settle_s(&self, f: f64) -> f64 {
        let b = self.setup.tracking.bandwidth_hz(f);
        let tracker = SETTLE_WIDTHS / b;
        let structure = self.group_delay.at(f, 2.0 * b);
        // The identification has the lag taken out, so its group delay does
        // not hold the transport: add it once.
        SETTLE_TRACKER_TIMES * tracker + SETTLE_STRUCTURE_TIMES * structure + self.lag_s
    }

    /// One sample: the drive frame for it, written into `out` (`out_channels`
    /// wide), and the input frame that arrived with it.
    #[inline]
    pub fn sample(&mut self, input: &[f32], out: &mut [f32]) {
        if !self.engaged {
            self.engage();
        }
        // The tone.
        let (_, _, phi) = self.osc.next();
        // The stop ramp.
        if matches!(self.state, CoreState::Stopping(_)) {
            self.stop_scale *= self.ramp_per_frame;
            if self.stop_scale < RAMP_FLOOR_SCALE {
                self.stop_scale = 0.0;
                if let CoreState::Stopping(o) = self.state {
                    self.state = CoreState::Ended(o);
                    self.event(EventKind::Ended { outcome: o });
                }
            }
        }
        out.iter_mut().for_each(|v| *v = 0.0);
        if !matches!(self.state, CoreState::Ended(_)) {
            for (i, r) in self.ramps.iter_mut().enumerate() {
                let (mag, ph) = r.next();
                let v = mag * (phi + ph).cos() * self.stop_scale;
                if v.abs() > self.worst_played {
                    self.worst_played = v.abs();
                }
                // The played-sample abort, belt and braces behind the vector
                // guards: a drive sample at or over its abort ends the run.
                if v.abs() >= self.abort_vpk[i] && self.state == CoreState::Running {
                    self.state = CoreState::Stopping(Outcome::DriveAbort { drive: i as u8 });
                }
                out[self.slots.drives[i]] = v as f32;
            }
        }
        // The trackers, against the phase that produced this sample's response.
        self.phase_ring[self.ring_head] = phi;
        let lag = self.phase_ring.len() - 1;
        let back = (self.ring_head + self.phase_ring.len() - lag) % self.phase_ring.len();
        let retarded = self.phase_ring[back] + FRAC_PI_2;
        self.ring_head = (self.ring_head + 1) % self.phase_ring.len();
        for (t, &s) in self.ctl_trackers.iter_mut().zip(&self.slots.controls) {
            t.update(input[s] as f64, retarded);
        }
        for (t, &s) in self.lim_trackers.iter_mut().zip(&self.slots.limits) {
            t.update(input[s] as f64, retarded);
        }
        self.frame_in_tick += 1;
        if self.frame_in_tick >= self.tick_frames {
            self.frame_in_tick = 0;
            for r in &mut self.ramps {
                r.land();
            }
            self.update();
        }
    }

    /// Read the trackers into the per-update buffers.
    fn read_trackers(&mut self) {
        for (c, t) in self.controls.iter_mut().zip(&self.ctl_trackers) {
            let (re, im) = t.complex();
            *c = C64::new(re, im);
        }
        for (c, t) in self.limits.iter_mut().zip(&self.lim_trackers) {
            let (re, im) = t.complex();
            *c = C64::new(re, im);
        }
    }

    fn limit_bounds_at(&mut self, f: f64) {
        for (b, l) in self.bounds.iter_mut().zip(&self.setup.limits) {
            *b = l.profile.target_accel_g_pk(f);
        }
    }

    fn model_at(&mut self, f: f64) {
        let g = &self.ident.frequency_hz;
        interp_stack(
            g,
            self.ident.response.data(),
            self.n * self.m,
            f,
            &mut self.h,
        );
        if self.p > 0 {
            interp_stack(
                g,
                self.ident.limit_response.data(),
                self.p * self.m,
                f,
                &mut self.t,
            );
        }
    }

    /// The first law call: engage, before any sample is played.
    fn engage(&mut self) {
        self.engaged = true;
        let f = self.osc.frequency();
        let d = self.first_drive(f);
        self.event(EventKind::Engaged);
        self.start_ramps(&d);
        self.emit_record(f, 0.0, 0.0, 0.0, Hold::Engage, true);
        self.index = 1;
    }

    /// The law's engage at `f` (the run's start, and every step or manual
    /// frequency after it), bounded by the first-drive rule, through the
    /// guards after the law's; opens the settle window there.
    fn first_drive(&mut self, f: f64) -> Vec<C64> {
        let lv = self.level();
        reference_at(&self.setup, f, &mut self.reference);
        self.reference.iter_mut().for_each(|r| *r *= lv);
        self.limit_bounds_at(f);
        self.model_at(f);
        self.sched.settle_for(self.settle_s(f));
        let depth = self.notch.depth();
        for (j, r) in self.judged.iter_mut().zip(&self.reference) {
            *j = r * depth;
        }
        let proposed = self.call_law(true, f, 0.0, 0.0, Hold::Engage, true);
        self.record.engage = true;
        let mut d = match proposed {
            Some(()) => self.proposed.clone(),
            None => self.in_force.clone(),
        };
        // The first drive's bound: the identification's inverse drive here
        // plus 6 dB, or a share of the ceilings without a trusted line.
        let trusted = {
            let g = &self.ident.frequency_hz;
            let k = g.partition_point(|&x| x < f).clamp(1, g.len() - 1);
            self.ident.trusted[k - 1] && self.ident.trusted[k]
        };
        let bound = if trusted {
            match reg_pinv(&self.h, self.n, self.m, FIRST_DRIVE_EPS) {
                Ok((z, _)) => {
                    let mut d0 = vec![ZERO; self.m];
                    guards::mat_vec(&z, self.m, self.n, &self.reference, &mut d0);
                    guards::norm(&d0) * 10f64.powf(crate::limits::FIRST_DRIVE_MARGIN_DB / 20.0)
                }
                Err(_) => 0.0,
            }
        } else {
            crate::limits::FIRST_DRIVE_UNTRUSTED_FRACTION
                * guards::norm_real(&self.ceiling_vpk)
                * lv
        };
        self.record.first_drive_scale = 1.0;
        if let Some(s) = guards::first_drive_scale(&d, bound) {
            guards::scale(&mut d, s);
            self.record.first_drive_scale = s;
        }
        self.record.cap_bound = false;
        self.record.rotation_bound = false;
        self.record.largest_rotation_deg = 0.0;
        self.finish_guards(&mut d, f, 0.0);
        self.engage_settle = true;
        d
    }

    /// Call the law into `self.proposed`. `None` is a miss (the drive in
    /// force is held); the miss rule is applied here.
    fn call_law(
        &mut self,
        engage: bool,
        f: f64,
        sweep_hz: f64,
        sweep_oct: f64,
        hold: Hold,
        settling: bool,
    ) -> Option<()> {
        let dt = if engage { 0.0 } else { self.tick_s };
        let level = self.level();
        self.law_out.reset();
        self.law_values.clear();
        let inputs = TickInputs {
            index: self.index,
            dt_s: dt,
            frequency_hz: f,
            sweep_hz_per_s: sweep_hz,
            sweep_oct_per_min: sweep_oct,
            level,
            reference: &self.reference,
            controls: &self.controls,
            limits: &self.limits,
            limit_bounds: &self.bounds,
            drive_in_force: &self.in_force,
            hold,
            notch_scale: self.notch.depth(),
            settling,
        };
        let law = &mut self.law;
        let out = &mut self.law_out;
        let values = &mut self.law_values;
        let t0 = self.clock.now_s();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if engage {
                law.engage(&inputs, out, values)
            } else {
                law.tick(&inputs, out, values)
            }
        }));
        let took = (self.clock.now_s() - t0).max(0.0);
        let r: Result<(), LawFault> = match caught {
            Err(p) => Err(LawFault::Crashed(hertzforge_sine::law::guard::panic_text(
                &*p,
            ))),
            Ok(Err(f)) => Err(f),
            Ok(Ok(())) if took > self.budget_s => Err(LawFault::OverBudget {
                took_s: took,
                budget_s: self.budget_s,
            }),
            Ok(Ok(())) => match check_outputs(&self.law_out, self.m, self.n) {
                Err(rule) => Err(LawFault::BadOutput(rule)),
                Ok(()) if self.law_values.iter().any(|(_, v)| !v.is_finite()) => {
                    Err(LawFault::BadOutput(OutputRule::NonFinite))
                }
                Ok(()) => Ok(()),
            },
        };
        // The record of the call.
        let rec = &mut self.record;
        rec.law_called = true;
        rec.law_s = took;
        rec.values = [f64::NAN; crate::limits::MAX_LAW_VALUES];
        for (i, v) in self.law_values.iter() {
            if i < rec.values.len() {
                rec.values[i] = v;
            }
        }
        rec.drive_asked = [ZERO; MAX_DRIVES];
        rec.drive_in_force = [ZERO; MAX_DRIVES];
        rec.drive_in_force[..self.m].copy_from_slice(&self.in_force);
        match r {
            Ok(()) => {
                self.misses = 0;
                rec.miss = None;
                rec.law_faulted = self.law_out.faulted;
                rec.request = self.law_out.request;
                for (a, d) in rec.drive_asked.iter_mut().zip(&self.law_out.drive) {
                    *a = *d;
                }
                self.proposed.copy_from_slice(&self.law_out.drive);
                Some(())
            }
            Err(fault) => {
                self.misses += 1;
                rec.miss = Some(fault.reason());
                rec.law_faulted = true;
                rec.request = None;
                self.event(EventKind::LawMiss {
                    reason: fault.reason(),
                });
                // A law that cannot answer at engage, or misses too many in a
                // row, takes the controlled stop.
                if engage || self.misses >= self.setup.guards.law_misses {
                    self.stop(Outcome::LawFault);
                }
                None
            }
        }
    }

    /// The guards every drive passes after the law's own: the backstop
    /// notch, the drive ceilings, the drive abort; then readings.
    fn finish_guards(&mut self, d: &mut [C64], f: f64, dt: f64) {
        // The backstop notch.
        if self.p > 0 {
            guards::mat_vec(&self.t, self.p, self.m, d, &mut self.predicted);
            // The rise cap reads the drive as played; an engage (the start,
            // a step) has no drive before it at this frequency.
            let played = if dt > 0.0 {
                guards::norm(&self.played)
            } else {
                0.0
            };
            let norms = (guards::norm(d), played);
            let (s, _) = self.notch.apply(&self.predicted, &self.bounds, dt, norms);
            guards::scale(d, s);
        }
        self.record.notch_scale = self.notch.scale();
        // A drive over its ceiling scales the whole vector.
        self.record.ceiling_scale = 1.0;
        if let Some(s) = guards::ceiling_scale(d, &self.ceiling_vpk) {
            guards::scale(d, s);
            self.record.ceiling_scale = s;
            self.over_ceiling_count += 1;
            if !self.over_ceiling {
                self.event(EventKind::OverCeiling { scale: s });
            }
            self.over_ceiling = true;
        } else {
            self.over_ceiling = false;
        }
        // A drive at its abort ends the run (unreachable after the ceiling
        // scale unless an abort sits at its ceiling; kept as the last word).
        if let Some(i) = guards::drive_over_abort(d, &self.abort_vpk) {
            self.stop(Outcome::DriveAbort { drive: i as u8 });
        }
        // Readings.
        self.model_at(f);
        self.record.opposition_ratio =
            opposition_ratio(&self.h, self.n, self.m, d).unwrap_or(f64::NAN);
        let (share, which) = largest_share(d);
        self.record.largest_share = share;
        self.record.largest_share_drive = which as u8;
    }

    /// The share of the reference the limit channels allow now: the scale
    /// that puts the most loaded limit channel at its bound when the drive is
    /// the identification's inverse for the reference, through each channel's
    /// measured-over-identified ratio. Above 1 when they allow more than the
    /// whole reference; infinite with no bound here.
    fn notch_allowed(&mut self) -> f64 {
        let Ok((z, _)) = reg_pinv(&self.h, self.n, self.m, FIRST_DRIVE_EPS) else {
            return f64::INFINITY;
        };
        let mut d = vec![ZERO; self.m];
        guards::mat_vec(&z, self.m, self.n, &self.reference, &mut d);
        let mut y = vec![ZERO; self.p];
        guards::mat_vec(&self.t, self.p, self.m, &d, &mut y);
        let mut allowed = f64::INFINITY;
        for (j, (b, v)) in self.bounds.iter().zip(&y).enumerate() {
            if let Some(b) = b {
                let pred = self.notch.kappa(j) * v.norm();
                if pred > 0.0 {
                    allowed = allowed.min(b / pred);
                }
            }
        }
        allowed
    }

    fn start_ramps(&mut self, d: &[C64]) {
        for (r, x) in self.ramps.iter_mut().zip(d) {
            r.go(*x, self.tick_frames);
        }
        self.in_force.copy_from_slice(d);
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_record(
        &mut self,
        f: f64,
        sweep_hz: f64,
        sweep_oct: f64,
        dt: f64,
        hold: Hold,
        settling: bool,
    ) {
        let rec = &mut self.record;
        rec.index = self.index;
        rec.time_s = self.index as f64 * self.tick_s;
        rec.frequency_hz = f;
        rec.level_db = self.level_db;
        rec.sweep_oct_per_min = sweep_oct;
        rec.sweep_hz_per_s = sweep_hz;
        rec.dt_s = dt;
        rec.hold = hold;
        rec.settling = settling;
        for c in 0..self.n {
            rec.reference[c] = self.reference[c];
            rec.judged_reference[c] = self.judged[c];
            rec.control[c] = self.controls[c];
        }
        for j in 0..self.p {
            rec.limit[j] = self.limits[j];
            rec.limit_bound[j] = self.bounds[j].unwrap_or(f64::NAN);
        }
        for i in 0..self.m {
            rec.drive[i] = self.in_force[i];
        }
        self.records.tick(&self.record);
        self.record.engage = false;
        self.record.law_called = false;
    }

    fn drain_commands(&mut self) {
        let Some(rx) = self.commands.take() else {
            return;
        };
        while let Ok(c) = rx.try_recv() {
            match c {
                Command::Stop => self.stop(Outcome::Stopped),
                Command::Hold => {
                    if !self.sched.operator_held() {
                        self.sched.hold();
                        self.event(EventKind::Held { by_law: false });
                    }
                }
                Command::Resume => {
                    if self.sched.operator_held() {
                        self.sched.resume();
                        self.event(EventKind::Resumed);
                    }
                }
                Command::GoTo(hz) => {
                    if let Some(to_hz) = self.sched.go_to(hz) {
                        self.event(EventKind::ManualFrequency { to_hz });
                    }
                }
                Command::StepLevel(db) => {
                    self.sched.step_level_db(db);
                }
            }
        }
        self.commands = Some(rx);
    }

    /// Judge the controls and limit channels at `f`; returns a trip.
    fn judge(&mut self, f: f64, settling: bool) -> Option<Outcome> {
        let lv = self.level();
        for c in 0..self.n {
            let r = self.judged[c].norm();
            let a = self.controls[c].norm();
            let err = 20.0 * (a.max(1e-300) / r.max(1e-300)).log10();
            self.record.error_db[c] = err;
            if settling {
                self.outside_abort[c] = 0;
                continue;
            }
            let p = &self.setup.controls[c].profile;
            let scale = r / self.reference[c].norm().max(1e-300);
            if let Some((lo, hi)) = p.abort_bounds(f) {
                let (lo, hi) = (lo * lv * scale, hi * lv * scale);
                if a < lo || a > hi {
                    self.outside_abort[c] += 1;
                    if self.outside_abort[c] >= self.setup.guards.abort_ticks {
                        return Some(Outcome::ControlAbort { control: c as u8 });
                    }
                } else {
                    self.outside_abort[c] = 0;
                }
            }
            if let Some((lo, hi)) = p.alarm_bounds(f) {
                let out = a < lo * lv * scale || a > hi * lv * scale;
                if out && !self.in_alarm[c] {
                    self.event(EventKind::Alarm {
                        control: c as u8,
                        error_db: err,
                    });
                } else if !out && self.in_alarm[c] {
                    self.event(EventKind::AlarmCleared { control: c as u8 });
                }
                self.in_alarm[c] = out;
            }
        }
        // Limit channels: past the top of the abort band ends the run.
        for (j, l) in self.setup.limits.iter().enumerate() {
            if let Some((_, hi)) = l.profile.abort_bounds(f) {
                if self.limits[j].norm() > hi {
                    return Some(Outcome::LimitAbort { limit: j as u8 });
                }
            }
        }
        // The shakers' ratings, on the control that reads each one.
        if self.setup.guards.kinematic_monitor && !settling {
            let w = TAU * f;
            for r in &self.ratings {
                let a_g = self.controls[r.control].norm();
                let a = a_g * 9.80665;
                let over = r.max_acceleration_g_pk.is_some_and(|x| a_g > x)
                    || r.max_velocity_m_s_pk.is_some_and(|x| a / w > x)
                    || r.max_displacement_mm_pk
                        .is_some_and(|x| a / (w * w) * 1e3 > x);
                if over {
                    return Some(Outcome::Kinematic {
                        control: r.control as u8,
                    });
                }
            }
        }
        None
    }

    /// One control update, at the end of a tick's samples.
    fn update(&mut self) {
        self.drain_commands();
        if self.state != CoreState::Running {
            return;
        }
        let dt = self.tick_s;
        let f = self.osc.frequency();
        self.read_trackers();
        // The drive as played this interval, before any carry: what the caps
        // measure the next drive's change against.
        self.played.copy_from_slice(&self.in_force);
        self.carry_level(f, dt);
        // The reference here, and the bounds.
        let lv = self.level();
        reference_at(&self.setup, f, &mut self.reference);
        self.reference.iter_mut().for_each(|r| *r *= lv);
        // While the backstop notch holds the drive down the controls are
        // judged against the reference scaled by the same factor.
        self.limit_bounds_at(f);
        self.model_at(f);
        // While the backstop notch is in, the controls are judged against the
        // reference scaled to its depth: it follows the share of the
        // reference the limit channels allow (`notch_allowed`), deeper at
        // once, back up past the hysteresis at the release rate.
        let depth = if self.p > 0 {
            let allowed = self.notch_allowed();
            self.notch.track_depth(allowed, dt)
        } else {
            1.0
        };
        let binding = depth < 1.0;
        if binding && !self.notch_binding {
            self.event(EventKind::NotchIn { depth });
        } else if !binding && self.notch_binding {
            self.event(EventKind::NotchOut);
        }
        self.notch_binding = binding;
        self.record.notch_depth = depth;
        for ((j, r), d) in self
            .judged
            .iter_mut()
            .zip(&self.reference)
            .zip(&self.law_notch)
        {
            *j = r * (depth * d);
        }
        // The notch learns how the limit channels read against the
        // identification, from settled readings only: in a settle window (a
        // step, a level move) the readings still answer the drive before it,
        // and a ratio learned from them is a loop on lagging readings.
        if self.p > 0 && !self.sched.settling() {
            guards::mat_vec(&self.t, self.p, self.m, &self.in_force, &mut self.predicted);
            self.notch
                .observe(&self.limits, &self.predicted, &self.bounds);
        }
        // Judge (outside settle windows).
        let settling = self.sched.settling();
        if let Some(o) = self.judge(f, settling) {
            self.stop(o);
            self.emit_record(f, 0.0, 0.0, dt, Hold::Free, settling);
            return;
        }
        // Schedule the next interval.
        let req = self.request.take();
        let st = self.sched.step(dt, req);
        self.record.request_outcome = st.request;
        if req.is_some() && st.request == RequestOutcome::Granted && st.held {
            self.event(EventKind::Held { by_law: true });
        }
        if st.level_changed {
            // The level in force follows from the next update on, carrying
            // the drive with it (`carry_level`); the response takes the
            // structure's own time to follow, so the controls are not judged
            // until it has.
            self.level_target_db = st.level_db;
            self.event_at(EventKind::Level, st.level_db);
            self.sched.settle_for(self.settle_s(st.frequency_hz));
        }
        if st.jumped && !st.done {
            // A new step (or a manual frequency): the law engages again at the
            // new frequency, its first drive there bounded as at the start, and
            // the settle window opens.
            for t in self
                .ctl_trackers
                .iter_mut()
                .chain(self.lim_trackers.iter_mut())
            {
                t.restart_settle();
            }
            let fj = st.frequency_hz;
            if self.osc.try_set_frequency(fj).is_err() {
                self.stop(Outcome::FrequencyOutOfRange);
                return;
            }
            let bw = self.setup.tracking.bandwidth_hz(fj);
            for t in self
                .ctl_trackers
                .iter_mut()
                .chain(self.lim_trackers.iter_mut())
            {
                t.set_bn_hz(bw);
            }
            let d = self.first_drive(fj);
            self.divergence.update(false, dt);
            self.start_ramps(&d);
            self.emit_record(fj, 0.0, 0.0, 0.0, Hold::Engage, true);
            self.index += 1;
            return;
        }
        if !self.sched.settling() {
            self.engage_settle = false;
        }
        let hold = if self.engage_settle {
            Hold::Engage
        } else if self.notch_binding || self.notch.scale() < 1.0 {
            Hold::Notch
        } else if st.held && !self.sched.operator_held() {
            Hold::Held
        } else {
            Hold::Free
        };
        let settling_now = self.sched.settling();
        // The law.
        let asked = self.call_law(
            false,
            f,
            st.sweep_hz_per_s,
            st.sweep_oct_per_min,
            hold,
            settling_now,
        );
        let mut d = std::mem::take(&mut self.next);
        d.copy_from_slice(&self.in_force);
        let mut cap_bound = false;
        if asked.is_some() {
            d.copy_from_slice(&self.proposed);
            self.request = self.law_out.request;
            // A law's notched reference, taken under the design's rule.
            self.record.notched = false;
            if self.law_out.notched {
                let lowers = self
                    .law_out
                    .notched_reference
                    .iter()
                    .zip(&self.reference)
                    .all(|(a, r)| a.norm() <= r.norm() * (1.0 + 1e-12));
                let near = self
                    .bounds
                    .iter()
                    .zip(&self.limits)
                    .enumerate()
                    .any(|(j, (b, l))| {
                        b.is_some_and(|b| {
                            let pred = self.notch.kappa(j) * l.norm();
                            20.0 * (b / pred.max(1e-300)).log10()
                                <= self.setup.guards.notch.near_bound_db
                        })
                    });
                let releasing = self.notch.depth() < 1.0;
                if lowers && (near || releasing) {
                    for c in 0..self.n {
                        self.law_notch[c] = self.law_out.notched_reference[c].norm()
                            / self.reference[c].norm().max(1e-300);
                    }
                    self.record.notched = true;
                } else {
                    self.law_notch.fill(1.0);
                    self.event(EventKind::NotchedReferenceRefused);
                }
            } else {
                self.law_notch.fill(1.0);
            }
            // The caps per drive, against the drive as played (a level carry
            // counts toward them).
            let caps = guards::cap_drives(&self.played, &mut d, &self.setup.guards, dt);
            cap_bound = caps.magnitude_bound || caps.rotation_bound;
            self.record.cap_bound = caps.magnitude_bound;
            self.record.rotation_bound = caps.rotation_bound;
            self.record.largest_rotation_deg = caps.largest_rotation_deg;
        } else {
            self.record.cap_bound = false;
            self.record.rotation_bound = false;
            self.record.largest_rotation_deg = 0.0;
        }
        if self.divergence.update(cap_bound, dt) {
            self.stop(Outcome::Divergence);
        }
        self.record.first_drive_scale = 1.0;
        self.finish_guards(&mut d, f, dt);
        // The next frequency, phase-continuous; refused outside the oscillator.
        if self.osc.try_set_frequency(st.frequency_hz).is_err() {
            self.stop(Outcome::FrequencyOutOfRange);
        }
        let bw = self.setup.tracking.bandwidth_hz(self.osc.frequency());
        for t in self
            .ctl_trackers
            .iter_mut()
            .chain(self.lim_trackers.iter_mut())
        {
            t.set_bn_hz(bw);
        }
        if st.done {
            self.stop(Outcome::Completed);
        }
        self.start_ramps(&d);
        self.emit_record(
            f,
            st.sweep_hz_per_s,
            st.sweep_oct_per_min,
            dt,
            hold,
            settling_now,
        );
        self.next = d;
        self.index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::law::LawValues;
    use crate::setup::*;
    use hertzforge_sdm::MatrixStack;
    use hertzforge_sine::profile::{ProfileRow, SineProfile};
    use hertzforge_sine::scheduler::{DwellFor, DwellPlan, LadderConfig, RungFor};

    /// A law that asks the same drive every update.
    struct Constant(C64);

    impl MimoSineLaw for Constant {
        fn name(&self) -> &str {
            "constant"
        }
        fn engage(
            &mut self,
            _: &TickInputs<'_>,
            o: &mut TickOutputs,
            _: &mut LawValues,
        ) -> Result<(), LawFault> {
            o.drive.fill(self.0);
            Ok(())
        }
        fn tick(
            &mut self,
            _: &TickInputs<'_>,
            o: &mut TickOutputs,
            _: &mut LawValues,
        ) -> Result<(), LawFault> {
            o.drive.fill(self.0);
            Ok(())
        }
    }

    struct Clock;
    impl LawClock for Clock {
        fn now_s(&self) -> f64 {
            0.0
        }
    }

    /// A two-by-two core on an identity identification, its law asking
    /// `ask` volts on every drive, ceilings 1 V and aborts 2 V.
    fn core(ask: f64) -> MimoSineCore {
        let mut p = SineProfile::new("flat");
        p.global_abort_db = (60.0, 60.0);
        p.push(ProfileRow::accel(5.0, 1.0));
        p.push(ProfileRow::accel(2000.0, 1.0));
        let setup = MimoSineSetup {
            controls: (0..2)
                .map(|i| ControlChannel {
                    channel: i,
                    profile: p.clone(),
                    phase: vec![],
                })
                .collect(),
            drives: (0..2)
                .map(|i| DriveChannel {
                    channel: 10 + i,
                    ceiling_vpk: 1.0,
                    abort_vpk: 2.0,
                })
                .collect(),
            limits: vec![],
            phase_specified: false,
            test: TestPlan::Dwell(DwellPlan {
                freq_hz: 100.0,
                for_: DwellFor::Seconds(10.0),
            }),
            schedule: Schedule {
                mode: LevelMode::Auto,
                ladder: LadderConfig {
                    sysid_level_db: -20.0,
                    start_db: 0.0,
                    table_db: vec![0.0],
                    per_rung: RungFor::Seconds(100.0),
                    full_for: RungFor::Seconds(100.0),
                },
            },
            fs_hz: 4096.0,
            tick_s: 0.01,
            tracking: Default::default(),
            identification: Default::default(),
            law: Default::default(),
            requests: Default::default(),
            guards: Default::default(),
        };
        setup.validate().unwrap();
        let grid = vec![50.0, 200.0];
        let mut h = MatrixStack::zeros(2, 2, 2);
        for k in 0..2 {
            h.set(k, 0, 0, C64::new(1.0, 0.0));
            h.set(k, 1, 1, C64::new(1.0, 0.0));
        }
        let id = LawIdentification {
            frequency_hz: grid,
            response: h,
            limit_response: MatrixStack::zeros(2, 0, 2),
            coherence: vec![1.0; 4],
            limit_coherence: vec![],
            trusted: vec![false; 2],
        };
        let parts = CoreParts {
            slots: Slots {
                in_channels: 2,
                out_channels: 2,
                controls: vec![0, 1],
                limits: vec![],
                drives: vec![0, 1],
            },
            identification: id,
            lag_frames: 0,
            law: Box::new(Constant(C64::new(ask, 0.0))),
            law_values: 0,
            clock: Arc::new(Clock),
            law_budget_s: None,
            ratings: vec![],
            records: RecordSink::none(),
            commands: None,
        };
        MimoSineCore::new(setup, 4096.0, parts).unwrap()
    }

    /// Play `secs` of the core with the drive looped back as the input
    /// (a unit structure); the largest output sample.
    fn play(c: &mut MimoSineCore, secs: f64) -> f32 {
        let mut out = [0.0f32; 2];
        let mut inp = [0.0f32; 2];
        let mut big = 0.0f32;
        for _ in 0..(secs * 4096.0) as usize {
            c.sample(&inp, &mut out);
            inp = out;
            big = big.max(out[0].abs()).max(out[1].abs());
        }
        big
    }

    #[test]
    fn mimo_sine_core_a_drive_at_its_abort_ends_the_run_even_past_the_ceiling() {
        // The ceiling guard is what keeps a drive off its abort; with it
        // raised past the abort, the drive abort is the last word.
        let mut c = core(3.0);
        c.ceiling_vpk = vec![10.0, 10.0];
        play(&mut c, 3.0);
        assert!(
            matches!(
                c.state(),
                CoreState::Ended(Outcome::DriveAbort { .. })
                    | CoreState::Stopping(Outcome::DriveAbort { .. })
            ),
            "{:?}",
            c.state()
        );
    }

    #[test]
    fn mimo_sine_core_every_drive_stays_under_its_ceiling_whatever_the_law_asks() {
        let mut c = core(3.0);
        let big = play(&mut c, 3.0);
        assert!(big <= 1.0 + 1e-6, "{big}");
        assert!(c.over_ceiling_count() > 0);
    }

    #[test]
    fn mimo_sine_core_an_abort_walks_the_drive_to_zero_on_the_ramp() {
        let mut c = core(0.5);
        play(&mut c, 2.0);
        let mut out = [0.0f32; 2];
        let inp = [0.0f32; 2];
        c.stop(Outcome::ControlAbort { control: 0 });
        let mut peaks = Vec::new();
        let mut peak = 0.0f32;
        for k in 0..(1.5 * 4096.0) as usize {
            c.sample(&inp, &mut out);
            peak = peak.max(out[0].abs());
            if k % 410 == 409 {
                peaks.push(peak);
                peak = 0.0;
            }
        }
        // 0.1 s windows: each about 12 dB under the last, never a cliff; at
        // zero within the ramp's 1 s and held there.
        for w in peaks.windows(2).take(8) {
            let db = 20.0 * (w[1] / w[0]).log10();
            assert!((db + 12.0).abs() < 1.5, "{db}");
        }
        assert_eq!(*peaks.last().unwrap(), 0.0);
        assert_eq!(
            c.state(),
            CoreState::Ended(Outcome::ControlAbort { control: 0 })
        );
        assert!(c.drive_is_zero());
    }

    #[test]
    fn mimo_sine_core_ramp_moves_amplitude_and_phase_without_a_step() {
        let mut r = DriveRamp::zero();
        r.go(C64::from_polar(1.0, 3.0), 10);
        let mut last = 0.0;
        for _ in 0..10 {
            let (m, ph) = r.next();
            assert!(m - last <= 0.1 + 1e-12);
            assert!((ph - 3.0).abs() < 1e-12);
            last = m;
        }
        r.land();
        // From phase 3 to phase -3 the short way: through pi, 0.28 rad.
        r.go(C64::from_polar(1.0, -3.0), 4);
        let (_, ph) = r.next();
        assert!(ph > 3.0 && ph - 3.0 < 0.08, "{ph}");
    }

    #[test]
    fn mimo_sine_core_phase_points_interpolate_in_log_frequency() {
        let pts = [
            PhasePoint {
                freq_hz: 10.0,
                phase_deg: 0.0,
            },
            PhasePoint {
                freq_hz: 1000.0,
                phase_deg: 90.0,
            },
        ];
        assert!((phase_at(&pts, 100.0).to_degrees() - 45.0).abs() < 1e-9);
        assert!((phase_at(&pts, 1.0).to_degrees()).abs() < 1e-12);
        assert!((phase_at(&pts, 5000.0).to_degrees() - 90.0).abs() < 1e-9);
    }
}
