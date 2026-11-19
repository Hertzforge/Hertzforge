//! The narrowbands: core-owned parts, not a law (design D4).
//!
//! - **Shape.** A rectangle of added density at the band's current center,
//!   its edge lines weighted by the share of their bin the band covers, over
//!   the identified plant: drive density `gain * level * w_k / |H_k|^2`.
//! - **Level, in the band's own moving frame (D7).** Per analysis frame, the
//!   mean of the residual over the band's inner half (lines within a quarter
//!   width of that frame's center); averaged frame to frame, so a sweep does
//!   not smear it and its degrees of freedom add up. The reference is the floor
//!   target plus the band's level over the same lines (sum, never maximum).
//! - **Servo.** Log-proportional on the band's power gain, capped per loop.
//! - **Judgement.** The band's alarm and abort are widened to its own
//!   chi-square scatter at [`BAND_CONFIDENCE`] (the data sheet's rule).

/// The confidence a band's tolerance is widened to (the data sheet's).
pub const BAND_CONFIDENCE: f64 = 0.90;
/// Lines each side of a band left out of the floor estimate beyond its edge
/// lines. Measured: a band 20 dB over the floor, synthesized and read through
/// the run's own frames, reads +13.7, +4.2, +0.9 and +0.3 dB one to four lines
/// out, so three are left out (the floor's guard test).
pub const BAND_GUARD_LINES: usize = 3;
/// The band servo's proportional gain on the power error, per loop.
pub const DEFAULT_BAND_SERVO_GAIN: f64 = 0.5;
/// The band servo's largest move in one loop, dB.
pub const DEFAULT_BAND_SERVO_CAP_DB: f64 = 1.0;
/// Effective resolution of a Hann line in lines (its noise bandwidth): `n`
/// adjacent lines are worth about `n / 1.5` independent ones.
const HANN_ENBW_LINES: f64 = 1.5;

/// Weights of each line of a grid (line `k` at `f0 + k df`, its bin half a
/// line either side) for a band `[lo, hi)` Hz: the share of the bin the band
/// covers, 0 to 1.
pub fn band_weights(f0: f64, df: f64, lo: f64, hi: f64, out: &mut [f64]) {
    for (k, w) in out.iter_mut().enumerate() {
        let c = f0 + k as f64 * df;
        let (a, b) = (c - 0.5 * df, c + 0.5 * df);
        *w = ((hi.min(b) - lo.max(a)) / df).clamp(0.0, 1.0);
    }
}

/// The inner lines of a band at `center` of `width`: those within a quarter
/// width of the center, `[a, b)` on a grid from `f0` at `df`, at least the
/// line nearest the center.
pub fn inner_lines(f0: f64, df: f64, center: f64, width: f64, lines: usize) -> (usize, usize) {
    let q = width / 4.0;
    let a = ((center - q - f0) / df).ceil().max(0.0) as usize;
    let b = (((center + q - f0) / df).floor() + 1.0).max(0.0) as usize;
    let mid = ((center - f0) / df).round().max(0.0) as usize;
    if b <= a {
        (mid.min(lines), (mid + 1).min(lines))
    } else {
        (a.min(lines), b.min(lines))
    }
}

/// Degrees of freedom of one frame's mean over `n` adjacent Hann lines.
pub fn frame_dof(n: usize) -> f64 {
    2.0 * (n as f64 / HANN_ENBW_LINES).max(1.0) * crate::floor::dof_per_frame() / 2.0
}

/// A band's level, read in its own moving frame and averaged frame to frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BandLevel {
    sum_measured: f64,
    sum_reference: f64,
    frames: f64,
    dof: f64,
    depth: f64,
}

impl BandLevel {
    /// An empty average that forgets past `depth` frames (0: never).
    pub fn new(depth: usize) -> Self {
        BandLevel {
            depth: depth as f64,
            ..Default::default()
        }
    }

    /// Forget everything.
    pub fn reset(&mut self) {
        *self = BandLevel::new(self.depth as usize);
    }

    /// Add one frame: the residual `psd` and the reference `reference` (floor
    /// target plus band, at the level the frame played) over the frame's inner
    /// lines `[a, b)`.
    pub fn add_frame(&mut self, psd: &[f64], reference: &[f64], lines: (usize, usize)) {
        let (a, b) = lines;
        if b <= a || b > psd.len() || b > reference.len() {
            return;
        }
        let n = (b - a) as f64;
        let m = psd[a..b].iter().sum::<f64>() / n;
        let r = reference[a..b].iter().sum::<f64>() / n;
        if self.depth > 0.0 && self.frames >= self.depth {
            let keep = (self.depth - 1.0) / self.depth;
            self.sum_measured *= keep;
            self.sum_reference *= keep;
            self.dof *= keep;
            self.frames *= keep;
        }
        self.sum_measured += m;
        self.sum_reference += r;
        self.frames += 1.0;
        self.dof += frame_dof(b - a);
    }

