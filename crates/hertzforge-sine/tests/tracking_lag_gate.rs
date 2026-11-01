//! The sweep-rate-vs-tracking-bandwidth gate.
//!
//! A tracking filter whose reference is not delay-compensated under-reads a fast
//! sweep by `1/(1 + (df/bw)^2)`, where `df` is how far the sweep moves the
//! commanded frequency inside one drive-to-response transport lag. The servo
//! closes on the under-read and overdrives the true response by the same factor,
//! and every protection on the demodulated path is looking at the same wrong
//! number. There is no in-run detection, so the honest place to catch it is
//! before the run starts, from two numbers the config already fixes.
//!
//! The two rows that matter are the two configurations the chassis actually ran:
//! the wire probe's (60-500 Hz, `FixedHz(10)`, 60 oct/min) must REFUSE, and the
//! gate sections' (proportional corner, 1-4 oct/min) must pass untouched.

use hertzforge_dsp::sweep::SweepRate;
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_sine::service::config::{
    assess_tracking_lag, TrackingLagVerdict, TRACKING_LAG_ADVISORY_RATIO, TRACKING_LAG_REFUSE_RATIO,
};

/// The chassis rate and the lag the sysID cross-correlation measured there.
const FS: f64 = 51_200.0;
const MEASURED_LAG: usize = 2089;
const PROBE_BAND: (f64, f64) = (60.0, 500.0);

/// THE RED ROW. The wire probe's exact configuration must be refused, and the
/// refusal must name BOTH numbers - the beat and the tracking corner - because
/// which of the two knobs to turn is the operator's actual question.
#[test]
fn the_probe_configuration_is_refused_naming_both_numbers() {
    let v = assess_tracking_lag(
        PROBE_BAND,
        SweepRate::LogOctPerMin(60.0),
        BandwidthMode::FixedHz(10.0),
        MEASURED_LAG,
        FS,
    );
    println!("  verdict: {v:#?}");
    assert!(
        v.is_refusal(),
        "the probe's config must be refused; got ratio {:.3}",
        v.ratio()
    );
    let msg = v.message().expect("a refusal carries a message");
    // BOTH numbers, by value, in the sentence.
    assert!(
        msg.contains("13.9") || msg.contains("14.0"),
        "the message must state the BEAT in Hz (~13.95 Hz at 500 Hz): {msg}"
    );
    assert!(
        msg.contains("10.00 Hz"),
        "the message must state the effective TRACKING CORNER (10 Hz): {msg}"
    );
    // ...and the thresholds it was judged against.
    assert!(
        msg.contains(&format!("{TRACKING_LAG_ADVISORY_RATIO}"))
            && msg.contains(&format!("{TRACKING_LAG_REFUSE_RATIO}")),
        "the message must state both thresholds: {msg}"
    );
    // The worst point is the band top, which is where the closed form puts it.
    match v {
        TrackingLagVerdict::Refuse { at_hz, ratio, .. } => {
            assert!(
                (at_hz - 500.0).abs() < 1.0,
                "worst case should be the band top, got {at_hz:.1} Hz"
            );
            assert!(
                (ratio - 1.395).abs() < 0.02,
                "closed form: df 13.95 Hz / bw 10 Hz = 1.395; got {ratio:.3}"
            );
        }
        _ => unreachable!(),
    }
}

/// THE UNTOUCHED ROW. The gate sections sweep at 1-4 oct/min on a PROPORTIONAL
/// corner, which is the pairing that has no frequency anchor - the beat and the
/// bandwidth both scale with `f`, so the ratio is flat and small. These configs
/// must not be disturbed by the gate at all.
#[test]
fn the_gate_configurations_pass_untouched() {
    for rate in [1.0, 2.0, 4.0] {
        let v = assess_tracking_lag(
            PROBE_BAND,
            SweepRate::LogOctPerMin(rate),
            BandwidthMode::Proportional {
                pct: 10.0,
                max_hz: 100.0,
            },
            MEASURED_LAG,
            FS,
        );
        println!("  {rate} oct/min proportional: ratio {:.4}", v.ratio());
        assert!(
            matches!(v, TrackingLagVerdict::Ok { .. }),
            "{rate} oct/min on a proportional corner must pass: {v:#?}"
        );
    }
}

