//! Per-pulse record store: every fired
//! pulse's time history is captured and its SRS + kinematics (+ optional PVSS) are
//! computed on ingest via hertzforge-dsp, then stored for browse/overlay.
//!
//! # What is computed on ingest
//!
//! When a pulse is ingested ([`PulseStore::ingest`]) the crate computes, via
//! hertzforge-dsp:
//! - [`hertzforge_dsp::shock::pulse_kinematics`] - the pure-integrator kinematics (peak
//!   accel / velocity, residuals, pk-pk displacement; the no-HP policy);
//! - [`hertzforge_dsp::srs::srs_analyze`] - the maximax-acceleration SRS on the supplied
//!   frequency grid;
//! - optionally [`hertzforge_dsp::srs::srs_pseudo_velocity`] - the PVSS.
//!
//! The result is a [`PulseRecord`] the UI browses and overlays. The store
//! also produces a [`ShockSnapshotPayload`] for hertzforge-testkit's `Snapshot<P>` and
//! wires per-pulse capture through a [`hertzforge_capture::CaptureSink`].

use hertzforge_capture::{CaptureSink, Mark, Snapshot};
use hertzforge_dsp::shock::{pulse_kinematics, PulseKinematics};
use hertzforge_dsp::srs::{srs_analyze, srs_pseudo_velocity};
use serde::{Deserialize, Serialize};

/// The quality factor `Q` used for the per-pulse SRS/PVSS (`zeta = 1/(2 Q)`). `Q = 10`
/// (`zeta = 0.05`, 5% damping) is the SRS convention `srs_analyze` documents. Exposed
/// as a constant so the ingest and any downstream re-analysis agree.
pub const DEFAULT_SRS_Q: f64 = 10.0;

/// A serializable copy of [`hertzforge_dsp::shock::PulseKinematics`]. hertzforge-dsp's struct is
/// `Copy` but not `Serialize`; this crate mirrors it so a [`PulseRecord`] (and the
/// snapshot payload) round-trips through JSON. Field docs match the hertzforge-dsp original
/// (in/g units; pure no-HP integrator).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PulseKinematicsRecord {
    /// Peak absolute acceleration over the record, g.
    pub peak_accel_g: f64,
    /// Peak absolute velocity over the record, in/s.
    pub peak_velocity_ips: f64,
    /// Velocity at the last sample (net velocity change), in/s. Nonzero for an
    /// uncompensated pulse; ~0 for a self-nulling waveform.
    pub residual_velocity_ips: f64,
    /// Maximum (most positive) displacement over the record, in.
    pub peak_disp_in: f64,
    /// Minimum (most negative) displacement over the record, in.
    pub min_disp_in: f64,
    /// Peak-to-peak displacement span, in (the stroke the pulse demands).
    pub pk_pk_disp_in: f64,
    /// Displacement at the last sample (net displacement change), in.
    pub residual_disp_in: f64,
}

impl From<PulseKinematics> for PulseKinematicsRecord {
    fn from(k: PulseKinematics) -> Self {
        PulseKinematicsRecord {
            peak_accel_g: k.peak_accel_g,
            peak_velocity_ips: k.peak_velocity_ips,
            residual_velocity_ips: k.residual_velocity_ips,
            peak_disp_in: k.peak_disp_in,
            min_disp_in: k.min_disp_in,
            pk_pk_disp_in: k.pk_pk_disp_in,
            residual_disp_in: k.residual_disp_in,
        }
    }
}

