//! Rate adoption (the device owns the rate) and the per-shot cold task against a
//! backend double that MODELS the two behaviours the real cDAQ-9178 has and the Sim
//! plant does not: it COERCES the requested sample rate, and its `start()` prefills
//! the output with SILENCE when the caller has queued less than one lookahead.
//!
//! Those two behaviours are the entire 2026-08-08 chassis baseline. Without them a
//! test cannot tell the fixed code from the broken code, which is why the
//! `eigen-hal-sim` fixtures in `service_sim.rs` stayed green through the whole
//! period when the chassis was playing an 11 ms pulse ~1.1 ms wide with the
//! measured record sitting entirely behind 2048 frames of arm-time silence.
//!
//! # The red-mutations this file is built around
//!
//! * **Delete the adoption** (`arm_inner` synthesizes at `self.cfg.fs_hz`
//!   instead of the adopted rate) ->
//!   `the_record_is_synthesized_at_the_device_rate_not_the_request` and
//!   `the_played_record_lasts_the_commanded_time_in_seconds` both red, the
//!   second by a factor of ten.
//! * **`start()` at arm** (the earlier shape) ->
//!   `arm_parks_both_tasks_and_starts_nothing` reds on the lifecycle log, and
//!   `the_measured_record_has_no_silence_in_front_of_the_pulse` reds because the
//!   lookahead silence lands in front of the record.
//! * **Drop the pre-start preload** ->
//!   `the_measured_record_has_no_silence_in_front_of_the_pulse` reds for the same
//!   reason (the silence top-up has nothing to sit behind).
//! * **Shorten the quiet lead-in back to a fixed sample count** ->
//!   `the_quiet_lead_in_is_time_denominated_and_the_tolerance_window_is_whole`
//!   reds on `checked_samples`.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_dsp::shock::{CompensationConfig, CompensationShape};
use eigen_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use eigen_shock_host::service::firer::quiet_lead_frames;
use eigen_shock_host::service::report::EventKind;
use eigen_shock_host::{
    check_tolerance, BackendPulseFirer, ClassicalPulseSpec, ClassicalShape, ClassicalShockService,
    ClassicalTestConfig, ServiceError, ToleranceStandard,
};
use std::sync::{Arc, Mutex};

/// What the PROFILE asks for (`ClassicalTestConfig`'s default, the delegated
/// controller's Shock rate).
const REQUESTED_HZ: f64 = 5120.0;
/// What the DEVICE runs at. Ten times the request, the same order of magnitude
/// the 9234 coerces by on the real chassis (5120 requested, 51 200 delivered).
const ACTUAL_HZ: f64 = 51_200.0;
/// Frames the double demands be queued at `start()`, topping up with SILENCE
/// when the caller queued less. `NiBackend`'s is `block * lookahead_blocks`.
const LOOKAHEAD: usize = 512;
/// Frames per block. Small so a fire has many blocks (the per-block observer row
/// counts them) without the record being large.
const BLOCK: usize = 64;
/// The commanded main-pulse duration.
const PULSE_MS: f64 = 11.0;
/// Rectangular compensation shoulders at this multiple of the main duration,
/// each side - so the composite is `1 + 2 * SHOULDER_FRAC` main durations long.
const SHOULDER_FRAC: f64 = 4.0;

// ---------------------------------------------------------------------------
// The double
// ---------------------------------------------------------------------------

/// Everything the test wants to read back after the firer has been moved into
/// the service.
#[derive(Default)]
struct Recorded {
    /// Every lifecycle call, in order.
    calls: Vec<&'static str>,
    /// Every drive frame the device ACCEPTED, in order - the wire timeline.
    ao: Vec<f32>,
    /// Frames the device accepted that have not yet been read back out.
    queue: std::collections::VecDeque<f32>,
    running: bool,
    /// Warnings the driver "reports", drained by `take_warnings`.
    warnings: Vec<String>,
    /// True while the output-space query cannot answer: the state a
    /// real DAQmx AO task is in between `TASK_UNRESERVE` (what `stop()` does)
    /// and the first write that re-commits it. `Backend::output_space` renders
    /// that as `Some(0)`, which is indistinguishable from "full" - and reading
    /// it as full is what made the pre-start commit a no-op on the chassis.
    /// Only the blind double sets it.
    space_query_blind: bool,
}

