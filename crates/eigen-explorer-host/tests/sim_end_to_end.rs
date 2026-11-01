//! The SIM end-to-end gate: the whole Explorer host lifecycle against a plant, with
//! NUMERIC assertions on what came back out of the acquisition.
//!
//! # The plant is a WIRE, deliberately
//!
//! `SimConfig` is configured with no resonators, unity feedthrough, no noise and no
//! transport delay, so `response == drive` exactly. That is the software analogue of the
//! bench this module is aimed at (AO0 jumpered to AI0, nothing downstream), and it is
//! what makes "measured RMS x sqrt(2) is the commanded volts peak" a MEANINGFUL
//! assertion rather than a statement about a resonator's gain at whatever frequency the
//! test happened to pick.
//!
//! # What this can and cannot prove
//!
//! It proves the CONTROL CORE: that the clamps hold, that the slews bound travel, that a
//! retune lands where it was aimed, that a sweep traverses and halts, that a stop ramps
//! to exactly zero, and that the engine reports no faults across the whole run. It
//! proves nothing about TIMING - an EIGEN-SIM station runs the engine flat out with the
//! jitter guard explicitly disabled, so a green here is not evidence about the block
//! cadence or the compute margin (a result that cannot fail is not evidence). The
//! compute-margin evidence is the chassis gate's, in
//! `eigen-station/examples/ni_module_sweep.rs`.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_explorer_host::service::{ExplorerHostConfig, ExplorerHostService, RunState};
use eigen_explorer_host::slew::SlewLimits;
use eigen_explorer_host::step::{AmplitudeRamp, SweepDirection};
use eigen_module_abi::StationClass;

const FS_HZ: f64 = 8_192.0;
const BLOCK: usize = 256;

/// A plant that is a WIRE: no resonance, unity gain, no noise, no delay.
fn wire_plant() -> Box<dyn eigen_hal::Backend> {
    Box::new(
        eigen_hal_sim::SimConfig::new()
            .sample_rate_hz(FS_HZ)
            .resonances(Vec::new())
            .feedthrough(1.0)
            .noise_level(0.0)
            .transport_delay_samples(0)
            .build(),
    )
}

/// One control input + one drive row - the shape the chassis gate uses.
fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 10.0;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    t
}

/// Slews fast enough that the whole lifecycle fits in a few seconds of simulated signal,
/// and STATED here rather than defaulted so the timing assertions below are against a
/// number this file owns.
fn cfg() -> ExplorerHostConfig {
    ExplorerHostConfig {
        fs_hz: FS_HZ,
        band_hz: (20.0, 2_000.0),
        start_hz: 100.0,
        max_drive_vpk: 1.0,
        slew: SlewLimits {
            freq_oct_per_s: 20.0,
            drive_db_per_s: 60.0,
        },
        ramp: AmplitudeRamp::default(),
        display_nfft: 1_024,
        ring_blocks: 16,
    }
}

fn armed_service() -> ExplorerHostService {
    let mut svc = ExplorerHostService::new(cfg(), table(), wire_plant(), StationClass::EigenSim)
        .expect("the wire plant and the one-control table are a valid Explorer station")
        .with_block_frames(BLOCK);
    svc.arm().expect("EIGEN-SIM arms open-loop");
    svc.start().expect("armed -> running");
    svc
}

/// Run `batches` batches, servicing the analysis after EACH one so the ring never
/// overflows, and return the last status. Panics on any engine fault.
fn pump(svc: &mut ExplorerHostService, batches: u32) -> eigen_explorer_host::ExplorerStatus {
    let mut last = svc.service_analysis();
    for _ in 0..batches {
        let alive = svc
            .run_batches(1)
            .unwrap_or_else(|e| panic!("the run faulted mid-pump: {e}"));
        last = svc.service_analysis();
        if !alive {
            break;
        }
    }
    last
}

/// Pump until `done` or the budget runs out, returning the last status.
fn pump_until(
    svc: &mut ExplorerHostService,
    max_batches: u32,
    what: &str,
    mut done: impl FnMut(&eigen_explorer_host::ExplorerStatus) -> bool,
) -> eigen_explorer_host::ExplorerStatus {
    for _ in 0..max_batches {
        let alive = svc
            .run_batches(1)
            .unwrap_or_else(|e| panic!("the run faulted while waiting for {what}: {e}"));
        let st = svc.service_analysis();
        if done(&st) {
            return st;
        }
        if !alive {
            return st;
        }
    }
    panic!("gave up waiting for {what} after {max_batches} batches");
}

