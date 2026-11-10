//! The read surfaces a front end maps onto its wire, each against the run it describes:
//! station time, where each intent lands in the generated drive, the survey line handed
//! on, the pass and batch-gap times, the stop's cause, the derived motion's peak over a
//! pass, the count of every driver warning and each drained block's first input frame.
//!
//! None of these changes the run. Every test here would fail if its surface were absent
//! or stuck at a constant.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_explorer_host::analysis::SpectrumFrame;
use hertzforge_explorer_host::host::{HostNote, HostSink, HostView, Pacing, RunHost};
use hertzforge_explorer_host::profile_raster::ProfileRaster;
use hertzforge_explorer_host::service::{
    ExplorerHostConfig, ExplorerHostService, RunState, ServiceError,
};
use hertzforge_explorer_host::slew::SlewLimits;
use hertzforge_explorer_host::step::AmplitudeRamp;
use hertzforge_explorer_host::{
    FrequencyPart, IntentKind, IntentRecord, LevelPart, StopCause, SweepDirection,
};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_module_abi::StationClass;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FS: f64 = 8_192.0;
const BLOCK: usize = 128;

/// A simulated station that records every drive sample it accepts and, when asked, says
/// something from its driver on every poll of its warnings.
struct Recording {
    inner: Box<dyn Backend>,
    drive: Arc<Mutex<Vec<f32>>>,
    warn: bool,
    warned: u64,
}

impl Backend for Recording {
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
        let mut w = self.inner.take_warnings();
        if self.warn {
            self.warned += 1;
            w.push(format!("driver note {}", self.warned));
        }
        w
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
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, d: &[f32]) -> Result<usize, HalError> {
        let n = self.inner.write_drive(d)?;
        self.drive
            .lock()
            .unwrap()
            .extend_from_slice(&d[..n.min(d.len())]);
        Ok(n)
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

/// A wire, optionally behind an NI-like output lookahead.
fn wire(lookahead: Option<u32>) -> Box<dyn Backend> {
    let mut c = hertzforge_hal_sim::SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(Vec::new())
        .feedthrough(1.0)
        .noise_level(0.0)
        .transport_delay_samples(0);
    if let Some(l) = lookahead {
        c = c.min_lookahead_frames(l);
    }
    Box::new(c.build())
}

/// Two modes, so the survey line has a peak to find.
fn resonant() -> Box<dyn Backend> {
    Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(FS)
            .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
            .feedthrough(0.35)
            .noise_level(0.0)
            .build(),
    )
}

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    c.units = Units::AccelG;
    c.range_max = 10.0;
    t.channels.push(c);
    let mut d = Channel::new(2);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    t.channels.push(d);
    t
}

fn cfg(ring_blocks: usize) -> ExplorerHostConfig {
    ExplorerHostConfig {
        fs_hz: FS,
        band_hz: (20.0, 2_000.0),
        start_hz: 250.0,
        max_drive_vpk: 0.5,
        slew: SlewLimits {
            freq_oct_per_s: 4.0,
            drive_db_per_s: 240.0,
        },
        ramp: AmplitudeRamp::default(),
        display_nfft: 1_024,
        ring_blocks,
    }
}

struct Rig {
    svc: ExplorerHostService,
    drive: Arc<Mutex<Vec<f32>>>,
}

fn rig_on(plant: Box<dyn Backend>, warn: bool, ring_blocks: usize) -> Rig {
    let drive = Arc::new(Mutex::new(Vec::new()));
    let mut svc = ExplorerHostService::new(
        cfg(ring_blocks),
        table(),
        Box::new(Recording {
            inner: plant,
            drive: drive.clone(),
            warn,
            warned: 0,
        }),
        StationClass::HertzforgeSim,
    )
    .expect("a valid Explorer station")
    .with_block_frames(BLOCK);
    svc.arm().expect("arms open-loop");
    svc.start().expect("armed -> running");
    Rig { svc, drive }
}

