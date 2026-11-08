//! The reference pulse on its record, and `shock.preview` (schema/PROTOCOL.md,
//! "Classical Shock Configuration"): the composite (the main pulse and the
//! compensation that brings it to rest) at full level on the record it plays as,
//! its tolerance band, its kinematics and its quiet-window figure. The shell draws
//! a pulse from these and never computes one.
//!
//! The record is the composite with the quiet surround on both sides (the
//! controller's own rule, `max(0.5 T, 5 ms)`), padded to a power of two: the grid
//! the closed loop identifies, inverts and corrects on.

use super::config::{round, QuietWindow, WirePulse};
use super::view::{self, Band, TimeFigures};
use crate::codec::Arrays;
use crate::protocol::num;
use hertzforge_dsp::fft::next_pow2;
use hertzforge_dsp::shock::{pulse_kinematic_series, pulse_kinematics};
use hertzforge_shock_host::service::firer::quiet_lead_frames;
use hertzforge_shock_host::{Polarity, ToleranceStandard};
use serde_json::{json, Value};

/// What the reference is, in numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReferenceFacts {
    /// The composite's peak at full level, g (signed by the polarity's direction:
    /// always the magnitude).
    pub peak_g: f64,
    /// The main pulse's velocity change, in/s, as the tolerance check's nominal
    /// (signed: negative for a negative pulse).
    pub velocity_change_ips: f64,
    /// The composite's peak-to-peak displacement at full level, in.
    pub displacement_pk_pk_in: f64,
    /// The main pulse's first sample on the record.
    pub main_start: usize,
    /// The main pulse's length, samples.
    pub main_len: usize,
}

impl ReferenceFacts {
    pub fn to_json(&self) -> Value {
        json!({
            "peak_g": num(round(self.peak_g, 4)),
            "velocity_change_ips": num(round(self.velocity_change_ips, 4)),
            "displacement_pk_pk_in": num(round(self.displacement_pk_pk_in, 6)),
            "main_start": self.main_start,
            "main_len": self.main_len,
        })
    }
}

/// The reference on its record.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub fs_hz: f64,
    pub record_len: usize,
    /// The record at full level, g, polarity applied.
    pub full_level: Vec<f64>,
    /// The composite's first sample and its end (exclusive) on the record.
    pub composite: (usize, usize),
    pub facts: ReferenceFacts,
    pub quiet_window: QuietWindow,
    /// The standard's amplitude band half-width, a fraction of the peak.
    pub tol_frac: f64,
    /// The judged window: the main pulse with the standard's quiet window on
    /// both sides, `(first, end)` on the record.
    pub window: (usize, usize),
}

/// The reference for `pulse` under `standard` at `fs_hz`.
pub fn reference(pulse: &WirePulse, standard: ToleranceStandard, fs_hz: f64) -> Reference {
    let spec = pulse.spec();
    let comp = spec.synthesize(fs_hz);
    let quiet = quiet_lead_frames(pulse.duration_ms / 1000.0, fs_hz);
    let n_comp = comp.waveform_g.len();
    let record_len = next_pow2(n_comp + 2 * quiet).max(1);
    let sign = match pulse.polarity {
        Polarity::Positive => 1.0,
        Polarity::Negative => -1.0,
    };
    let mut full_level = vec![0.0; record_len];
    for (k, v) in comp.waveform_g.iter().enumerate() {
        full_level[quiet + k] = sign * v;
    }
    let main_start = quiet + comp.pre_len;
    let main_len = comp.main_len;
    let p = standard.params();
    let w_pre = (main_len as f64 * p.window_pre_frac).round() as usize;
    let w_post = (main_len as f64 * p.window_post_frac).round() as usize;
    let first = main_start.saturating_sub(w_pre);
    let end = (main_start + main_len + w_post).min(record_len);
    let a = pulse.amplitude_g;
    let worst = full_level[first..main_start]
        .iter()
        .chain(full_level[(main_start + main_len).min(end)..end].iter())
        .fold(0.0f64, |m, v| m.max(v.abs()));
    let nominal = spec.main_pulse(fs_hz);
    let nominal_dv = pulse_kinematics(&nominal, fs_hz).residual_velocity_ips;
    // The composite's own kinematics, as the controller's feasibility takes them:
    // its compensation is solved for the composite alone, and the step into a
    // rectangular shoulder from the quiet lead adds half a sample's area that
    // the solve never saw.
    let kin = pulse_kinematics(&full_level[quiet..quiet + n_comp], fs_hz);
    Reference {
        fs_hz,
        record_len,
        composite: (quiet, quiet + n_comp),
        facts: ReferenceFacts {
            peak_g: kin.peak_accel_g,
            velocity_change_ips: sign * nominal_dv,
            displacement_pk_pk_in: kin.pk_pk_disp_in,
            main_start,
            main_len,
        },
        quiet_window: QuietWindow {
            worst_frac: if a > 0.0 { worst / a } else { 0.0 },
            limit_frac: p.amp_tol_frac_of_peak,
        },
        tol_frac: p.amp_tol_frac_of_peak,
        window: (first, end),
        full_level,
    }
}

