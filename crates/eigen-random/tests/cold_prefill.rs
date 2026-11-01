//! A COLD output's first clocked sample is DRIVE, not silence.
//!
//! The defect: `PlantLoop::open` pre-wrote `lookahead_frames` of ZEROS and
//! started the AO clock before any drive existed, giving the host exactly
//! `lookahead / rate` (30 ms on the cDAQ) to deliver the first real write. At
//! nfft 32768 the work in that window did not fit and the AO underran its own
//! prefill (measured: samples-generated frozen at exactly 512).
//!
//! The fix: the clock starts in `run_segment`'s cold-path prologue, AFTER a
//! synthesized LEAD-IN's head is in the device buffer; the lead-in's remainder
//! seeds the delivery queue's low-water reserve, and the RECORD itself is
//! untouched (no cursor, full length) - the wire plays [lead-in][record] as
//! one continuous stream.
//!
//! RED-MUTATIONS for this file (each named check reds under one):
//!  * restore the zero pre-write in `open()`  -> `open_writes_nothing` reds;
//!  * start the clock in `open()`             -> `open_does_not_start` reds;
//!  * prefill zeros instead of the lead-in    -> `head_is_drive` reds;
//!  * drop the queue-reserve adoption         -> `wire_is_leadin_then_record`
//!    reds (zeros between lead-in and record);
//!  * skip the prefill on resume-after-suspend -> the resume half reds.
//!
//! The double models the CONTRACT (an output-buffer contract with a lookahead,
//! writes recorded in arrival order, start() stamped) and not the physics -
//! the underrun itself only the chassis can exhibit, and the bench probes own
//! that half.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
    OutputBufferContract, UnderrunPolicy,
};
use eigen_hal_sim::{SimBackend, SimConfig};
use eigen_module_abi::StationClass;
use eigen_random::service::engine::PlantLoop;
use eigen_random::LoopConfig;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LOOKAHEAD_FRAMES: u32 = 8;

#[derive(Default)]
struct WriteLog {
    /// Every write_drive payload in arrival order, with whether start() had run.
    writes: Vec<(bool, Vec<f32>)>,
    started: bool,
    start_calls: u32,
}

/// A Sim-backed station that DECLARES an output-buffer contract (the Sim itself
/// declares none) and records every drive write against the start() boundary.
struct RecordingStation {
    inner: SimBackend,
    log: Arc<Mutex<WriteLog>>,
}

impl RecordingStation {
    fn new(log: Arc<Mutex<WriteLog>>) -> Self {
        RecordingStation {
            inner: SimConfig::new()
                .sample_rate_hz(8192.0)
                .resonances(vec![(200.0, 2.5)])
                .noise_level(2e-4)
                .feedthrough(0.35)
                .shared_clock(true)
                .seed(0x50_50)
                .max_input_channels(1)
                .max_output_channels(1)
                .build(),
            log,
        }
    }
}

