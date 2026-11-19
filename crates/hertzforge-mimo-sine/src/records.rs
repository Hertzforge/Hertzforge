//! What the run hands its caller: one record per control update, the run's
//! timeline, and the law-call record with its replay.
//!
//! The loop thread sends with a call that never waits (a bounded channel's
//! `try_send`); a record the caller has not taken in time is dropped and
//! counted, never waited for. The records are fixed-size values (the sizes are
//! the first version's limits), so a send allocates nothing. Files are the
//! caller's: the control service writes them on its own thread.

use crate::law::{LawSetup, LawValues, MimoSineLawFactory};
use crate::limits::{MAX_CONTROLS, MAX_DRIVES, MAX_LAW_VALUES, MAX_LIMITS};
use crate::schedule::RequestOutcome;
use crate::tick::{Hold, LawFault, Request, TickInputs, TickOutputs};
use hertzforge_sdm::C64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::Arc;

const ZERO: C64 = C64::new(0.0, 0.0);

/// One control update, as recorded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickRecord {
    /// Updates since engage: 0 at engage.
    pub index: u64,
    /// Seconds since engage at the end of this update.
    pub time_s: f64,
    /// Controls, drives and limit channels in this run.
    pub controls: u8,
    pub drives: u8,
    pub limits: u8,
    /// The frequency the controls were measured at, Hz.
    pub frequency_hz: f64,
    /// The scheduled level, dB against full level.
    pub level_db: f64,
    /// The sweep rate for the next interval, octaves per minute, signed.
    pub sweep_oct_per_min: f64,
    /// The same rate in hertz per second, signed.
    pub sweep_hz_per_s: f64,
    /// Seconds since the last law call, as the law was told.
    pub dt_s: f64,
    /// What the law was told holds the drive.
    pub hold: Hold,
    /// Whether a settle window was open (no judging inside it).
    pub settling: bool,
    /// The reference at this frequency and level, EU pk.
    pub reference: [C64; MAX_CONTROLS],
    /// The reference the controls were judged against (a law's notched
    /// reference where the core accepted it, else the reference).
    pub judged_reference: [C64; MAX_CONTROLS],
    /// The tracked controls, EU pk.
    pub control: [C64; MAX_CONTROLS],
    /// Each control's error, dB, tracked over judged reference.
    pub error_db: [f64; MAX_CONTROLS],
    /// The tracked limit channels.
    pub limit: [C64; MAX_LIMITS],
    /// Each limit channel's bound here; NaN where there is none.
    pub limit_bound: [f64; MAX_LIMITS],
    /// The drive in force the law was told, volts peak.
    pub drive_in_force: [C64; MAX_DRIVES],
    /// The drive the law asked for (zero on a miss).
    pub drive_asked: [C64; MAX_DRIVES],
    /// The drive played from this update on, after every guard.
    pub drive: [C64; MAX_DRIVES],
    /// Whether the law was called this update (not on an update that ended
    /// the run before the law's turn).
    pub law_called: bool,
    /// Whether the law's answer was used (`None`) or the miss's reason.
    pub miss: Option<&'static str>,
    /// Whether this call was the law's engage.
    pub engage: bool,
    /// The law's call time, seconds.
    pub law_s: f64,
    /// The law's own judgement that it is faulted.
    pub law_faulted: bool,
    /// The law's values, by declared index; NaN where not reported.
    pub values: [f64; MAX_LAW_VALUES],
    /// The law's request, and what came of it.
    pub request: Option<Request>,
    pub request_outcome: RequestOutcome,
    /// Whether a law's notched reference was accepted this update.
    pub notched: bool,
    /// The backstop notch's scale, 0 to 1.
    pub notch_scale: f64,
    /// The depth the controls were judged at while the backstop notch bound:
    /// the share of the reference the limit channels allow, 1 when free.
    pub notch_depth: f64,
    /// The scale a drive over its ceiling put on the vector (1 when none).
    pub ceiling_scale: f64,
    /// The first drive's bound's scale (1 when none or not binding).
    pub first_drive_scale: f64,
    /// A change or rise cap bound.
    pub cap_bound: bool,
    /// The rotation cap bound.
    pub rotation_bound: bool,
    /// The largest turn of a drive's phase the law asked for, degrees.
    pub largest_rotation_deg: f64,
    /// The opposition ratio of the drive played (NaN where it could not be
    /// formed).
    pub opposition_ratio: f64,
    /// The largest share of the drive one shaker carries, and which.
    pub largest_share: f64,
    pub largest_share_drive: u8,
}