/// THE SIM GATE, as one run: arm at 0 V -> 0.2 Vpk -> retune 100 -> 400 -> sweep a
/// decade -> ramped stop. Every step asserts a NUMBER off the acquisition.
#[test]
fn the_full_explorer_lifecycle_on_a_wire_plant() {
    let mut svc = armed_service();

    // --- 1. ARM POSTURE: 0.000 V, and the wire says so. ----------------------------
    let st = pump(&mut svc, 8);
    assert_eq!(st.state, RunState::Running);
    assert_eq!(
        st.commanded_vpk, 0.0,
        "an armed Explorer commands exactly 0 V"
    );
    let armed_rms = st.per_channel_rms[0];
    assert!(
        armed_rms < 0.02,
        "an armed, uncommanded run put {armed_rms} V RMS on the wire"
    );
    assert!(
        st.measured_freq_hz.is_none(),
        "there is no tone to measure at 0 V; a number here would be the noise floor \
         wearing a frequency's name"
    );
    // The rate was ADOPTED, and the Sim never coerces, so the two agree - and both are
    // reported so a station that DID coerce would be visible.
    assert_eq!(st.actual_fs_hz, FS_HZ);
    assert_eq!(st.requested_fs_hz, FS_HZ);

    // --- 2. RAMP TO 0.2 Vpk. --------------------------------------------------------
    assert_eq!(svc.set_amplitude_vpk(0.2).unwrap(), 0.2);
    pump_until(&mut svc, 400, "the drive ramp to reach 0.2 Vpk", |s| {
        (s.commanded_vpk - 0.2).abs() < 1e-9
    });
    // Let the analysis window fill AT the settled level before measuring it.
    let st = pump(&mut svc, 16);
    let measured_vpk = st.per_channel_rms[0] * std::f64::consts::SQRT_2;
    assert!(
        (measured_vpk - 0.2).abs() / 0.2 < 0.05,
        "measured {measured_vpk} Vpk against a commanded 0.2 Vpk (>5% off)"
    );

    // --- 3. RETUNE 100 -> 400 Hz. ---------------------------------------------------
    assert_eq!(st.commanded_freq_hz, 100.0);
    assert_eq!(svc.set_frequency(400.0).unwrap(), 400.0);
    pump_until(&mut svc, 400, "the tone to arrive at 400 Hz", |s| {
        (s.commanded_freq_hz - 400.0).abs() < 1e-9
    });
    // The measured-frequency window needs 0.5 s of accumulated SIGNAL to mature.
    let st = pump_until(&mut svc, 400, "a matured frequency measurement", |s| {
        s.measured_freq_hz.is_some()
    });
    let measured_hz = st.measured_freq_hz.unwrap();
    assert!(
        (measured_hz - 400.0).abs() / 400.0 < 0.02,
        "measured {measured_hz} Hz against a commanded 400 Hz (>2% off)"
    );
    // The amplitude did not move while the frequency did (a live
    // transition is neutral in the OTHER physical quantity).
    let held_vpk = st.per_channel_rms[0] * std::f64::consts::SQRT_2;
    assert!(
        (held_vpk - 0.2).abs() / 0.2 < 0.05,
        "the retune moved the level: {held_vpk} Vpk"
    );

    // --- 4. SWEEP A DECADE, UP THEN DOWN. -------------------------------------------
    svc.set_frequency(200.0).unwrap();
    pump_until(&mut svc, 400, "the tone to reach 200 Hz", |s| {
        (s.commanded_freq_hz - 200.0).abs() < 1e-9
    });
    // 600 oct/min = 10 oct/s, comfortably inside the 20 oct/s slew ceiling so the sweep
    // rate (not the ceiling) is what governs.
    svc.start_sweep(SweepDirection::Up, 600.0).unwrap();
    let st = pump_until(
        &mut svc,
        2_000,
        "the up sweep to reach the band edge",
        |s| s.sweep.map(|w| w.halted).unwrap_or(false),
    );
    assert_eq!(
        st.target_freq_hz, 2_000.0,
        "an up sweep halts AT the band high edge"
    );
    assert!(
        st.target_freq_hz / 200.0 >= 10.0,
        "the up sweep covered less than a decade"
    );
    // HALT-AND-DWELL: a long dwell past the edge must not walk the tone back down.
    let st = pump(&mut svc, 20);
    assert!(
        (st.target_freq_hz - 2_000.0).abs() < 1e-9,
        "the sweep bounced off the edge instead of dwelling: {}",
        st.target_freq_hz
    );

    svc.start_sweep(SweepDirection::Down, 600.0).unwrap();
    let st = pump_until(
        &mut svc,
        2_000,
        "the down sweep to reach the band edge",
        |s| s.sweep.map(|w| w.halted).unwrap_or(false),
    );
    assert_eq!(
        st.target_freq_hz, 20.0,
        "a down sweep halts AT the band low edge"
    );
    assert!(
        2_000.0 / st.target_freq_hz >= 10.0,
        "the down sweep covered less than a decade"
    );

    // --- 5. RAMPED STOP. -------------------------------------------------------------
    svc.request_stop();
    assert_eq!(svc.state(), RunState::Stopping);
    let st = pump_until(&mut svc, 2_000, "the stop ramp to complete", |s| {
        s.state == RunState::Stopped
    });
    assert_eq!(
        st.commanded_vpk, 0.0,
        "a completed stop commands exactly 0 V"
    );
    let end = st.end.as_ref().expect("a run that ENDED must say so");
    assert_eq!(end.state, RunState::Stopped);
    assert!(
        end.reason.contains("ramped"),
        "the end record must name what ended it, got: {}",
        end.reason
    );

    // --- 6. NO ENGINE FAULTS, ANYWHERE IN THE RUN. -----------------------------------
    assert_eq!(
        st.health_faults, 0,
        "scheduling-health faults during the run"
    );
    assert_eq!(st.ring_overruns, 0, "raw-ring drops inside the engine");
    assert_eq!(
        st.blocks_dropped, 0,
        "the analysis ring dropped blocks: the pump fell behind"
    );
    assert!(st.observed_trip.is_none(), "an unexpected limit trip");
    assert!(
        st.engine_ticks > 0 && st.frames_generated > 0 && st.frames_analyzed > 0,
        "the run must actually have run: {st:?}"
    );
}

