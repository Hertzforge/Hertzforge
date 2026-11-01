//! The daemon-side Data Recorder: thin orchestration over `eigen-recorder` and the
//! station factory.
//!
//! # Scope: the arrival-day REMOTE MINIMUM, and what came after it
//!
//! The split-deployment design named the Recorder as the daemon's v1 module coverage,
//! and it was the right FIRST one: monitor-only (a synthetic silent drive feeds zeros
//! into an injected drive channel; there is no output, no sweep, no level ladder), so
//! the remote surface could be proven end to end WITHOUT a remote peer ever being able
//! to command drive.
//!
//! **CORRECTED 2026-08-08 (byte beats doc).** The line this paragraph used to carry -
//! "it is deliberately the only one" - stopped being true when
//! [`crate::random::RandomHost`] landed as the first drive-producing module over the
//! seam. Coverage is now Recorder + Random, and which of them owns the hardware at any
//! moment is [`crate::core::Core::live_module`]'s answer, not a fact any single host
//! carries. Every module WITHOUT a host here still answers
//! `ProtocolError::LocalOnly`.
//!
//! # THE CONTROL LOOP NEVER CROSSES THE NETWORK
//!
//! Everything in this file runs on the hardware host. The pump thread owns the
//! service, calls `pump()` in a loop, and publishes DECIMATED DISPLAY traces on a
//! broadcast channel. What crosses the wire is that broadcast, packed per connection.
//! No remote peer is inside any loop.
//!
//! # Why it publishes UNPACKED traces
//!
//! Each connection has its own selectors, so each packs its own bytes (see
//! [`crate::frames`]). Publishing packed frames would force one channel set and one
//! cadence on every peer and turn the selectors into receiver-side filtering, which
//! saves nothing on the wire - the thing the selectors exist to save.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eigen_hal::Backend;
use eigen_protocol::ProtocolError;
use eigen_recorder::service::{RecState, RecorderConfig, RecorderService};
use eigen_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
};

use crate::provenance::{mint_run_id, now_unix, RunProvenance};

/// The service, over whatever the factory built.
type Svc = RecorderService<Box<dyn Backend>>;

/// Poll cadence between `pump()` segments. Mirrors the Tauri layer's `TICK`.
const TICK: Duration = Duration::from_millis(15);

/// The reserved synthetic channel ids the recorder's FRF traces ride, per
/// `ui/IPC_CONTRACT.md` (930-932, RESERVED for recorder). Duplicated here rather than
/// imported for the same reason as the packer - see [`crate::frames`].
pub const CH_FRF_MAG: u32 = 930;
/// FRF ordinary-coherence trace.
pub const CH_FRF_COH: u32 = 931;
/// PSD-average marker trace.
pub const CH_PSD_MARKER: u32 = 932;

/// One published display batch: a spectrum window, or a raw time-domain view.
/// One bus for the whole daemon; the server packs per subscription.
#[derive(Debug, Clone)]
pub enum DisplayBatch {
    /// Spectrum traces on a shared grid.
    Spectrum(SpectrumBatch),
    /// Envelope (min/max) columns per channel.
    Scope(ScopeBatch),
    /// Decimated real samples per channel (display-grade decimation).
    Wave(WaveBatch),
}

/// One raw-envelope batch: per-channel min/max columns for one loop segment.
#[derive(Debug, Clone)]
pub struct ScopeBatch {
    /// Seconds per COLUMN.
    pub dt_s: f32,
    /// Sample time of the newest column, seconds (run-relative).
    pub t_end_s: f64,
    /// The run these belong to; `0` = unattributed.
    pub run_id: u64,
    /// `(channel_id, rms, peak, mins, maxs)`.
    // A positional tuple documented above, built and packed in place.
    #[allow(clippy::type_complexity)]
    pub channels: Vec<(u16, f32, f32, Vec<f32>, Vec<f32>)>,
}

/// One decimated-waveform batch for one loop segment.
#[derive(Debug, Clone)]
pub struct WaveBatch {
    /// Seconds per SAMPLE (1 / decimated rate).
    pub dt_s: f32,
    /// Sample time of the newest sample, seconds (run-relative).
    pub t_end_s: f64,
    /// The run these belong to; `0` = unattributed.
    pub run_id: u64,
    /// `(channel_id, samples)`.
    pub channels: Vec<(u16, Vec<f32>)>,
}