/// One fired pulse's record: the captured time history plus everything computed from
/// it on ingest. This is the browse/overlay unit and the per-pulse row in the shock
/// report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseRecord {
    /// Fire sequence number within the run (0-based, monotonically increasing).
    pub seq: u64,
    /// The commanded level for this pulse, dB relative to full (from the ladder).
    pub level_db: f64,
    /// The measured control-channel acceleration time history, g.
    pub time_history: Vec<f64>,
    /// The sample rate of `time_history`, Hz.
    pub fs_hz: f64,
    /// Kinematics from the pure-integrator, computed on ingest.
    pub kinematics: PulseKinematicsRecord,
    /// The maximax-acceleration SRS on `srs_freqs`, g.
    pub srs: Vec<f64>,
    /// The natural-frequency grid the SRS is on, Hz.
    pub srs_freqs: Vec<f64>,
    /// The pseudo-velocity SRS (PVSS) on `srs_freqs`, in/s. `None` if not
    /// requested at ingest.
    pub pvss: Option<Vec<f64>>,
    /// When the pulse fired, seconds since the Unix epoch.
    pub fired_at_unix: u64,
}

impl PulseRecord {
    /// The peak acceleration of this pulse, g (convenience over the kinematics).
    pub fn peak_accel_g(&self) -> f64 {
        self.kinematics.peak_accel_g
    }

    /// The pk-pk displacement (stroke) this pulse demanded, in.
    pub fn pk_pk_disp_in(&self) -> f64 {
        self.kinematics.pk_pk_disp_in
    }
}

/// The in-memory per-pulse store. Append-only during a run; the UI reads
/// records back for browse and overlay.
///
/// This is a plain owned collection (no I/O); the DURABLE copy of a pulse's time
/// history is the capture sink's `telemetry.f32`, and the browsable state is
/// snapshotted via [`ShockSnapshotPayload`]. Keeping the store itself I/O-free makes
/// it exhaustively testable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PulseStore {
    records: Vec<PulseRecord>,
    next_seq: u64,
}

impl PulseStore {
    /// A new, empty store.
    pub fn new() -> Self {
        PulseStore::default()
    }

    /// Ingest a fired pulse: compute kinematics + SRS (+ optional PVSS) via hertzforge-dsp,
    /// assign the next sequence number, append, and return the stored record's `seq`.
    ///
    /// - `time_history_g`: the measured control-channel acceleration, g.
    /// - `fs_hz`: its sample rate.
    /// - `srs_freqs`: the natural-frequency grid for the SRS (e.g. from
    ///   [`hertzforge_dsp::srs::octave_freqs`]).
    /// - `level_db`: the commanded level for the pulse.
    /// - `compute_pvss`: also compute the pseudo-velocity SRS.
    /// - `fired_at_unix`: fire timestamp (seconds since epoch; the caller supplies it
    ///   so ingest stays deterministic and testable - no clock read here).
    pub fn ingest(
        &mut self,
        time_history_g: Vec<f64>,
        fs_hz: f64,
        srs_freqs: Vec<f64>,
        level_db: f64,
        compute_pvss: bool,
        fired_at_unix: u64,
    ) -> u64 {
        let kin: PulseKinematicsRecord = pulse_kinematics(&time_history_g, fs_hz).into();
        let srs = srs_analyze(&time_history_g, fs_hz, &srs_freqs, DEFAULT_SRS_Q);
        let pvss = if compute_pvss {
            Some(srs_pseudo_velocity(
                &time_history_g,
                fs_hz,
                &srs_freqs,
                DEFAULT_SRS_Q,
            ))
        } else {
            None
        };
        let seq = self.next_seq;
        self.next_seq += 1;
        self.records.push(PulseRecord {
            seq,
            level_db,
            time_history: time_history_g,
            fs_hz,
            kinematics: kin,
            srs,
            srs_freqs,
            pvss,
            fired_at_unix,
        });
        seq
    }

    /// All stored records, in fire order.
    pub fn records(&self) -> &[PulseRecord] {
        &self.records
    }

    /// The number of stored pulses.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// True if no pulses are stored.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Get a stored pulse by its sequence number (not vector index). `O(n)` but the
    /// counts are small; kept by `seq` so callers reference the stable id even if the
    /// store is ever pruned.
    pub fn get(&self, seq: u64) -> Option<&PulseRecord> {
        self.records.iter().find(|r| r.seq == seq)
    }

