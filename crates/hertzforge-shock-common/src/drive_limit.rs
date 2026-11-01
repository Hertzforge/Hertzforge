//! Drive peak limiting (the honesty split), shared by both shock modules.
//!
//! # The honesty split (load-bearing - do not blur it)
//!
//! There are TWO fundamentally different enforcement timings, and this module
//! keeps them in separate types and separate calls so neither can masquerade as the
//! other:
//!
//! - **Peak-ACCEL limiting is PREDICTIVE (pre-fire).** With a measured drive->accel
//!   FRF (from the reduced-level system ID, same lineage as the SAND2016-1335C
//!   predictive notching in `hertzforge-srs::limit`), the peak acceleration a next pulse
//!   would produce is predictable BEFORE firing. [`DriveLimit::pre_fire_accel`]
//!   returns a [`PreFireDecision`] with the scale-down to apply BEFORE the pulse.
//! - **CURRENT / VOLTAGE limiting is BETWEEN-PULSE measured enforcement.** V/I
//!   require a physically WIRED monitor channel (confirmed 2026-07-04: the
//!   amplifier V/I drops live in the CHANNEL TABLE as ordinary channels with V / A
//!   engineering units and standard mV-per-EU sensitivities). Enforcement compares
//!   the MEASURED peak of the LAST pulse against the ceiling and scales the NEXT
//!   pulse ([`DriveLimit::between_pulse`]). It is NOT a pre-fire prediction: the
//!   first over-ceiling pulse has already happened by the time it is caught. A
//!   drive->V/I FRF could in principle make V/I predictive too, but current near amp
//!   compression is NONLINEAR, so a predictive V/I monitor
//!   ([`DriveMonitorKind::Predictive`] + [`DriveLimit::pre_fire_monitor`]) carries
//!   that caveat explicitly in its docs.
//!
//! # Path A: V/I are unobservable -> monitors are INERT (greyed)
//!
//! On the delegated path the amplifier V/I drops are not observable through the
//! delegated loop ([`crate::path::PathCapabilities::vi_limiting_observable`] is
//! `false` - the ONE source of truth; this module CONSULTS it, never re-derives path
//! logic). [`DriveLimit::between_pulse`] marks every V/I-class monitor
//! [`MonitorEnforcement::InertPathA`] there: no scale-down, no false sense of
//! protection, and the UI greys the ceilings from the same boolean. Accel-EU
//! monitors stay enforceable on both paths (pulse capture exists on Path A too).
//!
//! # Generic over the channel's engineering unit
//!
//! A monitor is `(channel name, EU string, ceiling, kind)` - deliberately GENERIC
//! over the channel's engineering-unit value. The channel-table `Units` enum gaining
//! Voltage/Current variants is a SEPARATE, deferred step; nothing here depends on it.
//! The `eu` string ("V", "A", "g", ...) is carried for classification
//! ([`DriveMonitor::is_vi`]) and display only.
//!
//! # Scale-down model (both timings)
//!
//! Peaks scale linearly with the drive scale (the same linear-plant assumption the
//! FRF encodes), so the corrective factor is `ceiling / peak`. For the predictive
//! path that makes the scaled pulse fit the ceiling exactly; for the between-pulse
//! path it is the best linear estimate, re-verified by the next pulse's measurement
//! (and genuinely approximate near amplifier compression - see above).

use crate::path::PathCapabilities;
use hertzforge_dsp::fft;
use serde::{Deserialize, Serialize};

/// When a monitor's ceiling is enforced (the two timings are NOT
/// interchangeable; see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveMonitorKind {
    /// Enforced PRE-FIRE from a predicted peak (requires a measured drive->channel
    /// FRF). Honest for accel; for V/I it inherits the amp-compression nonlinearity
    /// caveat.
    Predictive,
    /// Enforced BETWEEN pulses from the MEASURED peak of the last pulse (requires
    /// the monitor physically wired to an input channel). The first breach has
    /// already fired by the time it is caught - this is enforcement, not
    /// prediction.
    BetweenPulse,
}

