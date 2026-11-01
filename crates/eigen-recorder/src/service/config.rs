//! Recorder service configuration + the run-state enum.
//!
//! [`RecorderConfig`] is the serde-serializable knob set the UI edits: the
//! acquisition selection ([`AcquisitionConfig`]), the trigger mode
//! ([`TriggerMode`]), the recorder-level policy (auto-rearm, timed duration,
//! watchdog toggle, overload fraction, FRF pair), and the report metadata.
//!
//! Two policy defaults are load-bearing:
//!
//! * `watchdog_enabled` defaults to **false**: a monitor module
//!   should not e-stop a test it is not driving. When false the engine gets a
//!   channel-table clone with the abort limits STRIPPED, so the engine
//!   watchdog monitors nothing; the per-channel overload latches still run on
//!   the REAL table inside the service.
//! * `timed_record_s` is interpreted in FRAMES internally: the Sim plant
//!   free-runs with no wall clock, so a timed recording counts
//!   `round(timed_record_s * native_rate_hz)` frames, not seconds of
//!   wall-clock.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::acq::AcquisitionConfig;
use crate::trigger::TriggerMode;

/// One user-defined report metadata field (key / label / value). Matches the
/// `MetaField` shape the other module services use so the UI has one editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaField {
    /// Stable key (machine name), e.g. `run_number`.
    pub key: String,
    /// Human label, e.g. `Run Number`.
    pub label: String,
    /// The value (freeform text; the UI supplies typed editors).
    pub value: String,
}

impl MetaField {
    /// A field with an empty value.
    pub fn new(key: &str, label: &str) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            value: String::new(),
        }
    }
}

/// The standard, initially-empty metadata field set every recorder report
/// carries (Run Number, Test Article, SN, Operator).
pub fn default_metadata() -> Vec<MetaField> {
    vec![
        MetaField::new("run_number", "Run Number"),
        MetaField::new("test_article", "Test Article"),
        MetaField::new("sn", "SN"),
        MetaField::new("operator", "Operator"),
    ]
}

/// The default overload-proxy fraction: a channel latches an
/// overload when `|eu| >= overload_fraction * range_max`.
pub const DEFAULT_OVERLOAD_FRACTION: f64 = 0.9;

/// The recorder service configuration (serde).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecorderConfig {
    /// The DAQ / analysis selection. Capture is ALWAYS native; the
    /// decimation selector models the analysis ladder only.
    pub acq: AcquisitionConfig,
    /// How recordings start: free-run, manual, or a level trigger.
    pub trigger_mode: TriggerMode,
    /// Auto-rearm count: after the first trigger fire, how many more times the
    /// service re-arms into a NEW sub-recording. 0 = single recording.
    pub auto_rearm: u32,
    /// Pre-trigger ring depth in SECONDS at the native rate. `None` uses
    /// the [`crate::trigger::DEFAULT_PRETRIGGER_S`] default; values above
    /// [`crate::trigger::MAX_PRETRIGGER_S`] clamp there.
    pub pretrigger_s: Option<f64>,
    /// Timed-recording duration in SECONDS, converted to frames internally
    /// (`round(timed_record_s * native_rate_hz)`). `None` = manual stop.
    pub timed_record_s: Option<f64>,
    /// The engine abort watchdog REQUEST. When the effective value is false the
    /// engine table clone has its abort limits stripped; when true,
    /// `AbortWatchdog::poll` calls `emergency_stop` on the recorder's OWN
    /// backend only (documented; the recorder drives nothing else).
    ///
    /// This field is the request, NOT the answer. What actually arms is
    /// [`resolve_watchdog`], because the right default is station-dependent: see
    /// its doc. The field default stays FALSE, which is what an EIGEN-SIM
    /// station still gets.
    pub watchdog_enabled: bool,
    /// Whether [`RecorderConfig::watchdog_enabled`] is the OPERATOR's explicit
    /// choice rather than the field's own default.
    ///
    /// A plain `bool` cannot tell "the operator turned this off" from "nobody
    /// ever touched it", and on a hardware station those must land differently:
    /// untouched arms when limits exist, deliberately-off stays off and puts a
    /// persistent banner on screen. `#[serde(default)]` so a saved setup written
    /// before this field existed reads as NOT explicit - which is exactly what it was.
    #[serde(default)]
    pub watchdog_explicit: bool,
    /// Overload-proxy fraction. Default
    /// [`DEFAULT_OVERLOAD_FRACTION`].
    pub overload_fraction: f64,
    /// Optional live-FRF pair `(reference_channel, response_channel)` as
    /// 0-based indices into the ENABLED input channels. `None` = no live FRF.
    pub frf_pair: Option<(usize, usize)>,
    /// User-defined report metadata (defaults from [`default_metadata`]).
    pub metadata: Vec<MetaField>,
    /// Data root for run directories. `None` = the service's supplied default.
    pub data_root: Option<PathBuf>,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            acq: AcquisitionConfig::default(),
            trigger_mode: TriggerMode::Manual,
            auto_rearm: 0,
            pretrigger_s: None,
            timed_record_s: None,
            watchdog_enabled: false,
            watchdog_explicit: false,
            overload_fraction: DEFAULT_OVERLOAD_FRACTION,
            frf_pair: None,
            metadata: default_metadata(),
            data_root: None,
        }
    }
}

