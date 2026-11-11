//! What a Mixed Mode run says about itself: its state, the start-up phase, one
//! row per part, the floor's summary, the totals, the outcome and a trip that
//! names its part; and the run's event log.

use crate::parts::{Direction, MixedConfig, PartBody, ToneAmplitude};
use hertzforge_random::RunMode;
use serde::{Deserialize, Serialize};

/// The run's lifecycle, as Sine's run states (the same names on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixedState {
    /// Built and validated; nothing armed.
    Idle,
    /// Identification: Random's broadband pretest is running.
    SysId,
    /// The coherence gate is judging the identification.
    CoherenceGate,
    /// The gate failed: parked until the run is rebuilt. No release verb.
    CoherenceHold,
    /// The gate passed and the run holds for the operator's review; nothing is
    /// driven until [`super::MixedService::proceed_after_review`].
    AwaitingReview,
    /// The loop is closed and the level schedule is moving.
    Engaged,
    /// At the schedule's last level.
    FullLevel,
    /// Stopped cleanly (every part's schedule ended, the full-level time ran
    /// out, or the operator stopped).
    Stopped,
    /// A protection tripped; the drive was taken down at once.
    Aborted,
}

impl MixedState {
    /// The stable wire name.
    pub fn name(self) -> &'static str {
        match self {
            MixedState::Idle => "idle",
            MixedState::SysId => "sys_id",
            MixedState::CoherenceGate => "coherence_gate",
            MixedState::CoherenceHold => "coherence_hold",
            MixedState::AwaitingReview => "awaiting_review",
            MixedState::Engaged => "engaged",
            MixedState::FullLevel => "full_level",
            MixedState::Stopped => "stopped",
            MixedState::Aborted => "aborted",
        }
    }

    /// Whether the loop is closed and a protection can fire.
    pub fn is_running(self) -> bool {
        matches!(self, MixedState::Engaged | MixedState::FullLevel)
    }

    /// Whether the run has ended.
    pub fn is_terminal(self) -> bool {
        matches!(self, MixedState::Stopped | MixedState::Aborted)
    }
}

/// Where a running test is in its start-up order: the floor alone until it has
/// equalized, then tones and bands ramp in, then every part runs; a stop
/// ramps every part out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixedPhase {
    /// Not driving.
    Idle,
    /// The floor alone at the schedule's start level, equalizing.
    FloorOnly,
    /// Tones and bands ramping in from their feed-forward.
    PartsRampingIn,
    /// Every part under control; part clocks running.
    Running,
    /// Every part ramping out for a controlled stop.
    RampingDown,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixedOutcome {
    /// Every part's schedule ended, or the full-level time ran out.
    Completed,
    /// The operator stopped it.
    Stopped,
    /// A protection tripped ([`MixedStatus::trip`] says which, and on which part).
    Aborted,
    /// A law stopped giving a usable output; the core took its controlled stop.
    LawFault,
}

/// A part's kind, as the parts list names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    /// The floor.
    Floor,
    /// A tone.
    Tone,
    /// A narrowband.
    Narrowband,
}