/// One monitored channel with a peak ceiling. Generic over the
/// channel's engineering unit - see the module docs (the Units-enum change is a
/// separate deferred step).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveMonitor {
    /// The channel-table channel name this monitor reads.
    pub channel: String,
    /// The channel's engineering unit as a display/classification string:
    /// `"V"` (volts), `"A"` (amps), `"g"` (acceleration), etc.
    pub eu: String,
    /// The peak ceiling in the channel's EU. Must be finite and > 0.
    pub peak_ceiling: f64,
    /// Which enforcement timing this monitor uses (the honesty split).
    pub kind: DriveMonitorKind,
}

impl DriveMonitor {
    /// Whether this monitor is a VOLTAGE/CURRENT-class channel - the class that is
    /// unobservable on Path A. Classified from the EU string,
    /// case-insensitively: `V`/`volt`/`volts` and `A`/`amp`/`amps`/`ampere(s)`.
    pub fn is_vi(&self) -> bool {
        let eu = self.eu.trim().to_ascii_lowercase();
        matches!(eu.as_str(), "v" | "volt" | "volts" | "mv")
            || matches!(
                eu.as_str(),
                "a" | "amp" | "amps" | "ampere" | "amperes" | "ma"
            )
    }

    /// Validate the monitor's configuration.
    pub fn validate(&self) -> Result<(), DriveLimitError> {
        if self.channel.trim().is_empty() {
            return Err(DriveLimitError::BadConfig(
                "monitor channel name is empty".into(),
            ));
        }
        if !self.peak_ceiling.is_finite() || self.peak_ceiling <= 0.0 {
            return Err(DriveLimitError::BadConfig(format!(
                "monitor '{}': peak_ceiling must be finite and > 0 (got {})",
                self.channel, self.peak_ceiling
            )));
        }
        Ok(())
    }
}

/// The drive-limiting configuration for a shock test: an optional
/// pre-fire peak-accel ceiling plus any number of monitored channels. Per the
/// identity-vs-test scope line, ceilings live HERE (test config), not in the
/// channel identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveLimit {
    /// Predictive peak-acceleration ceiling, g ([`DriveLimit::pre_fire_accel`]).
    /// `None` = no accel ceiling configured.
    pub peak_accel_g: Option<f64>,
    /// The monitored channels (V/I/accel/... peaks with ceilings).
    pub monitors: Vec<DriveMonitor>,
}

impl DriveLimit {
    /// Validate the whole configuration.
    pub fn validate(&self) -> Result<(), DriveLimitError> {
        if let Some(a) = self.peak_accel_g {
            if !a.is_finite() || a <= 0.0 {
                return Err(DriveLimitError::BadConfig(format!(
                    "peak_accel_g must be finite and > 0 (got {a})"
                )));
            }
        }
        for m in &self.monitors {
            m.validate()?;
        }
        Ok(())
    }

    /// PRE-FIRE peak-accel check (peak accel is the ONE quantity that is
    /// honestly predictable before firing, via the measured drive->accel FRF).
    ///
    /// `predicted_peak_accel_g` is the predicted peak of the NEXT pulse at the
    /// scale about to be commanded - typically [`predict_peak_accel_g`] on the
    /// drive-scaled reference. Returns the decision: `allowed` when the prediction
    /// fits the ceiling, else the scale-down factor (`ceiling / predicted`, < 1) to
    /// apply BEFORE firing so the fired pulse fits (peaks scale linearly with the
    /// drive scale). With no ceiling configured the decision is trivially allowed.
    ///
    /// A non-finite prediction refuses to fire (`allowed = false`, scale `0.0`):
    /// garbage in must not fire a pulse.
    pub fn pre_fire_accel(&self, predicted_peak_accel_g: f64) -> PreFireDecision {
        let Some(ceiling) = self.peak_accel_g else {
            return PreFireDecision {
                allowed: true,
                scale_factor: 1.0,
                reason: "no peak-accel ceiling configured".into(),
            };
        };
        Self::predictive_decision(predicted_peak_accel_g, ceiling, "peak accel", "g")
    }

