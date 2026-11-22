//! Which lines one analysis frame leaves out of the floor estimate (design
//! D7, theory 2.5): every tone that plays, dwelling or sweeping, over its
//! footprint at the frame's middle widened by half its travel in the frame;
//! every servoed band over its span plus a guard. Built on single-output
//! Mixed Mode's grid helpers ([`hertzforge_mixed::floor::exclusion_mask`],
//! [`hertzforge_mixed::floor::mark_span`]).

use hertzforge_mixed::band::BAND_GUARD_LINES;
use hertzforge_mixed::floor::{exclusion_mask, mark_span};

/// A tone's frequency over one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneSpan {
    /// Frequency at the frame's first sample, Hz.
    pub start_hz: f64,
    /// Frequency at its last sample, Hz.
    pub end_hz: f64,
}

impl ToneSpan {
    /// A tone that does not move in the frame.
    pub fn dwelling(f_hz: f64) -> Self {
        ToneSpan {
            start_hz: f_hz,
            end_hz: f_hz,
        }
    }

    /// The frequency at the frame's middle, Hz.
    pub fn middle_hz(&self) -> f64 {
        0.5 * (self.start_hz + self.end_hz)
    }

    /// How far it moves in the frame, Hz.
    pub fn travel_hz(&self) -> f64 {
        (self.end_hz - self.start_hz).abs()
    }
}

/// Mark one frame's excluded lines on a grid (`df` spacing, line `k` at
/// `k df`, `mask.len()` lines): each tone in `tones` from its middle less
/// `half_hz` plus half its travel to its middle plus the same; each band
/// `(lo, hi)` in `bands` (the servoed bands' edges at the frame's middle,
/// widened by their own travel by the caller) from its lower edge less
/// `guard` lines to its upper edge plus `guard` lines. Clears the mask
/// first. Does not allocate.
pub fn frame_mask(
    df: f64,
    tones: &[ToneSpan],
    half_hz: f64,
    bands: &[(f64, f64)],
    guard: usize,
    mask: &mut [bool],
) {
    exclusion_mask(df, &[], half_hz, bands, guard, mask);
    for t in tones {
        let w = half_hz + 0.5 * t.travel_hz();
        let mid = t.middle_hz();
        mark_span(df, mid - w, mid + w, mask);
    }
}

/// [`frame_mask`] with single-output Mixed Mode's band guard
/// ([`BAND_GUARD_LINES`]).
pub fn frame_mask_default_guard(
    df: f64,
    tones: &[ToneSpan],
    half_hz: f64,
    bands: &[(f64, f64)],
    mask: &mut [bool],
) {
    frame_mask(df, tones, half_hz, bands, BAND_GUARD_LINES, mask);
}

/// A band's edges over a frame in which its center moves from `start` to
/// `end`: the lowest lower edge and the highest upper edge.
pub fn band_span_over_frame(
    width: &hertzforge_mixed::BandWidth,
    start_center_hz: f64,
    end_center_hz: f64,
) -> (f64, f64) {
    let a = width.edges_at(start_center_hz);
    let b = width.edges_at(end_center_hz);
    (a.0.min(b.0), a.1.max(b.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_mixed_dwelling_tone_marks_its_half_width() {
        let mut m = vec![false; 50];
        frame_mask(1.0, &[ToneSpan::dwelling(20.0)], 2.0, &[], 3, &mut m);
        let marked: Vec<usize> = (0..50).filter(|&k| m[k]).collect();
        assert_eq!(marked, vec![18, 19, 20, 21, 22]);
    }

    #[test]
    fn mimo_mixed_sweeping_tone_is_widened_by_half_its_travel() {
        let mut m = vec![false; 50];
        // Middle 20 Hz, travel 4 Hz: 20 +- (2 + 2).
        let t = ToneSpan {
            start_hz: 18.0,
            end_hz: 22.0,
        };
        frame_mask(1.0, &[t], 2.0, &[], 3, &mut m);
        let marked: Vec<usize> = (0..50).filter(|&k| m[k]).collect();
        assert_eq!(marked, (16..=24).collect::<Vec<_>>());
    }

    #[test]
    fn mimo_mixed_band_marks_span_and_guard() {
        let mut m = vec![false; 50];
        frame_mask(1.0, &[], 2.0, &[(20.0, 25.0)], 3, &mut m);
        let marked: Vec<usize> = (0..50).filter(|&k| m[k]).collect();
        assert_eq!(marked, (17..29).collect::<Vec<_>>());
    }
}