impl Backend for RecordingStation {
    fn caps(&self) -> Caps {
        Caps {
            abort_latency_bound: Some(Duration::from_millis(20)),
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: LOOKAHEAD_FRAMES,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            ..self.inner.caps()
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        let mut log = self.log.lock().unwrap();
        log.started = true;
        log.start_calls += 1;
        drop(log);
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let accepted = self.inner.write_drive(drive)?;
        let mut log = self.log.lock().unwrap();
        let started = log.started;
        log.writes.push((started, drive[..accepted].to_vec()));
        Ok(accepted)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}

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

/// A drive record whose every sample is nonzero and position-identifying, so a
/// zero prefill, a duplicated head, or a shifted cursor each produce a
/// distinct, visible mismatch.
fn ramp(len: usize) -> Vec<f64> {
    (0..len).map(|i| 1e-3 * (i as f64 + 1.0)).collect()
}

fn flat_writes(writes: &[(bool, Vec<f32>)]) -> Vec<f32> {
    writes.iter().flat_map(|(_, w)| w.iter().copied()).collect()
}

#[test]
fn cold_start_plays_drive_from_the_first_clocked_sample() {
    let log = Arc::new(Mutex::new(WriteLog::default()));
    let backend = Box::new(RecordingStation::new(Arc::clone(&log)));
    let mut cfg = LoopConfig::demo_default();
    let mut plant = PlantLoop::open(backend, StationClass::EigenSim, &mut cfg, table())
        .expect("open must succeed");

    // --- open_writes_nothing + open_does_not_start ---
    {
        let l = log.lock().unwrap();
        assert!(
            l.writes.is_empty(),
            "open() must not pre-write anything (it used to write a lookahead of zeros)"
        );
        assert!(
            !l.started,
            "open() must not start the output clock (the 30 ms deadline was born here)"
        );
    }

    let want = plant.cold_leadin_frames();
    assert_eq!(
        want,
        3 * LOOKAHEAD_FRAMES as usize,
        "a cold plant with a 1-output contract asks for three lookaheads of lead-in"
    );
    let leadin: Vec<f64> = (0..want).map(|i| 2e-3 * (i as f64 + 1.0)).collect();
    let drive = ramp(cfg.window_len());
    let seg = plant.run_segment(leadin.clone(), drive.clone());
    assert!(seg.faulted.is_none(), "segment faulted: {:?}", seg.faulted);

    let l = log.lock().unwrap();
    assert!(
        l.started && l.start_calls >= 1,
        "the segment must start the clock"
    );

    // --- head_is_drive: the pre-start writes are the LEAD-IN's head, verbatim,
    //     and nonzero (the Sim double accepts everything, so the whole lead-in
    //     lands pre-start) ---
    let pre_start: Vec<f32> = l
        .writes
        .iter()
        .take_while(|(started, _)| !started)
        .flat_map(|(_, w)| w.iter().copied())
        .collect();
    assert!(
        !pre_start.is_empty() && pre_start.len() <= leadin.len(),
        "the pre-start writes are a prefix of the lead-in ({} of {})",
        pre_start.len(),
        leadin.len()
    );
    for (i, v) in pre_start.iter().enumerate() {
        let want = leadin[i] as f32;
        assert_eq!(
            *v, want,
            "pre-start frame {i} must be leadin[{i}], got {v} vs {want}"
        );
        assert_ne!(
            *v, 0.0,
            "pre-start frame {i} is ZERO - the silence prefill is back"
        );
    }

    // --- wire_is_leadin_then_record: everything written is leadin then
    //     drive[0..], contiguous, each exactly once ---
    let all = flat_writes(&l.writes);
    assert!(all.len() > leadin.len());
    for (i, v) in all.iter().enumerate() {
        let want = if i < leadin.len() {
            leadin[i] as f32
        } else {
            drive.get(i - leadin.len()).copied().unwrap_or(0.0) as f32
        };
        assert_eq!(
            *v, want,
            "wire sample {i} diverges from [lead-in][record] - zeros or a \
             duplicated span show up exactly here"
        );
    }
}

#[test]
fn resume_after_suspend_prefills_the_next_segments_drive() {
    let log = Arc::new(Mutex::new(WriteLog::default()));
    let backend = Box::new(RecordingStation::new(Arc::clone(&log)));
    let mut cfg = LoopConfig::demo_default();
    let mut plant = PlantLoop::open(backend, StationClass::EigenSim, &mut cfg, table())
        .expect("open must succeed");

    let want = plant.cold_leadin_frames();
    let first_leadin: Vec<f64> = (0..want).map(|i| 2e-3 * (i as f64 + 1.0)).collect();
    let first = ramp(cfg.window_len());
    let seg = plant.run_segment(first_leadin, first);
    assert!(
        seg.faulted.is_none(),
        "segment 0 faulted: {:?}",
        seg.faulted
    );

    // The hold: stop + discontinuity, exactly what the engage seam, the
    // coherence hold and the review hold all do.
    plant.suspend();
    log.lock().unwrap().writes.clear();
    log.lock().unwrap().started = false;

    // A DIFFERENT lead-in, so a stale carry (the pre-suspend queue) or a zero
    // prefill are both distinguishable from the correct resume head.
    assert_eq!(
        plant.cold_leadin_frames(),
        3 * LOOKAHEAD_FRAMES as usize,
        "a suspended plant is cold again and asks for a fresh lead-in"
    );
    let second_leadin: Vec<f64> = (0..3 * LOOKAHEAD_FRAMES as usize)
        .map(|i| 3e-3 * (i as f64 + 1.0) + 0.25)
        .collect();
    let second: Vec<f64> = ramp(cfg.window_len()).iter().map(|v| v + 0.5).collect();
    let seg = plant.run_segment(second_leadin.clone(), second.clone());
    assert!(
        seg.faulted.is_none(),
        "resume segment faulted: {:?}",
        seg.faulted
    );

    let l = log.lock().unwrap();
    let pre_start: Vec<f32> = l
        .writes
        .iter()
        .take_while(|(started, _)| !started)
        .flat_map(|(_, w)| w.iter().copied())
        .collect();
    assert!(
        !pre_start.is_empty(),
        "the resume must prefill the fresh lead-in before restarting"
    );
    for (i, v) in pre_start.iter().enumerate() {
        assert_eq!(
            *v, second_leadin[i] as f32,
            "resume pre-start frame {i} must be the resume's own lead-in"
        );
    }
}