/// A backend that coerces the rate and silence-prefills at `start()`.
///
/// The input is the output, jumpered: `read_input` echoes the frames the device
/// accepted, in order. That is the bench wiring (AO0 -> AI0) and it is what makes
/// "is the measured record aligned with the pulse" a question this test can
/// actually answer.
struct RecordingBackend {
    rec: Arc<Mutex<Recorded>>,
    /// Output buffer depth, frames. Bigger than one lookahead so the preload
    /// and the top-up both fit, the way a real AO buffer is.
    capacity: usize,
    /// Model the unreserved-task space query: answer `Some(0)` from
    /// every `stop()` until the next accepted write. Off by default so the
    /// existing rows keep measuring what they were written to measure.
    blind_space: bool,
}

impl RecordingBackend {
    fn new() -> (Self, Arc<Mutex<Recorded>>) {
        Self::build(false)
    }

    /// The double with the blind-space behaviour: `output_space()` reports `Some(0)`
    /// on a parked task because the driver cannot answer, exactly as the real
    /// AO does between `TASK_UNRESERVE` and the write that re-commits it.
    fn new_blind() -> (Self, Arc<Mutex<Recorded>>) {
        Self::build(true)
    }

    fn build(blind_space: bool) -> (Self, Arc<Mutex<Recorded>>) {
        let rec = Arc::new(Mutex::new(Recorded {
            space_query_blind: blind_space,
            ..Recorded::default()
        }));
        (
            RecordingBackend {
                rec: Arc::clone(&rec),
                capacity: LOOKAHEAD * 8,
                blind_space,
            },
            rec,
        )
    }

    fn note(&self, what: &'static str) {
        self.rec.lock().expect("rec").calls.push(what);
    }
}

impl Backend for RecordingBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 4,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: LOOKAHEAD as u32,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            // The REQUEST the firer will make. `configure` answers with the
            // device's own rate, which is the whole point.
            sample_rate_hz: REQUESTED_HZ,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }

    fn open(&mut self) -> Result<(), HalError> {
        self.note("open");
        Ok(())
    }

    fn close(&mut self) -> Result<(), HalError> {
        self.note("close");
        Ok(())
    }

    fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.rec.lock().expect("rec").warnings)
    }

    fn configure(
        &mut self,
        _t: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.note("configure");
        assert!(
            (acq.requested_rate_hz - REQUESTED_HZ).abs() < 1e-9,
            "the firer must request the rate it read off caps(), got {}",
            acq.requested_rate_hz
        );
        Ok(ConfiguredActual {
            // THE COERCION.
            actual_rate_hz: ACTUAL_HZ,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }

    fn arm(&mut self) -> Result<(), HalError> {
        self.note("arm");
        Ok(())
    }

    /// Mirrors `NiBackend::start`: whatever is queued when this is called is the
    /// prefill, and a queue shorter than one lookahead is topped up with SILENCE
    /// - BEHIND what is already there.
    fn start(&mut self) -> Result<(), HalError> {
        self.note("start");
        let mut r = self.rec.lock().expect("rec");
        let queued = r.queue.len();
        if queued < LOOKAHEAD {
            for _ in queued..LOOKAHEAD {
                r.queue.push_back(0.0);
                r.ao.push(0.0);
            }
        }
        r.running = true;
        Ok(())
    }

    /// Mirrors the NI stop: tasks down, and the unreserve drops whatever was
    /// still queued.
    fn stop(&mut self) -> Result<(), HalError> {
        self.note("stop");
        let mut r = self.rec.lock().expect("rec");
        r.running = false;
        r.queue.clear();
        // UNRESERVE: the buffer is gone and, on the blind double, so is the
        // driver's ability to answer how much room it has.
        r.space_query_blind = self.blind_space;
        Ok(())
    }

    /// Jumpered: the input IS the output, one frame at a time, in order.
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let mut r = self.rec.lock().expect("rec");
        if !r.running {
            return Ok(InputRead::NoDataYet);
        }
        let n_ch = 1usize;
        let frames = (out.len() / n_ch).min(r.queue.len());
        if frames == 0 {
            return Ok(InputRead::NoDataYet);
        }
        for f in 0..frames {
            let v = r.queue.pop_front().unwrap_or(0.0);
            out[f * n_ch] = v;
        }
        Ok(InputRead::Frames(frames))
    }

    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let mut r = self.rec.lock().expect("rec");
        let free = self.capacity.saturating_sub(r.queue.len());
        let accepted = drive.len().min(free);
        for v in &drive[..accepted] {
            r.queue.push_back(*v);
            r.ao.push(*v);
        }
        if accepted > 0 {
            // The write re-commits the task, so the space query can answer again.
            r.space_query_blind = false;
        }
        Ok(accepted)
    }

    fn output_space(&mut self) -> Option<usize> {
        let r = self.rec.lock().expect("rec");
        if r.space_query_blind {
            // What `NiBackend::output_space` returns when the DAQmx query fails:
            // the conservative "do not top up", indistinguishable from FULL.
            return Some(0);
        }
        Some(self.capacity.saturating_sub(r.queue.len()))
    }

    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus::default()
    }

    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.note("emergency_stop");
        let mut r = self.rec.lock().expect("rec");
        r.running = false;
        r.queue.clear();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 100.0;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    t
}

