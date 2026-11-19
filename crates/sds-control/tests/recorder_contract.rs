//! The Recorder contract (schema/PROTOCOL.md, "Recorder").
//!
//! Three things are pinned. Every method in the contract's table is served (none is
//! refused `unknown_method`). The configuration defaults the contract
//! states are the Recorder core's own wherever the core has one, with the derived numbers
//! of the contract's example worked from the core's own arithmetic. And the names the wire
//! offers for the core's windows, averaging, slopes and trigger modes cover every variant
//! the core has, so a variant added to the core is a compile error here until the wire
//! decides on it.

use hertzforge_recorder::analysis::hop_frames;
use hertzforge_recorder::service::config::{default_metadata, DEFAULT_OVERLOAD_FRACTION};
use hertzforge_recorder::trigger::{DEFAULT_PRETRIGGER_S, MAX_PRETRIGGER_S};
use hertzforge_recorder::{
    AcquisitionConfig, AveragingKind, LevelTrigger, RecorderConfig, Slope, TriggerMode, WindowKind,
};
use sds_control::protocol::ErrorCode;
use sds_control::server::Service;
use serde_json::Map;

/// The methods of the contract's table, in its order.
const RECORDER_METHODS: [&str; 16] = [
    "recorder.configure",
    "recorder.config",
    "recorder.start",
    "recorder.arm",
    "recorder.disarm",
    "recorder.record",
    "recorder.cut",
    "recorder.stop",
    "recorder.abort",
    "recorder.recordings",
    "recorder.marks",
    "recorder.reset",
    "recorder.clear",
    "recorder.state",
    "recorder.log",
    "recorder.subscribe",
];

#[tokio::test]
async fn recorder_methods_are_served() {
    let svc = Service::new();
    let conn = svc.connect("recorder contract".into());
    // Every method of the contract's table answers as itself: an answer, or a refusal
    // of its own (no configuration, no control), never `unknown_method`.
    for method in RECORDER_METHODS {
        if let Err(e) = svc.call(&conn, method, &Map::new()).await {
            assert_ne!(e.code, ErrorCode::UnknownMethod, "{method}: {e:?}");
        }
    }
}

#[test]
fn recorder_wire_defaults_are_the_cores_defaults() {
    // PROTOCOL.md, "Recorder Configuration": the default of every field the core has one
    // for. The rate is the exception: the wire asks 12 800 Hz and always sends a rate, so
    // the core's own default rate is never used.
    let core = RecorderConfig::default();
    assert_eq!(core.trigger_mode, TriggerMode::Manual, "trigger.mode");
    // `count` is the recordings after an arm; the core counts the re-arms after the first.
    assert_eq!(core.auto_rearm + 1, 1, "trigger.count");
    assert_eq!(
        core.pretrigger_s, None,
        "trigger.pre_s: the core's default below"
    );
    assert_eq!(DEFAULT_PRETRIGGER_S, 1.0, "trigger.pre_s");
    assert_eq!(MAX_PRETRIGGER_S, 10.0, "trigger.pre_s at most");
    assert_eq!(core.timed_record_s, None, "trigger.record_s");
    // A level row at `range_max` 5: the default hysteresis is 1% of it, no holdoff.
    let level = LevelTrigger::new(0, 0.25, Slope::Rising, 5.0);
    assert!(
        (level.hysteresis_eu - 0.05).abs() < 1e-12,
        "trigger.hysteresis"
    );
    assert_eq!(level.slope, Slope::Rising, "trigger.slope");
    assert_eq!(level.holdoff_s, 0.0, "trigger.holdoff_s");
    assert_eq!(core.frf_pair, None, "analysis.frf");
    assert_eq!(core.overload_fraction, 0.9, "the overload indicator");
    assert_eq!(DEFAULT_OVERLOAD_FRACTION, 0.9);
    let keys: Vec<&str> = core.metadata.iter().map(|m| m.key.as_str()).collect();
    assert_eq!(
        keys,
        ["run_number", "test_article", "sn", "operator"],
        "metadata"
    );
    assert_eq!(default_metadata(), core.metadata);

    let acq = &core.acq;
    assert_eq!(acq.block_size, 2048, "analysis.block");
    assert_eq!(acq.window, WindowKind::Hann, "analysis.window");
    assert_eq!(acq.averaging, AveragingKind::Linear, "analysis.averaging");
    assert_eq!(acq.average_count, 8, "analysis.averages");
    assert_eq!(acq.analysis_decimation, 1, "analysis.decimation");
    // `analysis.overlap_pct` null is the window's own.
    assert_eq!(acq.overlap_pct, WindowKind::Hann.default_overlap_pct());
    assert_eq!(WindowKind::Hann.default_overlap_pct(), 50.0);
    assert_eq!(WindowKind::Flattop.default_overlap_pct(), 75.0);
    assert_eq!(WindowKind::Rectangular.default_overlap_pct(), 0.0);
    assert_eq!(WindowKind::BlackmanHarris.default_overlap_pct(), 66.7);

    // `capture.keep_free_gb` 1 is the writer's own least floor, 1 GiB, and the writer
    // flushes and syncs every 5 s.
    assert_eq!(hertzforge_capture::MIN_FREE_FLOOR_BYTES, 1 << 30);
    assert_eq!(
        hertzforge_capture::DEFAULT_FLUSH_INTERVAL.as_secs_f64(),
        5.0
    );

    // The derived numbers of the contract's example: 12 800 Hz on an NI station, four
    // rows, the default analysis, a 0.5 s pre-window and a 1.0 s length.
    let fs = 12_800.0;
    assert_eq!(
        hertzforge_hal::default_block_frames(fs),
        256,
        "grid.block_frames"
    );
    let at_rate = AcquisitionConfig {
        native_rate_hz: fs,
        ..acq.clone()
    };
    assert_eq!(at_rate.df_hz(), 6.25, "analysis.df_hz");
    assert_eq!(at_rate.spectral_lines(), 800, "analysis.lines");
    assert_eq!(at_rate.span_hz(), 5000.0, "analysis.span_hz");
    assert_eq!(at_rate.block_seconds(), 0.16, "analysis.block_s");
    let hop = hop_frames(at_rate.block_size, at_rate.overlap_pct);
    let averages_frames = at_rate.block_size + (at_rate.average_count - 1) * hop;
    assert_eq!(averages_frames, 9216);
    assert!(
        (averages_frames as f64 / fs - 0.72).abs() < 1e-12,
        "analysis.averages_s"
    );
    assert_eq!((0.5_f64 * fs).round(), 6400.0, "trigger.pre_frames");
    assert_eq!((1.0_f64 * fs).round(), 12_800.0, "trigger.record_frames");
    assert_eq!(4.0 * 4.0 * fs, 204_800.0, "disk.bytes_per_s");
}

