//! The SWEEP RASTER: the one uniform frequency grid every sine display product is
//! resampled onto, and the resampler that puts them there.
//!
//! # Why this is one function and not two
//!
//! A sine run's display products are swept traces: `(frequency, value)` pairs
//! logged as the tone travels, non-uniform in frequency by construction (a log
//! sweep spends far longer per Hz at the bottom of the band). A spectrum frame
//! carries a magnitude vector on a UNIFORM grid described by `(f0, df)`. So every
//! trace has to be resampled onto a shared grid before it can cross, and every
//! trace has to land on the SAME grid or the overlays do not overlay.
//!
//! Two packers produce those frames - the local Tauri host (`sweep_frames` in
//! `ui/src-tauri/src/sine.rs`) and the daemon's `SineHost` - and they publish
//! on the SAME reserved trace ids to the SAME decoder. The same id must mean
//! the same shape on both paths. Two copies of a 256-point grid rule is a drift
//! point with nothing to gain, and the way it would fail is quiet - the remote
//! overlay would be stretched against the local one and both would look like
//! plausible curves.
//!
//! # BAND-SPANNING, not an FFT grid
//!
//! The raster runs from the run's `band_lo` to its `band_hi` in
//! [`SWEEP_RASTER_POINTS`] steps. It is deliberately NOT the `0..fs/2` FFT grid
//! that `ProfileRaster` (the Explorer's peak-hold accumulator) uses: a sine run
//! measures nothing outside its band, and rendering its traces against a grid that
//! is mostly empty would put the whole test in the left-hand tenth of the pane.
//!
//! The grid is LINEAR in frequency even though the pane reads log-x. That is the
//! existing contract (a `SpectrumFrame` describes its x-axis as `f0 + k*df` and has
//! no way to say "logarithmic"), and changing it would be a wire change, not a
//! packer change.

use crate::service::report::TracePoint;

/// Points on the sweep raster.
///
/// 256 is the local packer's long-standing value and it is kept rather than
/// re-derived: it is the number every sine overlay on both paths is already drawn
/// at, and the two must match exactly for the overlays to line up. It is a DISPLAY
/// resolution - four points per pixel-ish on a wide pane - not a measurement one;
/// the measurement lives in the traces this resamples FROM.
pub const SWEEP_RASTER_POINTS: usize = 256;

/// The shared uniform frequency grid a sine run's display products ride.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepRaster {
    /// First bin frequency, Hz - the band's LOW edge.
    pub f0_hz: f64,
    /// Bin spacing, Hz.
    pub df_hz: f64,
    /// The bin frequencies. `SWEEP_RASTER_POINTS` long.
    pub freqs: Vec<f64>,
}

impl SweepRaster {
    /// The raster spanning `band` = `(lo, hi)`.
    ///
    /// A DEGENERATE band - a dwell, whose "band" is one frequency, or an
    /// inverted/NaN pair - gets a narrow window around the low edge rather than a
    /// zero or negative `df`. A `df` of zero would make every bin the same
    /// frequency and the consumer would draw a vertical line at `f0`; a negative
    /// one would run the x-axis backwards. Neither is a thing to put on a pane, so
    /// the degenerate case answers with the narrowest HONEST grid: the dwell's own
    /// frequency, +/- a tenth of a percent.
    pub fn over_band(band: (f64, f64)) -> SweepRaster {
        let (lo, hi) = band;
        let (lo, hi) = if lo.is_finite() && hi.is_finite() && lo > 0.0 && hi > lo {
            (lo, hi)
        } else if lo.is_finite() && lo > 0.0 {
            (lo * 0.999, lo * 1.001)
        } else {
            // No usable frequency at all. One Hz of grid, so the arithmetic below
            // stays finite and the pane shows an empty axis rather than NaNs.
            (1.0, 2.0)
        };
        let df = ((hi - lo) / (SWEEP_RASTER_POINTS as f64 - 1.0)).max(1e-9);
        SweepRaster {
            f0_hz: lo,
            df_hz: df,
            freqs: (0..SWEEP_RASTER_POINTS)
                .map(|k| lo + k as f64 * df)
                .collect(),
        }
    }

    /// The number of bins. Always [`SWEEP_RASTER_POINTS`]; a method so a caller
    /// sizing a buffer reads it off the raster it actually has.
    pub fn len(&self) -> usize {
        self.freqs.len()
    }

    /// Never true - a raster always has [`SWEEP_RASTER_POINTS`] bins. Present
    /// because clippy asks for it beside `len`, and answering honestly is cheaper
    /// than an allow.
    pub fn is_empty(&self) -> bool {
        self.freqs.is_empty()
    }

    /// Evaluate `f(freq)` at every bin.
    pub fn map<F: FnMut(f64) -> f64>(&self, mut f: F) -> Vec<f32> {
        self.freqs.iter().map(|&x| f(x) as f32).collect()
    }

    /// Resample a swept `(freq, value)` trace onto this grid.
    ///
    /// Zero where the sweep has not reached yet, which is what makes a partially
    /// completed pass render as a partial curve instead of as a curve with invented
    /// ends.
    ///
    /// The trace is SORTED by frequency first, on a copy. A bidirectional sweep
    /// logs its points in time order, so the up-pass and the down-pass interleave
    /// in frequency; interpolating that unsorted would zig-zag between the two
    /// passes. Sorting reads the latest monotone envelope over both, which is what
    /// an operator watching a bounce expects to see.
    pub fn resample_trace(&self, points: &[TracePoint]) -> Vec<f32> {
        if points.is_empty() {
            return vec![0.0; self.len()];
        }
        let mut pairs: Vec<(f64, f64)> = points.iter().map(|p| (p.freq_hz, p.value)).collect();
        pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let freqs: Vec<f64> = pairs.iter().map(|p| p.0).collect();
        let values: Vec<f64> = pairs.iter().map(|p| p.1).collect();
        self.resample_curve(&freqs, &values)
    }