/// A fixed corner at the DEPLOYED 1 oct/min also passes. This is the row that
/// says the deployed pair is fine, which is the operator-facing half of the
/// tracking-lag finding.
#[test]
fn the_deployed_slow_sweep_passes_on_a_fixed_corner_too() {
    let v = assess_tracking_lag(
        PROBE_BAND,
        SweepRate::LogOctPerMin(1.0),
        BandwidthMode::FixedHz(10.0),
        MEASURED_LAG,
        FS,
    );
    println!("  1 oct/min FixedHz(10): ratio {:.4}", v.ratio());
    assert!(matches!(v, TrackingLagVerdict::Ok { .. }), "{v:#?}");
}

/// The band between the thresholds warns rather than refuses. Found by bisecting
/// the sweep rate, so the test cannot drift out of the band if a threshold moves.
#[test]
fn the_middle_band_advises_instead_of_refusing() {
    let probe = |rate: f64| {
        assess_tracking_lag(
            PROBE_BAND,
            SweepRate::LogOctPerMin(rate),
            BandwidthMode::FixedHz(10.0),
            MEASURED_LAG,
            FS,
        )
    };
    // The ratio is very nearly linear in rate, so aim at the midpoint of the two
    // thresholds and then confirm rather than assume.
    let target = 0.5 * (TRACKING_LAG_ADVISORY_RATIO + TRACKING_LAG_REFUSE_RATIO);
    let mut rate = 60.0 * target / probe(60.0).ratio();
    for _ in 0..40 {
        let r = probe(rate).ratio();
        if (r - target).abs() < 1e-4 {
            break;
        }
        rate *= target / r;
    }
    let v = probe(rate);
    println!("  {rate:.2} oct/min FixedHz(10): {v:#?}");
    assert!(
        matches!(v, TrackingLagVerdict::Advisory { .. }),
        "a ratio of {:.3} between {TRACKING_LAG_ADVISORY_RATIO} and \
         {TRACKING_LAG_REFUSE_RATIO} must ADVISE, not refuse: {v:#?}",
        v.ratio()
    );
    assert!(v.message().is_some(), "an advisory still says why");
}

/// No lag, no beat, no verdict. The check cannot manufacture a refusal out of a
/// configuration with nothing to be late.
#[test]
fn a_zero_lag_can_never_fail_the_check() {
    for rate in [1.0, 60.0, 600.0] {
        let v = assess_tracking_lag(
            PROBE_BAND,
            SweepRate::LogOctPerMin(rate),
            BandwidthMode::FixedHz(1.0),
            0,
            FS,
        );
        assert!(
            matches!(v, TrackingLagVerdict::Ok { ratio } if ratio == 0.0),
            "{v:#?}"
        );
    }
}

/// A DOWNWARD sweep beats just as hard as an upward one: the reference is behind
/// the signal either way. Signs are the classic place for this kind of check to
/// be half-implemented.
#[test]
fn a_downward_sweep_is_judged_the_same_as_an_upward_one() {
    let up = assess_tracking_lag(
        PROBE_BAND,
        SweepRate::LogOctPerMin(60.0),
        BandwidthMode::FixedHz(10.0),
        MEASURED_LAG,
        FS,
    );
    let down = assess_tracking_lag(
        PROBE_BAND,
        SweepRate::LogOctPerMin(-60.0),
        BandwidthMode::FixedHz(10.0),
        MEASURED_LAG,
        FS,
    );
    assert!(
        down.is_refusal(),
        "a downward sweep must refuse too: {down:#?}"
    );
    // Not identical (the log law is asymmetric in the two directions) but the
    // same order, and both well past the refusal line.
    assert!(
        (down.ratio() / up.ratio() - 1.0).abs() < 0.1,
        "up {:.3} vs down {:.3}",
        up.ratio(),
        down.ratio()
    );
}

