//! THE CLICK-RATE INVARIANCE GATE: a hand-walked survey must record the same level at a
//! frequency however fast the operator clicks.
//!
//! # The operator's finding (2026-08-08)
//!
//! > "the max level is too slow to read as I manually sweep... [on the reference
//! > controller] I can click as fast as possible and it consistently produces the same
//! > value. Must be a loop time and/or averaging issue."
//!
//! The curve was right where he PAUSED and low where he clicked fast.
//!
//! # What this file measures, and why it is a WRAPPER and not the plain Sim
//!
//! The defect is a PIPELINE misalignment, and HERTZFORGE-SIM cannot show it: the Sim's
//! `OutputBufferContract::min_lookahead_frames` is 4, so the Explorer's lead-in puts 8
//! frames (~1 ms) of drive in flight and the capture is aligned to within a millisecond by
//! accident. The NI chassis contracts `lookahead_blocks (2) x block_frames` and the
//! lead-in doubles it, so the real number is **four block periods** - and since
//! `hertzforge_hal::default_block_frames` targets 20 ms, that is **~80 ms of signal on any
//! station**, 64.6 ms for the shipped Explorer profile (see
//! `ExplorerHostService::pipeline_frames` for the arithmetic).
//!
//! [`Pipelined`] is the smallest honest model of that: a backend wrapper that advertises a
//! DEEP lookahead and holds the drive it is written in its own FIFO before handing it to
//! the plant underneath. It is deliberately deeper than the chassis (250 ms against
//! 65 ms), because the property under test is that the capture reads the pipeline off the
//! station rather than assuming a number, and exaggerating it is what keeps the test from
//! passing by coincidence at some particular click rate.
//!
//! # The measured-frequency window, behind the same pipeline (2026-11-01)
//!
//! The last two tests here are the NI bench's retune row: a 100 -> 400 Hz retune read back
//! as 379.41 Hz while a meter on the jumper read 400. The window opened on samples that were
//! still the response to the drive from before the retune. [`Pipelined::chassis`] models
//! the chassis's own 551-frame pipeline and reproduces that number to the hundredth.
//!
//! # The instrumentation is the point
//!
//! [`walk`] prints one line per click - the commanded frequency, the pipeline the service
//! computed at the retune, the frames generated / consumed it came from, and the level the
//! survey recorded - BEFORE any assertion runs. The numbers, not the pass/fail, are what
//! settled the hypothesis.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_explorer_host::service::{ExplorerHostConfig, ExplorerHostService};
use hertzforge_explorer_host::slew::SlewLimits;
use hertzforge_explorer_host::step::AmplitudeRamp;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
    OutputBufferContract,
};
use hertzforge_module_abi::StationClass;
use std::collections::VecDeque;

const FS_HZ: f64 = 8_192.0;
const BLOCK: usize = 256;
/// The advertised lookahead, chosen so the modelled pipeline is 2048 frames = **250 ms**,
/// about four times the chassis's own 64.6 ms.
///
/// DELIBERATELY EXAGGERATED, and the reason is the THRESHOLD - which is the whole finding.
/// The point for a frequency is overwritten on every pass, so a contaminated early window
/// is flushed by later clean ones: the last level written before the operator clicks away
/// is wrong only while
///
/// ```text
/// dwell < pipeline + measurement window
/// ```
///
/// On the chassis that is 64.6 + 100 = **165 ms, i.e. faster than ~6 clicks/s**. A test
/// built at the chassis's own depth would therefore have to click faster than 6/s to say
/// anything, and would go quietly green the day someone changed `TARGET_BLOCK_SECONDS`.
/// Modelling a deeper pipeline puts the ordinary 3 and 5 clicks/s walks inside the band
/// where the rule is falsifiable, which is where a test belongs.
const LOOKAHEAD_FRAMES: u32 = (4 * BLOCK) as u32;
/// Frames of acquisition one analysis pass consumes: `BATCHES_PER_ITER (1) x
/// TICKS_PER_BATCH (2) x block`, the daemon's own pump cadence.
const PASS_FRAMES: usize = 2 * BLOCK;
const DRIVE_VPK: f64 = 0.25;
/// Where the walk parks after its last asserted stop. Inside the band, and NOT one of
/// [`STOPS`] - it exists only to give the final stop the departure every other stop got.
const PARK_HZ: f64 = 1_500.0;

/// The rate the chassis's acquisition module programs for an 8192 Hz request: the top of its
/// `13.1072 MHz / (256 n)` ladder at or above the request, `n = 6`.
const CHASSIS_RATE_HZ: f64 = 13_107_200.0 / (256.0 * 6.0);
/// The chassis acquisition module's own group delay at that rate, in frames - the figure
/// its backend reports and `ExplorerHostService::pipeline_frames` adds.
const CHASSIS_GROUP_DELAY: u32 = 39;