/// One part's row in the status.
///
/// Units by kind: a tone's `target` and `measured` are g peak (its tracked
/// amplitude) and `drive` volts peak; a band's are g^2/Hz (the mean over the
/// inner part of the band, floor plus band) and `drive` its share of the drive,
/// volts rms; the floor's are g rms over clean lines and `drive` volts rms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartRow {
    /// The part's id.
    pub id: String,
    /// Its kind.
    pub kind: PartKind,
    /// Whether it plays now (switched on, inside its window, not ended).
    pub on: bool,
    /// Whether it has ended.
    pub done: bool,
    /// Frequency (tone) or center (band) now, Hz; `None` for the floor.
    pub freq_hz: Option<f64>,
    /// The target at the present level.
    pub target: f64,
    /// The measurement; `None` until the part has a usable reading (a tone's
    /// tracker settled and above the floor, a band's first frame).
    pub measured: Option<f64>,
    /// `measured` over `target`, dB (20 log for a tone, 10 log for the floor's
    /// rms-squared and a band's density).
    pub error_db: Option<f64>,
    /// The part's drive now.
    pub drive: f64,
    /// Whether the core holds this part's drive (a limit binds, or its law's
    /// output was not used).
    pub held: bool,
    /// Whether its alarm is set.
    pub alarm: bool,
    /// Whether a limit channel holds it down.
    pub limiting: bool,
    /// Whether its reading is usable: a tone's tracker has settled (and the
    /// tone stands above the floor in its band), a band has its first frame,
    /// the floor its first loop.
    pub settled: bool,
    /// A band's degrees of freedom in its moving-frame average; `None`
    /// otherwise.
    pub dof: Option<f64>,
    /// A sweep's completed one-direction passes; 0 otherwise.
    pub sweeps_done: u32,
    /// A sweep's direction now; `None` otherwise.
    pub direction: Option<Direction>,
    /// The part's ramp scale now, 0 (out) to 1 (in at its level): every change
    /// of a part's level is a ramp, and this is where the ramp is.
    #[serde(default)]
    pub scale: f64,
    /// Whether the part is switched on, as last switched (its configured
    /// `enabled`, then the operator's live switch).
    #[serde(default)]
    pub enabled: bool,
    /// The rate the part's frequency moves now, signed, in its schedule's
    /// unit (Hz per second or octaves per minute; a follower's is its root's
    /// times the ratio, in the root's unit); 0 at a dwell or while held.
    /// `None` for the floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
    /// A band's width now, Hz (a share band widens as it rises); `None`
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_hz: Option<f64>,
    /// A band's servo gain now, dB on its feed-forward density; `None`
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f64>,
    /// A tone's (or the floor's) law calls not used, in a row now.
    #[serde(default)]
    pub misses: u32,
    /// A tone's (or the floor's) law calls a core cap bound, in all.
    #[serde(default)]
    pub capped: u64,
}

/// One part's line in the run's report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartSummary {
    /// The part's id.
    pub id: String,
    /// Its kind.
    pub kind: PartKind,
    /// The target at the end, as the status row states it.
    pub target: f64,
    /// The last usable measurement.
    pub measured: Option<f64>,
    /// The last error, dB.
    pub error_db: Option<f64>,
    /// The largest error seen, dB (signed, the larger magnitude).
    pub worst_error_db: Option<f64>,
    /// Seconds its alarm was set (sampled a chunk at a time).
    pub alarm_s: f64,
}

/// The floor's summary.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct FloorSummary {
    /// The floor's reference rms at the present level, g.
    pub rms_target_g: f64,
    /// The residual's rms over clean lines, g; `None` before the first loop.
    pub rms_g: Option<f64>,
    /// `rms_g` against `rms_target_g`, dB.
    pub error_db_rms: Option<f64>,
    /// Lines left out of this loop's floor average (tones' exclusions, bands).
    pub lines_out: usize,
    /// Clean lines outside the alarm band.
    pub alarm_lines: usize,
    /// Clean lines outside the abort band.
    pub abort_lines: usize,
    /// Degrees of freedom of the floor estimate.
    pub dof: f64,
    /// Lines of the grid inside the floor's band.
    #[serde(default)]
    pub lines_in_band: usize,
}

/// The run's totals: shown, not judged (the drive peak is the one judged).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct MixedTotals {
    /// The control composite's rms, every part in, g.
    pub control_rms_g: Option<f64>,
    /// The summed drive's peak over the last chunk, volts peak: judged against
    /// the drive ceiling every frame.
    pub drive_vpk: f64,
    /// The summed drive's rms over the last chunk, volts.
    pub drive_vrms: f64,
    /// Whether the hard clamp at the drive ceiling cut the summed drive in
    /// the last chunk (the last backstop under the parts' own holds).
    #[serde(default)]
    pub clamped: bool,
}