fn spec() -> ClassicalPulseSpec {
    let mut s = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, PULSE_MS);
    s.compensation = CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: SHOULDER_FRAC,
        post_frac: SHOULDER_FRAC,
        symmetric: true,
    };
    s
}

type Svc = ClassicalShockService<BackendPulseFirer<RecordingBackend>>;

fn armed(tag: &str) -> (Svc, Arc<Mutex<Recorded>>) {
    armed_on(tag, RecordingBackend::new())
}

/// The same fixture over the blind-space double.
fn armed_blind(tag: &str) -> (Svc, Arc<Mutex<Recorded>>) {
    armed_on(tag, RecordingBackend::new_blind())
}

fn armed_on(
    tag: &str,
    built: (RecordingBackend, Arc<Mutex<Recorded>>),
) -> (Svc, Arc<Mutex<Recorded>>) {
    let (backend, rec) = built;
    let firer = BackendPulseFirer::new(backend, table(), eigen_module_abi::StationClass::EigenSim)
        .expect("valid table")
        .with_block_frames(BLOCK);
    let mut cfg = ClassicalTestConfig::new(spec());
    cfg.schedule = eigen_shock_common::ShockLevelSchedule::manual_default();
    // Judge the pulse against what was commanded, without aborting the fixture
    // on the double's perfectly-clean echo.
    cfg.abort_on_tolerance_failure = false;
    let mut svc = ClassicalShockService::new(
        cfg,
        table(),
        firer,
        Some(std::env::temp_dir().join(format!("eigen-d1d2-{tag}-{}", std::process::id()))),
    )
    .expect("service builds");
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    (svc, rec)
}

/// The quiet surround the service will have installed, in frames at the DEVICE
/// rate.
fn quiet_at_device() -> usize {
    quiet_lead_frames(PULSE_MS / 1000.0, ACTUAL_HZ)
}

// ---------------------------------------------------------------------------
// Rate adoption - the device owns the rate
// ---------------------------------------------------------------------------