/// A backend wrapper that puts a DEEP output pipeline between the drive it is written and
/// the plant that responds to it.
///
/// Two halves, both needed:
/// * `caps().output_buffer.min_lookahead_frames` is raised to [`LOOKAHEAD_FRAMES`], which
///   is what makes the Explorer's lead-in pre-load the pipeline and the engine's
///   `OutputDelivery` hold it there for the rest of the run.
/// * `write_drive` parks the frames in `fifo`; `read_input` hands the inner plant exactly
///   one block's worth from the FRONT of that FIFO before pulling the response. So the
///   response the host reads back is the drive it wrote `fifo.len()` frames ago.
struct Pipelined {
    inner: Box<dyn Backend>,
    fifo: VecDeque<f32>,
    n_drive: usize,
    block_frames: usize,
    /// The lookahead `caps()` advertises, in frames.
    lookahead_frames: u32,
    /// The rate `configure` COERCES a request to before the plant sees it, the way the
    /// chassis's delta-sigma ladder does (`None` passes the request through).
    coerce_rate_hz: Option<f64>,
    /// An acquisition-path group delay, REPORTED in `ConfiguredActual` and MODELLED as that
    /// many zero frames at the front of the FIFO, so the response to drive frame `n` reaches
    /// input frame `n + group_delay_frames` - which is what the service's pipeline
    /// arithmetic adds the reported figure for.
    group_delay_frames: u32,
}

impl Pipelined {
    /// The exaggerated 250 ms pipeline the survey walks run behind (see [`LOOKAHEAD_FRAMES`]).
    fn new(inner: Box<dyn Backend>) -> Self {
        Pipelined {
            inner,
            fifo: VecDeque::new(),
            n_drive: 1,
            block_frames: BLOCK,
            lookahead_frames: LOOKAHEAD_FRAMES,
            coerce_rate_hz: None,
            group_delay_frames: 0,
        }
    }

    /// The CHASSIS's own geometry, as `ExplorerHostService::pipeline_frames` documents it
    /// for the shipped Explorer profile: a request coerced to [`CHASSIS_RATE_HZ`], the
    /// contracted `2 x block` lookahead, and the [`CHASSIS_GROUP_DELAY`]-frame acquisition
    /// filter delay. 551 frames = 64.6 ms in all, at the block the caller configures.
    fn chassis(inner: Box<dyn Backend>, block_frames: usize) -> Self {
        Pipelined {
            inner,
            fifo: VecDeque::new(),
            n_drive: 1,
            block_frames,
            lookahead_frames: (2 * block_frames) as u32,
            coerce_rate_hz: Some(CHASSIS_RATE_HZ),
            group_delay_frames: CHASSIS_GROUP_DELAY,
        }
    }
}

impl Backend for Pipelined {
    fn caps(&self) -> Caps {
        let mut c = self.inner.caps();
        c.output_buffer = Some(OutputBufferContract {
            min_lookahead_frames: self.lookahead_frames,
            underrun_policy: c
                .output_buffer
                .map(|o| o.underrun_policy)
                .unwrap_or(hertzforge_hal::UnderrunPolicy::RampDownThenAbort),
        });
        c
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        t: &ChannelTable,
        a: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        let req = AcquisitionConfig::new(
            self.coerce_rate_hz.unwrap_or(a.requested_rate_hz),
            a.block_frames,
        );
        let mut actual = self.inner.configure(t, &req)?;
        self.n_drive = t
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .count()
            .max(1);
        self.block_frames = actual.block_frames.max(1);
        self.fifo.clear();
        if self.group_delay_frames > 0 {
            self.fifo.extend(std::iter::repeat_n(
                0.0f32,
                self.group_delay_frames as usize * self.n_drive,
            ));
            actual.group_delay_frames = Some(self.group_delay_frames);
        }
        Ok(actual)
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
        // Hand the plant ONE block from the front of the pipeline, then let it respond.
        let want = self.block_frames * self.n_drive;
        let take = want.min(self.fifo.len());
        if take > 0 {
            let chunk: Vec<f32> = self.fifo.drain(..take).collect();
            let _ = self.inner.write_drive(&chunk);
        }
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, d: &[f32]) -> Result<usize, HalError> {
        self.fifo.extend(d.iter().copied());
        Ok(d.len() / self.n_drive.max(1))
    }
    fn output_space(&mut self) -> Option<usize> {
        // Unbounded on purpose: the DEPTH under test is the one the lookahead contract
        // creates, not a back-pressure limit this wrapper invented.
        None
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}

/// A plant that is a WIRE: no resonance, unity gain, no noise, no transport delay. Every
/// frequency answers at exactly the commanded volts, so the ONLY thing that can move a
/// recorded level is the capture rule.
fn wire_plant() -> Box<dyn Backend> {
    Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(FS_HZ)
            .resonances(Vec::new())
            .feedthrough(1.0)
            .noise_level(0.0)
            .transport_delay_samples(0)
            .build(),
    )
}