/// The stop ramp is SMOOTH. Asserted on the DRIVE the step generated, one
/// sample at a time, because a per-block average cannot see a cliff inside a block.
#[test]
fn the_stop_ramp_never_cliffs() {
    use eigen_engine::ProcessStep;
    use eigen_explorer_host::step::{ExplorerProcessStep, StepBuild};

    let (mut step, _h) = ExplorerProcessStep::new(StepBuild {
        fs_hz: FS_HZ,
        out_channels: 1,
        band_hz: (20.0, 2_000.0),
        start_hz: 200.0,
        max_drive_vpk: 1.0,
        slew: SlewLimits {
            freq_oct_per_s: 20.0,
            drive_db_per_s: 60.0,
        },
        ramp: AmplitudeRamp::default(),
        ring_blocks: 4,
        block_frames: BLOCK,
        n_interleave: 1,
    })
    .unwrap();

    // Get to 0.5 Vpk, then stop.
    step.set_target_vpk(0.5);
    let mut drive = Vec::new();
    let n = (2.0 * FS_HZ) as usize;
    step.process(&[], n, &mut drive);
    assert!((step.drive_vpk() - 0.5).abs() < 1e-9);

    step.set_target_vpk(0.0);
    let mut ramp = Vec::new();
    step.process(&[], n, &mut ramp);
    assert_eq!(step.drive_vpk(), 0.0, "the ramp must REACH zero");
    assert_eq!(*ramp.last().unwrap(), 0.0);

    // The ENVELOPE is what must not cliff, not the carrier: the tone itself legitimately
    // swings a full cycle every 41 samples at 200 Hz. Take the envelope as the running
    // peak over one cycle and assert no cycle-to-cycle drop beyond what 60 dB/s allows
    // over that cycle (a factor of 10^(60/20 * T/20)), plus the final step to zero, which
    // is bounded by the stated 40 dB ramp span.
    let cycle = (FS_HZ / 200.0).ceil() as usize;
    let env: Vec<f64> = ramp
        .chunks(cycle)
        .map(|c| c.iter().fold(0.0f64, |m, &v| m.max((v as f64).abs())))
        .collect();
    let per_cycle_ratio = 10f64.powf(60.0 * (cycle as f64 / FS_HZ) / 20.0);
    // The floor the ramp steps to zero from: 40 dB below the 0.5 V anchor.
    let step_floor = 0.5 * 10f64.powf(-40.0 / 20.0);
    for w in env.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b == 0.0 {
            assert!(
                a <= step_floor * 1.5,
                "the drive stepped to zero from {a} V, far above the {step_floor} V ramp floor"
            );
            break;
        }
        assert!(
            a / b <= per_cycle_ratio * 1.10,
            "the envelope dropped by a factor of {} in one cycle, past the {per_cycle_ratio} \
             the 60 dB/s slew allows",
            a / b
        );
    }
}