/// The service adopts the DEVICE's rate and reports BOTH numbers.
#[test]
fn the_service_adopts_the_device_rate_and_reports_both() {
    let (svc, _rec) = armed("adopt");
    assert_eq!(svc.requested_fs_hz(), REQUESTED_HZ);
    assert_eq!(svc.actual_fs_hz(), Some(ACTUAL_HZ));
    assert_eq!(svc.fs_hz(), ACTUAL_HZ, "fs_hz() is the ADOPTED rate");

    let st = svc.status();
    assert_eq!(st.requested_fs_hz, REQUESTED_HZ);
    assert_eq!(st.actual_fs_hz, Some(ACTUAL_HZ));

    // The coercion is on the record, with both authorities named. This is the
    // check with two GENUINE authorities (the operator's request against the
    // device's answer); the arm gate's rate band cannot red post-adoption
    // because by then both of its sides are the adopted number.
    let coerced = svc.log().events().iter().find_map(|e| match &e.kind {
        EventKind::RateAdopted {
            requested_hz,
            actual_hz,
            relative_error,
        } => Some((*requested_hz, *actual_hz, *relative_error)),
        _ => None,
    });
    let (req, act, rel) = coerced.expect("a 10x coercion must be logged, not swallowed");
    assert_eq!(req, REQUESTED_HZ);
    assert_eq!(act, ACTUAL_HZ);
    assert!((rel - 0.9).abs() < 1e-9, "relative error {rel}");
}

/// **RED-MUTATION (a).** The record is synthesized at the ACTUAL rate, asserted
/// on SAMPLE COUNTS.
///
/// Restore `let comp = self.cfg.spec.synthesize(self.cfg.fs_hz);` in `arm_inner`
/// and the composite arrives ten times short.
#[test]
fn the_record_is_synthesized_at_the_device_rate_not_the_request() {
    let (mut svc, _rec) = armed("counts");
    assert!(svc.fire_next_pulse().expect("fire"));

    let store = svc.pulse_store();
    let ingested = store.records().last().expect("one record").clone();

    let at_device = spec().synthesize(ACTUAL_HZ).waveform_g.len();
    let at_request = spec().synthesize(REQUESTED_HZ).waveform_g.len();
    assert!(
        at_device > at_request * 9,
        "the fixture is only meaningful if the two rates give very different \
         sample counts: {at_device} vs {at_request}"
    );

    let quiet = quiet_at_device();
    assert_eq!(
        ingested.time_history.len(),
        at_device + 2 * quiet,
        "the measured record is quiet lead-in + composite + quiet lead-out, all \
         at the DEVICE rate ({ACTUAL_HZ} Hz), not the requested {REQUESTED_HZ} Hz"
    );
    // The store's own declared rate is the device's, so anything read back out
    // of it later denominates correctly too.
    assert_eq!(ingested.fs_hz, ACTUAL_HZ);
}

/// **RED-MUTATION (b).** The played record lasts the COMMANDED TIME.
///
/// The counts test above can be satisfied by any consistent rate; this one is
/// the physics. The composite is `1 + 2 x SHOULDER_FRAC` main durations long, so
/// at 11 ms and 4x shoulders it is 99 ms - and it must be 99 ms of the DEVICE's
/// clock, because the device is what plays it.
///
/// With the adoption deleted the composite is synthesized at 5120 Hz and played
/// at 51 200: 9.9 ms on the wire, an order of magnitude short. That is the
/// 2026-08-08 defect and this assertion is what refuses it.
#[test]
fn the_played_record_lasts_the_commanded_time_in_seconds() {
    let (mut svc, _rec) = armed("seconds");
    assert!(svc.fire_next_pulse().expect("fire"));

    let rec = svc
        .pulse_store()
        .records()
        .last()
        .expect("one record")
        .clone();
    let quiet = quiet_at_device();
    let composite_frames = rec.time_history.len() - 2 * quiet;

    // Duration in SECONDS, on the clock the record was actually played at.
    let composite_s = composite_frames as f64 / rec.fs_hz;
    let commanded_s = (PULSE_MS / 1000.0) * (1.0 + 2.0 * SHOULDER_FRAC);
    let one_sample = 4.0 / rec.fs_hz; // rounding slack, both shoulders + main
    assert!(
        (composite_s - commanded_s).abs() < one_sample,
        "the record the device played lasts {:.6} s; the operator commanded an \
         {PULSE_MS} ms pulse with {SHOULDER_FRAC}x shoulders, which is {:.6} s",
        composite_s,
        commanded_s
    );

    // And the quiet surround is real time too, not a sample count.
    let quiet_s = quiet as f64 / rec.fs_hz;
    assert!(
        quiet_s >= 0.005 - 1e-9,
        "the quiet lead-in is {quiet_s:.6} s, below the 5 ms floor"
    );
}