    /// PRE-FIRE check for a [`DriveMonitorKind::Predictive`] monitor channel:
    /// same math as [`Self::pre_fire_accel`], against the named monitor's ceiling.
    /// Returns `None` when no Predictive monitor with that channel name exists.
    ///
    /// The caller supplies the predicted peak in the channel's EU (via a separately
    /// identified drive->channel FRF). For V/I channels the nonlinearity caveat
    /// applies: current near amplifier compression does not scale linearly, so a
    /// predictive V/I decision is an approximation - prefer
    /// [`DriveMonitorKind::BetweenPulse`] for V/I unless the FRF was identified
    /// close to the working level.
    pub fn pre_fire_monitor(
        &self,
        channel: &str,
        predicted_peak_eu: f64,
    ) -> Option<PreFireDecision> {
        let m = self
            .monitors
            .iter()
            .find(|m| m.kind == DriveMonitorKind::Predictive && m.channel == channel)?;
        Some(Self::predictive_decision(
            predicted_peak_eu,
            m.peak_ceiling,
            &m.channel,
            &m.eu,
        ))
    }

    fn predictive_decision(predicted: f64, ceiling: f64, what: &str, eu: &str) -> PreFireDecision {
        if !predicted.is_finite() || predicted < 0.0 {
            return PreFireDecision {
                allowed: false,
                scale_factor: 0.0,
                reason: format!(
                    "{what}: predicted peak {predicted} {eu} is not a valid magnitude; refusing \
                     to fire"
                ),
            };
        }
        if predicted <= ceiling {
            PreFireDecision {
                allowed: true,
                scale_factor: 1.0,
                reason: format!(
                    "{what}: predicted peak {predicted:.4} {eu} within ceiling {ceiling:.4} {eu}"
                ),
            }
        } else {
            let scale = ceiling / predicted;
            PreFireDecision {
                allowed: false,
                scale_factor: scale,
                reason: format!(
                    "{what}: predicted peak {predicted:.4} {eu} exceeds ceiling {ceiling:.4} \
                     {eu}; scale the drive by {scale:.4} before firing"
                ),
            }
        }
    }