/// A plant with STRUCTURE: two modes, so the level is a different number at every stop.
///
/// # Why the wire cannot be the only plant here
///
/// A mean-removed RMS of a sine is its amplitude WHATEVER its frequency, so on a wire a
/// window contaminated with the previous tone still reads the right number - the flat
/// jumper is blind to a frequency MISATTRIBUTION by construction (a result that cannot fail
/// is not evidence). The jumper's job is the invariance floor; this plant is the
/// discriminator, and it is also the bench the operator was standing at when he reported
/// the defect - a shaker has modes.
fn resonant_plant() -> Box<dyn Backend> {
    Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(FS_HZ)
            .resonances(vec![(300.0, 12.0), (720.0, 9.0)])
            .feedthrough(1.0)
            .noise_level(0.0)
            .transport_delay_samples(0)
            .build(),
    )
}

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::VoltageV;
    ctrl.range_max = 10.0;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    t
}

/// The slew the capture-rule walks run at.
///
/// A FAST slew, stated rather than defaulted: those walks are about the CAPTURE rule, and
/// before manual retunes became instant a 1 oct/s ceiling would have put most of a fast
/// click inside the travel, which was a slew question with its own answer. It is kept at
/// 40 now that a click no longer reads the ceiling at all, so those rows keep measuring
/// what they always measured; [`SHIPPED_SLEW_OCT_PER_S`] is the row that exercises the
/// ceiling an operator actually ships with.
const FAST_SLEW_OCT_PER_S: f64 = 40.0;

/// The SHIPPED frequency-slew ceiling (`SlewLimits::default().freq_oct_per_s`).
///
/// The coarse-walk gate runs at this, and that is the point of it: it is the value on a
/// real operator's panel, and the value at which a 20 % click used to spend 263 ms
/// travelling.
const SHIPPED_SLEW_OCT_PER_S: f64 = 1.0;

fn cfg_at_slew(freq_oct_per_s: f64) -> ExplorerHostConfig {
    ExplorerHostConfig {
        fs_hz: FS_HZ,
        band_hz: (20.0, 2_000.0),
        start_hz: 100.0,
        max_drive_vpk: 1.0,
        slew: SlewLimits {
            freq_oct_per_s,
            drive_db_per_s: 60.0,
        },
        ramp: AmplitudeRamp::default(),
        display_nfft: 1_024,
        ring_blocks: 64,
    }
}

/// One recorded survey point, with the evidence that produced it.
#[derive(Debug, Clone, Copy)]
struct Stop {
    hz: f64,
    /// The pipeline the SERVICE computed at the moment of the retune.
    pipeline_frames: usize,
    frames_generated: u64,
    /// The level the survey ended up holding for this frequency, if any.
    level: Option<f64>,
}

/// Walk `freqs` at `dwell_frames` of signal per click and return what the survey recorded.
///
/// The survey memory here is the same rule the daemon's `ProfileRaster` applies: the
/// points the service drains are folded in by frequency, a revisit overwriting. Keeping it
/// in this file rather than depending on `hertzforge-backendd` keeps the gate on the CAPTURE.
fn walk(
    label: &str,
    plant: fn() -> Box<dyn Backend>,
    freqs: &[f64],
    dwell_frames: usize,
) -> Vec<Stop> {
    walk_at_slew(label, plant, freqs, dwell_frames, FAST_SLEW_OCT_PER_S)
}

