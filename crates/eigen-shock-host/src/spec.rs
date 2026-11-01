//! The classical pulse library spec: shape +
//! amplitude + duration + compensation config, with serde round-trip, main-pulse
//! generation via `eigen_dsp::shock::pulse`, Path-B synthesis via
//! `eigen_dsp::shock::compensate_pulse`, and the ideal velocity-change relations
//! that back the pulse-substitution equivalence check.

use eigen_dsp::integrate::G_TO_IN_S2;
use eigen_dsp::shock::{
    compensate_pulse, pulse, CompensatedPulse, CompensationConfig, CompensationShape,
};
use serde::{Deserialize, Serialize};

/// Default rise/fall edge fraction for [`ClassicalShape::Rectangle`]. A pure
/// rectangle has infinite slew (unrealizable and numerically ill-posed - see the
/// eigen-dsp `pulse::rectangle` docs); the spec-level Rectangle therefore carries
/// short 5% edges by convention, making it a nominally-flat-top pulse with
/// bounded slew.
pub const RECTANGLE_EDGE_FRAC: f64 = 0.05;

/// The classical-shock pulse shape (MIL-STD-810 / IEC 60068-2-27 pulse library).
/// Maps one-to-one onto the
/// `eigen_dsp::shock::pulse` generators.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ClassicalShape {
    /// Half-sine `A*sin(pi*t/T)`.
    HalfSine,
    /// Haversine (versed sine / raised cosine): zero endpoint slope.
    Haversine,
    /// Terminal-peak sawtooth: linear ramp 0 -> A at the last sample.
    TerminalPeakSawtooth,
    /// Initial-peak sawtooth: A at the first sample, linear ramp down to 0.
    InitialPeakSawtooth,
    /// Trapezoid: linear rise over `rise_frac` of the duration, plateau at A,
    /// linear fall over `fall_frac`. Fractions are clamped/scaled exactly as the
    /// eigen-dsp generator documents (each to `[0, 1]`, proportionally scaled if
    /// their sum exceeds 1).
    Trapezoid {
        /// Fraction of the duration spent rising.
        rise_frac: f64,
        /// Fraction of the duration spent falling.
        fall_frac: f64,
    },
    /// Nominally flat-top rectangle with [`RECTANGLE_EDGE_FRAC`] rise/fall edges
    /// (bounded slew; see the constant's docs).
    Rectangle,
}

impl ClassicalShape {
    /// The ideal (continuous-time) velocity-change coefficient `c` such that the
    /// pulse's velocity change is `dV = c * A * T` (amplitude `A`, duration `T`).
    ///
    /// Closed forms: half-sine `2/pi`; haversine and both sawtooths `1/2`;
    /// trapezoid `1 - (rise + fall)/2` (after the eigen-dsp clamping rules);
    /// rectangle `1 - RECTANGLE_EDGE_FRAC` (a trapezoid with 5% edges). These are
    /// the relations behind the standards' pulse-substitution equivalence
    /// (matching dV across shapes) - e.g. MIL-STD-810's classical
    /// pulses are compared on their velocity change when a substitute shape is
    /// used.
    pub fn ideal_velocity_change_coefficient(&self) -> f64 {
        match *self {
            ClassicalShape::HalfSine => 2.0 / std::f64::consts::PI,
            ClassicalShape::Haversine => 0.5,
            ClassicalShape::TerminalPeakSawtooth => 0.5,
            ClassicalShape::InitialPeakSawtooth => 0.5,
            ClassicalShape::Trapezoid {
                rise_frac,
                fall_frac,
            } => {
                // Mirror the eigen-dsp generator's clamping so the coefficient
                // matches what the generator actually produces.
                let mut rf = rise_frac.clamp(0.0, 1.0);
                let mut ff = fall_frac.clamp(0.0, 1.0);
                if rf + ff > 1.0 {
                    let scale = 1.0 / (rf + ff);
                    rf *= scale;
                    ff *= scale;
                }
                1.0 - (rf + ff) / 2.0
            }
            ClassicalShape::Rectangle => 1.0 - RECTANGLE_EDGE_FRAC,
        }
    }
}