/// A LINEAR sweep law is worst at the BAND BOTTOM, not the top: the beat is
/// constant in Hz while a proportional corner shrinks with frequency. The scan
/// has to find that on its own.
#[test]
fn a_linear_law_is_judged_at_the_band_bottom() {
    let v = assess_tracking_lag(
        PROBE_BAND,
        SweepRate::LinearHzPerSec(200.0),
        BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        MEASURED_LAG,
        FS,
    );
    println!("  linear 200 Hz/s, proportional corner: {v:#?}");
    match v {
        TrackingLagVerdict::Refuse { at_hz, .. } | TrackingLagVerdict::Advisory { at_hz, .. } => {
            assert!(
                (at_hz - PROBE_BAND.0).abs() < 1.0,
                "a linear law's worst point is the band bottom, got {at_hz:.1} Hz"
            );
        }
        TrackingLagVerdict::Ok { ratio } => {
            panic!("200 Hz/s against a 6 Hz corner at 60 Hz should not pass (ratio {ratio:.3})")
        }
    }
}

// ---------------------------------------------------------------------------
// The gate is WIRED, not merely present.
// ---------------------------------------------------------------------------

/// The check reaches an actual `arm()`. Uses the SimBackend's own 4-frame
/// lookahead as the estimate, with a config hostile enough that even 4 frames
/// of lag beats a narrow corner - which is the point: this row proves the wiring,
/// the rows above prove the arithmetic.
#[test]
fn arm_refuses_a_configuration_the_check_rejects() {
    use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
    use hertzforge_hal_sim::SimConfig;
    use hertzforge_sine::profile::{ProfileRow, SineProfile};
    use hertzforge_sine::service::{ServiceError, SineService};
    use hertzforge_sine::{
        BidirSweepConfig, CaptureConfig, ColaConfig, CompositeMode, CompressionPreset,
        ControlChannel, ControlSet, FeedforwardConfig, LadderConfig, NotchGovernorConfig, RungFor,
        SineMode, SineTestConfig, SweepDirection, SweepTarget,
    };

    let fs = 16_384.0;
    let mut profile = SineProfile::new("flat");
    profile.push(ProfileRow::accel(60.0, 1.0));
    profile.push(ProfileRow::accel(500.0, 1.0));

    let mut table = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    table.channels.push(ctrl);
    table.channels.push(drv);

    // 4 frames at 16384 Hz is 0.244 ms. To beat a corner by more than half at
    // 500 Hz that needs a very fast sweep and a very narrow corner - 3000
    // oct/min against a 0.02 Hz corner. Absurd as a test, which is fine: the
    // question here is whether arm() consults the check at all.
    let cfg = SineTestConfig {
        profile,
        mode: SineMode::Sweep(BidirSweepConfig {
            band: (60.0, 500.0),
            rate: hertzforge_dsp::sweep::SweepRate::LogOctPerMin(3000.0),
            start_hz: 60.0,
            initial_direction: SweepDirection::Up,
            target: SweepTarget::Sweeps(1),
        }),
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 1,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        limit_profiles: Vec::new(),
        blend_w: 1.0,
        compression: CompressionPreset::Fast,
        tracking_bw: BandwidthMode::FixedHz(0.02),
        feedforward: FeedforwardConfig::default(),
        notch: NotchGovernorConfig::default(),
        ladder: LadderConfig {
            sysid_level_db: -20.0,
            start_db: 0.0,
            table_db: vec![0.0],
            per_rung: RungFor::Sweeps(1),
            full_for: RungFor::Sweeps(1),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.5,
        drive_vpk_ceiling: 1e4,
        drive_vpk_abort: 1e5,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames: 64,
        fs_hz: None,
        metadata: Vec::new(),
    };

    let sim = SimConfig::new()
        .sample_rate_hz(fs)
        .resonance(250.0, 20.0)
        .noise_level(1e-4)
        .feedthrough(0.35)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(1);
    let mut svc = SineService::new(
        cfg,
        table,
        Box::new(sim.build()),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        1,
        None,
    )
    .expect("the config is otherwise valid - the gate lives at arm, not at new");

    svc.assess_feasibility()
        .expect("no shaker record, so feasibility is not evaluated");
    let err = svc.arm().expect_err("arm must refuse");
    let text = format!("{err}");
    println!("  arm refused with: {text}");
    assert!(
        matches!(err, ServiceError::Config(_)),
        "a tracking-lag refusal is a CONFIG refusal, not a plant fault: {err:?}"
    );
    assert!(
        text.contains("tracking bandwidth") && text.contains("lag"),
        "the arm refusal must carry the check's own sentence: {text}"
    );
}
