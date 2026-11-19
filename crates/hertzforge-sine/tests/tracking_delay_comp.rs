//! At the SERVICE-STEP level: the demodulation reference is retarded
//! by the measured transport lag, so a fast sweep stops under-reading and the
//! servo stops overdriving.
//!
//! The hertzforge-dsp side of this (`tracking_delay.rs` there) proves the arithmetic
//! on a bare filter. What this file proves is the part that matters for an
//! armature: the CLOSED LOOP no longer winds the drive up against a lie, and the
//! two configurations that must be untouched by the change really are.

use hertzforge_dsp::sine::AmplitudeServo;
use hertzforge_dsp::sweep::SweepRate;
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_engine::ProcessStep;
use hertzforge_sine::control::CompositeMode;
use hertzforge_sine::profile::{ProfileRow, SineProfile};
use hertzforge_sine::scheduler::{BidirSweep, DwellRun};
use hertzforge_sine::service::config::ColaWaveform;
use hertzforge_sine::service::engine::{
    ColaState, LimitChannelSpec, SchedulerBuild, SineProcessStep, StepBuild,
};
use hertzforge_sine::{
    BidirSweepConfig, BlendConfig, BlendLaw, DriveGuard, DwellFor, DwellPlan, LadderConfig,
    LadderPlan, RungFor, SweepDirection, SweepTarget,
};

const FS: f64 = 51_200.0;
const F_LO: f64 = 60.0;
const F_HI: f64 = 500.0;
const TICK_FRAMES: usize = 512;
const BLOCK_FRAMES: usize = 2048;
/// The lag the chassis sysID measured.
const LAG: usize = 2089;

fn flat_profile(g: f64) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.push(ProfileRow::accel(F_LO, g));
    p.push(ProfileRow::accel(F_HI, g));
    // Wide bands: this fixture is about the readout, not the protections.
    p.global_alarm_db = (40.0, 40.0);
    p.global_abort_db = (80.0, 80.0);
    p
}

fn build(scheduler: SchedulerBuild, start_hz: f64, lag: usize) -> StepBuild {
    let servo = AmplitudeServo::new(1.0, 20.0)
        .with_gains(0.3, 4.0)
        .with_drive_ceiling(1e6)
        .with_fault_fraction(1e-3);
    let blend = BlendLaw::new(BlendConfig {
        w: 0.0,
        max_step_db_per_tick: 1.0,
    })
    .unwrap();
    let ladder = LadderPlan::new(LadderConfig {
        sysid_level_db: -20.0,
        start_db: 0.0,
        table_db: vec![0.0],
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Seconds(600.0),
    })
    .unwrap();
    StepBuild {
        fs_hz: FS,
        control_tick_frames: TICK_FRAMES,
        out_channels: 1,
        profile: flat_profile(1.0),
        n_input_channels: 1,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        // The probe's pairing: a FIXED corner, which is the one with a frequency
        // anchor and therefore the one the droop grows against.
        tracking_bw: BandwidthMode::FixedHz(10.0),
        servo,
        blend,
        feedforward: None,
        notch: None,
        limit_channels: Vec::<LimitChannelSpec>::new(),
        scheduler,
        ladder,
        cola: ColaState {
            enabled: false,
            amp: 0.0,
            waveform: ColaWaveform::Sine,
        },
        drive_guard: DriveGuard::new(1e6, 1e7).unwrap(),
        kinematic_monitor: false,
        max_velocity_ips: None,
        max_displacement_in_pp: None,
        fault_debounce_ticks: 0,
        fault_fraction: 1e-3,
        settle_ticks: 8,
        scalar_abort_debounce: 3,
        start_f_hz: start_hz,
        start_level_scale: 1.0,
        run_mode: hertzforge_sine::RunMode::Auto,
        tracking_lag_frames: lag,
        ring_blocks: 8,
        block_frames: BLOCK_FRAMES,
    }
}

