//! The PER-SHOT COLD TASK shape, hardware-free.
//!
//! `HalPulsePlant` used to `start()` the device at ARM and keep it running for
//! the life of the run. Three defects came out of that on the real cDAQ-9178:
//!
//! 1. the backend's start-time silence prefill (it tops a short output buffer up
//!    with zeros so a non-regenerating AO can legally start) landed AHEAD of the
//!    pulse, so the acquired record began with a block of silence where the
//!    leading edge belonged - the 2026-08-08 baseline probe measured NO
//!    correlation peak at all, the whole 624-frame record sitting behind a
//!    2048-frame prefill;
//! 2. an operator pause between arm and fire filled the input ring with nobody
//!    reading it (`-200279`, sticky);
//! 3. the input read cursor started at ARM while the output write cursor started
//!    at each FIRE, so the measured record drifted from the pulse by the
//!    accumulated inter-pulse wall time.
//!
//! The fix is structural: bring-up PARKS, and each fire commits the whole record
//! BEFORE starting, plays, then stops and unreserves. This file asserts the three
//! properties that shape encodes, on a backend that records exactly what the
//! device saw and when.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner,
};
use hertzforge_srs::service::plant::{HalPulsePlant, PlantError, PulsePlant};
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;

/// A loopback backend that records the LIFECYCLE ORDER and every drive sample it
/// was actually given.
///
/// `cap_frames` models a bounded output buffer (`0` = unbounded), because the
/// interesting case for the pre-start commit is the one where the device cannot
/// take the whole record in one go and the remainder has to be re-offered in
/// order behind it.
struct Recorder {
    /// Lifecycle calls in order (writes and reads excluded - they are counted).
    calls: Vec<&'static str>,
    /// Every drive sample the device ACTUALLY received, in order.
    received: Vec<f32>,
    /// `received.len()` at the moment `start()` was called. The cold-task property.
    received_at_start: usize,
    /// Frames queued at the device, drained by reads.
    queued: VecDeque<f32>,
    cap_frames: usize,
    n_ch: usize,
}

impl Recorder {
    fn new(cap_frames: usize) -> Self {
        Recorder {
            calls: Vec::new(),
            received: Vec::new(),
            received_at_start: usize::MAX,
            queued: VecDeque::new(),
            cap_frames,
            n_ch: 1,
        }
    }
}

impl Backend for Recorder {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 4,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: None,
            sample_rate_hz: 8192.0,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.calls.push("open");
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.calls.push("close");
        Ok(())
    }
    fn configure(
        &mut self,
        _t: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.calls.push("configure");
        Ok(ConfiguredActual {
            actual_rate_hz: acq.requested_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.calls.push("arm");
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.calls.push("start");
        self.received_at_start = self.received.len();
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.calls.push("stop");
        // A stopped-and-unreserved task discards whatever it still held.
        self.queued.clear();
        Ok(())
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.calls.push("emergency_stop");
        self.queued.clear();
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let frames = (out.len() / self.n_ch).min(self.queued.len());
        if frames == 0 {
            return Ok(InputRead::NoDataYet);
        }
        for f in 0..frames {
            let d = self.queued.pop_front().unwrap_or(0.0);
            for c in 0..self.n_ch {
                out[f * self.n_ch + c] = d;
            }
        }
        Ok(InputRead::Frames(frames))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let offered = drive.len();
        let room = if self.cap_frames == 0 {
            offered
        } else {
            self.cap_frames.saturating_sub(self.queued.len())
        };
        let accepted = offered.min(room);
        for s in &drive[..accepted] {
            self.received.push(*s);
            self.queued.push_back(*s);
        }
        Ok(accepted)
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus::default()
    }
}

fn table_1ctrl() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    t.channels.push(c);
    let mut d = Channel::new(2);
    d.role = ChannelRole::Drive;
    t.channels.push(d);
    t
}