/// [`walk`] with the frequency-slew ceiling named, for the coarse-walk gate.
fn walk_at_slew(
    label: &str,
    plant: fn() -> Box<dyn Backend>,
    freqs: &[f64],
    dwell_frames: usize,
    freq_oct_per_s: f64,
) -> Vec<Stop> {
    let mut svc = ExplorerHostService::new(
        cfg_at_slew(freq_oct_per_s),
        table(),
        Box::new(Pipelined::new(plant())),
        StationClass::HertzforgeSim,
    )
    .expect("the plant behind a deep pipeline is a valid Explorer station")
    .with_block_frames(BLOCK);
    svc.arm().expect("HERTZFORGE-SIM arms open-loop");
    svc.start().expect("armed -> running");

    // Survey memory: (frequency, level), last write wins.
    let mut memory: Vec<(f64, f64)> = Vec::new();
    let drain = |svc: &mut ExplorerHostService, memory: &mut Vec<(f64, f64)>| {
        for p in svc.take_profile_points() {
            match memory
                .iter_mut()
                .find(|(f, _)| (*f - p.freq_hz).abs() < 1e-9)
            {
                Some(slot) => slot.1 = p.amplitude,
                None => memory.push((p.freq_hz, p.amplitude)),
            }
        }
    };

    // Get to level first. The drive ramp is not what this file measures.
    svc.set_amplitude_vpk(DRIVE_VPK).unwrap();
    for _ in 0..600 {
        let alive = svc.run_batches(1).expect("the ramp must not fault");
        let st = svc.service_analysis();
        drain(&mut svc, &mut memory);
        if !alive || (st.commanded_vpk - DRIVE_VPK).abs() < 1e-12 {
            break;
        }
    }
    // Let the pipeline fill with the settled level before the first click, so stop 1 is
    // not measuring the tail of the ramp.
    for _ in 0..40 {
        svc.run_batches(1).expect("no fault");
        svc.service_analysis();
        drain(&mut svc, &mut memory);
    }
    memory.clear();

    // The dwell in PASSES, rounded rather than ceiled: the pass is the granularity the
    // pump actually clicks in, and ceiling every dwell up to the next whole pass would
    // quietly hand every walk a longer dwell than the click rate it claims.
    let passes = ((dwell_frames as f64) / PASS_FRAMES as f64)
        .round()
        .max(1.0) as usize;
    println!(
        "\n== {label} ==  dwell {} frames = {:.0} ms = {passes} pass(es), \
         freq slew {freq_oct_per_s} oct/s",
        passes * PASS_FRAMES,
        (passes * PASS_FRAMES) as f64 / FS_HZ * 1e3
    );
    let mut stops = Vec::new();
    for &hz in freqs {
        svc.set_frequency(hz).unwrap();
        let st = svc.status_cell().read().expect("a published status");
        let at = Stop {
            hz,
            pipeline_frames: st.pipeline_frames,
            frames_generated: st.frames_generated,
            level: None,
        };
        for _ in 0..passes {
            svc.run_batches(1).expect("the walk must not fault");
            svc.service_analysis();
            drain(&mut svc, &mut memory);
        }
        stops.push(at);
    }
    // THE PARK CLICK. Every asserted stop must get a real DEPARTURE, and the last one has
    // none until the operator clicks away from it - its response is still in the pipeline
    // when the walk's clicks run out. So the walk ends with one more click, to a frequency
    // nothing asserts, and runs long enough for that click to ARRIVE. This is not extra
    // dwell smuggled onto the last stop: it is the same departure every other stop got.
    svc.set_frequency(PARK_HZ).unwrap();
    let flush_passes = (LOOKAHEAD_FRAMES as usize * 2) / PASS_FRAMES + 2;
    for _ in 0..flush_passes {
        svc.run_batches(1).expect("the flush must not fault");
        svc.service_analysis();
        drain(&mut svc, &mut memory);
    }
    let _ = svc.abort("walk complete");

    for s in stops.iter_mut() {
        s.level = memory
            .iter()
            .find(|(f, _)| (*f - s.hz).abs() < 1e-6)
            .map(|(_, a)| *a);
    }
    for s in stops.iter() {
        println!(
            "  {:7.1} Hz  pipeline {:5} frames ({:6.1} ms)  generated {:8}  level {}",
            s.hz,
            s.pipeline_frames,
            s.pipeline_frames as f64 / FS_HZ * 1e3,
            s.frames_generated,
            match s.level {
                Some(v) => format!(
                    "{v:.5} V  ({:+6.1} % of drive)",
                    (v / DRIVE_VPK - 1.0) * 100.0
                ),
                None => "-- not recorded --".to_string(),
            }
        );
    }
    stops
}

/// The frequencies the walks visit. Spread over the band, and none of them a harmonic of
/// another, so a level borrowed from the previous stop cannot pass as this one's.
const STOPS: [f64; 8] = [120.0, 190.0, 275.0, 360.0, 470.0, 610.0, 780.0, 950.0];