/// One published analysis window: every trace, unpacked, on a shared grid.
#[derive(Debug, Clone)]
pub struct SpectrumBatch {
    /// First bin frequency (always 0 for the recorder's analysis grid).
    pub f0_hz: f64,
    /// Bin spacing.
    pub df_hz: f64,
    /// The analysis window counter this batch belongs to.
    pub window_seq: u32,
    /// The run these traces belong to; `0` = unattributed.
    pub run_id: u64,
    /// `(channel_id, control_group, magnitudes)`. Per-channel PSD rides the REAL
    /// channel id (1..=N); the FRF traces ride the reserved 930-932.
    pub traces: Vec<(u32, Option<u32>, Arc<Vec<f32>>)>,
}

/// Live pump metrics, for the soak harness and the status surface. The recorded
/// duration is the segment's LAG MARGIN (wall time beyond the frames'
/// real-time span at the actual rate), not raw elapsed - see the pump loop.
#[derive(Debug, Default)]
pub struct PumpMetrics {
    /// Segments pumped.
    pub segments: AtomicU64,
    /// Frames pumped.
    pub frames: AtomicU64,
    /// SchedulingFaults the service has counted (retried or not).
    pub scheduling_faults: AtomicU64,
    /// Every segment's wall-clock duration, microseconds. Kept in full rather than as
    /// a running quantile: a 30-minute soak at ~15 ms is ~120k samples, which is a
    /// megabyte, and an exact p99 beats an approximate one when the number is a gate.
    pub latencies_us: Mutex<Vec<u64>>,
}

impl PumpMetrics {
    fn record(&self, dur: Duration, frames: u64, faults: u64) {
        self.segments.fetch_add(1, Ordering::Relaxed);
        self.frames.fetch_add(frames, Ordering::Relaxed);
        self.scheduling_faults.store(faults, Ordering::Relaxed);
        if let Ok(mut v) = self.latencies_us.lock() {
            v.push(dur.as_micros() as u64);
        }
    }

    /// The p99 segment latency, in seconds. `None` before any segment.
    pub fn p99_s(&self) -> Option<f64> {
        let v = self.latencies_us.lock().ok()?;
        if v.is_empty() {
            return None;
        }
        let mut sorted = v.clone();
        sorted.sort_unstable();
        // Nearest-rank p99: the smallest value at or above 99% of the samples. For
        // n < 100 that is the max, which is the honest answer for a short run rather
        // than an interpolated one that reads lower than anything observed.
        let idx = ((sorted.len() as f64 * 0.99).ceil() as usize).saturating_sub(1);
        Some(sorted[idx.min(sorted.len() - 1)] as f64 / 1e6)
    }
}

/// The daemon's recorder host.
pub struct RecorderHost {
    svc: Arc<Mutex<Option<Svc>>>,
    stop: Arc<AtomicBool>,
    pump: Mutex<Option<std::thread::JoinHandle<()>>>,
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    /// The provenance of the run currently configured, once one is.
    provenance: Mutex<Option<RunProvenance>>,
    /// Live metrics, shared with the pump thread.
    pub metrics: Arc<PumpMetrics>,
    data_root: std::path::PathBuf,
    /// The station this daemon was configured to build.
    kind: BackendKind,
    ni_devices: Vec<String>,
    /// Per-channel control groups, captured at configure so the channel SELECTORS can
    /// resolve `ChannelSel::Group` against the live table.
    groups: Mutex<Vec<(u32, Option<u32>)>>,
}

impl RecorderHost {
    /// A host bound to a station kind and a data root. Builds nothing until
    /// [`RecorderHost::configure`].
    ///
    /// `tx` is the daemon's ONE display bus, shared with every other module host, so
    /// a connection subscribes once and receives whichever module is live.
    pub fn new(
        kind: BackendKind,
        ni_devices: Vec<String>,
        data_root: std::path::PathBuf,
        tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    ) -> Self {
        RecorderHost {
            svc: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            pump: Mutex::new(None),
            tx,
            provenance: Mutex::new(None),
            metrics: Arc::new(PumpMetrics::default()),
            data_root,
            kind,
            ni_devices,
            groups: Mutex::new(Vec::new()),
        }
    }

