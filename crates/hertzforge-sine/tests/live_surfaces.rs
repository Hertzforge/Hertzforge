//! The surfaces a host's wire reads while a Sine run is live: the live status
//! (station time, the sweep's rate and direction, the hold, the rung, the drive
//! and the reference output as played, every enabled input with its phase
//! against the drive, the identification's facts) and the display record
//! (every input against a log frequency grid and against time).
//!
//! None of it may change the run: these are readouts. The rest of the suite,
//! unchanged, is what shows that.
//!
//! Falsified once each (session log 2026-11-08, Sine core):
//!
//! * `live_status()` read from the copy `run_chunk` drains instead of the live
//!   cell: every closed-loop test here reds (t_s stays 0; the inputs are empty).
//! * The phase readout without the reference-to-drive offset (the filter's own
//!   phase): `the_phase_is_against_the_drive_and_reads_a_pure_delay_as_one` reds
//!   (the reference channel reads 8.0 frames of delay instead of 12.0).
//! * The display grid built linear in frequency:
//!   `the_display_grid_is_logarithmic_and_follows_the_sweep` reds.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_hal_sim::SimConfig;
use hertzforge_sine::law::{LawFault, SineOutputRule};
use hertzforge_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, LimitProfile, SineMode, SineTestConfig,
};
use hertzforge_sine::{
    BidirSweepConfig, CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig,
    LadderConfig, NotchGovernorConfig, ProfileRow, RunState, RungFor, SineProfile, SineService,
    SweepDirection, SweepRate, SweepTarget, TripKind, DISPLAY_GRID_POINTS,
};

const FS: f64 = 16_384.0;
const SEED: u64 = 0x51E5_0B01;
const BAND: (f64, f64) = (60.0, 500.0);

/// Control 1, a disabled monitor 2, a reference 3 (the Sim plays it the
/// commanded drive), a monitor 4 with a limit profile, and the drive 5. The
/// inputs, in table order, are 1, 3 and 4.
fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    c.units = Units::AccelG;
    t.channels.push(c);
    let mut m = Channel::new(2);
    m.role = ChannelRole::Monitor;
    m.units = Units::AccelG;
    m.enabled = false;
    t.channels.push(m);
    let mut r = Channel::new(3);
    r.role = ChannelRole::Reference;
    r.units = Units::VoltageV;
    t.channels.push(r);
    let mut l = Channel::new(4);
    l.role = ChannelRole::Monitor;
    l.units = Units::AccelG;
    t.channels.push(l);
    let mut d = Channel::new(5);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    t.channels.push(d);
    t
}

fn flat(g: f64) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.push(ProfileRow::accel(BAND.0, g));
    p.push(ProfileRow::accel(BAND.1, g));
    p
}