// ---------------------------------------------------------------------------
// Serde mirrors for the eigen-dsp compensation types.
//
// eigen-dsp is dependency-free by design (no serde), so its CompensationShape /
// CompensationConfig cannot derive Serialize/Deserialize. The established
// pattern for this (eigen-shock-common's PulseKinematicsRecord) is a local
// mirror; serde's remote derive is that pattern without duplicating the public
// type - the spec's `compensation` field IS eigen-dsp's CompensationConfig and
// the mirror exists only inside the serialization code.
// ---------------------------------------------------------------------------

/// Serde remote mirror of [`eigen_dsp::shock::CompensationShape`].
#[derive(Serialize, Deserialize)]
#[serde(remote = "CompensationShape")]
enum CompensationShapeDef {
    HalfSine,
    Rectangular,
    RoundedRectangular,
}

/// Serde remote mirror of [`eigen_dsp::shock::CompensationConfig`].
#[derive(Serialize, Deserialize)]
#[serde(remote = "CompensationConfig")]
struct CompensationConfigDef {
    #[serde(with = "CompensationShapeDef")]
    shape: CompensationShape,
    pre_frac: f64,
    post_frac: f64,
    symmetric: bool,
}

/// A classical-shock pulse specification: WHAT to fire. The shape/amplitude/
/// duration describe the MAIN (uncompensated) pulse - the thing a delegated
/// (Path A) run downloads verbatim - and `compensation` describes
/// the host-side (Path B) compensation applied by [`Self::synthesize`].
///
/// Units: amplitude in g, duration in milliseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassicalPulseSpec {
    /// The pulse shape.
    pub shape: ClassicalShape,
    /// Peak acceleration, g.
    pub amplitude_g: f64,
    /// Main-pulse duration, milliseconds.
    pub duration_ms: f64,
    /// Host-side (Path B ONLY) compensation configuration. Ignored
    /// by the delegated fire path, which downloads the uncompensated main pulse.
    #[serde(with = "CompensationConfigDef")]
    pub compensation: CompensationConfig,
}

impl ClassicalPulseSpec {
    /// A spec with the default double-sided compensation config.
    pub fn new(shape: ClassicalShape, amplitude_g: f64, duration_ms: f64) -> Self {
        ClassicalPulseSpec {
            shape,
            amplitude_g,
            duration_ms,
            compensation: CompensationConfig::default(),
        }
    }

    /// Validate the spec: amplitude and duration finite and positive; trapezoid
    /// fractions finite and non-negative; the compensation fractions finite and
    /// positive (the used ones - `post_frac` is ignored when `symmetric`).
    pub fn validate(&self) -> Result<(), SpecError> {
        if !self.amplitude_g.is_finite() || self.amplitude_g <= 0.0 {
            return Err(SpecError::BadAmplitude(self.amplitude_g));
        }
        if !self.duration_ms.is_finite() || self.duration_ms <= 0.0 {
            return Err(SpecError::BadDuration(self.duration_ms));
        }
        if let ClassicalShape::Trapezoid {
            rise_frac,
            fall_frac,
        } = self.shape
        {
            if !rise_frac.is_finite()
                || rise_frac < 0.0
                || !fall_frac.is_finite()
                || fall_frac < 0.0
            {
                return Err(SpecError::BadTrapezoidFracs {
                    rise_frac,
                    fall_frac,
                });
            }
        }
        let pre = self.compensation.pre_frac;
        if !pre.is_finite() || pre <= 0.0 {
            return Err(SpecError::BadCompensationFrac(pre));
        }
        if !self.compensation.symmetric {
            let post = self.compensation.post_frac;
            if !post.is_finite() || post <= 0.0 {
                return Err(SpecError::BadCompensationFrac(post));
            }
        }
        Ok(())
    }