/// **THE ACCEPTANCE PROPERTY** (the operator's own): on a flat plant, every recorded level
/// is the drive, whatever the click rate.
///
/// 1 click/s, 3 clicks/s and 5 clicks/s over the same stops. The 5/s walk is allowed to
/// SKIP a stop (a dwell too short to hold a clean window records nothing, which is the
/// honesty rule the traversal wipe established) but it is NOT allowed to record a
/// low value.
///
/// RED-MUTATION: align the post-retune guard to the COMMAND instead of to the arrival
/// (drop the `pipeline_frames` term in `ProfileTracer::observe_pipelined`) and the 3/s and
/// 5/s walks red on the VALUES - measured 2026-08-08, see the file header.
#[test]
fn a_walk_records_the_same_level_however_fast_the_operator_clicks() {
    let per_click = |clicks_per_s: f64| (FS_HZ / clicks_per_s).round() as usize;

    let slow = walk("WIRE, 1 click/s", wire_plant, &STOPS, per_click(1.0));
    let mid = walk("WIRE, 3 clicks/s", wire_plant, &STOPS, per_click(3.0));
    let fast = walk("WIRE, 5 clicks/s", wire_plant, &STOPS, per_click(5.0));

    // The pipeline the service computed must be the one the wrapper models: the lead-in
    // generates 2 x lookahead and the delivery queue holds the contract for the rest of
    // the run. A pipeline that read ~0 here would mean this file is not testing anything.
    for s in slow.iter().chain(mid.iter()).chain(fast.iter()) {
        assert!(
            s.pipeline_frames >= LOOKAHEAD_FRAMES as usize,
            "the modelled pipeline collapsed to {} frames at {} Hz; this gate would pass \
             for free",
            s.pipeline_frames,
            s.hz
        );
    }

    // THE INVARIANCE. Every level that was recorded is the drive.
    let mut recorded_fast = 0;
    for walked in [&slow, &mid, &fast] {
        for s in walked.iter() {
            let Some(v) = s.level else { continue };
            let err = (v - DRIVE_VPK).abs() / DRIVE_VPK;
            assert!(
                err < 0.05,
                "{} Hz recorded {v:.5} V against a {DRIVE_VPK} V drive on a WIRE \
                 ({:+.1} %) - a survey level that depends on the click rate",
                s.hz,
                (v / DRIVE_VPK - 1.0) * 100.0
            );
        }
    }
    for s in fast.iter() {
        if s.level.is_some() {
            recorded_fast += 1;
        }
    }

    // The slow and mid walks must record EVERY stop - skipping is only the fast walk's
    // allowance, and a rule that recorded nothing anywhere would satisfy the loop above.
    for (walked, name) in [(&slow, "1 click/s"), (&mid, "3 clicks/s")] {
        for s in walked.iter() {
            assert!(
                s.level.is_some(),
                "the {name} walk recorded nothing at {} Hz; a dwell that long must produce \
                 a point",
                s.hz
            );
        }
    }
    assert!(
        recorded_fast >= STOPS.len() / 2,
        "the 5 clicks/s walk recorded only {recorded_fast}/{} stops - skipping is allowed, \
         abandoning the survey is not",
        STOPS.len()
    );

    // And the walks AGREE with each other stop by stop, which is the property stated the
    // way the operator stated it.
    for (a, b) in slow.iter().zip(mid.iter()) {
        if let (Some(x), Some(y)) = (a.level, b.level) {
            assert!(
                (x - y).abs() / x < 0.05,
                "{} Hz reads {x:.5} V at 1 click/s and {y:.5} V at 3 clicks/s",
                a.hz
            );
        }
    }
    for (a, b) in slow.iter().zip(fast.iter()) {
        if let (Some(x), Some(y)) = (a.level, b.level) {
            assert!(
                (x - y).abs() / x < 0.05,
                "{} Hz reads {x:.5} V at 1 click/s and {y:.5} V at 5 clicks/s",
                a.hz
            );
        }
    }
}

/// **THE DISCRIMINATOR**: on a plant whose level is a different number at every stop, does
/// a fast walk record THIS frequency's level or the PREVIOUS one's?
///
/// The reference is a PAUSED walk - one full second of dwell per stop, which is what the
/// operator means by "the curve is right where I pause". Every faster walk is compared
/// against it stop by stop. No analytic plant model is needed and none is asserted: the
/// claim is the operator's own, that the number must not depend on how long he waited.
///
/// This is the test the wire plant cannot be (a mean-removed RMS of a sine is its
/// amplitude whatever its frequency, so a misattributed window still reads right on a
/// jumper). It is printed before it is asserted; the numbers are the evidence.
#[test]
fn a_fast_walk_over_a_structured_plant_reads_the_frequency_it_is_at() {
    let per_click = |clicks_per_s: f64| (FS_HZ / clicks_per_s).round() as usize;

    let paused = walk(
        "RESONANT, paused (1 click/s)",
        resonant_plant,
        &STOPS,
        per_click(1.0),
    );
    let mid = walk(
        "RESONANT, 3 clicks/s",
        resonant_plant,
        &STOPS,
        per_click(3.0),
    );
    let fast = walk(
        "RESONANT, 5 clicks/s",
        resonant_plant,
        &STOPS,
        per_click(5.0),
    );

    // The plant must actually HAVE structure, or this gate is the wire gate again.
    let levels: Vec<f64> = paused.iter().filter_map(|s| s.level).collect();
    assert!(
        levels.len() == STOPS.len(),
        "the paused reference did not record every stop: {paused:#?}"
    );
    let (lo, hi) = levels
        .iter()
        .fold((f64::MAX, 0.0f64), |(l, h), &v| (l.min(v), h.max(v)));
    assert!(
        hi / lo > 3.0,
        "the reference curve spans only {:.2}x ({lo:.4}..{hi:.4} V); a plant this flat \
         cannot tell a misattributed window from a correct one",
        hi / lo
    );

    println!("\n== RESONANT: fast walks against the paused reference ==");
    let mut worst = 0.0f64;
    for (i, s) in STOPS.iter().enumerate() {
        let r = paused[i].level.expect("checked above");
        let m = mid[i].level;
        let f = fast[i].level;
        println!(
            "  {s:7.1} Hz  paused {r:.5} V   3/s {}   5/s {}",
            match m {
                Some(v) => format!("{v:.5} V ({:+7.1} %)", (v / r - 1.0) * 100.0),
                None => "-- not recorded --".to_string(),
            },
            match f {
                Some(v) => format!("{v:.5} V ({:+7.1} %)", (v / r - 1.0) * 100.0),
                None => "-- not recorded --".to_string(),
            }
        );
        for v in [m, f].into_iter().flatten() {
            worst = worst.max((v / r - 1.0).abs());
        }
    }
    println!(
        "  worst deviation from the paused reference: {:+.1} %",
        worst * 100.0
    );

    for (walked, name) in [(&mid, "3 clicks/s"), (&fast, "5 clicks/s")] {
        for (i, s) in STOPS.iter().enumerate() {
            let Some(v) = walked[i].level else { continue };
            let r = paused[i].level.expect("checked above");
            let err = (v / r - 1.0).abs();
            assert!(
                err < 0.10,
                "{s} Hz reads {v:.5} V at {name} against {r:.5} V paused ({:+.1} %) - the \
                 survey level depends on how long the operator waited",
                (v / r - 1.0) * 100.0
            );
        }
    }
}