impl TickRecord {
    /// An empty record for `controls`, `drives` and `limits`.
    pub fn empty(controls: usize, drives: usize, limits: usize) -> Self {
        TickRecord {
            index: 0,
            time_s: 0.0,
            controls: controls as u8,
            drives: drives as u8,
            limits: limits as u8,
            frequency_hz: 0.0,
            level_db: 0.0,
            sweep_oct_per_min: 0.0,
            sweep_hz_per_s: 0.0,
            dt_s: 0.0,
            hold: Hold::Free,
            settling: false,
            reference: [ZERO; MAX_CONTROLS],
            judged_reference: [ZERO; MAX_CONTROLS],
            control: [ZERO; MAX_CONTROLS],
            error_db: [0.0; MAX_CONTROLS],
            limit: [ZERO; MAX_LIMITS],
            limit_bound: [f64::NAN; MAX_LIMITS],
            drive_in_force: [ZERO; MAX_DRIVES],
            drive_asked: [ZERO; MAX_DRIVES],
            drive: [ZERO; MAX_DRIVES],
            law_called: false,
            miss: None,
            engage: false,
            law_s: 0.0,
            law_faulted: false,
            values: [f64::NAN; MAX_LAW_VALUES],
            request: None,
            request_outcome: RequestOutcome::None,
            notched: false,
            notch_scale: 1.0,
            notch_depth: 1.0,
            ceiling_scale: 1.0,
            first_drive_scale: 1.0,
            cap_bound: false,
            rotation_bound: false,
            largest_rotation_deg: 0.0,
            opposition_ratio: f64::NAN,
            largest_share: 0.0,
            largest_share_drive: 0,
        }
    }

    /// The tracked controls of this run.
    pub fn controls(&self) -> &[C64] {
        &self.control[..self.controls as usize]
    }

    /// The drive played from this update on.
    pub fn drives(&self) -> &[C64] {
        &self.drive[..self.drives as usize]
    }

    /// The errors of this run's controls, dB.
    pub fn errors_db(&self) -> &[f64] {
        &self.error_db[..self.controls as usize]
    }
}

/// How a run ended, by its stable name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The schedule completed and the drive ramped out.
    Completed,
    /// The operator stopped the run; the drive ramped out.
    Stopped,
    /// A control passed its abort band outside a settle window.
    ControlAbort { control: u8 },
    /// A limit channel passed the top of its abort band.
    LimitAbort { limit: u8 },
    /// A drive reached its abort.
    DriveAbort { drive: u8 },
    /// A control's response passed a shaker's rating.
    Kinematic { control: u8 },
    /// The law missed too many updates in a row, or could not be built.
    LawFault,
    /// A cap bound without a break for the divergence time.
    Divergence,
    /// The frequency left what the oscillator can play.
    FrequencyOutOfRange,
    /// The run thread failed; the station was stopped at once.
    RunThreadFailure,
    /// The station stopped the run (a watchdog trip, a stall, a lost link).
    Station,
}

impl Outcome {
    /// The stable record name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Completed => "completed",
            Outcome::Stopped => "stopped",
            Outcome::ControlAbort { .. } => "control_abort",
            Outcome::LimitAbort { .. } => "limit_abort",
            Outcome::DriveAbort { .. } => "drive_abort",
            Outcome::Kinematic { .. } => "kinematic",
            Outcome::LawFault => "law_fault",
            Outcome::Divergence => "divergence",
            Outcome::FrequencyOutOfRange => "frequency_out_of_range",
            Outcome::RunThreadFailure => "run_thread_failure",
            Outcome::Station => "station",
        }
    }

    /// Whether the run ended on a fault (anything but completed or stopped).
    pub fn is_abort(&self) -> bool {
        !matches!(self, Outcome::Completed | Outcome::Stopped)
    }
}

/// One entry of the run's timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    /// Seconds since engage.
    pub time_s: f64,
    /// The update it happened on.
    pub index: u64,
    /// The frequency then, Hz.
    pub frequency_hz: f64,
    /// The level then, dB.
    pub level_db: f64,
    pub kind: EventKind,
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventKind {
    /// The law gave its first drive.
    Engaged,
    /// The scheduled level moved.
    Level,
    /// The schedule was held (by the operator, or a granted law request).
    Held { by_law: bool },
    /// The schedule resumed.
    Resumed,
    /// The operator sent the tone to `to_hz` (the event's frequency is
    /// where it was).
    ManualFrequency { to_hz: f64 },
    /// The limit channels began to hold the reference down: the depth they
    /// allow of it.
    NotchIn { depth: f64 },
    /// The limit channels allow the whole reference again.
    NotchOut,
    /// A law's notched reference was refused (not lower, or no limit near).
    NotchedReferenceRefused,
    /// A control left its alarm band (outside a settle window).
    Alarm { control: u8, error_db: f64 },
    /// A control came back inside its alarm band.
    AlarmCleared { control: u8 },
    /// A law call was not used.
    LawMiss { reason: &'static str },
    /// A drive over its ceiling scaled the vector (the first update of a run
    /// of them).
    OverCeiling { scale: f64 },
    /// The run began its stop ramp, for this reason.
    Stopping { outcome: Outcome },
    /// The drive reached zero and the run ended.
    Ended { outcome: Outcome },
}

impl EventKind {
    /// The stable record name.
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Engaged => "engaged",
            EventKind::Level => "level",
            EventKind::Held { .. } => "held",
            EventKind::Resumed => "resumed",
            EventKind::ManualFrequency { .. } => "manual_frequency",
            EventKind::NotchIn { .. } => "notch_in",
            EventKind::NotchOut => "notch_out",
            EventKind::NotchedReferenceRefused => "notched_reference_refused",
            EventKind::Alarm { .. } => "alarm",
            EventKind::AlarmCleared { .. } => "alarm_cleared",
            EventKind::LawMiss { .. } => "law_miss",
            EventKind::OverCeiling { .. } => "over_ceiling",
            EventKind::Stopping { .. } => "stopping",
            EventKind::Ended { .. } => "ended",
        }
    }
}