fn rig() -> Rig {
    rig_on(wire(None), false, 16)
}

impl Rig {
    fn pass(&mut self) -> hertzforge_explorer_host::ExplorerStatus {
        self.svc.run_batches(1).expect("no fault");
        self.svc.service_analysis()
    }

    fn passes(&mut self, n: usize) -> hertzforge_explorer_host::ExplorerStatus {
        let mut last = self.svc.service_analysis();
        for _ in 0..n {
            last = self.pass();
        }
        last
    }

    fn drive(&self) -> Vec<f32> {
        self.drive.lock().unwrap().clone()
    }
}

/// The first generated frame two drive records differ at.
fn first_difference(a: &[f32], b: &[f32]) -> Option<u64> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x.to_bits() != y.to_bits())
        .map(|i| i as u64)
}

#[test]
fn explorer_frames_in_counts_the_stations_frames_not_the_generated_ones() {
    // A deep output queue, so frames generated run well ahead of frames acquired.
    let mut r = rig_on(wire(Some(512)), false, 64);
    let mut tapped = 0u64;
    let mut st = r.svc.service_analysis();
    for _ in 0..40 {
        r.svc.run_batches(1).unwrap();
        st = r
            .svc
            .service_analysis_tapped(|b, s| tapped += (b.len() / s) as u64);
    }
    assert_eq!(st.blocks_dropped, 0);
    assert_eq!(
        st.frames_in, tapped,
        "station time counts what the station acquired"
    );
    assert_eq!(st.frames_in, 40 * 2 * BLOCK as u64, "two blocks a batch");
    assert!(
        st.frames_generated >= st.frames_in + 1024,
        "the drive runs a pipeline ahead: generated {} acquired {}",
        st.frames_generated,
        st.frames_in
    );
    assert_eq!(r.svc.frames_in(), st.frames_in);
}

#[test]
fn explorer_each_intent_names_the_generated_frame_its_change_first_appears_in() {
    // Level from 0 V, against a run that never raises it.
    let take = |raise_at: Option<usize>| {
        let mut r = rig();
        let mut rec = None;
        let mut input_before = 0;
        for i in 0..30 {
            if Some(i) == raise_at {
                input_before = r.svc.frames_in();
                rec = Some(r.svc.steer(None, Some(LevelPart::Vpk(0.2))).unwrap());
            }
            r.pass();
        }
        (r.drive(), rec, input_before)
    };
    let (quiet, _, _) = take(None);
    let (raised, steered, input_before) = take(Some(7));
    let steered = steered.unwrap();
    assert_eq!(
        first_difference(&quiet, &raised),
        Some(steered.frame),
        "the level's frame"
    );
    assert_eq!(steered.input_frame, input_before);
    assert!(
        steered.frame > steered.input_frame,
        "the drive is written ahead"
    );

    // At a level, a retune alone, a level alone, and both in one steer, each against the
    // same run without it: all three first appear in the frame the answer names.
    let run = |at: usize, part: Option<(Option<FrequencyPart>, Option<LevelPart>)>| {
        let mut r = rig();
        r.svc.set_amplitude_vpk(0.2).unwrap();
        let mut frame = None;
        for i in 0..40 {
            if i == at {
                if let Some((f, l)) = part {
                    frame = Some(r.svc.steer(f, l).unwrap().frame);
                }
            }
            r.pass();
        }
        (r.drive(), frame)
    };
    // A retune is phase-continuous: the frame it names is the first generated at the new
    // frequency, and that frame's sample continues the old phase, so the waveform's value
    // first differs one frame later. A level moves the named frame's own sample.
    let (base, _) = run(20, None);
    for (part, after) in [
        ((Some(FrequencyPart::Hz(400.0)), None), 1),
        ((None, Some(LevelPart::Vpk(0.1))), 0),
        (
            (Some(FrequencyPart::Hz(400.0)), Some(LevelPart::Vpk(0.1))),
            0,
        ),
    ] {
        let (moved, frame) = run(20, Some(part));
        assert_eq!(
            first_difference(&base, &moved),
            frame.map(|f| f + after),
            "{part:?}"
        );
    }
}