/// The COARSE walk, at the SHIPPED slew: 20 %-per-click steps across a decade, three
/// clicks a second, and every stop must anchor a point.
///
/// # What this gate is for
///
/// The operator's workflow is the coarse button, clicked along "as fast as possible" and
/// read off the screen. A 20 % click is `log2(1.2)` = **0.263 octaves**, which at the
/// shipped 1 oct/s ceiling was **263 ms of TRAVEL**. The capture records nothing while the
/// tone is moving - a frequency change that is not a sweep queues a fresh pending tone and
/// re-arms the transient guard - so at three clicks a second (a 333 ms dwell) almost the
/// whole dwell was travel and the survey anchored few or no points. That is the floor
/// the bench observed, and it was a SLEW defect wearing a capture defect's costume.
///
/// Manual retunes now land in ONE FRAME (`hertzforge_explorer_host::step`), so the only things
/// between a click and its point are the pipeline, the post-arrival guard and the
/// measurement window.
///
/// # Why the WIRE plant and not the resonant one
///
/// The claim is COVERAGE - that a stop is anchored at all - and a flat plant makes the
/// expected level a single known number, so a recorded point can be checked for being the
/// right VALUE and not merely present. The frequency-misattribution question belongs to
/// [`a_fast_walk_over_a_structured_plant_reads_the_frequency_it_is_at`], which is where a
/// structured plant is the load-bearing part.
///
/// RED-MUTATION, run 2026-08-09: force the `jump_pending` branch in
/// `ExplorerProcessStep::slew_frequency` off (paced clicks again) and this test fails on
/// coverage - the printed per-stop table is the evidence.
#[test]
fn a_coarse_walk_at_the_shipped_slew_anchors_every_stop() {
    // 100 Hz upward in 20 % steps until the decade is covered: 100 -> ~1032 Hz.
    let mut stops = vec![100.0f64];
    while *stops.last().expect("seeded") < 1_000.0 {
        stops.push(stops.last().expect("seeded") * 1.2);
    }
    let per_click = (FS_HZ / 3.0).round() as usize; // ~3 clicks/s, the operator's cadence

    let walked = walk_at_slew(
        "WIRE, COARSE 20 %/click at 3 clicks/s, SHIPPED slew",
        wire_plant,
        &stops,
        per_click,
        SHIPPED_SLEW_OCT_PER_S,
    );

    // The pipeline must still be the modelled one - a collapsed pipeline would make this
    // gate pass for free, exactly as in the invariance test.
    for s in walked.iter() {
        assert!(
            s.pipeline_frames >= LOOKAHEAD_FRAMES as usize,
            "the modelled pipeline collapsed to {} frames at {:.1} Hz",
            s.pipeline_frames,
            s.hz
        );
    }

    let recorded = walked.iter().filter(|s| s.level.is_some()).count();
    println!(
        "\n  coarse walk: {recorded}/{} stops anchored a profile point at the shipped \
         {SHIPPED_SLEW_OCT_PER_S} oct/s ceiling",
        stops.len()
    );
    assert_eq!(
        recorded,
        stops.len(),
        "the coarse walk anchored only {recorded}/{} stops. A 20 % click is 0.263 oct; if \
         the tone is still travelling when the next click lands the survey has nothing to \
         record, and the operator's curve has holes exactly where he clicked",
        stops.len()
    );

    // And every anchored point is the drive: coverage bought by recording a mid-travel
    // frequency's level would satisfy the count above and is not what was asked for.
    for s in walked.iter() {
        let v = s.level.expect("every stop anchored, asserted above");
        assert!(
            (v - DRIVE_VPK).abs() / DRIVE_VPK < 0.05,
            "{:.1} Hz recorded {v:.5} V against a {DRIVE_VPK} V drive on a WIRE ({:+.1} %)",
            s.hz,
            (v / DRIVE_VPK - 1.0) * 100.0
        );
    }
}

