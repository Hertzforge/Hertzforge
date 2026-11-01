//! The measured noise-floor probe (generalized from a fixed `[f64; NCH]` array to a runtime
//! interleave width).
//!
//! Measures each channel's ACTUAL noise floor at the start of a run, so the "is this
//! signal or noise?" thresholds are derived from the bench in front of the operator
//! rather than from a number someone reasoned out once.

/// Frames of acquisition the probe accumulates before it reports (~0.25 s at 65.536 kHz).
/// Long enough to average a broadband floor; short enough to mature before an operator's
/// first drive press (the probe is invalidated by one - see [`NoiseFloorProbe`]).
pub const NOISE_FLOOR_PROBE_FRAMES: usize = 16_384;

/// What the noise-floor probe knows about a slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FloorProbe {
    /// The measured RMS noise floor of this slot, with no drive applied.
    Measured(f64),
    /// Not knowable: the probe never matured, a drive change contaminated the window, or
    /// the input read a dead zero. Consumers MUST fail closed.
    Unknown,
}

/// Measures each channel's ACTUAL noise floor at the start of a run.
///
/// ## The window: armed, running, drive still at zero
///
/// A run starts with the drive at its floor - on a delegated controller the device's run-control
/// restarts at the band low edge with the drive at its floor and the OEM's ~1.9 s pre-run
/// blackout is already over by the time telemetry flows; on a host-synthesized drive the
/// arm posture is 0.000 V by construction. So the first frames of a run are, by
/// construction, each channel reading with NO drive applied - which is exactly its noise
/// floor, on that bench, that day, with that cabling and that front-end gain.
///
/// This works in EITHER posture: on the bench the loopback slot reads the drive's floor,
/// and in the field the control channel reads the accelerometer's floor with the shaker
/// still.
///
/// ## Invalidation, and failing CLOSED
///
/// One drive change before the probe matures contaminates it - the channel is no longer
/// reading "no drive". A contaminated or never-matured probe reports [`FloorProbe::Unknown`],
/// and every consumer must then fail CLOSED (no drive anchor; no frequency correction)
/// rather than assume a floor. A floor of exactly zero is treated as a DEAD input, not a
/// quiet one, and also fails closed. An absurdly HIGH floor needs no special case: it
/// raises the threshold, nothing clears it, and everything stays clamped - the safe
/// direction already.
///
/// ## Interleave width is a parameter
///
/// The probe was written against a delegated controller's fixed 8-channel telemetry frame
/// and stored its accumulators in `[f64; NCH]`. It now takes `n_interleave` at construction
/// and keeps `Vec`s, because a host-closes-loop station's frame is as wide as its
/// configured task. A slot at or beyond that width is [`FloorProbe::Unknown`] - the same
/// fail-closed
/// answer the probe already gave for every other unknowable case.
#[derive(Debug, Clone)]
pub struct NoiseFloorProbe {
    /// Per-slot sum of squares over the probe window (`n_interleave` long).
    sumsq: Vec<f64>,
    n_interleave: usize,
    frames: usize,
    /// Set when the drive moves before maturity: the window no longer means "no drive".
    contaminated: bool,
    /// The matured per-slot RMS floor (`n_interleave` long).
    floor_rms: Option<Vec<f64>>,
}

impl NoiseFloorProbe {
    /// A fresh probe over an `n_interleave`-wide frame.
    pub fn new(n_interleave: usize) -> Self {
        Self {
            sumsq: vec![0.0; n_interleave],
            n_interleave,
            frames: 0,
            contaminated: false,
            floor_rms: None,
        }
    }

    /// The interleave width this probe folds frames at.
    pub fn n_interleave(&self) -> usize {
        self.n_interleave
    }

    /// Restart the probe (a fresh arm / soft re-arm puts the drive back at its floor).
    pub fn reset(&mut self) {
        *self = Self::new(self.n_interleave);
    }

    /// The drive moved: if the probe has not matured, its window is no longer "no drive"
    /// and it can never be trusted for this run.
    pub fn contaminate(&mut self) {
        if self.floor_rms.is_none() {
            self.contaminated = true;
        }
    }

    /// True once a drive change has invalidated the window (and it had not matured).
    pub fn contaminated(&self) -> bool {
        self.contaminated
    }

    /// True once the probe has accumulated its full window and published a floor.
    pub fn matured(&self) -> bool {
        self.floor_rms.is_some()
    }