impl RecorderConfig {
    /// Validate the config (delegates to the acquisition validation, then
    /// checks the recorder-level knobs). Returns the first violation.
    pub fn validate(&self) -> Result<(), String> {
        self.acq.validate()?;
        if let Some(s) = self.timed_record_s {
            if s <= 0.0 || s.is_nan() {
                return Err(format!("timed_record_s must be positive (got {s})"));
            }
        }
        if self.overload_fraction <= 0.0 || self.overload_fraction.is_nan() {
            return Err(format!(
                "overload_fraction must be positive (got {})",
                self.overload_fraction
            ));
        }
        Ok(())
    }

    /// Timed-recording length in FRAMES at the native rate, or `None`
    /// for a manual-stop recording.
    pub fn timed_record_frames(&self) -> Option<u64> {
        self.timed_record_s
            .map(|s| (s * self.acq.native_rate_hz).round().max(0.0) as u64)
    }
}

/// What the watchdog policy actually decided, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchdogArming {
    /// Whether the abort watchdog is ARMED for this session. This is the value
    /// that reaches `build_engine_table`, never `cfg.watchdog_enabled`.
    pub armed: bool,
    /// True when a HARDWARE station carries abort limits in its table and the
    /// watchdog is nonetheless off. The pane renders a persistent banner on this
    /// flag; it is deliberately not the same thing as `!armed`, because a table
    /// with no limits has nothing to disarm.
    pub disarmed_with_limits: bool,
    /// The one-line basis, shown verbatim. Every arming states its reason.
    pub basis: String,
}

/// Decide whether the recorder's abort watchdog arms, from the station class,
/// the table's limits, and whether the operator said anything.
///
/// ## Why the default is station-dependent
///
/// The field default is OFF, and on EIGEN-SIM that is right: the plant is
/// software, a false trip costs a re-run, and "a monitor should not e-stop a test
/// it is not driving" is a real argument when nothing physical is attached.
///
/// On a HARDWARE station (NI-SIM or NI-REAL) the argument inverts. The operator
/// has typed abort limits into the channel table - that is an assertion that
/// exceeding them matters - and the recorder holds the only backend handle in the
/// process. Defaulting OFF there means the limits are decoration: they render in
/// the grid, they export into the report, and nothing enforces them. The
/// resolution is that hardware-station evidence runs are validated with the
/// watchdog LIVE, so the default arms whenever the table carries a limit at all.
///
/// NI-SIM is included deliberately, and it is the case worth stating: the signal
/// on a driver-simulated device is not a measurement, so a trip there proves
/// nothing about a shaker. What it DOES prove is that the arm path, the limit
/// plumbing, and the e-stop call all work - which is the only thing that can be
/// proven before hardware arrives, and the thing that must not be discovered
/// broken on the first real armature.
///
/// Disarming stays possible and stays the operator's call. It is simply no longer
/// SILENT: an explicit off on a hardware station with limits present sets
/// `disarmed_with_limits`, and the pane carries a persistent banner for as long
/// as it is true.
pub fn resolve_watchdog(
    cfg: &RecorderConfig,
    station: eigen_module_abi::StationClass,
    table_has_limits: bool,
) -> WatchdogArming {
    let hardware = station.is_hardware();

    // 1. The operator spoke. Their choice stands, whatever the station.
    if cfg.watchdog_explicit {
        if cfg.watchdog_enabled {
            return WatchdogArming {
                armed: true,
                disarmed_with_limits: false,
                basis: format!("abort watchdog ARMED by the operator on the {station} station"),
            };
        }
        return WatchdogArming {
            armed: false,
            disarmed_with_limits: hardware && table_has_limits,
            basis: if hardware && table_has_limits {
                format!(
                    "LIMITS DISARMED: the channel table carries abort limits and this is the \
                     {station} station, but the abort watchdog was explicitly turned off. \
                     Nothing will e-stop on a limit breach for this session."
                )
            } else {
                format!("abort watchdog off by the operator's choice ({station})")
            },
        };
    }

    // 2. Nobody spoke. A hardware station with limits arms itself.
    if hardware && table_has_limits {
        return WatchdogArming {
            armed: true,
            disarmed_with_limits: false,
            basis: format!(
                "abort watchdog ARMED by default: the channel table carries abort limits and \
                 this is the {station} station"
            ),
        };
    }

    // 3. Everything else keeps the field default.
    WatchdogArming {
        armed: cfg.watchdog_enabled,
        disarmed_with_limits: false,
        basis: if table_has_limits {
            format!("abort watchdog off ({station}: a monitor does not e-stop a software plant)")
        } else {
            format!(
                "abort watchdog inactive: the channel table carries no abort limits ({station})"
            )
        },
    }
}

