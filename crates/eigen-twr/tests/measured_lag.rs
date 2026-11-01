//! The measured pipeline lag against BOTH sysid excitations.
//!
//! # Why both, and why this is not one test with a loop
//!
//! The rule for a borrowed estimator: list the SIGNAL premises next to
//! the code and re-verify each against the borrowing module's own generator.
//! The xcorr lag measurement came from Random, whose pretest drive is a
//! band-limited noise burst. TWR has TWO generators and they do not share an
//! autocorrelation:
//!
//! * `Excitation::Random` - COLA-flat band-limited PSD, fixed seed. Broadband,
//!   near-impulsive autocorrelation; the shape the estimator was written for.
//! * `Excitation::LogChirp` - a log sweep. Its autocorrelation has real
//!   structure (a swept tone correlates with a shifted copy of itself wherever
//!   the instantaneous frequencies line up), so it is the one that would break
//!   a noise-only premise. It gets its own named test rather than a comment
//!   claiming it works.
//!
//! Both run through the REAL player against a delay double, not against the
//! estimator in isolation, so what is pinned is the number the alignment
//! actually uses.
//!
//! # RED-MUTATION
//!
//! Force `measure_lag_xcorr` to return `None` and both tests red on
//! `LagSource::DerivedNoPeak`; return `Some((lag+1, r))` and both red on the
//! frame count. Verified at the commit that added this file.

use std::collections::VecDeque;

use eigen_channel_table::{Channel, ChannelRole, ChannelTable};
use eigen_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner,
};
use eigen_twr::player::{LagSource, ReferencePlayer};
use eigen_twr::sysid::{Excitation, SysIdConfig};
use eigen_twr::BackendReferencePlayer;

const FS: f64 = 8192.0;
/// The delay the double imposes. Not a round number, and not a multiple of the
/// block: an estimator that quantized to blocks would pass on 128 and fail here.
const DELAY: usize = 173;

/// A pure delay + gain, with no output contract and no declared group delay -
/// so the DERIVED baseline is 0 and only a measurement can find the truth.
/// A double whose derived lag happened to be right would prove nothing.
struct DelayPlant {
    queue: VecDeque<f32>,
    delay_left: usize,
    gain: f32,
}

impl Backend for DelayPlant {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 2,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: None,
            sample_rate_hz: FS,
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
        self.queue.clear();
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.delay_left = DELAY;
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
                out[n] = s * self.gain;
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
        self.queue.extend(drive.iter().copied());
        Ok(drive.len())
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

fn player() -> BackendReferencePlayer<DelayPlant> {
    BackendReferencePlayer::new(
        DelayPlant {
            queue: VecDeque::new(),
            delay_left: 0,
            gain: 1.5,
        },
        table(),
        eigen_module_abi::StationClass::EigenSim,
    )
    .unwrap()
    // The pad is the measurement's search bound, so it must cover the delay.
    // On a real station the device's lookahead does this; here the block is
    // requested explicitly to say so out loud.
    .with_block_frames(512)
}

fn excitation(kind: Excitation) -> Vec<f64> {
    SysIdConfig {
        excitation: kind,
        drive_rms: 0.05,
        duration_s: 0.5,
        nfft: 512,
        f_lo_hz: 50.0,
        f_hi_hz: 2000.0,
        min_mean_coherence: 0.8,
    }
    .build_excitation(FS)
}

fn measures_the_delay(kind: Excitation) {
    let mut p = player();
    p.arm().unwrap();
    assert_eq!(
        p.pipeline_lag_frames(),
        0,
        "this double declares no contract and no group delay, so the DERIVED \
         baseline is 0 - only a measurement can find {DELAY}"
    );
    assert!(
        p.rest_pad_frames() > DELAY,
        "the pad bounds the search: {} frames cannot find a {DELAY}-frame delay",
        p.rest_pad_frames()
    );

    let drive = excitation(kind);
    let resp = p.play(&drive).unwrap().unwrap();

    assert_eq!(p.lag_source(), LagSource::Measured, "{kind:?}");
    assert_eq!(p.pipeline_lag_frames(), DELAY, "{kind:?} measured lag");
    assert_eq!(resp.len(), drive.len());
    for (k, (&d, &y)) in drive.iter().zip(resp.iter()).enumerate() {
        assert!(
            (y - 1.5 * d).abs() < 1e-6,
            "{kind:?} response[{k}] = {y} vs 1.5 * drive[{k}] = {}",
            1.5 * d
        );
    }
}

#[test]
fn cola_flat_noise_excitation_measures_the_delay() {
    measures_the_delay(Excitation::Random);
}

#[test]
fn log_chirp_excitation_measures_the_delay() {
    measures_the_delay(Excitation::LogChirp);
}

/// The floor is what keeps a search from reporting a number when there is no
/// peak to report ("a result that cannot fail is not evidence"). A plant whose
/// output is uncorrelated with the drive must leave the lag on its derived
/// baseline and SAY the measurement declined.
#[test]
fn an_uncorrelated_response_declines_to_measure() {
    struct NoisePlant {
        seed: u64,
    }
    impl Backend for NoisePlant {
        fn caps(&self) -> Caps {
            Caps {
                loop_owner: LoopOwner::HostClosesLoop,
                native_modes: vec![],
                clock: ClockDomain::Shared,
                max_input_channels: 2,
                max_output_channels: 1,
                abort_latency_bound: None,
                output_buffer: None,
                sample_rate_hz: FS,
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
            for slot in out.iter_mut() {
                self.seed = self
                    .seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *slot = ((self.seed >> 40) as f32 / (1u32 << 23) as f32) - 1.0;
            }
            Ok(InputRead::Frames(out.len()))
        }
        fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
            Ok(drive.len())
        }
        fn poll_status(&mut self) -> BackendStatus {
            BackendStatus::default()
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            Ok(())
        }
    }

    let mut p = BackendReferencePlayer::new(
        NoisePlant { seed: 0x5EED },
        table(),
        eigen_module_abi::StationClass::EigenSim,
    )
    .unwrap()
    .with_block_frames(512);
    p.arm().unwrap();
    p.play(&excitation(Excitation::Random)).unwrap();
    assert_eq!(
        p.lag_source(),
        LagSource::DerivedNoPeak,
        "an uncorrelated pair must decline, not pick the best noise lag"
    );
    assert_eq!(p.pipeline_lag_frames(), 0, "the baseline stands");
}