    /// Resample an already-monotonic `(freqs, values)` curve onto this grid - the
    /// sysID `|H|` case, whose grid is uniform and ascending by construction.
    pub fn resample_curve(&self, freqs: &[f64], values: &[f64]) -> Vec<f32> {
        self.freqs
            .iter()
            .map(|&f| resample_at(freqs, values, f) as f32)
            .collect()
    }
}

/// Linear interpolation of a monotonic `(freqs, values)` curve at `f`.
///
/// Clamps to the end values outside the curve's own span rather than
/// extrapolating: an extrapolated overlay is a number nothing measured, drawn with
/// the same weight as the ones that were.
pub fn resample_at(freqs: &[f64], values: &[f64], f: f64) -> f64 {
    if freqs.is_empty() {
        return 0.0;
    }
    if f <= freqs[0] {
        return values.first().copied().unwrap_or(0.0);
    }
    let last = *freqs.last().unwrap_or(&0.0);
    if f >= last {
        return values.last().copied().unwrap_or(0.0);
    }
    for i in 0..freqs.len().saturating_sub(1) {
        let (f0, f1) = (freqs[i], freqs[i + 1]);
        if f >= f0 && f <= f1 {
            let t = if (f1 - f0).abs() < 1e-12 {
                0.0
            } else {
                (f - f0) / (f1 - f0)
            };
            let (v0, v1) = (
                values.get(i).copied().unwrap_or(0.0),
                values.get(i + 1).copied().unwrap_or(0.0),
            );
            return v0 + t * (v1 - v0);
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_raster_spans_the_band_and_starts_at_its_low_edge() {
        let r = SweepRaster::over_band((60.0, 500.0));
        assert_eq!(r.len(), SWEEP_RASTER_POINTS);
        assert!((r.f0_hz - 60.0).abs() < 1e-12, "f0 IS the band's low edge");
        assert!(
            (r.freqs[SWEEP_RASTER_POINTS - 1] - 500.0).abs() < 1e-9,
            "the last bin IS the band's high edge: {}",
            r.freqs[SWEEP_RASTER_POINTS - 1]
        );
        // BAND-SPANNING, not 0..fs/2: nothing below the band edge exists on this grid.
        assert!(r.f0_hz > 0.0);
    }

    /// A DWELL's band is one frequency, and a zero `df` would draw every bin at the
    /// same place. The degenerate case gets the narrowest honest window instead.
    #[test]
    fn a_degenerate_band_still_produces_a_usable_grid() {
        for band in [(100.0, 100.0), (500.0, 60.0), (f64::NAN, 200.0)] {
            let r = SweepRaster::over_band(band);
            assert!(r.df_hz > 0.0, "{band:?} gave df {}", r.df_hz);
            assert!(r.f0_hz.is_finite() && r.f0_hz > 0.0, "{band:?}");
            assert!(r.freqs.iter().all(|f| f.is_finite()), "{band:?}");
        }
    }

    /// A partially completed sweep renders as a PARTIAL curve: zero where the tone
    /// has not been, the measured value where it has.
    #[test]
    fn an_unreached_band_is_zero_not_extrapolated() {
        let r = SweepRaster::over_band((100.0, 200.0));
        let pts = vec![
            TracePoint {
                freq_hz: 100.0,
                value: 1.0,
            },
            TracePoint {
                freq_hz: 150.0,
                value: 2.0,
            },
        ];
        let out = r.resample_trace(&pts);
        assert!((out[0] - 1.0).abs() < 1e-6, "the first bin is measured");
        // Past the last measured point the curve CLAMPS rather than extrapolating -
        // it never invents a value above 2.0.
        assert!(
            out.iter().all(|&v| v <= 2.0 + 1e-6),
            "no bin may exceed the largest measured value: {:?}",
            out.iter().cloned().fold(f32::MIN, f32::max)
        );
    }

    /// A BIDIRECTIONAL sweep logs in TIME order, so its points interleave in
    /// frequency. Unsorted interpolation zig-zags between the two passes.
    #[test]
    fn a_bidirectional_trace_is_sorted_before_interpolation() {
        let r = SweepRaster::over_band((100.0, 200.0));
        // Up to 200, then back down to 100 - time order, not frequency order.
        let pts = vec![
            TracePoint {
                freq_hz: 100.0,
                value: 1.0,
            },
            TracePoint {
                freq_hz: 200.0,
                value: 2.0,
            },
            TracePoint {
                freq_hz: 150.0,
                value: 1.5,
            },
            TracePoint {
                freq_hz: 100.0,
                value: 1.0,
            },
        ];
        let out = r.resample_trace(&pts);
        // Monotone rising across the band, which it can only be if the points were
        // sorted: on the raw time order the segment 200 -> 150 runs backwards.
        for w in out.windows(2) {
            assert!(
                w[1] >= w[0] - 1e-5,
                "the resampled curve must not run backwards: {w:?}"
            );
        }
    }

    #[test]
    fn resample_at_clamps_outside_the_curve() {
        let f = [10.0, 20.0, 30.0];
        let v = [1.0, 2.0, 3.0];
        assert!((resample_at(&f, &v, 5.0) - 1.0).abs() < 1e-12);
        assert!((resample_at(&f, &v, 35.0) - 3.0).abs() < 1e-12);
        assert!((resample_at(&f, &v, 15.0) - 1.5).abs() < 1e-12);
        assert_eq!(resample_at(&[], &[], 15.0), 0.0);
    }
}