/// The `table_has_limits` predicate: does any row this recorder would MONITOR
/// carry an abort limit?
///
/// Drive rows are excluded because they carry no response to limit, and disabled
/// rows because they are not acquired - both mirror
/// `AbortWatchdog::new`'s own channel selection, so this predicate and the thing
/// it predicts cannot drift apart.
pub fn table_has_abort_limits(table: &eigen_channel_table::ChannelTable) -> bool {
    table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != eigen_channel_table::ChannelRole::Drive)
        .any(|c| eigen_engine::limits::ChannelLimiter::has_limit(&c.abort))
}

/// The recorder state machine.
///
/// `Reviewing` is deliberately NOT a run state: review/offline analysis loads a
/// recording from disk and drives the analysis structs directly, independent of
/// the live service state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecState {
    /// No config applied.
    Idle,
    /// Config applied and validated; the engine loop is not live.
    Configured,
    /// The engine loop is live and analysis is streaming; NO disk writes.
    Monitoring,
    /// Armed: the trigger engine is active and the pre-trigger ring is filling.
    Armed,
    /// A recording is in progress (the RecordingWriter is active).
    Recording,
    /// Cleanly stopped (operator stop, or all timed/auto-rearm recordings done).
    Stopped,
    /// The engine reported a BackendError (or the watchdog tripped); the run is
    /// latched faulted with the message preserved.
    Faulted,
}

impl RecState {
    /// True while the engine loop is live (Monitoring / Armed / Recording).
    pub fn is_live(self) -> bool {
        matches!(
            self,
            RecState::Monitoring | RecState::Armed | RecState::Recording
        )
    }
}

#[cfg(test)]
mod watchdog_policy_tests {
    use super::*;
    use eigen_channel_table::{AbortLimits, Channel, ChannelRole, ChannelTable};
    use eigen_module_abi::StationClass::{EigenSim, NiReal, NiSimulated};

    fn limited(index: u32, role: ChannelRole) -> Channel {
        let mut c = Channel::new(index);
        c.role = role;
        c.abort = AbortLimits {
            accel_g: Some(5.0),
            velocity_ips: None,
            displacement_mil: None,
        };
        c
    }

    fn table(rows: Vec<Channel>) -> ChannelTable {
        ChannelTable {
            channels: rows,
            ..ChannelTable::default()
        }
    }

    /// The predicate must select exactly the rows the watchdog would monitor -
    /// enabled, non-drive - or the policy fires on rows nothing watches.
    #[test]
    fn the_limits_predicate_matches_what_the_watchdog_actually_monitors() {
        assert!(!table_has_abort_limits(&table(vec![Channel::new(1)])));
        assert!(table_has_abort_limits(&table(vec![limited(
            1,
            ChannelRole::Control
        )])));
        assert!(table_has_abort_limits(&table(vec![limited(
            1,
            ChannelRole::Monitor
        )])));

        // A DRIVE row carries no response to limit - the watchdog skips it, so
        // the predicate must too.
        assert!(!table_has_abort_limits(&table(vec![limited(
            1,
            ChannelRole::Drive
        )])));

        // A DISABLED row is not acquired.
        let mut off = limited(1, ChannelRole::Control);
        off.enabled = false;
        assert!(!table_has_abort_limits(&table(vec![off])));
    }