// ---------------------------------------------------------------------------------------
// THE MEASURED-FREQUENCY WINDOW behind the same pipeline (2026-11-01).
// ---------------------------------------------------------------------------------------

/// What one Explorer run's measured-frequency window read back, with the evidence.
#[derive(Debug, Clone, Copy)]
struct MeterRun {
    fs_hz: f64,
    block_frames: usize,
    /// The pipeline the SERVICE computed at the retune.
    pipeline_frames: usize,
    /// The first reading to mature after the level was raised off 0.000 V at 100 Hz, and
    /// the analysis passes it took.
    liftoff: Option<(f64, usize)>,
    /// The first reading to mature after the 100 -> 400 Hz retune, and the passes it took.
    retune: Option<(f64, usize)>,
}

/// The chassis gate's own Explorer sequence (`ni_module_sweep.rs`, rows 1-3), run against
/// a wire behind `backend`: arm at 0 V, raise the level to 0.2 Vpk at 100 Hz, settle, retune
/// to 400 Hz, and take the FIRST reading the measured window matures after each change.
///
/// The first reading is the one that matters. The window resets on every change and the
/// panel prints whatever matures first beside the commanded frequency as its cross-check,
/// so a window that matures on samples still carrying the previous state is exactly the
/// number an operator reads.
fn meter_run(label: &str, backend: Pipelined, block_frames: usize) -> MeterRun {
    const DRIVE: f64 = 0.2;
    const MAX_PASSES: usize = 2_000;
    let cfg = ExplorerHostConfig {
        fs_hz: FS_HZ,
        band_hz: (50.0, 2_000.0),
        start_hz: 100.0,
        max_drive_vpk: 0.5,
        slew: SlewLimits {
            freq_oct_per_s: 8.0,
            drive_db_per_s: 30.0,
        },
        ramp: AmplitudeRamp::default(),
        display_nfft: 2_048,
        ring_blocks: 16,
    };
    let mut svc =
        ExplorerHostService::new(cfg, table(), Box::new(backend), StationClass::HertzforgeSim)
            .expect("a wire behind the pipeline is a valid Explorer station")
            .with_block_frames(block_frames);
    svc.arm().expect("HERTZFORGE-SIM arms open-loop");
    svc.start().expect("armed -> running");
    let pass = |svc: &mut ExplorerHostService| {
        svc.run_batches(1).expect("the meter run must not fault");
        svc.service_analysis()
    };

    for _ in 0..8 {
        pass(&mut svc);
    }

    // LIFT-OFF: the level leaves 0.000 V with the tone held at 100 Hz.
    svc.set_amplitude_vpk(DRIVE).unwrap();
    let mut liftoff = None;
    for n in 1..=MAX_PASSES {
        let st = pass(&mut svc);
        if liftoff.is_none() {
            liftoff = st.measured_freq_hz.map(|hz| (hz, n));
        }
        if liftoff.is_some() && (st.commanded_vpk - DRIVE).abs() < 1e-9 {
            break;
        }
    }
    for _ in 0..16 {
        pass(&mut svc);
    }

    // THE RETUNE, the bench row that failed.
    let commanded = svc.set_frequency(400.0).unwrap();
    assert_eq!(commanded, 400.0);
    let mut retune = None;
    let mut pipeline_frames = 0;
    for n in 1..=MAX_PASSES {
        let st = pass(&mut svc);
        if n == 1 {
            pipeline_frames = st.pipeline_frames;
        }
        if let Some(hz) = st.measured_freq_hz {
            retune = Some((hz, n));
            break;
        }
    }
    let fs_hz = svc.actual_fs_hz();
    let _ = svc.abort("meter run complete");

    let run = MeterRun {
        fs_hz,
        block_frames,
        pipeline_frames,
        liftoff,
        retune,
    };
    let show = |r: Option<(f64, usize)>, want: f64| match r {
        Some((hz, n)) => format!(
            "{hz:8.3} Hz ({:+6.2} % of {want} Hz) after {n} pass(es)",
            (hz / want - 1.0) * 100.0
        ),
        None => "-- never matured --".to_string(),
    };
    println!(
        "\n== {label} ==  fs {:.3} Hz, block {}, pipeline {} frames ({:.1} ms)\n  \
         lift-off @ 100 Hz: {}\n  retune 100 -> 400 Hz: {}",
        run.fs_hz,
        run.block_frames,
        run.pipeline_frames,
        run.pipeline_frames as f64 / run.fs_hz * 1e3,
        show(run.liftoff, 100.0),
        show(run.retune, 400.0),
    );
    run
}