/// A fire path that will not say what rate it runs at REFUSES to arm.
///
/// The tempting default - fall back to the requested rate - is precisely the
/// requested-rate defect, so it is worth one row proving the fallback does not exist.
#[test]
fn a_firer_with_no_actual_rate_refuses_to_arm() {
    struct MuteRate;
    impl eigen_shock_host::PulseFirer for MuteRate {
        fn path(&self) -> eigen_shock_common::DrivePath {
            eigen_shock_common::DrivePath::HostDac
        }
        fn arm(&mut self) -> Result<(), ServiceError> {
            Ok(())
        }
        fn actual_fs_hz(&self) -> Option<f64> {
            None
        }
        fn fire(
            &mut self,
            _d: &[f64],
            _db: f64,
            _c: &mut eigen_shock_host::service::firer::FireControl<'_>,
        ) -> Result<Option<Vec<f64>>, ServiceError> {
            panic!("must never fire: arm refuses first")
        }
        fn stop(&mut self) -> Result<(), ServiceError> {
            Ok(())
        }
        fn emergency_stop(&mut self) -> Result<(), ServiceError> {
            Ok(())
        }
    }
    let mut svc = ClassicalShockService::new(
        ClassicalTestConfig::new(spec()),
        table(),
        MuteRate,
        Some(std::env::temp_dir().join("eigen-d1-muterate")),
    )
    .expect("service builds");
    svc.assess_feasibility().expect("feasibility");
    let err = svc.arm().expect_err("arming must refuse");
    let text = err.to_string();
    assert!(
        text.contains("no actual sample rate"),
        "the refusal must say WHY: {text}"
    );
}

// ---------------------------------------------------------------------------
// Per-shot cold task
// ---------------------------------------------------------------------------

/// `arm()` verifies bring-up and PARKS. Nothing runs between arm and fire.
///
/// RED-MUTATION: restore `self.backend.start()` in `BackendPulseFirer::arm` and
/// the lifecycle log grows a `start` before any fire.
#[test]
fn arm_parks_both_tasks_and_starts_nothing() {
    let (_svc, rec) = armed("park");
    let calls = rec.lock().expect("rec").calls.clone();
    assert_eq!(
        calls,
        vec!["open", "configure", "arm", "stop"],
        "arm must verify bring-up and PARK; a `start` here is the arm-time \
         silence prefill and the armed-idle input overflow, both measured on \
         2026-08-08"
    );
}

/// Every fire is its own cold arm -> start -> stop cycle, and a second fire
/// repeats it rather than riding the first one's tasks.
#[test]
fn every_fire_is_its_own_cold_task() {
    let (mut svc, rec) = armed("cold");
    rec.lock().expect("rec").calls.clear();

    assert!(svc.fire_next_pulse().expect("fire 1"));
    let after_one = rec.lock().expect("rec").calls.clone();
    assert_eq!(after_one.first(), Some(&"arm"));
    assert_eq!(after_one.last(), Some(&"stop"));
    assert_eq!(
        after_one.iter().filter(|c| **c == "start").count(),
        1,
        "exactly one start per shot: {after_one:?}"
    );

    assert!(svc.fire_next_pulse().expect("fire 2"));
    let after_two = rec.lock().expect("rec").calls.clone();
    assert_eq!(
        after_two.iter().filter(|c| **c == "arm").count(),
        2,
        "the second shot arms its own task: {after_two:?}"
    );
    assert_eq!(after_two.iter().filter(|c| **c == "start").count(), 2);
    assert_eq!(after_two.iter().filter(|c| **c == "stop").count(), 2);
}