fn plant(cap_frames: usize) -> HalPulsePlant<Recorder> {
    HalPulsePlant::new(
        Recorder::new(cap_frames),
        table_1ctrl(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .expect("the table has a control channel")
    .with_block_frames(8)
}

/// The record the fire is handed, plus its zero ring-out, as the device should
/// see it.
fn record(pulse: &[f64], ring_out: usize) -> Vec<f32> {
    pulse
        .iter()
        .map(|&v| v as f32)
        .chain(std::iter::repeat_n(0.0f32, ring_out))
        .collect()
}

/// BRING-UP PARKS. `ensure_open` takes the device as far as Armed and no
/// further: nothing is clocked between arm and the first fire.
///
/// RED-MUTATION (verified by application): restore the `self.backend.start()?`
/// that `ensure_open` used to carry and this case reds - `start` appears in the
/// bring-up call list.
#[test]
fn a_ensure_open_verifies_bring_up_then_parks() {
    let mut p = plant(0);
    p.ensure_open().expect("bring-up");
    assert_eq!(
        p.backend().calls,
        vec!["open", "configure", "arm"],
        "bring-up must stop at arm: a started task between arm and fire is what \
         put the device's silence prefill ahead of the pulse"
    );
    assert_eq!(
        p.backend().received.len(),
        0,
        "nothing is written at bring-up"
    );
}

/// THE COLD-TASK PROPERTY. Every drive sample of the record is at the device BEFORE the
/// clock starts, so the first clocked sample is real drive and any silence the
/// backend adds at start lands BEHIND the record.
///
/// RED-MUTATION (verified by application): move the `self.backend.start()` call
/// above the pre-start commit loop in `HalPulsePlant::fire` and this case reds -
/// `received_at_start` drops to 0.
#[test]
fn b_the_whole_record_is_committed_before_the_clock_starts() {
    let mut p = plant(0); // unbounded output buffer: the whole record fits
    p.ensure_open().unwrap();
    let pulse: Vec<f64> = (1..=64).map(|i| i as f64).collect();
    let ring_out = 16usize;
    let want = record(&pulse, ring_out);

    let fired = p.fire(&pulse, ring_out, None).expect("the fire completes");

    assert_eq!(
        p.backend().received_at_start,
        want.len(),
        "the WHOLE record must be committed before start()"
    );
    assert_eq!(
        p.backend().received,
        want,
        "each sample reaches the device exactly once, in order"
    );
    assert_eq!(fired.control_g.len(), want.len());
    assert_eq!(
        p.backend().calls,
        vec!["open", "configure", "arm", "start", "stop"],
        "one arm/start/stop cycle per shot"
    );
}

/// The same, on a device whose output buffer is SMALLER than the record: the
/// pre-start commit takes what it can and the remainder is re-offered in order
/// behind it. The property that must hold either way is that the device receives
/// the record exactly once - the pre-start commit must not be replayed by the
/// block-wise play that follows it.
#[test]
fn c_a_bounded_output_buffer_still_receives_the_record_exactly_once() {
    const CAP: usize = 24;
    let mut p = plant(CAP);
    p.ensure_open().unwrap();
    let pulse: Vec<f64> = (1..=64).map(|i| i as f64).collect();
    let ring_out = 16usize;
    let want = record(&pulse, ring_out);
    assert!(want.len() > CAP, "setup invalid: the record must not fit");

    let fired = p.fire(&pulse, ring_out, None).expect("the fire completes");

    assert_eq!(
        p.backend().received_at_start,
        CAP,
        "the pre-start commit fills the buffer"
    );
    assert_eq!(
        p.backend().received,
        want,
        "no duplicated, dropped or re-ordered drive across the prefill seam"
    );
    assert_eq!(
        fired.control_g,
        want.iter().map(|&v| v as f64).collect::<Vec<_>>()
    );
}

/// Shot two re-arms. The task is unreserved by shot one's stop, so the next fire
/// takes the same known-good bring-up path rather than restarting a stopped but
/// still-reserved output (the measured zombie-AO, 2026-08-03).
#[test]
fn d_each_shot_is_its_own_arm_start_stop_cycle() {
    let mut p = plant(0);
    p.ensure_open().unwrap();
    let pulse: Vec<f64> = (1..=32).map(|i| i as f64).collect();
    p.fire(&pulse, 8, None).unwrap();
    p.fire(&pulse, 8, None).unwrap();
    assert_eq!(
        p.backend().calls,
        vec![
            "open",
            "configure",
            "arm",
            "start",
            "stop",
            // shot two: `ensure_open`'s arm was consumed by shot one, so this
            // fire arms for itself.
            "arm",
            "start",
            "stop"
        ]
    );
}

/// A CANCELLED fire leaves the task running and reports the committed queue
/// honestly - including the frames the pre-start commit put there, which the
/// stream helper alone cannot see.
#[test]
fn e_a_cancelled_fire_reports_the_pre_start_commit_and_does_not_park_itself() {
    let mut p = plant(0);
    p.ensure_open().unwrap();
    let pulse: Vec<f64> = (1..=64).map(|i| i as f64).collect();
    let ring_out = 16usize;
    let flag = AtomicBool::new(true); // already cancelled when the play begins

    let err = p
        .fire(&pulse, ring_out, Some(&flag))
        .expect_err("the flag is set");
    match err {
        PlantError::Cancelled {
            frames_done,
            frames_committed,
            frames_expected,
        } => {
            assert_eq!(frames_done, 0, "cancelled before any block was delivered");
            assert_eq!(frames_expected, pulse.len() + ring_out);
            assert_eq!(
                frames_committed,
                pulse.len() + ring_out,
                "the committed queue must include the PRE-START commit; reporting 0 \
                 here would understate what keeps driving after the flag flips"
            );
        }
        other => panic!("wrong error: {other:?}"),
    }
    assert!(
        !p.backend().calls.contains(&"stop"),
        "the fire path must NOT choose the teardown - the caller does (stop vs \
         emergency_stop), and pre-empting it would override an abort's taper"
    );

    // The caller's park is what stops it, and it is reachable from here.
    PulsePlant::stop(&mut p).expect("park");
    assert_eq!(p.backend().calls.last(), Some(&"stop"));
}

/// `emergency_stop` is a distinct verb on the seam, not an alias for `stop`.
#[test]
fn f_emergency_stop_reaches_the_backend_as_itself() {
    let mut p = plant(0);
    p.ensure_open().unwrap();
    PulsePlant::emergency_stop(&mut p).expect("emergency park");
    assert_eq!(p.backend().calls.last(), Some(&"emergency_stop"));
}

/// Rate adoption at the plant seam: `fs_hz()` reports the REQUESTED rate until bring-up and
/// the CONFIGURED rate after. The recorder configures at whatever was requested,
/// so this case pins the accessor contract; the coercion case lives in the
/// service suite where the consequences are observable.
#[test]
fn g_fs_hz_reports_the_configured_rate_after_bring_up() {
    let mut p = plant(0);
    let requested = p.fs_hz();
    p.ensure_open().unwrap();
    assert_eq!(p.requested_fs_hz(), requested);
    assert_eq!(
        PulsePlant::fs_hz(&p),
        requested,
        "this backend honors the request; the accessor still reads it back from \
         ConfiguredActual rather than from caps()"
    );
}