/// Why a run tripped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MixedTripKind {
    /// The floor's clean lines breached the abort band past `max_lines_out`.
    FloorAbort {
        /// Lines out.
        lines_out: usize,
    },
    /// The floor's rms left its abort band.
    FloorRmsAbort {
        /// dB off the target.
        error_db: f64,
    },
    /// A tone's tracked amplitude left its abort band (after the debounce).
    ToneAbort {
        /// dB off the target.
        error_db: f64,
        /// The drive ceiling, volts peak, when it held the tone down as it
        /// tripped; `None` otherwise.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held_at_vpk: Option<f64>,
    },
    /// A band's level left its abort band (widened to its own scatter).
    BandAbort {
        /// dB off the target.
        error_db: f64,
    },
    /// The summed drive's peak reached the abort.
    DriveVpk {
        /// Volts peak.
        realized_vpk: f64,
        /// Volts peak.
        limit_vpk: f64,
    },
    /// The random part's (floor and bands) rms reached its abort.
    RandomVrms {
        /// Volts rms over the last quarter second.
        realized_vrms: f64,
        /// Volts rms.
        limit_vrms: f64,
    },
    /// A kinematic limit on a raw input sample.
    Kinematic {
        /// The channel.
        channel_index: u32,
        /// `accel`, `velocity` or `displacement`.
        axis: String,
        /// The value.
        value: f64,
        /// The limit.
        limit: f64,
    },
    /// A limit channel's own abort.
    LimitChannel {
        /// The channel.
        channel_index: u32,
        /// The value.
        measured: f64,
        /// The limit.
        limit: f64,
    },
    /// An input's overload latch.
    Overload {
        /// The channel.
        channel_index: u32,
    },
    /// The floor law's change cap bound the same lines too long, or a tone's
    /// cap bound past its `divergence_s`.
    Divergence,
    /// A law's outputs were unusable `max_misses` times in a row.
    LawFault {
        /// Which law.
        role: super::LawRole,
        /// The reason's wire name (Random's or Sine's).
        reason: String,
    },
    /// The station reported a fault.
    Station {
        /// Its words.
        message: String,
    },
}

/// A trip: what tripped, on which part, when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixedTrip {
    /// What tripped.
    pub kind: MixedTripKind,
    /// The part it tripped on; `None` for a trip of the whole drive (its peak,
    /// a kinematic limit, an overload, the station).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    /// Station time, seconds since the loop engaged.
    pub t_s: f64,
}

impl MixedTrip {
    /// The run-end reason in plain words, naming the part ("Tone t3 abort").
    pub fn words(&self) -> String {
        let part = self.part.as_deref().unwrap_or("");
        match &self.kind {
            MixedTripKind::FloorAbort { lines_out } => {
                format!("Floor abort: {lines_out} lines out")
            }
            MixedTripKind::FloorRmsAbort { error_db } => {
                format!("Floor rms abort: {error_db:+.1} dB")
            }
            MixedTripKind::ToneAbort {
                error_db,
                held_at_vpk: None,
            } => {
                format!("Tone {part} abort: {error_db:+.1} dB")
            }
            MixedTripKind::ToneAbort {
                error_db,
                held_at_vpk: Some(v),
            } => {
                format!("Tone {part} abort: {error_db:+.1} dB; the {v:.3} V drive ceiling held it")
            }
            MixedTripKind::BandAbort { error_db } => {
                format!("Narrowband {part} abort: {error_db:+.1} dB")
            }
            MixedTripKind::DriveVpk {
                realized_vpk,
                limit_vpk,
            } => format!("Drive peak {realized_vpk:.3} V at the {limit_vpk:.3} V abort"),
            MixedTripKind::RandomVrms {
                realized_vrms,
                limit_vrms,
            } => {
                format!("Random drive {realized_vrms:.3} V rms at the {limit_vrms:.3} V rms abort")
            }
            MixedTripKind::Kinematic {
                channel_index,
                axis,
                value,
                limit,
            } => format!("Channel {channel_index} {axis} {value:.3} over {limit:.3}"),
            MixedTripKind::LimitChannel {
                channel_index,
                measured,
                limit,
            } => format!("Limit channel {channel_index}: {measured:.3} over {limit:.3}"),
            MixedTripKind::Overload { channel_index } => {
                format!("Channel {channel_index} overload")
            }
            MixedTripKind::Divergence if part.is_empty() => "Divergence".to_owned(),
            MixedTripKind::Divergence => format!("Divergence on {part}"),
            MixedTripKind::LawFault { role, reason } => match role {
                super::LawRole::Floor => format!("Floor law fault: {reason}"),
                super::LawRole::Tones => format!("Tone {part} law fault: {reason}"),
            },
            MixedTripKind::Station { message } => format!("Station fault: {message}"),
        }
    }
}