/// **The cold-task payoff.** The measured record has NO silence in front of the pulse.
///
/// The double's `start()` tops the output up to one lookahead with silence, the
/// way `NiBackend` does. The preload puts the head of the record in first, so
/// the top-up lands BEHIND it and the first clocked output sample is the first
/// sample of the record.
///
/// The assertion is on WHERE THE PEAK IS. The composite is quiet lead-in, then a
/// `SHOULDER_FRAC x T` pre-shoulder, then the main pulse - so the peak belongs at
/// `quiet + pre_len`. Under the earlier shape it would sit `LOOKAHEAD` frames
/// later, which is what the 2026-08-08 baseline measured as a NO-PEAK verdict
/// (the whole record was behind the prefill and never appeared in the read
/// window at all).
#[test]
fn the_measured_record_has_no_silence_in_front_of_the_pulse() {
    let (mut svc, rec) = armed("align");
    assert!(svc.fire_next_pulse().expect("fire"));

    let comp = spec().synthesize(ACTUAL_HZ);
    let quiet = quiet_at_device();
    let expect_peak_at = quiet + comp.pre_len;

    let record = svc
        .pulse_store()
        .records()
        .last()
        .expect("one record")
        .time_history
        .clone();
    let (peak_idx, peak) = record
        .iter()
        .enumerate()
        .fold((0usize, 0.0f64), |(bi, bv), (i, &v)| {
            if v.abs() > bv.abs() {
                (i, v)
            } else {
                (bi, bv)
            }
        });
    assert!(peak.abs() > 1.0, "the fixture fired nothing: peak {peak}");

    // Within a couple of samples: the half-sine peak is flat near its crest, so
    // the argmax can land either side of the true centre.
    let crest = expect_peak_at + comp.main_len / 2;
    assert!(
        (peak_idx as i64 - crest as i64).abs() <= 2,
        "the pulse peak is at sample {peak_idx}; with the record aligned by \
         construction it belongs at {crest} (quiet {quiet} + pre-shoulder {} + \
         half the main pulse). A {LOOKAHEAD}-frame offset here is the arm-time \
         silence prefill sitting where the leading edge belongs.",
        comp.pre_len
    );

    // The wire timeline confirms it from the other side: the device's FIRST
    // accepted frame is the record's first frame, not a prefill zero.
    let ao = rec.lock().expect("rec").ao.clone();
    assert!(ao.len() > LOOKAHEAD);
    let first_nonzero = ao.iter().position(|v| v.abs() > 1e-9).expect("some drive");
    assert!(
        first_nonzero < LOOKAHEAD,
        "the first real drive sample reached the device at frame {first_nonzero}, \
         behind a whole {LOOKAHEAD}-frame lookahead of silence"
    );
}