    /// BETWEEN-PULSE measured enforcement for the [`DriveMonitorKind::BetweenPulse`]
    /// monitors (V/I ceilings are enforced from the MEASURED peak of the
    /// last pulse - scale the NEXT pulse, never a pre-fire guarantee).
    ///
    /// `caps` is the drive path's capability table
    /// ([`crate::path::PathCapabilities`], the one source of path truth): when
    /// `vi_limiting_observable` is `false` (Path A, delegated), every V/I-class
    /// monitor is INERT - reported as [`MonitorEnforcement::InertPathA`], never
    /// scaling - because the drops are simply not observable through the delegated
    /// loop. Non-V/I (e.g. accel-EU) between-pulse monitors remain enforced on both
    /// paths.
    ///
    /// `measured_peaks` are the last pulse's per-channel peak magnitudes in each
    /// channel's EU (from the pulse capture). A monitor with no measurement reports
    /// [`MonitorEnforcement::NoMeasurement`] and does not scale (and the service
    /// should treat a persistently-unmeasured wired monitor as a rig fault).
    ///
    /// Returns the combined [`BetweenPulseDecision`]: `scale_factor` is the MINIMUM
    /// over all enforced, breached monitors (tightest limit wins - the same
    /// safety-monotone arbitration as the response notch), `1.0` when nothing
    /// breached. [`DriveMonitorKind::Predictive`] monitors are NOT evaluated here -
    /// they belong to the pre-fire path.
    pub fn between_pulse(
        &self,
        caps: &PathCapabilities,
        measured_peaks: &[MeasuredPeak],
    ) -> BetweenPulseDecision {
        let mut actions = Vec::new();
        let mut scale = 1.0f64;
        for m in &self.monitors {
            if m.kind != DriveMonitorKind::BetweenPulse {
                continue; // predictive monitors are pre-fire; not enforced here
            }
            if m.is_vi() && !caps.vi_limiting_observable {
                actions.push(MonitorAction {
                    channel: m.channel.clone(),
                    eu: m.eu.clone(),
                    enforcement: MonitorEnforcement::InertPathA,
                    measured_peak_eu: None,
                    peak_ceiling: m.peak_ceiling,
                    scale_factor: 1.0,
                    reason: format!(
                        "'{}' ({}) is a V/I monitor and V/I are unobservable on this drive path \
                         (Path A: delegated controller); ceiling is inert/greyed",
                        m.channel, m.eu
                    ),
                });
                continue;
            }
            let measured = measured_peaks
                .iter()
                .find(|p| p.channel == m.channel)
                .map(|p| p.peak_eu);
            let Some(peak) = measured else {
                actions.push(MonitorAction {
                    channel: m.channel.clone(),
                    eu: m.eu.clone(),
                    enforcement: MonitorEnforcement::NoMeasurement,
                    measured_peak_eu: None,
                    peak_ceiling: m.peak_ceiling,
                    scale_factor: 1.0,
                    reason: format!(
                        "no measured peak for '{}' on the last pulse; between-pulse enforcement \
                         needs the wired monitor's capture",
                        m.channel
                    ),
                });
                continue;
            };
            if !peak.is_finite() || peak < 0.0 {
                actions.push(MonitorAction {
                    channel: m.channel.clone(),
                    eu: m.eu.clone(),
                    enforcement: MonitorEnforcement::NoMeasurement,
                    measured_peak_eu: Some(peak),
                    peak_ceiling: m.peak_ceiling,
                    scale_factor: 1.0,
                    reason: format!(
                        "measured peak for '{}' is not a valid magnitude ({peak}); treating as \
                         unmeasured",
                        m.channel
                    ),
                });
                continue;
            }
            if peak > m.peak_ceiling {
                let s = m.peak_ceiling / peak;
                if s < scale {
                    scale = s;
                }
                actions.push(MonitorAction {
                    channel: m.channel.clone(),
                    eu: m.eu.clone(),
                    enforcement: MonitorEnforcement::Enforced,
                    measured_peak_eu: Some(peak),
                    peak_ceiling: m.peak_ceiling,
                    scale_factor: s,
                    reason: format!(
                        "'{}' measured peak {peak:.4} {} exceeded ceiling {:.4} {}; scale the \
                         NEXT pulse by {s:.4} (measured enforcement - this pulse already fired)",
                        m.channel, m.eu, m.peak_ceiling, m.eu
                    ),
                });
            } else {
                actions.push(MonitorAction {
                    channel: m.channel.clone(),
                    eu: m.eu.clone(),
                    enforcement: MonitorEnforcement::Enforced,
                    measured_peak_eu: Some(peak),
                    peak_ceiling: m.peak_ceiling,
                    scale_factor: 1.0,
                    reason: format!(
                        "'{}' measured peak {peak:.4} {} within ceiling {:.4} {}",
                        m.channel, m.eu, m.peak_ceiling, m.eu
                    ),
                });
            }
        }
        BetweenPulseDecision {
            scale_factor: scale,
            actions,
        }
    }
}

/// A pre-fire limiting decision (the PREDICTIVE side).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreFireDecision {
    /// Whether the pulse may fire AS-IS (prediction within the ceiling).
    pub allowed: bool,
    /// The drive scale to apply BEFORE firing so the pulse fits the ceiling
    /// (`1.0` when allowed; `ceiling/predicted < 1` on a predicted breach; `0.0`
    /// on an invalid prediction - refuse to fire).
    pub scale_factor: f64,
    /// Human-readable rationale for the run log / UI.
    pub reason: String,
}

/// One channel's measured peak from the LAST pulse, in the channel's EU (from the
/// pulse capture - the between-pulse enforcement input).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasuredPeak {
    /// The channel-table channel name.
    pub channel: String,
    /// The measured peak magnitude, in the channel's EU.
    pub peak_eu: f64,
}

/// How a between-pulse monitor was handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonitorEnforcement {
    /// The measurement was compared against the ceiling (scaled if breached).
    Enforced,
    /// V/I monitor on a path where V/I are unobservable (Path A) - INERT, greyed.
    /// No enforcement happened or could happen.
    InertPathA,
    /// No (valid) measured peak was supplied for the monitor's channel.
    NoMeasurement,
}