#[test]
fn explorer_intents_are_recorded_with_what_was_asked_and_adopted() {
    let mut r = rig();
    r.passes(3);
    let s = r
        .svc
        .steer(
            Some(FrequencyPart::StepOct(-1.0)),
            Some(LevelPart::Vpk(0.9)),
        )
        .unwrap();
    assert_eq!(s.hz, 125.0, "one octave down from 250 Hz");
    assert_eq!(s.vpk, 0.5, "clamped to the ceiling");
    assert!(!s.clamped_hz && s.clamped_vpk);
    let up = r.svc.steer(None, Some(LevelPart::StepDb(-6.0))).unwrap();
    assert!((up.vpk - 0.5 * 10f64.powf(-0.3)).abs() < 1e-12);
    r.svc.set_frequency(5_000.0).unwrap();
    let sw = r.svc.sweep(SweepDirection::Down, 90.0).unwrap();
    let hold = r.svc.hold().unwrap();
    r.svc.request_stop();
    let log = r.svc.take_intents();
    let kinds: Vec<IntentKind> = log.iter().map(|i| i.kind).collect();
    assert_eq!(log.len(), 6, "{kinds:?}");
    assert_eq!(
        log.iter().map(|i| i.seq).collect::<Vec<_>>(),
        (0..6).collect::<Vec<_>>()
    );
    match kinds[0] {
        IntentKind::Steer {
            frequency: Some(f),
            level: Some(l),
        } => {
            assert_eq!((f.asked, f.adopted, f.clamped), (125.0, 125.0, false));
            assert_eq!((l.asked, l.adopted, l.clamped), (0.9, 0.5, true));
        }
        other => panic!("{other:?}"),
    }
    match kinds[2] {
        IntentKind::Steer {
            frequency: Some(f),
            level: None,
        } => assert_eq!((f.asked, f.adopted, f.clamped), (5_000.0, 2_000.0, true)),
        other => panic!("{other:?}"),
    }
    assert_eq!(log[3], sw);
    assert_eq!(log[4], hold);
    assert_eq!(
        kinds[5],
        IntentKind::Stop {
            cause: StopCause::Operator
        }
    );
    assert!(r.svc.take_intents().is_empty(), "taken once");

    // A dB step from 0 V has nothing to scale, and refusing it applies nothing.
    let mut q = rig();
    let err = q
        .svc
        .steer(Some(FrequencyPart::Hz(300.0)), Some(LevelPart::StepDb(3.0)))
        .unwrap_err();
    assert_eq!(err, ServiceError::LevelZero);
    let st = q.passes(1);
    assert_eq!(
        st.target_freq_hz, 250.0,
        "the frequency part was not applied"
    );
    assert!(q.svc.take_intents().is_empty());
}

#[test]
fn explorer_the_stop_in_progress_names_its_cause() {
    for cause in [
        StopCause::Operator,
        StopCause::SafeStop,
        StopCause::Shutdown,
        StopCause::Limit,
    ] {
        let mut r = rig();
        r.svc.set_amplitude_vpk(0.2).unwrap();
        let st = r.passes(5);
        assert_eq!(st.stop, None, "no stop yet");
        r.svc.request_stop_for(cause);
        // A second request does not rename it.
        r.svc.request_stop_for(StopCause::Operator);
        let st = r.passes(2);
        assert_eq!(st.state, RunState::Stopping);
        assert_eq!(st.stop, Some(cause));
    }
}

