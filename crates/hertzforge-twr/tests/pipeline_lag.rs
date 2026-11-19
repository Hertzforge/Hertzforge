//! The drive/response alignment against a backend that really HAS a pipeline
//! lag - the one shape every other `hertzforge-twr` fixture is missing.
//!
//! # Why this file exists
//!
//! Every hand-written backend double in the crate declared `output_buffer:
//! None` and answered `group_delay_frames: None`, so `pipeline_lag_frames`
//! computed to `0 + 0` and the whole alignment block - the pad past the record,
//! the frames dropped off the response head - was DEAD CODE in the test suite.
//! The alignment is what the FRF, the coherence gate and the inverse update all
//! assume; a change that moved the lag would land green.
//!
//! The fixture landed FIRST, against the derived-lag shape, and MOVED here onto
//! the per-play cold task with the measured lag.
//! What it pins changed with it, and the change is the point: on a cold task
//! the derived formula is WRONG - there is no arm-time silence prefill ahead of
//! the record any more, so `min_lookahead + group_delay` over-states the delay
//! by the whole lookahead. `derived_lag_over_states_the_cold_task_delay` is
//! that statement as an assertion.
//!
//! # RED-MUTATION (run it before believing this file)
//!
//! Any one of these must red `aligned_response_survives_a_real_pipeline_lag`:
//!
//! * delete `response.drain(..lag)` in `player.rs`;
//! * shorten the rest pad to zero (the response runs out before the delayed
//!   record has finished arriving);
//! * skip the measurement and keep the derived baseline (the response is then
//!   trimmed by 19 frames where the plant delays 7).
//!
//! Verified red on all three at the commit that moved this file.

use std::collections::VecDeque;

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use hertzforge_twr::player::{LagSource, ReferencePlayer};
use hertzforge_twr::BackendReferencePlayer;

/// Frames the device wants queued before it will start (the contract the
/// derived formula reads).
const LOOKAHEAD: u32 = 12;
/// Frames the converter's decimation filter delays the INPUT by. On a cold
/// task this is the WHOLE delay - the truth the measurement must find.
const GROUP_DELAY: u32 = 7;
/// The device's output buffer, frames. Smaller than the records below on
/// purpose: most of every record STREAMS after `start`, which is the real TWR
/// shape and not an edge case.
const OUT_BUFFER: usize = 256;
const GAIN: f32 = 2.0;

/// A loopback with a real output buffer and a real converter delay, on a task
/// that is COLD at every play.
///
/// The lifecycle is modelled where it bites:
///
/// * `arm()` re-commits the task from parked, so the output buffer is EMPTY -
///   nothing survives from the previous play;
/// * `start()` tops the buffer up with SILENCE if the caller queued less than
///   `LOOKAHEAD` (what an NI AO does, and what makes an uncommitted record play
///   late behind a lookahead of nothing);
/// * the converter delay is re-armed at every `start()`, because a cold input
///   pipeline starts empty.
struct ColdTaskLoopback {
    queue: VecDeque<f32>,
    /// Frames of converter delay still to be emitted before real data.
    delay_left: usize,
    /// Silence frames `start()` had to insert. A non-zero value on a play whose
    /// record is longer than the lookahead means the commit did nothing.
    start_silence: usize,
    /// What `output_space()` answers before the first write of a play. `true`
    /// reproduces the failed-query posture: the device query failed and the backend
    /// renders it `Some(0)`.
    space_unknown_first: bool,
    first_write_done: bool,
    opens: usize,
    configures: usize,
    frames_accepted: usize,
}

impl ColdTaskLoopback {
    fn new() -> Self {
        ColdTaskLoopback {
            queue: VecDeque::new(),
            delay_left: 0,
            start_silence: 0,
            space_unknown_first: false,
            first_write_done: false,
            opens: 0,
            configures: 0,
            frames_accepted: 0,
        }
    }
}