/// An ABORT does not ramp - it goes straight to `emergency_stop` - and the run is
/// terminal afterwards, with intents refused rather than silently accepted into a dead
/// service.
#[test]
fn an_abort_is_immediate_and_terminal() {
    let mut svc = armed_service();
    svc.set_amplitude_vpk(0.3).unwrap();
    pump(&mut svc, 20);
    svc.abort("operator E-stop drill").unwrap();
    assert_eq!(svc.state(), RunState::Aborted);
    let end = svc.end_record().expect("an aborted run must say so");
    assert!(
        end.reason.contains("operator E-stop drill"),
        "{}",
        end.reason
    );
    assert!(svc.abort_latch().is_tripped());
    assert!(
        svc.set_amplitude_vpk(0.5).is_err(),
        "a terminal run must refuse intents, not absorb them"
    );
    assert!(
        !svc.run_batches(1).unwrap(),
        "a terminal run must not run further batches"
    );
}

/// The MAILBOX path reaches the drive: a command posted from outside is applied between
/// batches, not dropped. A control that moves its own highlight without
/// reaching the service is a control that lies.
#[test]
fn a_mailbox_command_reaches_the_drive() {
    use eigen_explorer_host::ExplorerCommand;

    let mut svc = armed_service();
    let mail = svc.commands();
    mail.post(ExplorerCommand::SetFrequency(500.0));
    mail.post(ExplorerCommand::SetAmplitudeVpk(0.1));
    let st = pump(&mut svc, 4);
    assert_eq!(st.target_freq_hz, 500.0);
    assert_eq!(st.target_vpk, 0.1);

    // And an out-of-band one is CLAMPED, not obeyed and not ignored.
    mail.post(ExplorerCommand::SetFrequency(50_000.0));
    mail.post(ExplorerCommand::SetAmplitudeVpk(99.0));
    let st = pump(&mut svc, 4);
    assert_eq!(st.target_freq_hz, 2_000.0, "clamped to the band high edge");
    assert_eq!(st.target_vpk, 1.0, "clamped to the volts ceiling");

    // The status cell carries the same answer to an observer that never held the
    // service.
    let published = svc.status_cell().read().expect("a published status");
    assert_eq!(published.target_freq_hz, 2_000.0);
    assert_eq!(published.target_vpk, 1.0);

    let _ = svc.abort("test teardown");
}