/// One law's calls as the core has seen them.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RoleTally {
    /// Calls made.
    pub calls: u64,
    /// Calls whose output was not used.
    pub misses: u64,
    /// Misses whose output the core refused (a subset of `misses`).
    pub refused: u64,
    /// Calls a core cap bound.
    pub capped: u64,
    /// The longest call, microseconds.
    pub worst_call_us: f64,
}

/// The run's laws and their counters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixedLawStatus {
    /// The floor law's catalog id.
    pub floor_id: String,
    /// The tone law's catalog id.
    pub tones_id: String,
    /// The floor law's counters.
    pub floor: RoleTally,
    /// Each tone's law instance's counters, by part id, in parts-list order.
    pub tones: Vec<(String, RoleTally)>,
}

/// Everything the run says about itself each chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixedStatus {
    /// The lifecycle state.
    pub state: MixedState,
    /// Where the running test is in its start-up order.
    pub phase: MixedPhase,
    /// How it ended; `None` while it has not.
    pub outcome: Option<MixedOutcome>,
    /// The trip, when it tripped.
    pub trip: Option<MixedTrip>,
    /// Station time, seconds since the loop engaged, holds included.
    pub t_s: f64,
    /// Test time on the part clock, seconds, holds excluded.
    pub test_s: f64,
    /// The level in force, dB against full.
    pub level_db: f64,
    /// The schedule's rung index.
    pub rung_index: usize,
    /// Whether the level in force is the schedule's last.
    pub at_last_level: bool,
    /// How the schedule advances.
    pub run_mode: RunMode,
    /// Whether test progress is held.
    pub held: bool,
    /// One row per part, in parts-list order.
    pub parts: Vec<PartRow>,
    /// The floor.
    pub floor: FloorSummary,
    /// The totals.
    pub totals: MixedTotals,
    /// The identification's band coherence; 0 before it runs.
    pub band_coherence: f64,
    /// The share of the floor's band's lines whose identification is trusted
    /// (coherence and gain over their floors); 0 before it runs.
    #[serde(default)]
    pub trusted_frac: f64,
    /// Channels whose overload indicator has latched (channels the loop does
    /// not steer by; an overload on a control or limit channel aborts).
    #[serde(default)]
    pub overloads: Vec<u32>,
    /// Every enabled input's rms over its last half second of samples, tones
    /// in, in the input's own unit, as `(channel index, rms)` in table order.
    /// Empty until the first half second has been read.
    #[serde(default)]
    pub input_rms: Vec<(u32, f64)>,
    /// Whether a capture runs.
    pub capturing: bool,
    /// The laws and their counters; `None` until the loop engages.
    pub law: Option<MixedLawStatus>,
}