impl Reference {
    /// The tolerance band at `scale` times full level over the judged window:
    /// the nominal main pulse plus and minus the band in the main region, zero
    /// plus and minus it in the quiet window.
    pub fn band(&self, scale: f64) -> Band {
        let (first, end) = self.window;
        let (s, n) = (self.facts.main_start, self.facts.main_len);
        let half = self.tol_frac * self.facts.peak_g.abs() * scale.abs();
        let center = |i: usize| {
            if i >= s && i < s + n {
                self.full_level[i] * scale
            } else {
                0.0
            }
        };
        Band {
            start: first,
            hi: (first..end).map(|i| center(i) + half).collect(),
            lo: (first..end).map(|i| center(i) - half).collect(),
        }
    }

    /// The composite at full level, polarity applied: what feasibility assesses.
    pub fn composite_g(&self) -> &[f64] {
        &self.full_level[self.composite.0..self.composite.1]
    }

    /// The part of the record a screen shows: the composite with its quiet
    /// surround, not the padding after it.
    pub fn shown(&self) -> (usize, usize) {
        let (c0, c1) = self.composite;
        let quiet = c0;
        (0, (c1 + quiet).min(self.record_len))
    }

    /// The figures a pulse view of the reference alone is drawn from, at
    /// `scale` times full level.
    pub fn figures(&self, scale: f64) -> TimeFigures {
        let (a, b) = self.shown();
        let band = self.band(scale);
        // The band, cut to the shown part of the record.
        let lo_i = band.start.max(a);
        let hi_i = (band.start + band.hi.len()).min(b);
        let band = (lo_i < hi_i).then(|| Band {
            start: lo_i - a,
            hi: band.hi[lo_i - band.start..hi_i - band.start].to_vec(),
            lo: band.lo[lo_i - band.start..hi_i - band.start].to_vec(),
        });
        TimeFigures {
            start: a as i64 - self.facts.main_start as i64,
            fs_hz: self.fs_hz,
            reference: self.full_level[a..b].iter().map(|v| v * scale).collect(),
            band,
            measured: None,
        }
    }

    /// The reference's velocity and displacement on the shown part, at `scale`.
    pub fn kinematics(&self, scale: f64) -> (Vec<f64>, Vec<f64>) {
        let (a, b) = self.shown();
        let shown: Vec<f64> = self.full_level[a..b].iter().map(|v| v * scale).collect();
        let s = pulse_kinematic_series(&shown, self.fs_hz);
        (s.velocity_ips, s.disp_in)
    }
}

/// `shock.preview`'s result: the reference facts and the Pulse, Velocity and
/// Displacement views of the reference at full level.
pub fn preview_json(r: &Reference, points: Option<usize>, arrays: &mut Arrays) -> Value {
    let (vel, disp) = r.kinematics(1.0);
    preview_frame(
        &r.facts,
        &r.quiet_window,
        &r.figures(1.0),
        &vel,
        &disp,
        points,
        arrays,
    )
}