#[test]
fn explorer_the_derived_motion_is_the_peak_over_each_pass() {
    let mut r = rig();
    r.svc.set_frequency(100.0).unwrap();
    r.svc.set_amplitude_vpk(0.2).unwrap();
    let st = r.passes(400);
    assert!(!st.derived_settling);
    // 0.2 g peak at 100 Hz on the wire: velocity 0.2 x 386.09 / (2 pi 100) in/s peak.
    let want = 0.2 * 386.088_6 / (std::f64::consts::TAU * 100.0);
    let mut instantaneous_low = false;
    for _ in 0..20 {
        let st = r.pass();
        let (v, _) = st.derived_peak;
        assert!(
            (v - want).abs() / want < 0.03,
            "the pass's peak velocity {v} against {want}"
        );
        if st.derived.0.abs() < 0.9 * want {
            instantaneous_low = true;
        }
    }
    assert!(
        instantaneous_low,
        "the last sample is a phase sample, and the peak is not"
    );
}

#[test]
fn explorer_every_driver_warning_is_counted_beyond_the_kept_list() {
    let mut r = rig_on(wire(None), true, 16);
    let st = r.passes(50);
    assert_eq!(st.driver_warnings.len(), 32, "the kept list is bounded");
    assert_eq!(
        st.driver_warnings_total, 50,
        "one drained per batch, every one counted"
    );
}

#[test]
fn explorer_each_drained_block_carries_its_first_input_frame() {
    // A small ring, and passes skipped so it overflows: the drained blocks are then the
    // newest, and their first frame is not where the last pass left off.
    let mut r = rig_on(wire(None), false, 4);
    let mut starts: Vec<(u64, u64)> = Vec::new();
    let take = |svc: &mut ExplorerHostService, out: &mut Vec<(u64, u64)>| {
        svc.service_analysis_framed(|at, b, s| out.push((at, (b.len() / s) as u64)));
    };
    take(&mut r.svc, &mut starts);
    for _ in 0..3 {
        r.svc.run_batches(1).unwrap();
        take(&mut r.svc, &mut starts);
    }
    for w in starts.windows(2) {
        assert_eq!(w[1].0, w[0].0 + w[0].1, "contiguous: {starts:?}");
    }
    assert_eq!(starts[0].0, 0, "the run's first input frame");
    let seen = starts.last().map(|(a, n)| a + n).unwrap();
    for _ in 0..5 {
        r.svc.run_batches(1).unwrap();
    }
    let mut after = Vec::new();
    take(&mut r.svc, &mut after);
    let st = r.svc.service_analysis();
    assert!(st.blocks_dropped > 0, "the ring overflowed");
    let end = after.last().map(|(a, n)| a + n).unwrap();
    assert_eq!(
        end,
        r.svc.frames_in(),
        "the newest block ends at the station's count"
    );
    assert!(
        after[0].0 > seen,
        "the dropped blocks are a hole in station time: {seen} then {:?}",
        after[0]
    );
}

#[test]
fn explorer_the_survey_line_keeps_its_peak_and_clears_to_a_new_revision() {
    let mut r = rig_on(resonant(), false, 16);
    r.svc.set_amplitude_vpk(0.2).unwrap();
    r.passes(60);
    let mut rev = r.svc.survey_revision();
    for hz in [150.0, 180.0, 200.0, 220.0, 260.0, 400.0, 550.0, 700.0] {
        r.svc.set_frequency(hz).unwrap();
        r.passes(30);
        assert!(r.svc.survey_revision() > rev, "a visit changes the line");
        rev = r.svc.survey_revision();
    }
    let st = r.passes(1);
    let points = r.svc.survey_points().to_vec();
    assert!(points.len() >= 8, "{points:?}");
    let top = points
        .iter()
        .copied()
        .fold((0.0, f32::MIN), |b, p| if p.1 > b.1 { p } else { b });
    assert_eq!(st.survey_peak, Some(top));
    assert!(
        (top.0 - 200.0).abs() < 25.0 || (top.0 - 550.0).abs() < 25.0,
        "the peak is near a mode: {top:?}"
    );
    let before = r.svc.survey_revision();
    let after = r.svc.clear_survey();
    assert_eq!(after, before + 1);
    assert!(r.svc.survey_points().is_empty());
    // The held tone is measured again from the next pass, and nothing else is.
    let st = r.passes(10);
    assert!(st.survey_revision > after);
    assert_eq!(r.svc.survey_points().len(), 1);
    assert_eq!(r.svc.survey_points()[0].0, 700.0);
    assert_eq!(
        st.survey_peak.map(|p| p.0),
        Some(700.0),
        "the peak is the new line's"
    );
}