    /// Generate the MAIN (uncompensated) pulse at `fs_hz`, g. This is exactly
    /// what a delegated (Path A) run downloads: a plain one-signed
    /// classical pulse from the `eigen_dsp::shock::pulse` library, no shoulders.
    pub fn main_pulse(&self, fs_hz: f64) -> Vec<f64> {
        match self.shape {
            ClassicalShape::HalfSine => pulse::half_sine(self.amplitude_g, self.duration_ms, fs_hz),
            ClassicalShape::Haversine => {
                pulse::haversine(self.amplitude_g, self.duration_ms, fs_hz)
            }
            ClassicalShape::TerminalPeakSawtooth => {
                pulse::terminal_peak_sawtooth(self.amplitude_g, self.duration_ms, fs_hz)
            }
            ClassicalShape::InitialPeakSawtooth => {
                pulse::initial_peak_sawtooth(self.amplitude_g, self.duration_ms, fs_hz)
            }
            ClassicalShape::Trapezoid {
                rise_frac,
                fall_frac,
            } => pulse::trapezoid(
                self.amplitude_g,
                self.duration_ms,
                fs_hz,
                rise_frac,
                fall_frac,
            ),
            ClassicalShape::Rectangle => pulse::rectangle(
                self.amplitude_g,
                self.duration_ms,
                fs_hz,
                RECTANGLE_EDGE_FRAC,
                RECTANGLE_EDGE_FRAC,
            ),
        }
    }

    /// Synthesize the HOST-COMPENSATED (Path B ONLY) composite:
    /// generate the main pulse, then apply
    /// [`eigen_dsp::shock::compensate_pulse`], which SOLVES the pre/post shoulder
    /// amplitudes so the composite has zero net velocity and zero net
    /// displacement, verified through the pure (no-high-pass)
    /// integrator.
    ///
    /// Do NOT download this to the delegated path: the device compensates
    /// on-device, and the `ReferenceForPath::delegated` seam will reject the
    /// negative shoulders (that rejection IS the double-compensation guard).
    pub fn synthesize(&self, fs_hz: f64) -> CompensatedPulse {
        compensate_pulse(&self.main_pulse(fs_hz), fs_hz, &self.compensation)
    }

    /// The ideal (continuous-time) velocity change of the MAIN pulse, in g*s:
    /// `c * A * T` with `c` from
    /// [`ClassicalShape::ideal_velocity_change_coefficient`].
    pub fn ideal_velocity_change_gs(&self) -> f64 {
        self.shape.ideal_velocity_change_coefficient() * self.amplitude_g * self.duration_ms
            / 1000.0
    }

    /// The ideal velocity change in in/s (via [`G_TO_IN_S2`]).
    pub fn ideal_velocity_change_ips(&self) -> f64 {
        self.ideal_velocity_change_gs() * G_TO_IN_S2
    }
}

/// Pulse-substitution equivalence (the third tolerance check's
/// design-side helper): given a `target` classical pulse, compute the AMPLITUDE
/// (g) of a `substitute_shape` pulse of `substitute_duration_ms` whose velocity
/// change matches the target's. This is the standards' pulse-substitution rule -
/// when a different shape is run in place of the specified one, equivalence is
/// judged on velocity change:
///
/// ```text
///   A_sub = c_target * A_target * T_target / (c_sub * T_sub)
/// ```
///
/// with `c` the ideal velocity-change coefficient of each shape. Example: a
/// 20 g / 11 ms half-sine (`c = 2/pi`) substituted by a terminal-peak sawtooth
/// (`c = 1/2`) of the same duration requires `20 * (2/pi)/(1/2) = 25.46 g`.
///
/// Errors if the target spec is invalid or the substitute duration is not
/// positive/finite.
pub fn substitute_amplitude_for_velocity_change(
    target: &ClassicalPulseSpec,
    substitute_shape: ClassicalShape,
    substitute_duration_ms: f64,
) -> Result<f64, SpecError> {
    target.validate()?;
    if !substitute_duration_ms.is_finite() || substitute_duration_ms <= 0.0 {
        return Err(SpecError::BadDuration(substitute_duration_ms));
    }
    let c_sub = substitute_shape.ideal_velocity_change_coefficient();
    if c_sub <= 0.0 {
        return Err(SpecError::BadTrapezoidFracs {
            rise_frac: f64::NAN,
            fall_frac: f64::NAN,
        });
    }
    let dv_target_gs = target.ideal_velocity_change_gs();
    Ok(dv_target_gs / (c_sub * substitute_duration_ms / 1000.0))
}

