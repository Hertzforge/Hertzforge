//! SRS must never record a STALE frame as a measurement, hardware-free.
//!
//! The old `HalPulsePlant::fire` loop wrote
//! `self.backend.read_input(&mut frame).map_err(...)?;` - discarding the
//! `InputRead` variant entirely - and then pushed `frame` into the response
//! record unconditionally. On `NoDataYet` the backend writes NOTHING into the
//! buffer, so `frame` still held the PREVIOUS frame's samples and the loop
//! recorded them again as if they were a fresh measurement.
//!
//! That is the worst shape a data bug can take on a shock pulse: a duplicated
//! sample is indistinguishable from a real one downstream, it stretches the pulse
//! in time, and the SRS is computed from exactly the peak it distorts.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner,
};
use hertzforge_srs::service::plant::{HalPulsePlant, PulsePlant};
use std::collections::VecDeque;

/// A gain-2 loopback that answers `NoDataYet` on every Nth poll: a driver whose
/// block simply is not ready yet, which is a NORMAL steady state, not a fault.
struct HiccupBackend {
    buffered: VecDeque<f32>,
    polls: usize,
    /// Every `hiccup_every`-th poll returns `NoDataYet` with the buffer untouched.
    hiccup_every: usize,
    n_ch: usize,
}

impl Backend for HiccupBackend {
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
        Ok(ConfiguredActual {
            actual_rate_hz: acq.requested_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.polls += 1;
        if self.hiccup_every > 0 && self.polls.is_multiple_of(self.hiccup_every) {
            // NOTHING is written into `out`. Whatever the caller left there is
            // last block's data.
            return Ok(InputRead::NoDataYet);
        }
        let frames = (out.len() / self.n_ch).min(self.buffered.len());
        if frames == 0 {
            return Ok(InputRead::NoDataYet);
        }
        for f in 0..frames {
            let d = self.buffered.pop_front().unwrap_or(0.0);
            for c in 0..self.n_ch {
                out[f * self.n_ch + c] = d * 2.0;
            }
        }
        Ok(InputRead::Frames(frames))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.buffered.extend(drive.iter().copied());
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus::default()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
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

/// RED-MUTATION: discard `read_input`'s `InputRead` again (record the frame
/// buffer unconditionally, i.e. restore
/// `self.backend.read_input(&mut frame).map_err(...)?;` followed by an
/// unconditional `control.push(frame[..])`) and this test reds - the record comes
/// back LONGER than the drive and carries duplicated samples where the backend
/// wrote nothing.
#[test]
fn a_no_data_yet_poll_never_becomes_a_duplicated_measurement() {
    let backend = HiccupBackend {
        buffered: VecDeque::new(),
        polls: 0,
        hiccup_every: 3, // every third poll is empty
        n_ch: 1,
    };
    let mut plant = HalPulsePlant::new(
        backend,
        table_1ctrl(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .expect("the table has a control channel")
    // A small block so many polls (and therefore many hiccups) happen.
    .with_block_frames(8);
    plant.ensure_open().expect("the plant opens");

    // A pulse whose every sample is DISTINCT, so a duplicate is detectable. A
    // constant record would hide the bug completely.
    let pulse: Vec<f64> = (1..=64).map(|i| i as f64).collect();
    let ring_out = 16usize;
    let fired = plant
        .fire(&pulse, ring_out, None)
        .expect("the fire completes");

    // LENGTH: exactly one response frame per drive frame, no more.
    assert_eq!(
        fired.control_g.len(),
        pulse.len() + ring_out,
        "a NoDataYet poll must not add a frame to the record"
    );
    // VALUES: the gain-2 loopback, in order, with the ring-out zeros after it.
    let expect: Vec<f64> = pulse
        .iter()
        .map(|d| d * 2.0)
        .chain(std::iter::repeat_n(0.0, ring_out))
        .collect();
    assert_eq!(
        fired.control_g, expect,
        "no duplicated, dropped or re-ordered samples"
    );
    // And the hiccups genuinely happened, so this cannot pass vacuously.
    assert!(
        plant.backend().polls > (pulse.len() + ring_out) / 8,
        "setup invalid: the backend never actually answered NoDataYet"
    );
}

/// The same play, with NO hiccups, is the control: it proves the assertions above
/// are testing the hiccup handling and not something incidental to the fixture.
#[test]
fn a_clean_stream_produces_the_same_record() {
    let backend = HiccupBackend {
        buffered: VecDeque::new(),
        polls: 0,
        hiccup_every: 0, // never hiccups
        n_ch: 1,
    };
    let mut plant = HalPulsePlant::new(
        backend,
        table_1ctrl(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap()
    .with_block_frames(8);
    plant.ensure_open().unwrap();

    let pulse: Vec<f64> = (1..=64).map(|i| i as f64).collect();
    let fired = plant.fire(&pulse, 16, None).unwrap();
    let expect: Vec<f64> = pulse
        .iter()
        .map(|d| d * 2.0)
        .chain(std::iter::repeat_n(0.0, 16))
        .collect();
    assert_eq!(fired.control_g, expect);
}