    /// THE hardware-station rule: a hardware station with limits in
    /// the table arms by default. Both hardware classes, because NI-SIM is where
    /// the arm path gets proven before an armature exists.
    ///
    /// RED-MUTATION: flip the default - delete the `if hardware &&
    /// table_has_limits` arm, so an untouched config falls through to
    /// `cfg.watchdog_enabled` (false) - and both hardware assertions red while
    /// the EIGEN-SIM one stays green. That asymmetry is the point: the mutation
    /// is only visible on hardware.
    #[test]
    fn a_hardware_station_arms_the_watchdog_by_default_when_limits_exist() {
        let cfg = RecorderConfig::default();
        assert!(!cfg.watchdog_enabled, "the FIELD default is still off");
        assert!(!cfg.watchdog_explicit, "and nobody has spoken");

        for station in [NiSimulated, NiReal] {
            let r = resolve_watchdog(&cfg, station, true);
            assert!(r.armed, "{station}: limits present must arm by default");
            assert!(!r.disarmed_with_limits);
            assert!(r.basis.contains("ARMED by default"), "{}", r.basis);
            assert!(r.basis.contains(&station.to_string()), "{}", r.basis);
        }

        // EIGEN-SIM keeps today's default: the plant is software.
        let sim = resolve_watchdog(&cfg, EigenSim, true);
        assert!(!sim.armed, "EIGEN-SIM keeps the field default");
        assert!(
            !sim.disarmed_with_limits,
            "and it is not a DISARMED state - nothing was disarmed"
        );
    }

    #[test]
    fn a_hardware_station_with_no_limits_has_nothing_to_arm() {
        // Anti-vacuity for the rule above: the default is not "always on for
        // hardware", it is "on when the operator asserted a limit".
        let r = resolve_watchdog(&RecorderConfig::default(), NiReal, false);
        assert!(!r.armed);
        assert!(!r.disarmed_with_limits);
        assert!(r.basis.contains("no abort limits"), "{}", r.basis);
    }

    /// Disarming stays possible - and stops being silent.
    ///
    /// RED-MUTATION: return `disarmed_with_limits: false` unconditionally and
    /// this reds; the banner never renders and an operator can turn the
    /// watchdog off on hardware with nothing on screen to say so.
    #[test]
    fn an_explicit_disarm_on_hardware_raises_the_persistent_banner() {
        let cfg = RecorderConfig {
            watchdog_enabled: false,
            watchdog_explicit: true,
            ..Default::default()
        };
        let r = resolve_watchdog(&cfg, NiReal, true);
        assert!(!r.armed, "the operator's choice stands");
        assert!(r.disarmed_with_limits, "and it is announced");
        assert!(r.basis.contains("LIMITS DISARMED"), "{}", r.basis);
        assert!(
            r.basis.contains("Nothing will e-stop"),
            "the banner must say what it COSTS: {}",
            r.basis
        );

        // No limits in the table: nothing was disarmed, so no banner.
        let none = resolve_watchdog(&cfg, NiReal, false);
        assert!(!none.disarmed_with_limits);

        // EIGEN-SIM: an explicit off is just the default restated, not a banner.
        let on_sim = resolve_watchdog(&cfg, EigenSim, true);
        assert!(!on_sim.disarmed_with_limits);
    }

    #[test]
    fn an_explicit_arm_is_honoured_on_every_station() {
        let cfg = RecorderConfig {
            watchdog_enabled: true,
            watchdog_explicit: true,
            ..Default::default()
        };
        for station in [EigenSim, NiSimulated, NiReal] {
            let r = resolve_watchdog(&cfg, station, true);
            assert!(r.armed, "{station}");
            assert!(!r.disarmed_with_limits);
            assert!(r.basis.contains("by the operator"), "{}", r.basis);
        }
    }

    #[test]
    fn every_resolution_states_a_basis() {
        // A safety decision with no stated reason is the thing this project
        // keeps paying for. There is no path through the resolver that returns
        // an empty one.
        for station in [EigenSim, NiSimulated, NiReal] {
            for explicit in [false, true] {
                for enabled in [false, true] {
                    for limits in [false, true] {
                        let cfg = RecorderConfig {
                            watchdog_enabled: enabled,
                            watchdog_explicit: explicit,
                            ..Default::default()
                        };
                        let r = resolve_watchdog(&cfg, station, limits);
                        assert!(
                            !r.basis.trim().is_empty(),
                            "{station}/{explicit}/{enabled}/{limits}"
                        );
                        assert!(
                            !r.disarmed_with_limits || !r.armed,
                            "an ARMED watchdog can never also be disarmed"
                        );
                    }
                }
            }
        }
    }
}