/// The preview's result from its figures: the facts and the Pulse, Velocity and
/// Displacement views.
pub fn preview_frame(
    facts: &ReferenceFacts,
    quiet: &QuietWindow,
    f: &TimeFigures,
    velocity: &[f64],
    displacement: &[f64],
    points: Option<usize>,
    arrays: &mut Arrays,
) -> Value {
    let views = vec![
        view::pulse_view(f, points, arrays),
        view::kinematic_view(
            view::Kinematic::Velocity,
            f.start,
            f.fs_hz,
            velocity,
            None,
            points,
            arrays,
        ),
        view::kinematic_view(
            view::Kinematic::Displacement,
            f.start,
            f.fs_hz,
            displacement,
            None,
            points,
            arrays,
        ),
    ];
    json!({
        "derived": {
            "reference": facts.to_json(),
            "quiet_window": quiet.to_json(),
        },
        "views": views,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
    use hertzforge_shock_host::ClassicalShape;

    fn half_sine(comp: CompensationConfig) -> WirePulse {
        WirePulse {
            shape: ClassicalShape::HalfSine,
            amplitude_g: 20.0,
            duration_ms: 11.0,
            polarity: Polarity::Positive,
            compensation: comp,
        }
    }

    #[test]
    fn the_record_holds_the_composite_inside_its_quiet_surround_on_a_power_of_two() {
        let r = reference(
            &half_sine(super::super::config::default_compensation()),
            ToleranceStandard::Mil810_516,
            51_200.0,
        );
        assert!(r.record_len.is_power_of_two());
        let (c0, c1) = r.composite;
        assert!(r.full_level[..c0].iter().all(|v| *v == 0.0));
        assert!(r.full_level[c1..].iter().all(|v| *v == 0.0));
        assert!((r.facts.peak_g - 20.0).abs() < 1e-9);
        assert_eq!(
            r.facts.main_len,
            r.full_level[r.facts.main_start..]
                .len()
                .min(r.facts.main_len)
        );
        // 2 A T / pi for a half-sine, in in/s.
        let ideal = 2.0 / std::f64::consts::PI * 20.0 * 0.011 * 386.0886;
        assert!((r.facts.velocity_change_ips - ideal).abs() / ideal < 0.01);
    }

    #[test]
    fn the_libraries_half_sine_shoulders_leave_the_quiet_window_and_rectangular_ones_do_not() {
        let lib = CompensationConfig::default();
        assert_eq!(lib.shape, CompensationShape::HalfSine);
        let r = reference(&half_sine(lib), ToleranceStandard::Mil810_516, 51_200.0);
        assert!(!r.quiet_window.inside(), "{:?}", r.quiet_window);
        let r = reference(
            &half_sine(super::super::config::default_compensation()),
            ToleranceStandard::Mil810_516,
            51_200.0,
        );
        assert!(r.quiet_window.inside(), "{:?}", r.quiet_window);
        assert!((r.quiet_window.limit_frac - 0.15).abs() < 1e-12);
    }

    #[test]
    fn the_band_is_the_nominal_pulse_plus_and_minus_the_tolerance_and_zero_in_the_window() {
        let r = reference(
            &half_sine(super::super::config::default_compensation()),
            ToleranceStandard::Mil810_516,
            51_200.0,
        );
        let b = r.band(0.5);
        let s = r.facts.main_start;
        let peak_at = s + r.facts.main_len / 2;
        let half = 0.15 * 20.0 * 0.5;
        let k = peak_at - b.start;
        assert!((b.hi[k] - (r.full_level[peak_at] * 0.5 + half)).abs() < 1e-12);
        assert!(
            (b.lo[0] + half).abs() < 1e-12,
            "the window's band is centred on zero"
        );
        assert!((b.hi[0] - half).abs() < 1e-12);
    }

    #[test]
    fn a_negative_pulse_is_the_positive_one_negated() {
        let mut p = half_sine(super::super::config::default_compensation());
        let pos = reference(&p, ToleranceStandard::Mil810_516, 20_480.0);
        p.polarity = Polarity::Negative;
        let neg = reference(&p, ToleranceStandard::Mil810_516, 20_480.0);
        assert!(pos
            .full_level
            .iter()
            .zip(&neg.full_level)
            .all(|(a, b)| *a == -*b));
        assert_eq!(
            pos.facts.velocity_change_ips,
            -neg.facts.velocity_change_ips
        );
        assert_eq!(pos.facts.peak_g, neg.facts.peak_g);
    }
}