impl MixedStatus {
    /// The status of a run that has not started: one row per part with its
    /// full-level target at its starting frequency, nothing measured.
    pub fn idle(cfg: &MixedConfig) -> Self {
        let motions = cfg
            .motions()
            .unwrap_or_else(|_| vec![None; cfg.parts.len()]);
        let parts = cfg
            .parts
            .iter()
            .zip(&motions)
            .map(|(p, m)| {
                let start = |s: Option<&crate::parts::PartSchedule>| match s {
                    Some(crate::parts::PartSchedule::Sweep { start_hz, .. }) => Some(*start_hz),
                    _ => m.map(|m| m.lo_hz),
                };
                let (kind, freq_hz, target) = match &p.body {
                    PartBody::Floor(f) => (
                        PartKind::Floor,
                        None,
                        f.breakpoints.grms_of_table().unwrap_or(0.0),
                    ),
                    PartBody::Tone(t) => {
                        let f = start(Some(&t.schedule));
                        let g = match (&t.amplitude, f) {
                            (ToneAmplitude::Constant { g_pk }, _) => *g_pk,
                            (a, Some(f)) => a.g_pk_at(f).unwrap_or(0.0),
                            _ => 0.0,
                        };
                        (PartKind::Tone, f, g)
                    }
                    PartBody::Narrowband(b) => (
                        PartKind::Narrowband,
                        start(Some(&b.schedule)),
                        b.level_g2_hz,
                    ),
                    PartBody::Unknown { .. } => (PartKind::Tone, None, 0.0),
                };
                PartRow {
                    id: p.id.clone(),
                    kind,
                    on: false,
                    done: false,
                    freq_hz,
                    target,
                    measured: None,
                    error_db: None,
                    drive: 0.0,
                    held: false,
                    alarm: false,
                    limiting: false,
                    settled: false,
                    dof: (kind == PartKind::Narrowband).then_some(0.0),
                    sweeps_done: 0,
                    direction: None,
                    scale: 0.0,
                    enabled: p.enabled,
                    rate: p.schedule().map(|_| 0.0),
                    width_hz: match &p.body {
                        PartBody::Narrowband(b) => freq_hz.map(|f| {
                            let (lo, hi) = b.width.edges_at(f);
                            hi - lo
                        }),
                        _ => None,
                    },
                    gain_db: (kind == PartKind::Narrowband).then_some(0.0),
                    misses: 0,
                    capped: 0,
                }
            })
            .collect();
        let rms_target_g = cfg
            .floor()
            .and_then(|f| f.breakpoints.grms_of_table().ok())
            .unwrap_or(0.0);
        MixedStatus {
            state: MixedState::Idle,
            phase: MixedPhase::Idle,
            outcome: None,
            trip: None,
            t_s: 0.0,
            test_s: 0.0,
            level_db: cfg.schedule.start_db,
            rung_index: 0,
            at_last_level: false,
            run_mode: cfg.schedule.mode,
            held: false,
            parts,
            floor: FloorSummary {
                rms_target_g,
                ..FloorSummary::default()
            },
            totals: MixedTotals::default(),
            band_coherence: 0.0,
            trusted_frac: 0.0,
            overloads: Vec::new(),
            input_rms: Vec::new(),
            capturing: false,
            law: None,
        }
    }
}

/// One entry of the run's event log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixedEvent {
    /// Sequence number, from 0.
    pub seq: u64,
    /// Station time, seconds since the loop engaged (0 before).
    pub t_s: f64,
    /// What happened.
    pub kind: MixedEventKind,
}

/// What an event records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MixedEventKind {
    /// The state changed.
    State {
        /// From.
        from: MixedState,
        /// To.
        to: MixedState,
    },
    /// The start-up phase changed.
    Phase {
        /// To.
        to: MixedPhase,
    },
    /// The level in force changed.
    Level {
        /// dB against full.
        level_db: f64,
    },
    /// A part switched on or off (its window, its schedule's end, or the
    /// operator).
    Part {
        /// The part.
        part: String,
        /// On or off.
        on: bool,
    },
    /// Test progress held or resumed.
    Hold {
        /// Held.
        held: bool,
    },
    /// A protection tripped.
    Trip {
        /// The trip.
        trip: MixedTrip,
    },
    /// A law's output was not used.
    LawMiss {
        /// The part (`None` for the floor law).
        part: Option<String>,
        /// The reason's wire name.
        reason: String,
    },
    /// A plain note for the run log.
    Note {
        /// The words.
        text: String,
    },
}

/// The run's event log, in order.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MixedLog {
    events: Vec<MixedEvent>,
}

impl MixedLog {
    /// Append an event at station time `t_s`.
    pub fn push(&mut self, t_s: f64, kind: MixedEventKind) {
        let seq = self.events.len() as u64;
        self.events.push(MixedEvent { seq, t_s, kind });
    }

    /// Every event, oldest first.
    pub fn events(&self) -> &[MixedEvent] {
        &self.events
    }

    /// The events after sequence number `seq` (all of them for `None`).
    pub fn since(&self, seq: Option<u64>) -> &[MixedEvent] {
        let from = seq.map_or(0, |s| (s + 1).min(self.events.len() as u64) as usize);
        &self.events[from..]
    }
}