impl Backend for ColdTaskLoopback {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 2,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: LOOKAHEAD,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: 1000.0,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.opens += 1;
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn configure(
        &mut self,
        _t: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.configures += 1;
        Ok(ConfiguredActual {
            actual_rate_hz: acq.requested_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: Some(GROUP_DELAY),
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.queue.clear();
        self.first_write_done = false;
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        let short = (LOOKAHEAD as usize).saturating_sub(self.queue.len());
        for _ in 0..short {
            self.queue.push_front(0.0);
        }
        self.start_silence = short;
        self.delay_left = GROUP_DELAY as usize;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let mut n = 0usize;
        while n < out.len() {
            if self.delay_left > 0 {
                out[n] = 0.0;
                self.delay_left -= 1;
            } else if let Some(s) = self.queue.pop_front() {
                out[n] = s * GAIN;
            } else {
                break;
            }
            n += 1;
        }
        if n == 0 {
            return Ok(InputRead::NoDataYet);
        }
        Ok(InputRead::Frames(n))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let free = OUT_BUFFER.saturating_sub(self.queue.len());
        let accepted = drive.len().min(free);
        self.queue.extend(drive[..accepted].iter().copied());
        self.frames_accepted += accepted;
        self.first_write_done = true;
        Ok(accepted)
    }
    fn output_space(&mut self) -> Option<usize> {
        if self.space_unknown_first && !self.first_write_done {
            // The device query failed; the backend renders that `Some(0)`.
            return Some(0);
        }
        Some(OUT_BUFFER.saturating_sub(self.queue.len()))
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus::default()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    t.channels.push(drv);
    t
}

/// A deterministic broadband record. Broadband because the lag measurement
/// needs an autocorrelation with a sharp peak, and a smooth waveform correlates
/// almost as well one frame off as at the truth.
fn broadband(n: usize, seed: u64) -> Vec<f64> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as f64 / (1u64 << 30) as f64) - 1.0
        })
        .collect()
}

fn player() -> BackendReferencePlayer<ColdTaskLoopback> {
    BackendReferencePlayer::new(
        ColdTaskLoopback::new(),
        table(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap()
}

#[test]
fn derived_lag_over_states_the_cold_task_delay() {
    let mut p = player();
    p.arm().unwrap();
    // The baseline the first play runs on, and it is a PREDICTION.
    assert_eq!(
        p.pipeline_lag_frames(),
        LOOKAHEAD as usize + GROUP_DELAY as usize
    );
    assert_eq!(p.lag_source(), LagSource::Derived);

    p.play(&broadband(1024, 7)).unwrap();

    // ...and the measurement finds the truth: on a cold task there is no
    // silence prefill ahead of the record, so the lookahead term is not part of
    // the delay at all. Derived 19, measured 7.
    assert_eq!(p.lag_source(), LagSource::Measured);
    assert_eq!(p.pipeline_lag_frames(), GROUP_DELAY as usize);
}

#[test]
fn aligned_response_survives_a_real_pipeline_lag() {
    let mut p = player();
    p.arm().unwrap();
    // Play 1 (the sysid record's role) both measures the lag and is aligned by
    // it - the measurement happens before the trim, on the same raw record.
    let drive = broadband(1024, 11);
    let resp = p.play(&drive).unwrap().unwrap();

    assert_eq!(resp.len(), drive.len());
    for (k, (&d, &y)) in drive.iter().zip(resp.iter()).enumerate() {
        assert!(
            (y - GAIN as f64 * d).abs() < 1e-5,
            "response[{k}] = {y} pairs with drive[{k}] = {d}; a shift here is the \
             FRF/coherence/inverse pairing breaking silently"
        );
    }
    // Most of the record streamed after start: the underrun class is REAL for
    // TWR and the commit only covers the head.
    assert_eq!(p.last_committed_frames(), OUT_BUFFER);
    assert!(
        drive.len() + p.rest_pad_frames() > 4 * OUT_BUFFER,
        "this fixture is only honest while the record outruns the buffer"
    );
}

#[test]
fn alignment_holds_on_the_second_play_too() {
    let mut p = player();
    p.arm().unwrap();
    p.play(&broadband(1024, 11)).unwrap();
    let lag = p.pipeline_lag_frames();

    let drive = broadband(512, 29);
    let second = p.play(&drive).unwrap().unwrap();
    assert_eq!(p.pipeline_lag_frames(), lag, "the lag moved between plays");
    assert_eq!(second.len(), drive.len());
    for (k, (&d, &y)) in drive.iter().zip(second.iter()).enumerate() {
        assert!(
            (y - GAIN as f64 * d).abs() < 1e-5,
            "play 2 response[{k}] = {y} vs drive[{k}] = {d}"
        );
    }
}

/// A device that cannot answer the space query reports `Some(0)`, and on
/// a just-armed task that is the UNKNOWN, not "full". Read as full, the commit
/// becomes a no-op, `start()` inserts a whole lookahead of silence, and the
/// record plays behind it.
#[test]
fn a_zero_space_report_before_the_first_write_does_not_stop_the_commit() {
    let mut p = BackendReferencePlayer::new(
        ColdTaskLoopback {
            space_unknown_first: true,
            ..ColdTaskLoopback::new()
        },
        table(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    p.arm().unwrap();
    let drive = broadband(1024, 5);
    let resp = p.play(&drive).unwrap().unwrap();
    assert!(
        p.last_committed_frames() > 0,
        "nothing was committed before start(); `Some(0)` was read as full"
    );
    assert_eq!(
        p.backend().start_silence,
        0,
        "start() had to pad with silence, so the record plays late behind it"
    );
    for (&d, &y) in drive.iter().zip(resp.iter()) {
        assert!((y - GAIN as f64 * d).abs() < 1e-5);
    }
}