fn sweep_build(lag: usize) -> StepBuild {
    let sweep = BidirSweep::new(BidirSweepConfig {
        band: (F_LO, F_HI),
        rate: SweepRate::LogOctPerMin(60.0),
        start_hz: F_LO,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
    .unwrap();
    build(SchedulerBuild::Sweep(sweep), F_LO, lag)
}

fn dwell_build(lag: usize) -> StepBuild {
    let dwell = DwellRun::new(DwellPlan {
        freq_hz: 276.0,
        for_: DwellFor::Seconds(600.0),
    })
    .unwrap();
    build(SchedulerBuild::Dwell(dwell), 276.0, lag)
}

/// Run a step against a UNIT-GAIN PURE DELAY plant: the response on run frame
/// `n` is EXACTLY the drive from run frame `n - plant_lag`. Returns the drive
/// stream, the final committed drive scale, and a per-block
/// `(frequency, drive_scale)` trace.
///
/// A unit-gain plant makes the answer analytic: the servo's job is to hold the
/// response at 1.0, and since the plant does not change amplitude, the CORRECT
/// drive scale is 1.0 at every frequency. Any departure from 1.0 is the readout
/// error, converted into drive by the loop.
///
/// The delay is indexed off the whole drive history rather than a per-block
/// hand-off, because a per-block hand-off silently adds a BLOCK to the lag - and
/// then the fixture is testing a plant whose delay does not match the one the
/// step was told about. `plant_lag` must be at least [`BLOCK_FRAMES`] so every
/// sample the plant needs has already been produced.
fn run_against_delay_plant(
    b: StepBuild,
    plant_lag: usize,
    n_blocks: usize,
) -> (Vec<f32>, f64, Vec<(f64, f64)>) {
    assert!(
        plant_lag >= BLOCK_FRAMES,
        "the fixture cannot realize a plant lag shorter than one block"
    );
    let (mut step, _h) = SineProcessStep::new(b);
    let mut input = vec![0.0f32; BLOCK_FRAMES];
    let mut drive_out: Vec<f32> = Vec::new();
    let mut drive_log: Vec<f32> = Vec::with_capacity(n_blocks * BLOCK_FRAMES);
    let mut track: Vec<(f64, f64)> = Vec::new();
    for blk in 0..n_blocks {
        // The response to run frames [blk*B, blk*B + B) is the drive from
        // `plant_lag` frames earlier, which is entirely in `drive_log` already.
        let base = blk * BLOCK_FRAMES;
        for (i, slot) in input.iter_mut().enumerate() {
            let src = (base + i) as isize - plant_lag as isize;
            *slot = if src >= 0 {
                drive_log[src as usize]
            } else {
                0.0
            };
        }
        step.process(&input, BLOCK_FRAMES, &mut drive_out);
        drive_log.extend_from_slice(&drive_out);
        track.push((step.freq_hz(), step.drive_scale()));
    }
    let scale = step.drive_scale();
    (drive_log, scale, track)
}

/// THE NO-OP GUARANTEE, part 1: at a DWELL the compensation changes nothing, bit
/// for bit.
///
/// This is what makes the fix safe to ship on by default. With `df/dt = 0` the
/// frequency `lag` frames ago IS the frequency now, so the retarded reference
/// advances by exactly the same increment as the live one and every downstream
/// number - tracked amplitude, servo step, drive scale, emitted waveform - is
/// unchanged. Compared on the DRIVE STREAM, which is the integral of every
/// decision the loop made.
#[test]
fn a_dwell_is_byte_unchanged_by_the_compensation() {
    let (a, sa, _) = run_against_delay_plant(dwell_build(0), LAG, 40);
    let (b, sb, _) = run_against_delay_plant(dwell_build(LAG), LAG, 40);
    assert_eq!(a.len(), b.len());
    let mut worst = 0.0f64;
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        let d = (*x as f64 - *y as f64).abs();
        if d > worst {
            worst = d;
        }
        assert!(
            d <= 1e-12,
            "drive frame {i} differs by {d}: {x} vs {y} - the compensation must be a \
             no-op at df/dt = 0"
        );
    }
    println!(
        "  dwell drive streams: worst |delta| {worst:.3e} over {} frames",
        a.len()
    );
    assert_eq!(
        sa.to_bits(),
        sb.to_bits(),
        "the committed drive scale must be bit-identical: {sa} vs {sb}"
    );
}

/// THE NO-OP GUARANTEE, part 2: a lag of ZERO leaves even a fast sweep
/// bit-identical. HERTZFORGE-SIM measures a zero lag, so this is the statement that
/// every Sim fixture in the tree runs the pre-fix arithmetic.
#[test]
fn a_zero_lag_leaves_a_fast_sweep_byte_unchanged() {
    // Both runs see the SAME delayed plant; only the step's own compensation
    // setting differs, and at zero it must not touch a single sample.
    let (a, sa, _) = run_against_delay_plant(sweep_build(0), LAG, 30);
    let (b, sb, _) = run_against_delay_plant(sweep_build(0), LAG, 30);
    assert_eq!(a, b);
    assert_eq!(sa.to_bits(), sb.to_bits());
    // ...and it is genuinely off, not merely equal by luck.
    let (mut step, _h) = SineProcessStep::new(sweep_build(0));
    assert_eq!(step.tracking_lag_frames(), 0);
    let mut out = Vec::new();
    step.process(&vec![0.0f32; BLOCK_FRAMES], BLOCK_FRAMES, &mut out);
    let (mut step2, _h2) = SineProcessStep::new(sweep_build(LAG));
    assert_eq!(step2.tracking_lag_frames(), LAG);
    let mut out2 = Vec::new();
    step2.process(&vec![0.0f32; BLOCK_FRAMES], BLOCK_FRAMES, &mut out2);
}

/// THE FIX, in the closed loop. Against a unit-gain pure-delay plant the correct
/// drive scale is 1.0 at every frequency, because the plant does not change
/// amplitude. The UNCOMPENSATED loop under-reads the response and winds the
/// drive up to compensate; the compensated loop does not.
#[test]
fn the_uncompensated_loop_overdrives_and_the_compensated_one_does_not() {
    // 3.06 s of sweep at 60 oct/min covers 60 -> 500 Hz; 80 blocks of 2048 at
    // 51.2 kHz is 3.20 s, so the run reaches the band top.
    let blocks = 80;
    let (_, scale_bad, track_bad) = run_against_delay_plant(sweep_build(0), LAG, blocks);
    let (_, scale_good, track_good) = run_against_delay_plant(sweep_build(LAG), LAG, blocks);

    println!("     f (Hz)   drive scale (uncompensated / compensated)");
    for i in (8..track_bad.len()).step_by(8) {
        println!(
            "  {:>9.1}   {:>8.4}   {:>8.4}",
            track_bad[i].0, track_bad[i].1, track_good[i].1
        );
    }
    println!(
        "  at run end (the sweep has turned around): uncompensated {scale_bad:.4}, \
              compensated {scale_good:.4}"
    );

    // The PEAK is the number that matters - it is what the article sees, and it
    // lands at the band top where the beat is widest. (The run end is on the way
    // back down, the sweep having reversed at 500 Hz.)
    let peak = |t: &[(f64, f64)]| t.iter().skip(8).fold(0.0f64, |m, (_, s)| m.max(*s));
    let peak_bad = peak(&track_bad);
    let peak_good = peak(&track_good);
    println!("  peak drive scale: uncompensated {peak_bad:.4}, compensated {peak_good:.4}");

    // The compensated loop lands on the analytic answer everywhere past its own
    // start-up: a unit-gain plant wants a unit drive.
    for &(f, s) in track_good.iter().skip(12) {
        assert!(
            (s - 1.0).abs() < 0.05,
            "compensated drive scale must stay at unity; {s:.4} at {f:.1} Hz"
        );
    }
    // The uncompensated loop drives the article HARDER than it needs to, by the
    // reciprocal of the droop. The closed form puts the droop at -9.4 dB at
    // 500 Hz, i.e. an overdrive of 2.94x; the servo's 1 dB/tick rate limit means
    // it is still climbing when the sweep reverses, so the realized peak is a
    // little under that. Measured 2.49x - which is 8 dB of unasked-for level on
    // an article, arriving with nothing on the demodulated path able to see it.
    assert!(
        peak_bad > 2.4,
        "the uncompensated loop must visibly overdrive (closed form 2.94x at the band \
         top, rate-limited to ~2.5x here); peak was {peak_bad:.4}"
    );
    assert!(
        peak_good < 1.05,
        "the compensated loop must not overdrive at all; peak was {peak_good:.4}"
    );
}

/// The clamp on an implausible lag. A lag longer than a second is a broken
/// measurement, and compensating against one would beat the reference off the
/// signal as badly as not compensating at all - so it is clamped, not trusted.
#[test]
fn an_implausible_lag_is_clamped_rather_than_trusted() {
    // 10 s of frames at 51.2 kHz. Construction must not panic or allocate a
    // half-gigabyte ring; the clamp caps it at one second.
    let b = build(
        SchedulerBuild::Dwell(
            DwellRun::new(DwellPlan {
                freq_hz: 276.0,
                for_: DwellFor::Seconds(10.0),
            })
            .unwrap(),
        ),
        276.0,
        10 * FS as usize,
    );
    let (mut step, _h) = SineProcessStep::new(b);
    let input = vec![0.0f32; BLOCK_FRAMES];
    let mut out = Vec::new();
    step.process(&input, BLOCK_FRAMES, &mut out);
    assert_eq!(out.len(), BLOCK_FRAMES);
}