#[test]
fn recorder_wire_names_are_the_cores_variants() {
    // PROTOCOL.md, "Recorder Configuration": the wire's names for the core's choices.
    // Each match is exhaustive, so a variant the core gains does not compile here until
    // the wire has a name for it (or, as `FreeRun`, says it offers none).
    fn window(w: WindowKind) -> &'static str {
        match w {
            WindowKind::Hann => "hann",
            WindowKind::Flattop => "flattop",
            WindowKind::Rectangular => "rectangular",
            WindowKind::BlackmanHarris => "blackman_harris",
        }
    }
    fn averaging(a: AveragingKind) -> &'static str {
        match a {
            AveragingKind::Linear => "linear",
            AveragingKind::Exponential => "exponential",
            AveragingKind::PeakHold => "peak_hold",
        }
    }
    fn slope(s: Slope) -> &'static str {
        match s {
            Slope::Rising => "rising",
            Slope::Falling => "falling",
        }
    }
    // The wire's `start` mode has no core mode of its own: it is a manual recording begun
    // at the run's first frame. `FreeRun` fires only when asked, as `Manual` does, so the
    // wire does not offer it.
    fn mode(m: &TriggerMode) -> Option<&'static str> {
        match m {
            TriggerMode::FreeRun => None,
            TriggerMode::Manual => Some("manual"),
            TriggerMode::Level(_) => Some("level"),
        }
    }
    let windows = [
        WindowKind::Hann,
        WindowKind::Flattop,
        WindowKind::Rectangular,
        WindowKind::BlackmanHarris,
    ]
    .map(window);
    assert_eq!(
        windows,
        ["hann", "flattop", "rectangular", "blackman_harris"]
    );
    let averages = [
        AveragingKind::Linear,
        AveragingKind::Exponential,
        AveragingKind::PeakHold,
    ]
    .map(averaging);
    assert_eq!(averages, ["linear", "exponential", "peak_hold"]);
    assert_eq!(
        [Slope::Rising, Slope::Falling].map(slope),
        ["rising", "falling"]
    );
    let level = TriggerMode::Level(LevelTrigger::new(0, 0.25, Slope::Rising, 5.0));
    assert_eq!(mode(&TriggerMode::FreeRun), None);
    assert_eq!(mode(&TriggerMode::Manual), Some("manual"));
    assert_eq!(mode(&level), Some("level"));
}