/// A backend that cannot answer "how much room is left" must not be
/// read as a backend that has NO room - because the pre-start commit then
/// commits nothing and `start()` puts a whole lookahead of SILENCE where the
/// record's first frame belongs.
///
/// This is the row that was missing. The default double answers its space query
/// truthfully at every instant, so the whole adoption/cold-task suite stayed green
/// while the real chassis committed ZERO frames on every shot for the life of the
/// parked-task shape and every record played `LOOKAHEAD` frames late (measured
/// 2026-08-08: xcorr lag -2370 against a model of -321, residual -2049 = 2048 + 1, on
/// four commit depths that were all secretly zero). The mechanism: `stop()`
/// UNRESERVES the AO task, `DAQmxGetWriteSpaceAvail` fails on an unreserved
/// task, and `NiBackend::output_space` renders a failed query as `Some(0)` -
/// the deliberate conservative answer for a RUNNING task, and a lie about a
/// parked one whose buffer is empty by construction.
///
/// **RED-MUTATION:** restore `Some(0) => break` unconditionally in
/// `BackendPulseFirer::commit_whole_record` (delete the `done == 0` arm) and
/// this row reds on both assertions while every other row in the file stays
/// green - which is exactly what the chassis saw.
#[test]
fn a_blind_space_query_does_not_put_the_lookahead_silence_in_front_of_the_record() {
    let (mut svc, rec) = armed_blind("blindspace");
    assert!(svc.fire_next_pulse().expect("fire"));

    let comp = spec().synthesize(ACTUAL_HZ);
    let quiet = quiet_at_device();
    let crest = quiet + comp.pre_len + comp.main_len / 2;

    let record = svc
        .pulse_store()
        .records()
        .last()
        .expect("one record")
        .time_history
        .clone();
    let (peak_idx, peak) = record
        .iter()
        .enumerate()
        .fold((0usize, 0.0f64), |(bi, bv), (i, &v)| {
            if v.abs() > bv.abs() {
                (i, v)
            } else {
                (bi, bv)
            }
        });
    assert!(peak.abs() > 1.0, "the fixture fired nothing: peak {peak}");
    assert!(
        (peak_idx as i64 - crest as i64).abs() <= 2,
        "with a BLIND space query the pulse peak landed at sample {peak_idx}; it belongs at \
         {crest}. An offset of ~{LOOKAHEAD} here is start()'s silence prefill sitting in front \
         of a record the pre-start commit never committed."
    );

    // And from the wire side: the device's first accepted frame is the record's,
    // not a prefill zero.
    let ao = rec.lock().expect("rec").ao.clone();
    let first_nonzero = ao.iter().position(|v| v.abs() > 1e-9).expect("some drive");
    assert!(
        first_nonzero < LOOKAHEAD,
        "the first real drive sample reached the device at frame {first_nonzero}, behind a \
         whole {LOOKAHEAD}-frame lookahead of silence"
    );
}

/// The quiet lead-in is TIME-denominated, and the consequence that matters is
/// that the tolerance checker gets its WHOLE window.
///
/// **RED-MUTATION:** set the firer's quiet back to a fixed sample count (the
/// retired `DEFAULT_QUIET_SAMPLES = 200`) via `with_quiet_frames(200)` and
/// `checked_samples` falls below the full window - the checker silently examines
/// fewer samples, which is a green earned by not looking rather than by passing.
/// The two arms below are that mutation, run side by side.
#[test]
fn the_quiet_lead_in_is_time_denominated_and_the_tolerance_window_is_whole() {
    // A pulse whose compensation shoulders are SHORTER than the standard's
    // +/-0.4 T surround window, so the QUIET LEAD-IN is what protects the
    // window rather than the shoulders. With the 4x shoulders the rest of this
    // file uses, the window is buried inside them and the lead-in length would
    // not matter - which is exactly why a fixed 200 samples survived unnoticed.
    const LONG_MS: f64 = 40.0;
    let mut shallow = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, LONG_MS);
    shallow.compensation = CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: 0.25,
        post_frac: 0.25,
        symmetric: true,
    };

    let comp = shallow.synthesize(ACTUAL_HZ);
    let n_main = shallow.main_pulse(ACTUAL_HZ).len();
    // MIL-STD-810 516: +/-0.4 T each side. The full window is that plus the main.
    let window = (0.4 * n_main as f64).round() as usize * 2 + n_main;

    let record_with = |quiet: usize| -> usize {
        let mut r = vec![0.0f64; quiet];
        r.extend(comp.waveform_g.iter().copied());
        r.extend(std::iter::repeat_n(0.0f64, quiet));
        check_tolerance(&r, &shallow, ACTUAL_HZ, ToleranceStandard::Mil810_516)
            .amplitude_envelope
            .checked_samples
    };

    let time_denominated = quiet_lead_frames(LONG_MS / 1000.0, ACTUAL_HZ);
    assert_eq!(
        record_with(time_denominated),
        window,
        "the time-denominated lead-in ({time_denominated} frames = {:.4} ms at \
         {ACTUAL_HZ} Hz) must leave the checker its WHOLE {window}-sample window",
        time_denominated as f64 * 1000.0 / ACTUAL_HZ
    );

    // THE MUTATION, side by side: the retired fixed 200 samples is 3.9 ms at
    // this rate, short of the 4.4 ms the window needs, and the window clips.
    let fixed = 200usize;
    assert!(
        fixed < time_denominated,
        "this fixture only demonstrates anything if 200 samples is SHORTER than \
         the time-denominated lead-in at {ACTUAL_HZ} Hz"
    );
    assert!(
        record_with(fixed) < window,
        "a fixed {fixed}-sample lead-in must leave the window CLIPPED; if it does \
         not, this test is not guarding what it claims"
    );
}