/// DRIVER DIAGNOSTICS REACH THE STATUS.
///
/// `Backend::take_warnings` DRAINS. A service that never calls it does not defer the
/// warnings, it destroys them - which is the exact shape of the original defect: DAQmx
/// underrun and coercion warnings were collected by `NiBackend` and unreachable from
/// every `dyn Backend` caller for the life of the project. This pins that the Explorer
/// pump pulls them and puts them where a human can read them.
#[test]
fn driver_warnings_are_drained_into_the_status() {
    use eigen_hal::{
        AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
    };

    /// A wire plant that also has something to say.
    struct Chatty {
        inner: Box<dyn Backend>,
        pending: Vec<String>,
    }
    impl Backend for Chatty {
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
            std::mem::take(&mut self.pending)
        }
        fn configure(
            &mut self,
            t: &ChannelTable,
            a: &AcquisitionConfig,
        ) -> Result<ConfiguredActual, HalError> {
            self.inner.configure(t, a)
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
        fn read_input(&mut self, o: &mut [f32]) -> Result<InputRead, HalError> {
            self.inner.read_input(o)
        }
        fn write_drive(&mut self, d: &[f32]) -> Result<usize, HalError> {
            self.inner.write_drive(d)
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

    let backend = Box::new(Chatty {
        inner: wire_plant(),
        pending: vec!["DAQmx 200011: onboard AO buffer underflowed".into()],
    });
    let mut svc = ExplorerHostService::new(cfg(), table(), backend, StationClass::EigenSim)
        .unwrap()
        .with_block_frames(BLOCK);
    svc.arm().unwrap();
    svc.start().unwrap();
    let st = pump(&mut svc, 2);
    assert_eq!(
        st.driver_warnings.len(),
        1,
        "the driver's own words must reach the status, not be drained into nothing"
    );
    assert!(st.driver_warnings[0].contains("200011"));
    let _ = svc.abort("test teardown");
}

/// **A STEADY TONE READS ITS OWN FREQUENCY, whatever its phase against the block.**
///
/// The measured window is fed one acquisition block at a time. Counted per block, the
/// interval between the last sample of one block and the first of the next is time the
/// window counts and a crossing it never tests - and for a tone with a whole or half number
/// of cycles per block that interval sits at the SAME phase in every block, so the loss is
/// all or nothing, every window. At the chassis's own geometry (8533.33 Hz, 128-frame
/// blocks) one lost crossing per block is ~33 Hz on a 0.5 s window at ANY frequency.
///
/// The tones: two with a whole number of cycles per block (533.33 Hz = 8, 1066.67 Hz = 16),
/// one with a half (1500 Hz = 22.5), the bench's 400 Hz (6), and 777 Hz, which has no
/// relation to the block and so loses the PHASE-AVERAGED share, one interval per block
/// (-0.78 %, ~6 Hz).
///
/// The tolerance is a crossing and a half of the window (~1.5 Hz): the quantization no
/// counting rule can beat, with nothing to spare for a lost crossing per block.
///
/// RED, measured 2026-11-01 on the per-chunk feed: 1066.67 Hz read 1033.33 Hz (-3.13 %) and
/// 777 Hz 770.59 Hz (-0.83 %). 533.33, 1500 and 400 Hz happened to sit at a crossing-free
/// boundary phase on this plant; WHICH whole-cycle tones lose is set by the phase the
/// history leaves them at (behind the chassis pipeline it was 533.33 Hz, at -6.25 %), which
/// is why the set holds several. GREEN: every tone within 0.1 %.
#[test]
fn a_steady_tone_reads_its_own_frequency_whatever_its_phase_against_the_block() {
    // The chassis's coerced rate and its 128-frame block, so the boundary phases are the
    // ones the bench sees.
    let fs = 13_107_200.0 / (256.0 * 6.0);
    let block = 128;
    let cfg = ExplorerHostConfig {
        fs_hz: fs,
        band_hz: (50.0, 2_000.0),
        ..cfg()
    };
    let plant = Box::new(
        eigen_hal_sim::SimConfig::new()
            .sample_rate_hz(fs)
            .resonances(Vec::new())
            .feedthrough(1.0)
            .noise_level(0.0)
            .transport_delay_samples(0)
            .build(),
    );
    let mut svc = ExplorerHostService::new(cfg, table(), plant, StationClass::EigenSim)
        .expect("a wire at the chassis rate is a valid Explorer station")
        .with_block_frames(block);
    svc.arm().expect("EIGEN-SIM arms open-loop");
    svc.start().expect("armed -> running");
    svc.set_amplitude_vpk(0.2).unwrap();
    pump_until(&mut svc, 400, "the drive ramp to reach 0.2 Vpk", |s| {
        (s.commanded_vpk - 0.2).abs() < 1e-9
    });

    let tones = [
        533.333_333_333_333_4,
        1_066.666_666_666_666_7,
        1_500.0,
        400.0,
        777.0,
    ];
    let mut worst = (0.0f64, 0.0f64);
    for &hz in &tones {
        assert!((svc.set_frequency(hz).unwrap() - hz).abs() < 1e-9);
        // The THIRD matured reading: a steady tone, well clear of the retune.
        let mut readings = Vec::new();
        pump_until(
            &mut svc,
            400,
            "three matured readings at a held tone",
            |s| {
                if let Some(m) = s.measured_freq_hz {
                    readings.push(m);
                }
                readings.len() >= 3
            },
        );
        let got = readings[2];
        println!(
            "  {hz:9.3} Hz ({:5.2} cycles/block) reads {got:9.3} Hz ({:+6.2} %)",
            hz * block as f64 / fs,
            (got / hz - 1.0) * 100.0
        );
        if (got - hz).abs() > worst.1.abs() {
            worst = (hz, got - hz);
        }
    }
    assert!(
        worst.1.abs() < 1.5,
        "a steady {:.3} Hz tone read {:+.3} Hz off; a crossing per block is going uncounted",
        worst.0,
        worst.1
    );
}

/// **A RING DROP IS A GAP, and the window must not read across it.** The analysis ring
/// drops its oldest blocks when the pump falls behind; the blocks a pass then drains are
/// contiguous among themselves but not with the previous pass's. Carrying the last sample
/// across that gap would test an interval that is not one - a phase-random extra crossing
/// per gap, reading HIGH.
///
/// Forced here: a two-block ring serviced only every three batches (six blocks), so four
/// blocks are dropped before every pass.
///
/// The assertion is on the MEAN of eight steady readings, because a gap every 256 frames
/// cuts the window into 17 short runs and each run's crossing count is quantized on its
/// own: a single reading wanders by a Hz or two either way with the stream handled
/// correctly. A crossing tested across every gap does not wander - it is a bias, and the
/// mean keeps it (RED-MUTATION, run 2026-11-01: drop the `break_continuity` call in the
/// service and the mean at 777 Hz reads +6.27 Hz, where the stream handled correctly reads
/// +0.06 Hz).
#[test]
fn a_ring_drop_breaks_the_measured_stream() {
    let fs = 13_107_200.0 / (256.0 * 6.0);
    let block = 128;
    let cfg = ExplorerHostConfig {
        fs_hz: fs,
        band_hz: (50.0, 2_000.0),
        ring_blocks: 2,
        ..cfg()
    };
    let plant = Box::new(
        eigen_hal_sim::SimConfig::new()
            .sample_rate_hz(fs)
            .resonances(Vec::new())
            .feedthrough(1.0)
            .noise_level(0.0)
            .transport_delay_samples(0)
            .build(),
    );
    let mut svc = ExplorerHostService::new(cfg, table(), plant, StationClass::EigenSim)
        .expect("a wire at the chassis rate is a valid Explorer station")
        .with_block_frames(block);
    svc.arm().expect("EIGEN-SIM arms open-loop");
    svc.start().expect("armed -> running");
    svc.set_amplitude_vpk(0.2).unwrap();
    let lagging = |svc: &mut ExplorerHostService| {
        svc.run_batches(3).expect("no fault");
        svc.service_analysis()
    };
    for _ in 0..200 {
        if (lagging(&mut svc).commanded_vpk - 0.2).abs() < 1e-9 {
            break;
        }
    }

    let mut worst = (0.0f64, 0.0f64);
    for &hz in &[777.0, 1_234.5, 333.0] {
        svc.set_frequency(hz).unwrap();
        let mut readings = Vec::new();
        for _ in 0..1_000 {
            if let Some(m) = lagging(&mut svc).measured_freq_hz {
                readings.push(m);
                if readings.len() >= 10 {
                    break;
                }
            }
        }
        // Readings 3..=10: steady, well clear of the retune.
        assert!(
            readings.len() >= 10,
            "ten readings did not mature under drops"
        );
        let steady = &readings[2..10];
        let got = steady.iter().sum::<f64>() / steady.len() as f64;
        println!(
            "  {hz:8.1} Hz under drops: mean of 8 reads {got:9.3} Hz ({:+.3} Hz), singles {:?}",
            got - hz,
            steady
                .iter()
                .map(|m| format!("{:+.2}", m - hz))
                .collect::<Vec<_>>()
        );
        if (got - hz).abs() > worst.1.abs() {
            worst = (hz, got - hz);
        }
    }
    let st = svc.service_analysis();
    assert!(
        st.blocks_dropped > 0,
        "the ring never dropped a block; this test is not exercising a gap"
    );
    assert!(
        worst.1.abs() < 1.0,
        "under ring drops a steady {:.1} Hz read {:+.3} Hz off on the mean of eight readings; \
         the window is reading across the gaps",
        worst.0,
        worst.1
    );
}