/// Errors from spec validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SpecError {
    /// Amplitude was non-finite or `<= 0` g.
    BadAmplitude(f64),
    /// Duration was non-finite or `<= 0` ms.
    BadDuration(f64),
    /// Trapezoid fractions were non-finite or negative.
    BadTrapezoidFracs { rise_frac: f64, fall_frac: f64 },
    /// A used compensation shoulder fraction was non-finite or `<= 0`.
    BadCompensationFrac(f64),
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::BadAmplitude(a) => {
                write!(f, "spec: amplitude {a} g must be finite and > 0")
            }
            SpecError::BadDuration(d) => {
                write!(f, "spec: duration {d} ms must be finite and > 0")
            }
            SpecError::BadTrapezoidFracs {
                rise_frac,
                fall_frac,
            } => write!(
                f,
                "spec: trapezoid fractions must be finite and >= 0 (rise {rise_frac}, fall {fall_frac})"
            ),
            SpecError::BadCompensationFrac(v) => write!(
                f,
                "spec: compensation shoulder fraction {v} must be finite and > 0"
            ),
        }
    }
}

impl std::error::Error for SpecError {}

#[cfg(test)]
mod tests {
    use super::*;
    use eigen_dsp::shock::pulse_kinematics;

    const FS: f64 = 5120.0;

    fn all_shapes() -> Vec<ClassicalShape> {
        vec![
            ClassicalShape::HalfSine,
            ClassicalShape::Haversine,
            ClassicalShape::TerminalPeakSawtooth,
            ClassicalShape::InitialPeakSawtooth,
            ClassicalShape::Trapezoid {
                rise_frac: 0.1,
                fall_frac: 0.2,
            },
            ClassicalShape::Rectangle,
        ]
    }

    #[test]
    fn serde_round_trip_every_shape_and_compensation() {
        for shape in all_shapes() {
            for comp_shape in [
                CompensationShape::HalfSine,
                CompensationShape::Rectangular,
                CompensationShape::RoundedRectangular,
            ] {
                let spec = ClassicalPulseSpec {
                    shape,
                    amplitude_g: 20.0,
                    duration_ms: 11.0,
                    compensation: CompensationConfig {
                        shape: comp_shape,
                        pre_frac: 2.0,
                        post_frac: 1.25,
                        symmetric: false,
                    },
                };
                let json = serde_json::to_string(&spec).unwrap();
                let back: ClassicalPulseSpec = serde_json::from_str(&json).unwrap();
                assert_eq!(back, spec, "round-trip for {shape:?}/{comp_shape:?}");
            }
        }
    }

