//! The drive path plays one stream per output.
//!
//! `PlaybackStep` used to push one drive sample per frame while the engine took
//! `out_channels` samples per frame, so with two outputs one stream was spread
//! across both and half its frames were dropped. The multiple-output path
//! (`PlantLoop::open_outputs`, `DriveSynth::segment_factored`) interleaves one
//! sample per output per frame, the lead-in included.
//!
//! RED-MUTATION: make `PlaybackStep::process` emit one sample per frame again
//! (`drive_out.push(self.drive[self.pos] as f32)`) and
//! `two_outputs_each_play_their_own_stream` reds: the wire reads output 1's
//! samples as output 0's next frame.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
    OutputBufferContract, UnderrunPolicy,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::engine::{DriveFactor, DriveSynth, PlantLoop};
use hertzforge_random::LoopConfig;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LOOKAHEAD_FRAMES: u32 = 8;
const OUTPUTS: usize = 2;

/// Every `write_drive` payload, in arrival order.
type Wire = Arc<Mutex<Vec<f32>>>;

/// A two-output Sim station that declares an output-buffer contract and records
/// every sample written to its outputs.
struct RecordingStation {
    inner: SimBackend,
    wire: Wire,
}

impl Backend for RecordingStation {
    fn caps(&self) -> Caps {
        Caps {
            abort_latency_bound: Some(Duration::from_millis(20)),
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: LOOKAHEAD_FRAMES,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            ..self.inner.caps()
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
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
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let accepted = self.inner.write_drive(drive)?;
        self.wire
            .lock()
            .unwrap()
            .extend_from_slice(&drive[..accepted * OUTPUTS]);
        Ok(accepted)
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

fn station(wire: Wire) -> Box<dyn Backend> {
    Box::new(RecordingStation {
        inner: SimConfig::new()
            .sample_rate_hz(8192.0)
            .resonances(vec![(200.0, 2.5)])
            .noise_level(2e-4)
            .shared_clock(true)
            .seed(0x2D_2D)
            .max_input_channels(1)
            .max_output_channels(OUTPUTS as u32)
            .build(),
        wire,
    })
}

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 100.0;
    t.channels.push(ctrl);
    for k in 0..OUTPUTS {
        let mut drv = Channel::new(2 + k as u32);
        drv.role = ChannelRole::Drive;
        drv.units = Units::VoltageV;
        t.channels.push(drv);
    }
    t
}

/// Frame `f` of output `d`: distinct per output and per frame, never zero.
fn sample(d: usize, f: usize) -> f64 {
    if d == 0 {
        1e-3 * (f as f64 + 1.0)
    } else {
        -0.5 - 1e-3 * (f as f64 + 1.0)
    }
}

fn interleaved(frames: usize, offset: usize) -> Vec<f64> {
    let mut v = Vec::with_capacity(frames * OUTPUTS);
    for f in 0..frames {
        for d in 0..OUTPUTS {
            v.push(sample(d, f + offset));
        }
    }
    v
}

#[test]
fn two_outputs_each_play_their_own_stream() {
    let wire: Wire = Arc::new(Mutex::new(Vec::new()));
    let mut cfg = LoopConfig::demo_default();
    let mut plant = PlantLoop::open_outputs(
        station(Arc::clone(&wire)),
        StationClass::HertzforgeSim,
        &mut cfg,
        table(),
    )
    .expect("a two-output loop opens");
    assert_eq!(plant.n_outputs(), OUTPUTS);
    let lead = plant.cold_leadin_frames();
    assert_eq!(
        lead,
        3 * LOOKAHEAD_FRAMES as usize,
        "a cold loop whose backend declares exactly its two outputs asks for a lead-in"
    );
    let leadin = interleaved(lead, 0);
    let record = interleaved(cfg.window_len(), lead);
    let seg = plant.run_segment(leadin, record);
    assert!(seg.faulted.is_none(), "segment faulted: {:?}", seg.faulted);
    assert_eq!(seg.zero_filled, 0);

    // The wire is [lead-in][record], frame by frame, each output its own stream.
    let wire = wire.lock().unwrap();
    assert_eq!(wire.len() % OUTPUTS, 0, "whole frames only");
    let frames = wire.len() / OUTPUTS;
    assert!(frames > lead, "the record reached the wire");
    for f in 0..frames {
        for d in 0..OUTPUTS {
            let got = wire[f * OUTPUTS + d];
            let want = sample(d, f) as f32;
            assert_eq!(
                got, want,
                "frame {f}, output {d}: {got} on the wire, {want} expected"
            );
        }
    }

    // Peaks per output.
    assert_eq!(seg.drive_peaks.len(), OUTPUTS);
    let last = lead + cfg.window_len();
    let peak1 = (0..last).map(|f| sample(1, f).abs()).fold(0.0, f64::max);
    assert!(
        (seg.drive_peaks[1] - peak1).abs() < 1e-6,
        "output 1 peak {} against {peak1}",
        seg.drive_peaks[1]
    );
    assert!(seg.drive_peaks[0] < seg.drive_peaks[1]);
    assert_eq!(seg.drive_peak, seg.drive_peaks[1]);
}

/// A single drive row through `open_outputs` is the single-output layout.
#[test]
fn one_output_through_the_multiple_output_door_is_one_stream() {
    let mut cfg = LoopConfig::demo_default();
    let mut t = table();
    t.channels.pop();
    let backend: Box<dyn Backend> = Box::new(
        SimConfig::new()
            .sample_rate_hz(8192.0)
            .resonances(vec![(200.0, 2.5)])
            .shared_clock(true)
            .max_input_channels(1)
            .max_output_channels(1)
            .build(),
    );
    let mut plant = PlantLoop::open_outputs(backend, StationClass::HertzforgeSim, &mut cfg, t)
        .expect("one drive row opens");
    assert_eq!(plant.n_outputs(), 1);
    let record: Vec<f64> = (0..cfg.window_len()).map(|f| sample(0, f)).collect();
    let seg = plant.run_segment(Vec::new(), record);
    assert!(seg.faulted.is_none());
    assert_eq!(seg.drive_peaks.len(), 1);
}

fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let ab: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let aa: f64 = a.iter().map(|x| x * x).sum();
    let bb: f64 = b.iter().map(|x| x * x).sum();
    ab / (aa * bb).sqrt()
}

fn split(v: &[f64]) -> (Vec<f64>, Vec<f64>) {
    (
        v.iter().step_by(2).copied().collect(),
        v.iter().skip(1).step_by(2).copied().collect(),
    )
}

/// The outputs share each frame's random phases through the factor: a factor
/// whose two rows are equal plays the same stream on both outputs; an identity
/// factor plays two independent streams.
#[test]
fn the_factor_couples_the_outputs_through_shared_phases() {
    let cfg = LoopConfig::demo_default();
    let lines = cfg.nbins();
    let (lo, hi) = cfg.band_bins();
    let mut same = DriveFactor::zeros(lines, 2, 2);
    let mut ident = DriveFactor::zeros(lines, 2, 2);
    for line in lo..hi {
        let b = line * 4;
        same.data[b] = (1e-3, 0.0);
        same.data[b + 2] = (1e-3, 0.0);
        ident.data[b] = (1e-3, 0.0);
        ident.data[b + 3] = (1e-3, 0.0);
    }
    let mut synth = DriveSynth::with_outputs(&cfg, 2);
    let rec = synth.segment_factored(&same).expect("fits");
    assert_eq!(rec.len(), 2 * cfg.window_len());
    let (a, b) = split(&rec);
    assert!(a.iter().any(|&x| x != 0.0));
    for (x, y) in a.iter().zip(&b) {
        assert!((x - y).abs() < 1e-15, "equal rows play equal streams");
    }
    let mut synth = DriveSynth::with_outputs(&cfg, 2);
    let (a, b) = split(&synth.segment_factored(&ident).expect("fits"));
    let r = correlation(&a, &b);
    assert!(
        r.abs() < 0.05,
        "independent sources play uncorrelated streams: {r}"
    );
    // A factor of the wrong size is refused in words.
    let e = synth
        .segment_factored(&DriveFactor::zeros(lines, 3, 3))
        .unwrap_err();
    assert!(e.contains("3 outputs"), "{e}");
}