    /// Fold one interleaved chunk (stride [`n_interleave`](Self::n_interleave)) into the
    /// probe.
    pub fn push(&mut self, samples: &[f32]) {
        if self.floor_rms.is_some() || self.contaminated || self.n_interleave == 0 {
            return;
        }
        let frames = samples.len() / self.n_interleave;
        for f in 0..frames {
            let base = f * self.n_interleave;
            for (slot, acc) in self.sumsq.iter_mut().enumerate() {
                let v = f64::from(samples[base + slot]);
                *acc += v * v;
            }
        }
        self.frames += frames;
        if self.frames >= NOISE_FLOOR_PROBE_FRAMES {
            let n = self.frames as f64;
            let rms: Vec<f64> = self.sumsq.iter().map(|s| (s / n).max(0.0).sqrt()).collect();
            self.floor_rms = Some(rms);
        }
    }

    /// The measured floor of `slot`, or [`FloorProbe::Unknown`] when it must fail closed.
    pub fn floor(&self, slot: usize) -> FloorProbe {
        if self.contaminated || slot >= self.n_interleave {
            return FloorProbe::Unknown;
        }
        match &self.floor_rms {
            // A dead-zero input is broken, not quiet: refuse rather than let a zero floor
            // make every threshold zero and pass anything.
            Some(rms) if rms[slot] > 0.0 && rms[slot].is_finite() => {
                FloorProbe::Measured(rms[slot])
            }
            _ => FloorProbe::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NCH: usize = 8;

    fn mature(probe: &mut NoiseFloorProbe, width: usize, amplitude: f32) {
        let frames = NOISE_FLOOR_PROBE_FRAMES;
        let mut chunk = vec![0.0f32; frames * width];
        for (k, frame) in chunk.chunks_exact_mut(width).enumerate() {
            // A deterministic non-zero alternating floor: RMS == amplitude.
            frame[0] = if k % 2 == 0 { amplitude } else { -amplitude };
        }
        probe.push(&chunk);
    }

    #[test]
    fn an_unmatured_probe_is_unknown() {
        let probe = NoiseFloorProbe::new(NCH);
        assert!(!probe.matured());
        assert_eq!(probe.floor(0), FloorProbe::Unknown);
    }

    #[test]
    fn a_matured_probe_reports_the_measured_rms() {
        let mut probe = NoiseFloorProbe::new(NCH);
        mature(&mut probe, NCH, 0.25);
        assert!(probe.matured());
        let FloorProbe::Measured(f) = probe.floor(0) else {
            panic!("slot 0 carried a non-zero floor");
        };
        assert!((f - 0.25).abs() < 1e-6, "measured floor {f}, want 0.25");
        // A dead-zero slot fails closed rather than reporting a zero threshold.
        assert_eq!(probe.floor(1), FloorProbe::Unknown);
    }

    #[test]
    fn contamination_before_maturity_fails_closed_permanently() {
        let mut probe = NoiseFloorProbe::new(NCH);
        probe.contaminate();
        mature(&mut probe, NCH, 0.25);
        assert!(probe.contaminated());
        assert!(!probe.matured(), "a contaminated probe never publishes");
        assert_eq!(probe.floor(0), FloorProbe::Unknown);
    }

    #[test]
    fn contamination_after_maturity_is_ignored() {
        let mut probe = NoiseFloorProbe::new(NCH);
        mature(&mut probe, NCH, 0.25);
        probe.contaminate();
        assert!(!probe.contaminated());
        assert!(matches!(probe.floor(0), FloorProbe::Measured(_)));
    }

    /// The width is a parameter: a slot outside the frame is Unknown, not a panic and not
    /// a neighbour's floor.
    #[test]
    fn a_slot_outside_the_frame_is_unknown() {
        let mut probe = NoiseFloorProbe::new(2);
        assert_eq!(probe.n_interleave(), 2);
        mature(&mut probe, 2, 0.5);
        assert!(matches!(probe.floor(0), FloorProbe::Measured(_)));
        assert_eq!(probe.floor(2), FloorProbe::Unknown);
        assert_eq!(probe.floor(7), FloorProbe::Unknown);
    }

    #[test]
    fn reset_restarts_the_window_at_the_same_width() {
        let mut probe = NoiseFloorProbe::new(4);
        mature(&mut probe, 4, 0.25);
        assert!(probe.matured());
        probe.reset();
        assert!(!probe.matured());
        assert_eq!(probe.n_interleave(), 4);
        assert_eq!(probe.floor(0), FloorProbe::Unknown);
    }
}