/// Assert both first readings of `run` within the chassis gate's 2 %.
fn assert_meter_reads_the_settled_tone(run: &MeterRun) {
    for (what, reading, want) in [
        ("lift-off at 100 Hz", run.liftoff, 100.0),
        ("retune 100 -> 400 Hz", run.retune, 400.0),
    ] {
        let (hz, _) = reading.unwrap_or_else(|| panic!("{what}: no reading ever matured"));
        let err = (hz - want).abs() / want;
        assert!(
            err < 0.02,
            "{what}: the first matured reading was {hz:.3} Hz against {want} Hz ({:+.2} %) \
             behind a {}-frame pipeline - the window took samples that were still the \
             response to the drive from BEFORE the change",
            (hz / want - 1.0) * 100.0,
            run.pipeline_frames
        );
    }
}

/// **THE BENCH ROW, AT THE CHASSIS'S OWN DEPTH.** The NI gate (2026-11-01) read a
/// 100 -> 400 Hz retune back as 379.41 Hz, 5.15 % low, while a meter on the jumper read the
/// tone at 400 Hz: the drive was right and the READOUT was wrong.
///
/// The arithmetic says why, and this test reproduces it. A manual retune lands in one
/// generated frame, but the drive is written `pipeline_frames` ahead of the acquisition
/// (here 2 x 256 frames of lookahead plus a 39-frame filter delay = 551). The window used
/// to open on the second pass after the retune, whose samples began `551 - 256 = 295`
/// frames before the new tone arrived. 295 frames of 100 Hz in a 17-pass, 4352-frame
/// window leave 387 crossings where 400 Hz alone would give 408: 379.41 Hz.
///
/// RED, measured 2026-11-01 on the code before either measured-frequency fix: the retune
/// read **379.412 Hz
/// (-5.15 %), the bench's number to the hundredth**, and the lift-off 88.235 Hz (-11.76 %:
/// the zeros still in flight when the level left 0 V). With only the block-boundary fix
/// beneath it: 379.499 Hz and 88.256 Hz. GREEN: 400.74 Hz and 100.93 Hz, each
/// within one crossing of the window.
#[test]
fn the_measured_frequency_reads_the_new_tone_behind_the_chassis_pipeline() {
    let block = hertzforge_hal::default_block_frames(FS_HZ);
    let run = meter_run(
        "METER, wire behind the CHASSIS pipeline",
        Pipelined::chassis(wire_plant(), block),
        block,
    );
    // The geometry must be the chassis's, or this is not the bench row: the rate coerced,
    // and the service's pipeline = 2 x lookahead + group delay = 551 frames.
    assert!(
        (run.fs_hz - CHASSIS_RATE_HZ).abs() < 1e-9,
        "the service did not adopt the coerced rate: {} Hz",
        run.fs_hz
    );
    assert_eq!(
        run.pipeline_frames,
        2 * (2 * block) + CHASSIS_GROUP_DELAY as usize,
        "the modelled pipeline is not the chassis's 551 frames; this test would not be \
         reproducing the bench row"
    );
    assert_meter_reads_the_settled_tone(&run);
}

/// The same property behind the EXAGGERATED pipeline the survey walks use (250 ms), so it
/// cannot pass by the pipeline happening to be short against the window: before the fix
/// a quarter-second of the previous state sat in a half-second window.
///
/// RED, measured 2026-11-01 on the code before either measured-frequency fix: the retune
/// read 276.000 Hz (-31.0 %)
/// and the lift-off 48.000 Hz (-52.0 %); with only the block-boundary fix beneath it,
/// 287.070 Hz and 49.012 Hz. GREEN: 399.10 Hz and 99.02 Hz, each within one crossing.
#[test]
fn the_measured_frequency_reads_the_new_tone_behind_a_deep_pipeline() {
    let run = meter_run(
        "METER, wire behind a 250 ms pipeline",
        Pipelined::new(wire_plant()),
        BLOCK,
    );
    assert!(
        run.pipeline_frames >= 2 * LOOKAHEAD_FRAMES as usize,
        "the modelled pipeline collapsed to {} frames; this gate would pass for free",
        run.pipeline_frames
    );
    assert_meter_reads_the_settled_tone(&run);
}