/// One monitor's between-pulse outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorAction {
    /// The monitor's channel name.
    pub channel: String,
    /// The monitor's EU string.
    pub eu: String,
    /// How the monitor was handled.
    pub enforcement: MonitorEnforcement,
    /// The measured peak used (None when inert/unmeasured).
    pub measured_peak_eu: Option<f64>,
    /// The configured ceiling.
    pub peak_ceiling: f64,
    /// This monitor's next-pulse scale contribution (`1.0` = none).
    pub scale_factor: f64,
    /// Human-readable rationale for the run log / UI.
    pub reason: String,
}

/// The combined between-pulse decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BetweenPulseDecision {
    /// The scale to apply to the NEXT pulse: the minimum over all enforced,
    /// breached monitors (tightest limit wins); `1.0` when nothing breached.
    pub scale_factor: f64,
    /// Per-monitor outcomes (including inert and unmeasured ones, so the UI can
    /// show WHY a ceiling did or did not act).
    pub actions: Vec<MonitorAction>,
}

/// Errors from configuring a [`DriveLimit`].
#[derive(Debug, Clone, PartialEq)]
pub enum DriveLimitError {
    /// A configuration value was invalid.
    BadConfig(String),
}

impl std::fmt::Display for DriveLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriveLimitError::BadConfig(s) => write!(f, "drive limit config error: {s}"),
        }
    }
}

impl std::error::Error for DriveLimitError {}

/// Predict the peak acceleration a drive record would produce through a measured
/// drive->accel FRF magnitude: `peak( IFFT( FFT(drive) * |H| ) )` (the
/// predictive pre-fire path).
///
/// `frf_freqs`/`frf_mag` sample `|H(f)|` on an ascending frequency grid (from the
/// reduced-level system ID, e.g. [`hertzforge_dsp::frf::h_estimate_hv_with_coherence`]'s
/// magnitude on its bin grid); the magnitude is linearly interpolated onto the FFT
/// bin grid and HELD at the endpoints outside the measured range (out-of-band drive
/// content is never treated as producing zero response). Magnitude-only filtering is
/// a ZERO-PHASE response model - per-bin amplitudes are right, inter-bin phase
/// alignment is approximate - which is the same honest approximation
/// `hertzforge-srs::limit` documents for the predictive notch; budget it in the ceiling.
///
/// Returns `0.0` for degenerate inputs (empty drive/FRF, bad sample rate).
pub fn predict_peak_accel_g(
    drive_g: &[f64],
    fs_hz: f64,
    frf_freqs: &[f64],
    frf_mag: &[f64],
) -> f64 {
    if drive_g.is_empty()
        || fs_hz <= 0.0
        || !fs_hz.is_finite()
        || frf_freqs.is_empty()
        || frf_freqs.len() != frf_mag.len()
    {
        return 0.0;
    }
    let nfft = fft::next_pow2(drive_g.len());
    let nh = nfft / 2 + 1;
    let bin_hz = fs_hz / nfft as f64;

    let mut spec = fft::rfft(drive_g);
    // Apply |H| to the nonnegative-frequency half, mirror to the conjugate half so
    // the inverse transform stays real.
    for k in 0..nh {
        let h = interp_clamped(frf_freqs, frf_mag, k as f64 * bin_hz);
        spec[k].0 *= h;
        spec[k].1 *= h;
        if k > 0 && k < nfft - k {
            spec[nfft - k] = (spec[k].0, -spec[k].1);
        }
    }
    fft::ifft(&spec)
        .iter()
        .fold(0.0f64, |m, &(re, _)| m.max(re.abs()))
}

/// Linear interpolation of `(xs, ys)` at `x`, CLAMPED to the endpoint values outside
/// the sampled range. `xs` must be ascending.
fn interp_clamped(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    let n = xs.len();
    if n == 0 {
        return 0.0;
    }
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[n - 1] {
        return ys[n - 1];
    }
    let mut lo = 0usize;
    let mut hi = n - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if xs[mid] <= x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (x0, x1) = (xs[lo], xs[hi]);
    let (y0, y1) = (ys[lo], ys[hi]);
    if x1 <= x0 {
        return y0;
    }
    y0 + (y1 - y0) * (x - x0) / (x1 - x0)
}