fn cfg(mode: SineMode) -> SineTestConfig {
    SineTestConfig {
        profile: flat(1.0),
        mode,
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 1,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        // A bound far above anything the run reaches: it is read, never binds.
        limit_profiles: vec![LimitProfile {
            channel_index: 4,
            profile: flat(100.0),
            shared_name: None,
        }],
        blend_w: 0.5,
        compression: CompressionPreset::Fast,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        feedforward: FeedforwardConfig::default(),
        notch: NotchGovernorConfig::default(),
        ladder: LadderConfig {
            sysid_level_db: -20.0,
            start_db: -6.0,
            table_db: vec![-6.0, 0.0],
            per_rung: RungFor::Seconds(1.0),
            full_for: RungFor::Seconds(600.0),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.0,
        drive_vpk_ceiling: 1e4,
        drive_vpk_abort: 1e5,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames: 164,
        fs_hz: None,
        metadata: Vec::new(),
        law_guards: None,
        control_tick_s: None,
    }
}

fn sweep(rate_oct_per_min: f64) -> SineMode {
    SineMode::Sweep(BidirSweepConfig {
        band: BAND,
        rate: SweepRate::LogOctPerMin(rate_oct_per_min),
        start_hz: 100.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(8),
    })
}

fn dwell(hz: f64) -> SineMode {
    SineMode::Dwell(DwellPlan {
        freq_hz: hz,
        for_: DwellFor::Seconds(600.0),
    })
}

fn service(cfg: SineTestConfig) -> SineService {
    let sim = SimConfig::new()
        .sample_rate_hz(FS)
        .resonance(250.0, 10.0)
        .noise_level(1e-5)
        .feedthrough(0.35)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(SEED);
    SineService::new(
        cfg,
        table(),
        Box::new(sim.build()),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        SEED,
        None,
    )
    .expect("the configuration builds")
}

fn engaged(cfg: SineTestConfig) -> SineService {
    let mut s = service(cfg);
    s.assess_feasibility().expect("no shaker configured");
    s.arm().expect("the Sim declares a shared clock");
    s.run_sysid(4096).expect("identification");
    assert!(s.coherence_gate().expect("gate"));
    assert_eq!(s.state(), RunState::Engaged);
    s
}

/// Pump `seconds` of station time through `run_batches`, as a host's own run
/// thread does (it never calls `run_chunk`).
fn pump_batches(s: &mut SineService, seconds: f64) {
    // On the Sim a batch is two one-frame blocks.
    let batches = (seconds * FS / 2.0).ceil() as u32;
    let mut left = batches;
    while left > 0 {
        let n = left.min(4096);
        assert!(s.run_batches(n).expect("batches"), "the run ended early");
        left -= n;
    }
}

#[test]
fn a_host_pumping_batches_reads_the_tick_it_ran() {
    let mut s = engaged(cfg(sweep(8.0)));
    assert!(!s.drive_is_zero(), "the loop is driving");
    pump_batches(&mut s, 0.5);
    let st = s.live_status();
    assert!(st.t_s > 0.45 && st.t_s <= 0.5 + 1e-9, "t_s {}", st.t_s);
    // Station time is frames over the adopted rate, on the tick grid.
    let frames = st.t_s * FS;
    assert!((frames - frames.round()).abs() < 1e-6, "{frames}");
    assert_eq!(frames.round() as u64 % 164, 0, "a tick boundary");
    pump_batches(&mut s, 0.5);
    let st2 = s.live_status();
    assert!(
        (st2.t_s - st.t_s - 0.5).abs() < 164.0 / FS + 1e-9,
        "{} then {}",
        st.t_s,
        st2.t_s
    );
    s.stop().expect("stop");
    assert!(s.drive_is_zero(), "a stopped run drives nothing");
}

#[test]
fn the_sweep_reports_its_rate_direction_and_hold() {
    let mut s = engaged(cfg(sweep(8.0)));
    pump_batches(&mut s, 1.0);
    let st = s.live_status();
    assert_eq!(st.direction, Some(SweepDirection::Up));
    assert_eq!(st.sweep_rate_oct_per_min, Some(8.0));
    let law = st.freq_hz * std::f64::consts::LN_2 * 8.0 / 60.0;
    assert!(
        (st.sweep_rate_hz_per_s - law).abs() < 1e-9 * law,
        "{} vs {law}",
        st.sweep_rate_hz_per_s
    );
    assert!(!st.held);

    s.hold().expect("hold");
    pump_batches(&mut s, 0.1);
    let held = s.live_status();
    assert!(held.held);
    assert_eq!(held.sweep_rate_hz_per_s, 0.0);
    assert_eq!(held.sweep_rate_oct_per_min, Some(0.0));

    s.resume().expect("resume");
    s.reverse().expect("reverse");
    pump_batches(&mut s, 0.1);
    let down = s.live_status();
    assert_eq!(down.direction, Some(SweepDirection::Down));
    assert!(down.sweep_rate_hz_per_s < 0.0);
    assert_eq!(down.sweep_rate_oct_per_min, Some(-8.0));
}

#[test]
fn a_dwell_reports_no_sweep_and_the_ladder_reports_its_rung() {
    let mut s = engaged(cfg(dwell(150.0)));
    pump_batches(&mut s, 0.5);
    let st = s.live_status();
    assert_eq!(st.direction, None);
    assert_eq!(st.sweep_rate_hz_per_s, 0.0);
    assert_eq!(st.sweep_rate_oct_per_min, None);
    assert_eq!(st.rung_index, 0);
    assert!(!st.at_last_level);
    assert!((st.at_level_s - st.t_s).abs() < 2.0 * 164.0 / FS, "{st:?}");
    pump_batches(&mut s, 1.0);
    let st = s.live_status();
    assert_eq!(st.rung_index, 1, "the 1 s rung advanced");
    assert!(st.at_last_level, "0 dB is the last level");
    assert_eq!(
        s.state(),
        RunState::Engaged,
        "the ladder has not held it yet"
    );
}

#[test]
fn every_enabled_input_is_read_in_table_order() {
    let mut s = engaged(cfg(dwell(150.0)));
    pump_batches(&mut s, 1.5);
    let st = s.live_status();
    let ix: Vec<u32> = st.inputs.iter().map(|i| i.channel_index).collect();
    assert_eq!(ix, vec![1, 3, 4], "the disabled monitor is skipped");
    let ctrl = &st.inputs[0];
    assert!(ctrl.is_control && ctrl.role == "Control" && ctrl.unit == "g");
    assert!(ctrl.limit.is_none());
    assert!(
        (ctrl.tracked - st.measured_g.unwrap()).abs() < 1e-12,
        "the control row is the composite"
    );
    let reference = &st.inputs[1];
    assert!(!reference.is_control && reference.role == "Reference");
    // The Sim plays a reference input the commanded drive: its tracked amplitude
    // is the drive as played, and its rms a sine's.
    assert!(
        (reference.tracked / st.drive_vpk - 1.0).abs() < 0.02,
        "{} vs {}",
        reference.tracked,
        st.drive_vpk
    );
    assert!(
        (reference.rms * std::f64::consts::SQRT_2 / st.drive_vpk - 1.0).abs() < 0.05,
        "{} vs {}",
        reference.rms,
        st.drive_vpk
    );
    let limit = &st.inputs[2];
    assert_eq!(limit.limit, Some(100.0 * 10f64.powf(3.0 / 20.0)));
    assert!(st.cola_vpk.is_none());
    let id = st.identification.expect("identification ran");
    assert_eq!(id.gate_coherence, 0.0);
    assert!(id.passed && id.band_coherence > 0.9);
    assert!(id.trusted_fraction > 0.5 && id.trusted_fraction <= 1.0);
    assert!(st.law.is_none(), "no law is built yet");
}

/// The phase of a reference input, which the Sim plays the commanded drive, is
/// the station's drive-to-input delay and nothing else: a lag growing linearly
/// in frequency through zero, the same number of frames at two frequencies.
///
/// The phase is against the drive the controller produced on the same frame, so
/// the delay it reads is the whole engaged path. On this station that is 12
/// frames: the 4-frame engage lead-in the queue keeps, and 8 frames of
/// delivery. (The tracking filters' own reference starts with the first frame
/// the loop processed, so their raw phase reads only the 8; the identification,
/// whose burst plays without the lead-in, measures 0.)
#[test]
fn the_phase_is_against_the_drive_and_reads_a_pure_delay_as_one() {
    let mut delays = Vec::new();
    for hz in [100.0, 300.0] {
        let mut s = engaged(cfg(dwell(hz)));
        pump_batches(&mut s, 1.0);
        let st = s.live_status();
        let phase = st.inputs[1].phase_deg;
        assert!(phase <= 0.0, "a lag, not a lead: {phase} deg at {hz} Hz");
        delays.push(-phase / 360.0 / hz * FS);
    }
    let (d1, d2) = (delays[0], delays[1]);
    assert!(
        (d1 - d2).abs() < 0.1,
        "one delay at both frequencies: {d1:.3} and {d2:.3} frames"
    );
    assert!((d1 - 12.0).abs() < 0.25, "the engaged path: {d1:.3} frames");
}

#[test]
fn the_display_grid_is_logarithmic_and_follows_the_sweep() {
    let mut s = engaged(cfg(sweep(60.0)));
    pump_batches(&mut s, 1.0);
    let d = s.display();
    assert_eq!(d.grid_hz.len(), DISPLAY_GRID_POINTS);
    assert!((d.grid_hz[0] - BAND.0).abs() < 1e-9);
    assert!((d.grid_hz[DISPLAY_GRID_POINTS - 1] - BAND.1).abs() < 1e-6);
    let ratio = d.grid_hz[1] / d.grid_hz[0];
    assert!(d
        .grid_hz
        .windows(2)
        .all(|w| (w[1] / w[0] - ratio).abs() < 1e-9));
    // One second at 60 oct/min from 100 Hz: 100 to 200 Hz, contiguous.
    let visited: Vec<f64> = d
        .grid_hz
        .iter()
        .zip(&d.visited)
        .filter(|(_, &v)| v)
        .map(|(&f, _)| f)
        .collect();
    let (lo, hi) = (visited[0], visited[visited.len() - 1]);
    assert!((lo / 100.0).log2().abs() < 0.02, "{lo}");
    assert!((hi / 200.0).log2().abs() < 0.05, "{hi}");
    let first = d.visited.iter().position(|&v| v).unwrap();
    let last = d.visited.iter().rposition(|&v| v).unwrap();
    assert!(d.visited[first..=last].iter().all(|&v| v), "no gaps");
    assert!(d.sweep.measured[..first].iter().all(|v| v.is_nan()));
    let ix: Vec<u32> = d.sweep.inputs.iter().map(|i| i.channel_index).collect();
    assert_eq!(ix, vec![1, 3, 4]);
    assert!(d.sweep.inputs[2].limit.is_some() && d.sweep.inputs[0].limit.is_none());
    assert!(d.sweep.inputs[0].is_control);
    assert!(d.sweep.drive_vpk[last] > 0.0);
    // The reference curve is the profile at the present level across the band.
    let level = 10f64.powf(s.live_status().level_db / 20.0) as f32;
    assert!(d.reference.iter().all(|&r| (r - level).abs() < 1e-6));
    assert!(d.identification.is_some());
    // The time history is the same ticks, oldest first.
    assert!(!d.history_t_s.is_empty());
    assert!(d.history_t_s.windows(2).all(|w| w[1] > w[0]));
    assert_eq!(d.history.measured.len(), d.history_t_s.len());
}

#[test]
fn the_new_trip_kinds_carry_their_facts() {
    let fault = TripKind::LawFault {
        fault: LawFault::BadOutput(SineOutputRule::NonFinite),
        consecutive: 3,
    };
    let div = TripKind::Divergence { capped_s: 1.0 };
    let j = serde_json::to_string(&fault).unwrap();
    assert!(j.contains("LawFault") && j.contains("non_finite") && j.contains("3"));
    assert_eq!(serde_json::from_str::<TripKind>(&j).unwrap(), fault);
    let j = serde_json::to_string(&div).unwrap();
    assert_eq!(serde_json::from_str::<TripKind>(&j).unwrap(), div);
}