    /// Subscribe to the display stream.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<DisplayBatch> {
        self.tx.subscribe()
    }

    /// `(channel_id, control_group)` for every enabled input, so a subscription's
    /// `ChannelSel::Group` resolves against the live table - which is the authority on
    /// group membership.
    pub fn channel_groups(&self) -> Vec<(u32, Option<u32>)> {
        self.groups.lock().expect("groups poisoned").clone()
    }

    /// The provenance of the configured run, if any.
    pub fn provenance(&self) -> Option<RunProvenance> {
        self.provenance.lock().expect("provenance poisoned").clone()
    }

    /// Build the station and the service.
    ///
    /// `lease_owner` is the peer holding the run lease, recorded IN the provenance
    /// ("run lease with owner in provenance").
    pub fn configure(
        &self,
        cfg: RecorderConfig,
        table: eigen_channel_table::ChannelTable,
        lease_owner: &str,
    ) -> Result<(), ProtocolError> {
        cfg.validate()
            .map_err(|m| ProtocolError::InvalidArgument { message: m })?;

        let n_input = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != eigen_channel_table::ChannelRole::Drive)
            .count()
            .max(1);

        let spec = StationSpec {
            sim: SimPlantSpec {
                max_input_channels: n_input as u32,
                ..SimPlantSpec::default()
            },
            ni: NiSelection {
                inputs: eigen_station::physical_from_table(&table).0,
                outputs: eigen_station::physical_from_table(&table).1,
                derive_inputs: n_input,
                // The recorder's synthetic drive row: monitor-only, but the engine
                // needs an output slot to write its silence into.
                derive_outputs: 1,
                iepe: false,
                restrict_devices: self.ni_devices.clone(),
            },
            ni_block_frames: eigen_hal::default_block_frames(cfg.acq.native_rate_hz),
        };

        // THE ONE CONSTRUCTION POINT. A refusal here is a typed refusal all the way
        // to the remote operator - never a Sim substitution.
        let station = try_build_station_backend(self.kind, &spec).map_err(|e| {
            tracing::warn!(target: "eigen::backendd", "station refused: {e}");
            ProtocolError::Refused {
                message: e.to_string(),
            }
        })?;
        tracing::info!(target: "eigen::backendd", "{}", station.provenance.log_line());

        let run_id = mint_run_id();
        let prov = RunProvenance::assemble("recorder", run_id, &station, lease_owner, now_unix());
        tracing::info!(target: "eigen::backendd", "{}", prov.log_line());

        let groups: Vec<(u32, Option<u32>)> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != eigen_channel_table::ChannelRole::Drive)
            .map(|c| (c.index, c.control_group))
            .collect();

        // Block policy is PER-STATION, exactly as `with_block_frames`'s own doc
        // demands: a hardware station passes `default_block_frames(fs)`; the
        // EIGEN-SIM plant path leaves the sim-natural default (block = 1), which
        // its tick is sized around - overriding THAT desynchronizes the engine
        // from the plant and the pump produces nothing (found the hard way; the
        // live pump test below stops it coming back). Leaving the hardware half
        // of the contract unimplemented was the second 8/8-probe failure
        // (2026-07-30): the recorder re-configures the NI task at its own block,
        // so block=1 made the AO buffer (lookahead+1 blocks) THREE FRAMES - the
        // device consumed it in ~58 us, output_space never opened, and the
        // engine's stall deadman fired 500 ms in.
        let native_rate = cfg.acq.native_rate_hz;
        let mut svc = RecorderService::with_backend(
            cfg,
            table,
            station.backend,
            station.station,
            self.data_root.clone(),
        )
        .map_err(|e| ProtocolError::Refused {
            message: e.to_string(),
        })?;
        if station.station.is_hardware() {
            svc = svc.with_block_frames(eigen_hal::default_block_frames(native_rate));
        }
        svc.configure().map_err(|e| ProtocolError::Refused {
            message: e.to_string(),
        })?;

        *self.svc.lock().expect("svc poisoned") = Some(svc);
        *self.provenance.lock().expect("provenance poisoned") = Some(prov);
        *self.groups.lock().expect("groups poisoned") = groups;
        Ok(())
    }

    /// Begin monitoring and start the pump thread.
    pub fn start_monitor(&self) -> Result<(), ProtocolError> {
        self.with_svc(|s| s.start_monitor().map_err(rec_err))?;

        let mut slot = self.pump.lock().expect("pump poisoned");
        if slot.is_some() {
            return Ok(()); // already pumping
        }
        self.stop.store(false, Ordering::SeqCst);
        let svc = Arc::clone(&self.svc);
        let stop = Arc::clone(&self.stop);
        let metrics = Arc::clone(&self.metrics);
        let tx = self.tx.clone();
        let groups = self.channel_groups();
        let run_id = self
            .provenance()
            .and_then(|p| p.run_id.parse::<u64>().ok())
            .unwrap_or(0);

        *slot = Some(std::thread::spawn(move || {
            pump_loop(svc, stop, metrics, tx, groups, run_id)
        }));
        Ok(())
    }

    /// The driver-programmed rate of the live service, when one is up.
    pub fn actual_rate_hz(&self) -> Option<f64> {
        self.svc
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.actual_rate_hz()))
    }

    /// Arm the trigger.
    pub fn arm(&self) -> Result<(), ProtocolError> {
        self.with_svc(|s| s.arm().map_err(rec_err))
    }
    /// Begin recording now.
    pub fn record_now(&self) -> Result<(), ProtocolError> {
        self.with_svc(|s| s.record_now().map_err(rec_err))
    }
    /// End the current recording.
    pub fn stop_recording(&self) -> Result<(), ProtocolError> {
        self.with_svc(|s| s.stop_recording().map_err(rec_err))
    }

    /// Leave every live state and join the pump.
    pub fn stop_monitor(&self) -> Result<(), ProtocolError> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
        self.with_svc(|s| s.stop_monitor().map_err(rec_err))
    }

    /// THE SAFE-STOP PATH, invoked when the controller times out.
    ///
    /// It is `stop_monitor` - the recorder's CONTROLLED end - and not an abort or an
    /// `emergency_stop`. The recorder never drives, so its controlled stop is simply
    /// leaving the live state; the distinction still matters, because this is the
    /// function every future module's timeout path will be routed through, and the
    /// rule ("nobody is watching" is a reason to stop safely, never to slam the drive
    /// to zero) has to be written where that routing happens.
    ///
    /// Never fails: a safe-stop that refuses because the service is already idle
    /// would turn a timeout into an error the operator has to interpret.
    pub fn safe_stop(&self, reason: &str) {
        tracing::warn!(target: "eigen::backendd", reason, "SAFE-STOP: ramping the run down");
        if let Err(e) = self.stop_monitor() {
            tracing::warn!(target: "eigen::backendd", "safe-stop: {e}");
        }
    }

    /// Is a run live right now?
    pub fn is_live(&self) -> bool {
        self.svc
            .lock()
            .expect("svc poisoned")
            .as_ref()
            .is_some_and(|s| s.state().is_live())
    }

    /// Is a service BUILT - i.e. does this host hold a station, live or not?
    ///
    /// The `Claim::Configured` half of [`crate::core::Core::live_module`]. Distinct
    /// from [`RecorderHost::is_live`]: `configure` builds the station and mints the
    /// provenance, so from that moment the host owns hardware and has something to say
    /// about the data - but nothing is being recorded and nothing needs stopping.
    pub fn is_configured(&self) -> bool {
        self.svc.lock().expect("svc poisoned").as_ref().is_some()
    }

    /// The status JSON a remote status poll answers with.
    pub fn status_json(&self) -> String {
        let free = eigen_capture::free_space_bytes_for(&self.data_root).unwrap_or(0);
        let g = self.svc.lock().expect("svc poisoned");
        match g.as_ref() {
            Some(s) => {
                let st = s.status(free);
                serde_json::json!({
                    "configured": true,
                    "status": st,
                    "sessionSeconds": s.session_seconds(),
                    "schedulingFaults": s.scheduling_faults(),
                })
                .to_string()
            }
            None => serde_json::json!({
                "configured": false,
                "state": RecState::Idle,
            })
            .to_string(),
        }
    }

    /// Per-channel meters as JSON.
    pub fn meters_json(&self) -> String {
        let g = self.svc.lock().expect("svc poisoned");
        let rows = g.as_ref().map(|s| s.meters()).unwrap_or_default();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into())
    }

    /// The live FRF snapshot as JSON.
    pub fn frf_json(&self) -> String {
        let mut g = self.svc.lock().expect("svc poisoned");
        let snap = g.as_mut().and_then(|s| s.frf());
        serde_json::to_string(&snap).unwrap_or_else(|_| "null".into())
    }

    /// The recordings this service knows about, as JSON.
    pub fn recordings_json(&self) -> String {
        let g = self.svc.lock().expect("svc poisoned");
        let rows = g
            .as_ref()
            .map(|s| s.recordings_list(None))
            .unwrap_or_default();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into())
    }

    fn with_svc<T>(
        &self,
        f: impl FnOnce(&mut Svc) -> Result<T, ProtocolError>,
    ) -> Result<T, ProtocolError> {
        let mut g = self.svc.lock().expect("svc poisoned");
        match g.as_mut() {
            Some(s) => f(s),
            None => Err(ProtocolError::Refused {
                message: "the recorder is not configured on this core; send \
                          Recorder(Configure) first"
                    .into(),
            }),
        }
    }
}

