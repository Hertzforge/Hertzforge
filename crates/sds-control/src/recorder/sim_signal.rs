//! The Recorder's source on the simulator (schema/PROTOCOL.md, "The Recorder on the
//! Simulator"). The simulator's plant moves only when it is driven, and a Recorder run
//! drives nothing, so on the simulator each recorded row carries a stated signal added
//! to the plant's noise: the k-th recorded row, in table order, row k of [`SOURCE`], a
//! fifth row and after repeating the four. The source repeats every 2.000 s from the
//! run's first frame, at the rate the station programmed, in each row's unit.
//!
//! [`Tap`] wraps every Recorder station's backend: on the simulator it adds the
//! source to every frame read, and on any station it collects the driver's warnings
//! for the run's log. It names no simulator type. Tests may install another [`Signal`].

use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use std::f64::consts::PI;
use std::sync::{Arc, Mutex};

/// The source's period, seconds.
pub const PERIOD_S: f64 = 2.0;
/// A marker begins this far into each period, seconds.
pub const MARKER_AT_S: f64 = 0.5;
/// A marker's length, seconds.
pub const MARKER_S: f64 = 0.010;
/// A marker's peak, in the row's unit.
pub const MARKER_PEAK: f64 = 0.5;

/// What a row of the source carries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Part {
    /// A half-sine of [`MARKER_PEAK`] lasting [`MARKER_S`], beginning [`MARKER_AT_S`]
    /// into every period.
    Markers,
    /// A tone: frequency Hz, peak.
    Tone(f64, f64),
}

/// The source, one part a recorded row, in order.
pub const SOURCE: [Part; 4] = [
    Part::Markers,
    Part::Tone(250.0, 0.2),
    Part::Tone(433.0, 0.1),
    Part::Tone(1000.0, 0.15),
];

/// A part's value at station time `t` seconds.
pub fn part_value(part: Part, t: f64) -> f64 {
    match part {
        Part::Markers => {
            let into = t.rem_euclid(PERIOD_S) - MARKER_AT_S;
            if (0.0..MARKER_S).contains(&into) {
                MARKER_PEAK * (PI * into / MARKER_S).sin()
            } else {
                0.0
            }
        }
        Part::Tone(f, a) => a * (2.0 * PI * f * t).sin(),
    }
}

/// The source's value on column `column` at station frame `frame`, at `fs_hz`.
pub fn source_value(frame: u64, column: usize, fs_hz: f64) -> f64 {
    part_value(SOURCE[column % SOURCE.len()], frame as f64 / fs_hz)
}

/// A signal added to the station's inputs: `f(station frame, column, rate)`.
pub type Signal = Arc<dyn Fn(u64, usize, f64) -> f64 + Send + Sync>;

/// The stated source in words, for the record of a simulated run: each recorded row's
/// part in column order (a fifth row and after repeating the four).
pub fn stated_json() -> serde_json::Value {
    let parts: Vec<serde_json::Value> = SOURCE
        .iter()
        .map(|p| match p {
            Part::Markers => serde_json::json!({
                "kind": "markers", "peak": MARKER_PEAK, "length_s": MARKER_S,
                "at_s": MARKER_AT_S, "period_s": PERIOD_S}),
            Part::Tone(f, a) => serde_json::json!({"kind": "tone", "freq_hz": f, "peak": a}),
        })
        .collect();
    serde_json::json!({"stated": true, "period_s": PERIOD_S, "rows": parts})
}

/// The stated source as a [`Signal`].
pub fn stated() -> Signal {
    Arc::new(source_value)
}

/// The driver's warnings a [`Tap`] has collected, for the run's thread to take.
pub type Warnings = Arc<Mutex<Vec<String>>>;

/// A station's backend with its warnings collected and, on the simulator, a signal
/// added to its inputs.
pub struct Tap {
    inner: Box<dyn Backend>,
    signal: Option<Signal>,
    warnings: Warnings,
    fs_hz: f64,
    n_inputs: usize,
    frame: u64,
}

impl Tap {
    pub fn new(
        inner: Box<dyn Backend>,
        signal: Option<Signal>,
        n_inputs: usize,
        warnings: Warnings,
    ) -> Tap {
        Tap {
            inner,
            signal,
            warnings,
            fs_hz: 0.0,
            n_inputs: n_inputs.max(1),
            frame: 0,
        }
    }

    fn collect(&mut self) {
        let w = self.inner.take_warnings();
        if !w.is_empty() {
            match self.warnings.lock() {
                Ok(mut g) => g.extend(w),
                Err(p) => p.into_inner().extend(w),
            }
        }
    }
}

impl Backend for Tap {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        let r = self.inner.open();
        self.collect();
        r
    }
    fn close(&mut self) -> Result<(), HalError> {
        let r = self.inner.close();
        self.collect();
        r
    }
    fn take_warnings(&mut self) -> Vec<String> {
        Vec::new()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        let actual = self.inner.configure(table, acq);
        self.collect();
        let actual = actual?;
        self.fs_hz = actual.actual_rate_hz;
        Ok(actual)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        let r = self.inner.arm();
        self.collect();
        r
    }
    fn start(&mut self) -> Result<(), HalError> {
        let r = self.inner.start();
        self.collect();
        r
    }
    fn stop(&mut self) -> Result<(), HalError> {
        let r = self.inner.stop();
        self.collect();
        r
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let read = self.inner.read_input(out);
        self.collect();
        let read = read?;
        if let InputRead::Frames(n) = read {
            if let Some(signal) = self.signal.as_ref().filter(|_| self.fs_hz > 0.0) {
                for f in 0..n {
                    for c in 0..self.n_inputs {
                        let i = f * self.n_inputs + c;
                        if let Some(v) = out.get_mut(i) {
                            *v += signal(self.frame + f as u64, c, self.fs_hz) as f32;
                        }
                    }
                }
            }
            self.frame += n as u64;
        }
        Ok(read)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(drive)
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_source_markers_begin_half_a_second_into_each_period() {
        let fs = 12_800.0;
        // At 12 800 Hz a marker begins at station frame 6 400 + 25 600 k, lasts 128
        // frames, and rises through 0.25 21.33 frames after it begins.
        for k in 0..3u64 {
            let start = 6400 + 25_600 * k;
            assert_eq!(source_value(start - 1, 0, fs), 0.0);
            assert!(source_value(start, 0, fs).abs() < 1e-9);
            assert!(source_value(start + 21, 0, fs) < 0.25);
            assert!(source_value(start + 22, 0, fs) >= 0.25);
            assert!((source_value(start + 64, 0, fs) - 0.5).abs() < 1e-9);
            assert!(source_value(start + 128, 0, fs).abs() < 1e-9);
        }
        // The fifth row repeats the first.
        assert_eq!(source_value(6464, 4, fs), source_value(6464, 0, fs));
        // 250 Hz makes whole cycles in a 2048-frame block.
        assert!(source_value(2048, 1, fs).abs() < 1e-9);
    }
}