// ---------------------------------------------------------------------------
// The per-block observer
// ---------------------------------------------------------------------------

/// The host's observer is called from INSIDE the fire, once per delivered block,
/// with a monotonically advancing frame count that ends at the record length.
#[test]
fn the_block_observer_fires_per_block_during_the_fire() {
    let (mut svc, _rec) = armed("observer");
    let beats: Arc<Mutex<Vec<(usize, usize)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&beats);
    svc.set_block_observer(Box::new(move |done, total| {
        sink.lock().expect("beats").push((done, total));
    }));

    assert!(svc.fire_next_pulse().expect("fire"));

    let seen = beats.lock().expect("beats").clone();
    let record_len = svc
        .pulse_store()
        .records()
        .last()
        .expect("one record")
        .time_history
        .len();

    assert!(
        seen.len() > 1,
        "the observer must beat DURING the fire, not once at the end: {} beat(s) \
         for a {record_len}-frame record at a {BLOCK}-frame block",
        seen.len()
    );
    assert!(
        seen.len() >= record_len / BLOCK,
        "expected about {} beats, got {}",
        record_len / BLOCK,
        seen.len()
    );
    for w in seen.windows(2) {
        assert!(w[1].0 > w[0].0, "frame count must advance: {seen:?}");
    }
    assert!(seen.iter().all(|(_, total)| *total == record_len));
    assert_eq!(seen.last().expect("beats").0, record_len);

    // And it is removable - a host that vanishes must not leave a tap behind.
    svc.clear_block_observer();
    let before = beats.lock().expect("beats").len();
    assert!(svc.fire_next_pulse().expect("fire 2"));
    assert_eq!(beats.lock().expect("beats").len(), before);
}

// ---------------------------------------------------------------------------
// The cancel reaches the real block-play loop
// ---------------------------------------------------------------------------

/// The service's cancel flag reaches `eigen_hal::play_block_wise` through
/// `BackendPulseFirer::fire`, and the fire path is still PARKED afterwards.
///
/// The unit-level cancel row in `service/mod.rs` proves the service's handling
/// with a hand-rolled firer double; this one proves the real firer actually hands
/// the flag to the stream helper. **RED-MUTATION:** pass `None` instead of
/// `cancel` in `BackendPulseFirer::fire`'s `play_block_wise` call and this reds -
/// the fire runs to completion and no `FireCancelled` event appears.
///
/// The flag is raised from inside the per-block observer so the cancel lands on a real
/// block boundary mid-record.
#[test]
fn the_cancel_flag_reaches_the_block_play_and_the_task_still_parks() {
    let (mut svc, rec) = armed("cancel");
    let flag = svc.cancel_flag();
    svc.set_block_observer(Box::new(move |done, _total| {
        if done >= BLOCK * 3 {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }));
    rec.lock().expect("rec").calls.clear();

    assert!(
        !svc.fire_next_pulse().expect("a cancel is not an error"),
        "a cancelled fire reports the run as no longer running"
    );

    let ev = svc
        .log()
        .events()
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::FireCancelled {
                frames_done,
                frames_expected,
                ..
            } => Some((*frames_done, *frames_expected)),
            _ => None,
        })
        .expect("the cancel must reach play_block_wise and come back typed");
    assert!(
        ev.0 >= BLOCK && ev.0 < ev.1,
        "the cancel landed at frame {} of {} - it must be mid-record",
        ev.0,
        ev.1
    );

    // A cancelled fire must not leave a started task behind it.
    let calls = rec.lock().expect("rec").calls.clone();
    assert_eq!(
        calls.last(),
        Some(&"stop"),
        "the task must be parked even on the cancel path: {calls:?}"
    );
    assert!(!rec.lock().expect("rec").running);
}