impl Drop for RecorderHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
    }
}

fn rec_err(e: eigen_recorder::service::RecorderError) -> ProtocolError {
    ProtocolError::Refused {
        message: e.to_string(),
    }
}

/// The pump loop. Owns the service for the length of one segment at a time.
fn pump_loop(
    svc: Arc<Mutex<Option<Svc>>>,
    stop: Arc<AtomicBool>,
    metrics: Arc<PumpMetrics>,
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    groups: Vec<(u32, Option<u32>)>,
    run_id: u64,
) {
    let mut window_seq: u32 = 0;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        let segment_frames: u64;
        let rate_hz: f64;
        let batch = {
            let mut g = match svc.lock() {
                Ok(g) => g,
                Err(_) => break,
            };
            let Some(s) = g.as_mut() else { break };
            if !s.state().is_live() {
                break;
            }
            let (df, f0) = (s_df(s), 0.0);
            let frames = match s.pump() {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(target: "eigen::backendd", "recorder pump faulted: {e}");
                    metrics.record(started.elapsed(), 0, s.scheduling_faults());
                    break;
                }
            };
            segment_frames = frames;
            rate_hz = s.actual_rate_hz();
            // The health metric is the segment's LAG MARGIN: wall time beyond
            // what the frames REPRESENT at the driver's actual rate. On a
            // driver-paced station raw elapsed ~= the segment's real-time span
            // by construction (the reads wait for the data), so raw elapsed
            // can never satisfy a keeping-up gate - the first real soak read
            // p99 326 ms against a 250 ms budget while the true processing
            // margin was ~6 ms (2026-07-30). Lag is the number the -200279
            // failure mode actually punishes.
            let lag = if rate_hz > 0.0 {
                started
                    .elapsed()
                    .saturating_sub(Duration::from_secs_f64(frames as f64 / rate_hz))
            } else {
                started.elapsed()
            };
            metrics.record(lag, frames, s.scheduling_faults());

            let mut traces: Vec<(u32, Option<u32>, Arc<Vec<f32>>)> = Vec::new();
            for (slot, mags) in s.spectra() {
                let ch = groups
                    .get(slot)
                    .map(|(id, _)| *id)
                    .unwrap_or(slot as u32 + 1);
                let group = groups.get(slot).and_then(|(_, g)| *g);
                traces.push((
                    ch,
                    group,
                    Arc::new(mags.iter().map(|v| *v as f32).collect()),
                ));
            }
            if let Some(frf) = s.frf() {
                traces.push((
                    CH_FRF_MAG,
                    None,
                    Arc::new(frf.h1_mag.iter().map(|v| *v as f32).collect()),
                ));
                traces.push((
                    CH_FRF_COH,
                    None,
                    Arc::new(frf.coherence.iter().map(|v| *v as f32).collect()),
                ));
            }
            if traces.is_empty() {
                None
            } else {
                window_seq = window_seq.wrapping_add(1);
                Some(SpectrumBatch {
                    f0_hz: f0,
                    df_hz: df,
                    window_seq,
                    run_id,
                    traces,
                })
            }
        };

        if let Some(b) = batch {
            // A send with no subscribers is NOT an error and NOT a reason to stop:
            // the run belongs to the hardware host, and every remote peer
            // disconnecting is the client-vanish case the lease handles, not a
            // condition the pump reacts to.
            let _ = tx.send(DisplayBatch::Spectrum(b));
        }
        // RATE-LOCKED pacing (2026-07-27; second revision of this rule).
        //
        // v1 slept TICK unconditionally: throughput permanently below the device
        // rate (segment_time + TICK per segment) and the AI ring drained to
        // -200279 within seconds - found by the first cross-host wire probe
        // against the NI-SIM rig, 2026-07-26.
        //
        // v2 slept only on EMPTY segments: correct against a driver-paced
        // producer, but an UNPACED one (EIGEN-SIM synthesizes on demand, a
        // segment is never empty) spun this thread hot, hammered the service
        // mutex, and starved the session loop until its heartbeat declared a
        // LIVE controller dead - found by wire.rs
        // a_live_controller_is_never_timed_out, and bisected to the pacing change.
        //
        // v3: the frames just delivered are worth `frames/rate` seconds of wall
        // clock; sleep only the UNUSED remainder of that budget. A driver-paced
        // producer already spent the budget waiting on the driver (remainder
        // ~0), and a backlog drains at full speed (the -200279 fix stands). An
        // unpaced producer gets throttled to real time, which is the only
        // honest delivery rate a spectrum stream can claim anyway. Cap keeps
        // the stop flag responsive.
        const PACE_CHUNK: Duration = Duration::from_millis(100);
        if segment_frames == 0 {
            std::thread::sleep(TICK);
        } else if rate_hz > 0.0 {
            let budget = Duration::from_secs_f64(segment_frames as f64 / rate_hz);
            if let Some(mut remainder) = budget.checked_sub(started.elapsed()) {
                // The WHOLE remainder is slept (capping it would leak the
                // unslept part and the pump would outrun real time again -
                // caught by the_pump_never_outruns_real_time_on_an_unpaced_
                // station on first run); chunked so the stop flag stays
                // responsive during a long pace.
                while remainder > Duration::ZERO && !stop.load(Ordering::SeqCst) {
                    let nap = remainder.min(PACE_CHUNK);
                    std::thread::sleep(nap);
                    remainder = remainder.saturating_sub(nap);
                }
            }
        }
    }
}