/// The loop's side of the record channels: sends that never wait.
#[derive(Debug, Clone)]
pub struct RecordSink {
    ticks: Option<SyncSender<TickRecord>>,
    events: Option<SyncSender<Event>>,
    dropped: Arc<AtomicU64>,
}

impl RecordSink {
    /// A sink that keeps nothing.
    pub fn none() -> Self {
        RecordSink {
            ticks: None,
            events: None,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Channels of `tick_capacity` update records and `event_capacity` events,
    /// and the caller's receivers.
    pub fn channels(
        tick_capacity: usize,
        event_capacity: usize,
    ) -> (RecordSink, Receiver<TickRecord>, Receiver<Event>) {
        let (ts, tr) = std::sync::mpsc::sync_channel(tick_capacity.max(1));
        let (es, er) = std::sync::mpsc::sync_channel(event_capacity.max(1));
        (
            RecordSink {
                ticks: Some(ts),
                events: Some(es),
                dropped: Arc::new(AtomicU64::new(0)),
            },
            tr,
            er,
        )
    }

    /// Send an update record; never waits.
    pub fn tick(&self, r: &TickRecord) {
        if let Some(s) = &self.ticks {
            if let Err(TrySendError::Full(_)) = s.try_send(*r) {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Send an event; never waits.
    pub fn event(&self, e: Event) {
        if let Some(s) = &self.events {
            if let Err(TrySendError::Full(_)) = s.try_send(e) {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Records and events dropped because the caller did not take them in
    /// time.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// One replayed law call that did not give what the record holds.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayMismatch {
    /// The update.
    pub index: u64,
    /// The largest difference between a recorded and a replayed drive, volts.
    pub largest_drive_difference_v: f64,
    /// The replay's own fault, where it failed.
    pub fault: Option<String>,
}

/// Replay a run's law calls: build the law again from `setup`, feed it each
/// recorded update's inputs in order, and compare its drive with the drive
/// the record says it asked for. Updates where the law was not called are
/// skipped; a miss is replayed and compared as a miss.
/// Returns every update whose replay differs by more than `tolerance_v`.
pub fn replay(
    factory: &dyn MimoSineLawFactory,
    setup: &LawSetup,
    records: &[TickRecord],
    tolerance_v: f64,
) -> Result<Vec<ReplayMismatch>, LawFault> {
    let mut law = factory.build(setup)?;
    let mut out = TickOutputs::new(setup.drives, setup.controls);
    let mut values = LawValues::new(factory.describe().diagnostics.len());
    let mut bad = Vec::new();
    for r in records.iter().filter(|r| r.law_called) {
        let (n, m, p) = (r.controls as usize, r.drives as usize, r.limits as usize);
        let bounds: Vec<Option<f64>> = r.limit_bound[..p]
            .iter()
            .map(|b| (!b.is_nan()).then_some(*b))
            .collect();
        let level = 10f64.powf(r.level_db / 20.0);
        let inputs = TickInputs {
            index: r.index,
            dt_s: r.dt_s,
            frequency_hz: r.frequency_hz,
            sweep_hz_per_s: r.sweep_hz_per_s,
            sweep_oct_per_min: r.sweep_oct_per_min,
            level,
            reference: &r.reference[..n],
            controls: &r.control[..n],
            limits: &r.limit[..p],
            limit_bounds: &bounds,
            drive_in_force: &r.drive_in_force[..m],
            hold: r.hold,
            notch_scale: r.notch_scale,
            settling: r.settling,
        };
        out.reset();
        values.clear();
        let res = if r.engage {
            law.engage(&inputs, &mut out, &mut values)
        } else {
            law.tick(&inputs, &mut out, &mut values)
        };
        match res {
            Err(f) => {
                if r.miss.is_none() {
                    bad.push(ReplayMismatch {
                        index: r.index,
                        largest_drive_difference_v: f64::INFINITY,
                        fault: Some(f.to_string()),
                    });
                }
            }
            Ok(()) => {
                let diff = out
                    .drive
                    .iter()
                    .zip(&r.drive_asked[..m])
                    .map(|(a, b)| (a - b).norm())
                    .fold(0.0, f64::max);
                if diff > tolerance_v || out.drive.len() != m {
                    bad.push(ReplayMismatch {
                        index: r.index,
                        largest_drive_difference_v: diff,
                        fault: None,
                    });
                }
            }
        }
    }
    Ok(bad)
}