    /// The most-recently ingested pulse, if any.
    pub fn last(&self) -> Option<&PulseRecord> {
        self.records.last()
    }

    /// Collect references to the pulses whose `seq` is in `seqs`, in the order the
    /// SEQUENCES are given (the overlay-selection helper: the UI picks a
    /// set of pulses to overlay by id). Unknown seqs are skipped.
    pub fn select<'a>(&'a self, seqs: &[u64]) -> Vec<&'a PulseRecord> {
        seqs.iter().filter_map(|&s| self.get(s)).collect()
    }

    /// Build the snapshot payload from the current store contents plus the current run
    /// context. See [`ShockSnapshotPayload`].
    pub fn snapshot_payload(
        &self,
        current_level_db: f64,
        srs_q: f64,
        target_srs: Option<Vec<f64>>,
    ) -> ShockSnapshotPayload {
        ShockSnapshotPayload {
            pulses: self.records.clone(),
            current_level_db,
            srs_q,
            target_srs,
        }
    }
}

/// The shock-mode payload for hertzforge-testkit's `Snapshot<P>`.
///
/// hertzforge-testkit's `Snapshot` carries the mode-generic envelope (timestamp, opaque
/// settings, marks); each mode supplies its own payload. SMART supplies spectra +
/// profiles; Random supplies its own; SHOCK supplies the fired pulses + the run
/// context needed to render the overlays. This crate is the ONLY place the shock
/// payload is named (mirroring how hertzforge-testkit never names a mode payload).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShockSnapshotPayload {
    /// Every fired pulse, in order - the browse/overlay set.
    pub pulses: Vec<PulseRecord>,
    /// The current commanded level, dB (the ladder rung at snapshot time).
    pub current_level_db: f64,
    /// The `Q` the SRS/PVSS were computed at (so a reader re-analyzes consistently).
    pub srs_q: f64,
    /// The target SRS on the pulses' SRS grid, g, when a target is set (the overlay
    /// reference/tolerance curves). `None` for an open-loop pulse run.
    pub target_srs: Option<Vec<f64>>,
}

/// Build a full [`Snapshot`] (hertzforge-testkit envelope + shock payload) ready for
/// [`hertzforge_capture::save_snapshot`]. The caller supplies the envelope fields
/// (timestamp, opaque settings JSON, operator marks) and the payload; this helper just
/// assembles the generic bundle so a service does not re-derive the shape.
pub fn build_snapshot(
    unix_secs: u64,
    settings_json: String,
    marks: Vec<Mark>,
    payload: ShockSnapshotPayload,
) -> Snapshot<ShockSnapshotPayload> {
    Snapshot {
        unix_secs,
        settings_json,
        marks,
        payload,
    }
}

/// Stream a just-ingested pulse's time history to the run's capture sink (every pulse
/// is recorded). The sink writes interleaved f32; for a single control-channel
/// pulse the frame is one sample wide, so `frames == time_history.len()`.
///
/// This wires the store to hertzforge-testkit's [`CaptureSink`] the way the SMART/Random
/// services do: the reader (this call) never blocks on disk - the sink drops on a full
/// ring and bumps its drop count. Returns the sink's `push` result (true = queued,
/// false = dropped/disconnected).
///
/// The caller owns the sink's lifecycle ([`CaptureSink::start`] / [`CaptureSink::finish`]);
/// this helper only pushes one pulse's samples.
pub fn capture_pulse(sink: &CaptureSink, record: &PulseRecord, n_channels: usize) -> bool {
    debug_assert!(n_channels >= 1, "a capture needs at least one channel");
    // For a single control channel the interleaved frame is one sample wide. If the
    // capture is multi-channel the caller is expected to interleave upstream; here we
    // push the control-channel record as its own channel-0 stream and let the frame
    // count reflect the samples pushed (n_channels is recorded in the sidecar).
    let samples: Vec<f32> = record.time_history.iter().map(|&g| g as f32).collect();
    let frames = samples.len();
    sink.push(&samples, frames)
}