    #[test]
    fn serde_json_field_names_are_stable() {
        // The JSON surface is a config-file/IPC contract: pin the field names.
        let spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
        let json = serde_json::to_string(&spec).unwrap();
        for key in [
            "shape",
            "amplitude_g",
            "duration_ms",
            "compensation",
            "pre_frac",
            "post_frac",
            "symmetric",
            "HalfSine",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
    }

    #[test]
    fn synthesize_nulls_residuals_for_every_shape() {
        for shape in all_shapes() {
            let spec = ClassicalPulseSpec::new(shape, 30.0, 8.0);
            let comp = spec.synthesize(FS);
            assert!(
                comp.residual_velocity_ips.abs() < 1e-6,
                "{shape:?}: residual v {}",
                comp.residual_velocity_ips
            );
            assert!(
                comp.residual_displacement_in.abs() < 1e-8,
                "{shape:?}: residual d {}",
                comp.residual_displacement_in
            );
            assert!(comp.peak_displacement_in.is_finite() && comp.peak_displacement_in > 0.0);
            // The main pulse rides uncompensated in the middle.
            let main = spec.main_pulse(FS);
            assert_eq!(
                &comp.waveform_g[comp.pre_len..comp.pre_len + comp.main_len],
                &main[..]
            );
        }
    }

    #[test]
    fn ideal_dv_matches_numeric_integration() {
        // The closed-form coefficients must agree with the pure integrator on the
        // generated pulses, once the grid-realized duration (N-1)/fs is used in
        // place of the nominal duration (the generators span N samples with both
        // endpoints on the grid).
        for shape in all_shapes() {
            let spec = ClassicalPulseSpec::new(shape, 20.0, 11.0);
            let fs = 51_200.0; // fine grid so the sampled ramp/edges converge
            let main = spec.main_pulse(fs);
            let kin = pulse_kinematics(&main, fs);
            let t_realized = (main.len() - 1) as f64 / fs;
            let c = shape.ideal_velocity_change_coefficient();
            let ideal_ips = c * 20.0 * t_realized * G_TO_IN_S2;
            let rel = (kin.residual_velocity_ips - ideal_ips).abs() / ideal_ips;
            assert!(
                rel < 0.01,
                "{shape:?}: numeric dv {} vs ideal {} (rel {rel})",
                kin.residual_velocity_ips,
                ideal_ips
            );
        }
    }

    #[test]
    fn substitution_half_sine_to_sawtooth() {
        // 20 g / 11 ms half-sine -> TPS of the same duration: (2/pi)/(1/2) * 20.
        let target = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
        let a = substitute_amplitude_for_velocity_change(
            &target,
            ClassicalShape::TerminalPeakSawtooth,
            11.0,
        )
        .unwrap();
        let expect = 20.0 * (2.0 / std::f64::consts::PI) / 0.5;
        assert!((a - expect).abs() < 1e-9, "got {a}, want {expect}");

        // Same shape, same duration -> same amplitude.
        let same =
            substitute_amplitude_for_velocity_change(&target, ClassicalShape::HalfSine, 11.0)
                .unwrap();
        assert!((same - 20.0).abs() < 1e-9);

        // Longer substitute duration -> proportionally lower amplitude.
        let longer =
            substitute_amplitude_for_velocity_change(&target, ClassicalShape::HalfSine, 22.0)
                .unwrap();
        assert!((longer - 10.0).abs() < 1e-9);
    }

    #[test]
    fn validate_rejects_bad_specs() {
        let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
        spec.validate().unwrap();

        spec.amplitude_g = 0.0;
        assert!(matches!(spec.validate(), Err(SpecError::BadAmplitude(_))));
        spec.amplitude_g = 20.0;

        spec.duration_ms = -1.0;
        assert!(matches!(spec.validate(), Err(SpecError::BadDuration(_))));
        spec.duration_ms = 11.0;

        spec.shape = ClassicalShape::Trapezoid {
            rise_frac: -0.1,
            fall_frac: 0.1,
        };
        assert!(matches!(
            spec.validate(),
            Err(SpecError::BadTrapezoidFracs { .. })
        ));
        spec.shape = ClassicalShape::HalfSine;

        spec.compensation.pre_frac = 0.0;
        assert!(matches!(
            spec.validate(),
            Err(SpecError::BadCompensationFrac(_))
        ));
    }
}