    /// The mean measured level (floor plus band), g^2/Hz; `None` before a
    /// frame.
    pub fn measured(&self) -> Option<f64> {
        (self.frames > 0.0).then(|| self.sum_measured / self.frames)
    }

    /// The mean reference over the same frames, g^2/Hz.
    pub fn reference(&self) -> Option<f64> {
        (self.frames > 0.0).then(|| self.sum_reference / self.frames)
    }

    /// Measured over reference, dB.
    pub fn error_db(&self) -> Option<f64> {
        match (self.measured(), self.reference()) {
            (Some(m), Some(r)) if m > 0.0 && r > 0.0 => Some(10.0 * (m / r).log10()),
            _ => None,
        }
    }

    /// Degrees of freedom of the average.
    pub fn dof(&self) -> f64 {
        self.dof
    }

    /// Frames in the average.
    pub fn frames(&self) -> f64 {
        self.frames
    }
}

/// A band's servo: a power gain on its feed-forward density, moved each loop
/// by `gain` times the error in dB, at most `cap_db` either way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandServo {
    /// The gain, dB.
    pub gain_db: f64,
    /// Proportional gain on the dB error.
    pub k: f64,
    /// Largest move per loop, dB.
    pub cap_db: f64,
}

impl Default for BandServo {
    fn default() -> Self {
        BandServo {
            gain_db: 0.0,
            k: DEFAULT_BAND_SERVO_GAIN,
            cap_db: DEFAULT_BAND_SERVO_CAP_DB,
        }
    }
}

impl BandServo {
    /// One loop on the band's added level: `measured_added` and
    /// `reference_added` are the band's level over the floor (measured level
    /// less the floor's, against the configured level). Returns whether the
    /// cap bound.
    pub fn update(&mut self, measured_added: f64, reference_added: f64) -> bool {
        if reference_added.is_nan() || reference_added <= 0.0 {
            return false;
        }
        // A band read at or under the floor is pushed up by the cap.
        let e = if measured_added > 0.0 {
            10.0 * (reference_added / measured_added).log10()
        } else {
            self.cap_db / self.k.max(1e-9)
        };
        let step = (self.k * e).clamp(-self.cap_db, self.cap_db);
        self.gain_db += step;
        (self.k * e).abs() > self.cap_db
    }

    /// The power gain as a factor.
    pub fn gain(&self) -> f64 {
        10f64.powf(self.gain_db / 10.0)
    }
}

/// Add a band's drive density into `drive`: `gain * level * w_k / |H_k|^2`
/// where `|H_k|^2` is positive (a line with no plant gets nothing).
pub fn add_band_drive(weights: &[f64], level: f64, gain: f64, h_mag2: &[f64], drive: &mut [f64]) {
    for ((d, &w), &h) in drive.iter_mut().zip(weights).zip(h_mag2) {
        if w > 0.0 && h > 0.0 {
            *d += gain * level * w / h;
        }
    }
}

/// The standard normal quantile (Acklam's rational approximation, relative
/// error under 1.2e-9).
pub fn normal_quantile(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let p = p.clamp(1e-300, 1.0 - 1e-16);
    let pl = 0.02425;
    if p < pl {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - pl {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        -normal_quantile(1.0 - p)
    }
}

/// The chi-square quantile at `p` for `nu` degrees of freedom
/// (Wilson-Hilferty; within about 1 % of the tabulated value at 4 degrees of
/// freedom and better above).
pub fn chi2_quantile(nu: f64, p: f64) -> f64 {
    let z = normal_quantile(p);
    let h = 2.0 / (9.0 * nu);
    (nu * (1.0 - h + z * h.sqrt()).powi(3)).max(0.0)
}

/// The two-sided interval of a density estimate at `nu` degrees of freedom
/// and `confidence`, dB `(below, above)` as positive magnitudes:
/// `[nu / chi2(1 - a/2), nu / chi2(a/2)]`.
pub fn chi2_interval_db(nu: f64, confidence: f64) -> (f64, f64) {
    if nu.is_nan() || nu <= 0.0 {
        return (f64::INFINITY, f64::INFINITY);
    }
    let a = 1.0 - confidence;
    let lo = nu / chi2_quantile(nu, 1.0 - a / 2.0);
    let hi = nu / chi2_quantile(nu, a / 2.0).max(1e-300);
    (-10.0 * lo.log10(), 10.0 * hi.log10())
}

/// A band's tolerance `(minus, plus)` dB widened to its own scatter at `nu`
/// degrees of freedom: each side the larger of the stated side and the
/// interval's.
pub fn widened_db(stated: (f64, f64), nu: f64) -> (f64, f64) {
    let (lo, hi) = chi2_interval_db(nu, BAND_CONFIDENCE);
    (stated.0.max(lo), stated.1.max(hi))
}

#[cfg(test)]
mod tests;