#[test]
fn explorer_a_pass_and_the_gap_between_batches_are_timed() {
    let mut r = rig();
    r.svc.set_amplitude_vpk(0.2).unwrap();
    let st = r.passes(4);
    assert!(
        st.pass_last > Duration::ZERO,
        "a pass that drained blocks took time"
    );
    assert!(st.pass_worst >= st.pass_last);
    let quick = st.batch_gap_worst;
    r.svc.run_batches(1).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let st = r.pass();
    assert!(
        st.batch_gap_worst >= Duration::from_millis(30),
        "a 30 ms wait between two batches: {:?} (before {quick:?})",
        st.batch_gap_worst
    );
}

/// What the run host handed on.
#[derive(Default)]
struct Seen {
    survey: Vec<(u64, Vec<(f64, f32)>)>,
    intents: Vec<IntentRecord>,
    /// Each profile drawn, with how many survey lines had been handed on before it.
    drawn: Vec<(usize, Vec<f32>)>,
    spectra_df: f64,
    first_frames: Vec<(u64, u64)>,
    finished: bool,
}

struct Sink(Arc<Mutex<Seen>>);

impl HostSink for Sink {
    fn armed(&mut self, _fs_hz: f64, _display_nfft: usize) {}
    fn block(&mut self, _samples: &[f32], _stride: usize) {
        panic!("the pump hands blocks on with their first frame");
    }
    fn block_at(&mut self, first_frame: u64, samples: &[f32], stride: usize) {
        self.0
            .lock()
            .unwrap()
            .first_frames
            .push((first_frame, (samples.len() / stride) as u64));
    }
    fn dropped(&mut self, _frames: u64) {}
    fn spectra(&mut self, frames: &[SpectrumFrame], profile: Option<&[f32]>) -> bool {
        let mut s = self.0.lock().unwrap();
        s.spectra_df = frames[0].df_hz;
        if let Some(p) = profile {
            let n = s.survey.len();
            s.drawn.push((n, p.to_vec()));
        }
        true
    }
    fn survey(&mut self, revision: u64, points: &[(f64, f32)]) {
        self.0
            .lock()
            .unwrap()
            .survey
            .push((revision, points.to_vec()));
    }
    fn intents(&mut self, intents: &[IntentRecord]) {
        self.0.lock().unwrap().intents.extend_from_slice(intents);
    }
    fn publish(&mut self) {}
    fn finish(&mut self) {
        self.0.lock().unwrap().finished = true;
    }
    fn note(&mut self, _note: HostNote<'_>) {}
}

fn wait_for(host: &RunHost, what: &str, done: impl Fn(&HostView) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if done(&host.snapshot().view) {
            return;
        }
        assert!(Instant::now() < deadline, "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn explorer_the_run_host_hands_on_the_survey_the_intents_and_each_blocks_frame() {
    let svc = ExplorerHostService::new(cfg(64), table(), resonant(), StationClass::HertzforgeSim)
        .unwrap()
        .with_block_frames(BLOCK);
    let host = RunHost::new();
    host.install_paced(svc, None, Pacing::WallClock);
    let seen = Arc::new(Mutex::new(Seen::default()));
    host.start(Sink(seen.clone())).unwrap();
    wait_for(
        &host,
        "the run",
        |v| matches!(v, HostView::Status(st) if st.state == RunState::Running),
    );
    let mut answers = Vec::new();
    answers.push(
        host.steer(Some(FrequencyPart::Hz(180.0)), Some(LevelPart::Vpk(0.2)))
            .unwrap(),
    );
    for hz in [200.0, 300.0, 550.0] {
        std::thread::sleep(Duration::from_millis(250));
        answers.push(host.steer(Some(FrequencyPart::Hz(hz)), None).unwrap());
    }
    std::thread::sleep(Duration::from_millis(250));
    let cleared = host.clear_survey().unwrap();
    std::thread::sleep(Duration::from_millis(250));
    host.sweep(SweepDirection::Up, 60.0).unwrap();
    std::thread::sleep(Duration::from_millis(250));
    host.hold().unwrap();
    std::thread::sleep(Duration::from_millis(250));
    host.stop_for(StopCause::SafeStop);
    wait_for(
        &host,
        "the end",
        |v| matches!(v, HostView::Status(st) if st.state.is_terminal()),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !seen.lock().unwrap().finished {
        assert!(Instant::now() < deadline, "the pump finishes");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The status AFTER the pump's final pass: that pass drains the blocks still in the
    // ring and can record one more survey point, so a status read the moment the run
    // turned terminal can trail the last line handed on by one revision (seen once under
    // load).
    let end = match host.snapshot().view {
        HostView::Status(st) => st,
        other => panic!("{other:?}"),
    };
    assert_eq!(end.stop, Some(StopCause::SafeStop));
    let s = seen.lock().unwrap();

    // The intents, in order, as the answers named them.
    let steers: Vec<&IntentRecord> = s
        .intents
        .iter()
        .filter(|i| matches!(i.kind, IntentKind::Steer { .. }))
        .collect();
    assert_eq!(steers.len(), 4);
    for (rec, ans) in steers.iter().zip(&answers) {
        assert_eq!((rec.frame, rec.input_frame), (ans.frame, ans.input_frame));
    }
    let kinds: Vec<&str> = s
        .intents
        .iter()
        .map(|i| match i.kind {
            IntentKind::Steer { .. } => "steer",
            IntentKind::Sweep { .. } => "sweep",
            IntentKind::Hold { .. } => "hold",
            IntentKind::Stop { .. } => "stop",
        })
        .collect();
    assert_eq!(
        kinds,
        ["steer", "steer", "steer", "steer", "sweep", "hold", "stop"]
    );

    // The survey: the clear is handed on as an empty line at its revision, the last line
    // handed on is the run's last, and every profile drawn is the line handed on before
    // it, rendered.
    let (_, first_after) = s
        .survey
        .iter()
        .find(|(rev, _)| *rev > cleared)
        .expect("a line after the clear");
    assert_eq!(
        first_after.iter().map(|p| p.0).collect::<Vec<_>>(),
        [550.0],
        "after the clear only the tone held is on the line"
    );
    let rebuild = |pts: &[(f64, f32)]| {
        let mut r = ProfileRaster::new(0.0, s.spectra_df, 1_024 / 2 + 1);
        for (f, l) in pts {
            r.visit(*f, f64::from(*l));
        }
        assert_eq!(r.points(), pts, "a handed-on line is a raster's own points");
        r
    };
    let (last_rev, last) = s.survey.last().cloned().unwrap();
    assert_eq!(last_rev, end.survey_revision);
    assert!(!last.is_empty());
    assert_eq!(end.survey_peak, rebuild(&last).peak());
    assert!(s.drawn.len() > 5, "profiles were drawn");
    for (n, drawn) in &s.drawn {
        let mine = rebuild(&s.survey[n - 1].1).render().unwrap();
        assert_eq!(drawn.len(), mine.len());
        assert!(
            drawn
                .iter()
                .zip(&mine)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "the line handed on is the line drawn"
        );
    }

    // Every block in station time, end to end, no hole.
    assert_eq!(s.first_frames[0].0, 0);
    for w in s.first_frames.windows(2) {
        assert_eq!(w[1].0, w[0].0 + w[0].1);
    }
    let (a, n) = *s.first_frames.last().unwrap();
    assert_eq!(a + n, end.frames_in);
}
