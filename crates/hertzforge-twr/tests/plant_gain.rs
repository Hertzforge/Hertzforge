//! A plant whose gain is not 1, and a response that carries a constant offset.
//!
//! On the NI loopback (2026-11-08) a control channel declared at 500 mV/g on a
//! 1 V/g wire reads 2 g per volt. The identification measured it (band mean
//! 1.973), the seed drove the -12 dB rung at the right level (response peak
//! 0.0487 g against 0.0499 g) and every later play stayed at 10.7 to 12.1%
//! rms error: eight plays, `not_converged`. The same run declared at 1000 mV/g
//! gated every rung on its first play. In-band the correction worked (the
//! spectral error fell from 0.26 to 0.08 dB); 98% of the remaining error
//! energy was at 0 Hz, a 0.70 mV offset of the converters, which at 500 mV/g
//! is 1.39 mg, 9.6% of the -12 dB section's rms and 10.8% of the record's.
//! No correction acts at 0 Hz (every correction band starts above it), so an
//! error the law cannot reach decided the gate.
//!
//! These tests run the core with its control law (the built-in, as the wire
//! always sets) on a flat simulated plant of gain 2, with and without that
//! offset on the measured response.

mod common;

use common::{field_like_reference, player, FS};
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_twr::schedule::GateFailPolicy;
use hertzforge_twr::service::config::TwrConfig;
use hertzforge_twr::{RunSchedule, TwrControl, TwrService, TwrState};

/// The plant's gain: the response reads 2 EU per drive volt.
const GAIN: f64 = 2.0;

/// A flat plant of gain [`GAIN`]: no modes, the converters' delay, a noise
/// floor, on a fixed seed.
fn flat_gain_two() -> SimBackend {
    SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(Vec::new())
        .feedthrough(GAIN)
        .noise_level(2e-4)
        .transport_delay_samples(24)
        .shared_clock(true)
        .seed(0x71E5_1DE2_E001_0002)
        .build()
}

/// A station whose every measured sample carries a constant offset, as a
/// DC-coupled input chain does.
struct Offset<B: Backend> {
    inner: B,
    offset: f32,
}

impl<B: Backend> Backend for Offset<B> {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn take_warnings(&mut self) -> Vec<String> {
        self.inner.take_warnings()
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
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let r = self.inner.read_input(out)?;
        if let InputRead::Frames(n) = r {
            for v in out.iter_mut().take(n) {
                *v += self.offset;
            }
        }
        Ok(r)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(drive)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        self.inner.input_overloads()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}

/// The run as the wire builds it for a flat plant: the built-in law under the
/// control seam, the default ladder from -12 dB, a 10% gate with eight
/// iterations a rung, and a rung that does not gate ends the run.
fn config() -> TwrConfig {
    let mut c = TwrConfig::for_reference(field_like_reference());
    c.prep.highpass_hz = Some(20.0);
    c.prep.taper_frac = Some(0.05);
    c.pad_pre = 512;
    c.pad_post = 1024;
    c.sysid.drive_rms = 0.05;
    c.sysid.duration_s = 4.0;
    c.sysid.nfft = 2048;
    c.sysid.f_lo_hz = 25.0;
    c.sysid.f_hi_hz = 1500.0;
    c.inverse.tikhonov_beta_floor = 0.05;
    c.inverse.coherence_floor = 0.9;
    c.law.initial_drive_backoff_db = 0.0;
    c.law.ref_floor_db = 50.0;
    c.law.f_lo_hz = 25.0;
    c.law.f_hi_hz = 1400.0;
    c.law.min_masked_energy_frac = 0.9;
    c.schedule.table_db = vec![-12.0, -9.0, -6.0, -3.0, 0.0];
    c.schedule.gate_rms_error_pct = 10.0;
    c.schedule.max_iterations_per_level = 8;
    c.schedule.on_gate_fail = GateFailPolicy::Abort;
    c.run = RunSchedule::Once;
    c.control = Some(TwrControl::default());
    c
}

/// The rms of the -12 dB rung's level target over the reference's own
/// samples (its section): the scale the offset is set against.
fn rung_section_rms() -> f64 {
    let r = field_like_reference();
    let s = &r.samples;
    let rms = (s.iter().map(|v| v * v).sum::<f64>() / s.len() as f64).sqrt();
    rms * 10f64.powf(-12.0 / 20.0)
}

/// Run to the end: the identification, the review, then the ladder.
fn run(
    backend: Box<dyn Backend>,
) -> TwrService<hertzforge_twr::BackendReferencePlayer<Box<dyn Backend>>> {
    let mut svc = TwrService::new(player(backend), config()).unwrap();
    assert_eq!(svc.run_to_completion(10).unwrap(), TwrState::Review);
    svc.proceed().unwrap();
    svc.run_to_completion(200).unwrap();
    svc
}

fn table(svc: &TwrService<hertzforge_twr::BackendReferencePlayer<Box<dyn Backend>>>) -> String {
    svc.plays()
        .iter()
        .map(|p| {
            format!(
                "play {} at {} dB: rms {:.2}% peak {:.2}% gated {}",
                p.index, p.level_db, p.metrics.rms_error_pct, p.metrics.peak_error_pct, p.gated
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A plant of gain 2 is identified as 2 in band, the seed drives it at half
/// the target, and every rung gates on its first play.
#[test]
fn a_flat_plant_of_gain_two_is_identified_seeded_and_gated_on_every_first_play() {
    let svc = run(Box::new(flat_gain_two()));
    let report = table(&svc);
    assert_eq!(svc.state(), TwrState::Complete, "{report}");

    let band: Vec<f64> = svc
        .frf_mag()
        .into_iter()
        .filter(|(f, _)| (100.0..=1000.0).contains(f))
        .map(|(_, m)| m)
        .collect();
    let mean = band.iter().sum::<f64>() / band.len() as f64;
    assert!(
        (mean / GAIN - 1.0).abs() < 0.02,
        "|H| in band {mean:.4}, the plant's gain {GAIN}"
    );

    let iterations: Vec<_> = svc
        .plays()
        .iter()
        .filter(|p| p.iteration.is_some())
        .collect();
    assert_eq!(iterations.len(), 5, "{report}");
    for p in &iterations {
        assert!(p.gated && p.iteration == Some(1), "{report}");
        // The drive is the response over the gain: about half the response.
        let ratio = p.response_peak / p.drive_vpk;
        assert!(
            (ratio / GAIN - 1.0).abs() < 0.05,
            "play {}: response peak over drive peak {ratio:.4}",
            p.index
        );
    }
}

/// The same plant with a constant offset on every measured sample, 12% of the
/// -12 dB rung's section rms (the loopback carried 9.6%): the offset is not the
/// plant's motion and no correction reaches 0 Hz, so it is not error, and the
/// run gates every rung as it does without it.
#[test]
fn an_offset_on_the_measured_response_does_not_hold_a_plant_of_gain_two_at_its_first_rung() {
    let offset = 0.12 * rung_section_rms();
    let svc = run(Box::new(Offset {
        inner: flat_gain_two(),
        offset: offset as f32,
    }));
    let report = table(&svc);
    assert_eq!(
        svc.state(),
        TwrState::Complete,
        "offset {offset:.5} EU:\n{report}\ntrip {:?}",
        svc.trip()
    );
    for p in svc.plays().iter().filter(|p| p.iteration.is_some()) {
        assert!(p.gated && p.iteration == Some(1), "{report}");
        assert!(p.metrics.rms_error_pct < 5.0, "{report}");
    }
}