/// The analysis grid's bin spacing for a live service.
fn s_df(_s: &Svc) -> f64 {
    // The service does not expose its config; the grid is recomputed by the caller
    // from the config it sent. Kept as a function so the one place that needs to
    // change when the service does expose it is here.
    //
    // Until then the daemon reports the DEFAULT analysis grid, and the sidecar/status
    // carry the real acquisition config so a consumer is never left inferring it.
    eigen_recorder::acq::AcquisitionConfig::default().df_hz()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// p99 is NEAREST-RANK: for a short run it is the max, not an interpolated value
    /// that reads lower than anything actually observed. A soak gate that reported a
    /// latency nobody measured would be worse than no gate.
    #[test]
    fn the_p99_is_nearest_rank_and_never_below_an_observed_sample() {
        let m = PumpMetrics::default();
        assert_eq!(m.p99_s(), None, "no samples, no claim");

        for us in [1_000u64, 2_000, 3_000, 50_000] {
            m.record(Duration::from_micros(us), 100, 0);
        }
        // 4 samples: ceil(4 * 0.99) - 1 = 3 -> the max.
        assert_eq!(m.p99_s(), Some(0.05));
        assert_eq!(m.segments.load(Ordering::Relaxed), 4);
        assert_eq!(m.frames.load(Ordering::Relaxed), 400);

        // With 100 samples the p99 is the 99th, not the 100th - so one outlier does
        // not define the number, which is the property that makes p99 useful.
        let m = PumpMetrics::default();
        for i in 0..100u64 {
            m.record(Duration::from_micros(1_000 + i), 0, 0);
        }
        m.record(Duration::from_micros(999_999), 0, 0);
        let p99 = m.p99_s().unwrap();
        assert!(p99 < 0.9, "one outlier must not define p99: {p99}");
    }

    /// The fault count is the SERVICE's, mirrored - not something the daemon derives.
    /// A derived count would drift from the thing the gate is about.
    #[test]
    fn the_fault_count_mirrors_the_service() {
        let m = PumpMetrics::default();
        m.record(Duration::from_micros(1), 0, 0);
        assert_eq!(m.scheduling_faults.load(Ordering::Relaxed), 0);
        m.record(Duration::from_micros(1), 0, 3);
        assert_eq!(m.scheduling_faults.load(Ordering::Relaxed), 3);
    }

    /// THE PUMP ACTUALLY PRODUCES TRACES on the EIGEN-SIM station.
    ///
    /// Without this, every wire test that waits for a display frame would be
    /// diagnosing the TRANSPORT when the PRODUCER was the problem - and "no frames
    /// arrived", two layers up and across a socket, is one of the hardest failures in
    /// this system to read. It is a LIVE test (the real engine over the real Sim
    /// plant) and deliberately bounded: one configure, one monitor, first batch, stop.
    #[test]
    fn the_sim_station_pump_publishes_spectra() {
        let root = std::env::temp_dir().join(format!(
            "eigen-backendd-pump-{}-{}",
            std::process::id(),
            now_unix()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (tx, _) = tokio::sync::broadcast::channel(64);
        let host = RecorderHost::new(BackendKind::Sim, Vec::new(), root.clone(), tx);
        let table = eigen_channel_table::ChannelTable::default_with_channels(4);
        let cfg = eigen_recorder::service::RecorderConfig::default();

        host.configure(cfg, table, "test").expect("configure");
        let mut rx = host.subscribe();
        host.start_monitor().expect("start");

        let mut got: Option<SpectrumBatch> = None;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(DisplayBatch::Spectrum(b)) => {
                    got = Some(b);
                    break;
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
        host.stop_monitor().ok();
        let _ = std::fs::remove_dir_all(&root);

        let b = got.expect(
            "the Sim pump published NO batch - the PRODUCER, not the transport, is \
             the problem",
        );
        assert!(!b.traces.is_empty());
        assert!(b.df_hz > 0.0, "the grid must be stated");
        // The ids are the TABLE's (1-based), not slot indices: a remote subscription
        // selects on the table's numbering, and off-by-one here would silently send
        // the wrong traces to a selective subscriber.
        for (ch, _, mags) in &b.traces {
            assert!(*ch >= 1, "channel ids are 1-based, got {ch}");
            assert!(!mags.is_empty());
        }
        assert!(
            host.metrics.segments.load(Ordering::Relaxed) >= 1,
            "the metrics must have counted the segment the batch came from"
        );
    }

    /// THE PUMP IS RATE-LOCKED: on an UNPACED producer it must not deliver faster
    /// than real time.
    ///
    /// EIGEN-SIM synthesizes on demand, so a pump that paces itself off "did the
    /// segment come up empty" (pacing v2) never sleeps: it spins a thread hot, hammers
    /// the service mutex, and starves the session loop until the daemon's heartbeat
    /// declares a LIVE controller dead (wire.rs a_live_controller_is_never_timed_out,
    /// bisected to the pacing change). This pins the producer-side property directly:
    /// frames delivered over a wall-clock window must not exceed real time by more than
    /// a startup margin. The v2 behavior delivers orders of magnitude more and reds
    /// this decisively; driver-paced stations are unaffected (their budget is already
    /// spent in the driver wait).
    #[test]
    fn the_pump_never_outruns_real_time_on_an_unpaced_station() {
        let root = std::env::temp_dir().join(format!(
            "eigen-backendd-pace-{}-{}",
            std::process::id(),
            now_unix()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (tx, _) = tokio::sync::broadcast::channel(64);
        let host = RecorderHost::new(BackendKind::Sim, Vec::new(), root.clone(), tx);
        let table = eigen_channel_table::ChannelTable::default_with_channels(2);
        let cfg = eigen_recorder::service::RecorderConfig::default();
        let rate = cfg.acq.native_rate_hz;
        assert!(rate > 0.0, "the default config must state a rate");

        host.configure(cfg, table, "test").expect("configure");
        host.start_monitor().expect("start");

        // Let the pump run for a fixed wall-clock window, then compare frames
        // against the real-time budget. The window starts BEFORE the first
        // segment, so the margin also absorbs pump start-up.
        let window = Duration::from_millis(1_200);
        std::thread::sleep(window);
        let frames = host.metrics.frames.load(Ordering::Relaxed);
        host.stop_monitor().ok();
        let _ = std::fs::remove_dir_all(&root);

        let budget = rate * window.as_secs_f64();
        assert!(
            frames >= 1,
            "no frames at all - the pump never ran, which this test cannot judge"
        );
        assert!(
            (frames as f64) <= budget * 1.5,
            "the pump outran real time on an unpaced station: {frames} frames \
             delivered in {:.2} s against a real-time budget of {budget:.0} \
             (rate {rate} Hz) - the rate-locked pacing (v3) has regressed",
            window.as_secs_f64()
        );
    }
}
