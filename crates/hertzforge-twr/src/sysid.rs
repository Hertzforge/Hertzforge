//! System identification ("ranging" / pretest): low-level broadband excitation
//! through the [`crate::player::ReferencePlayer`], then the Welch Hv FRF +
//! coherence estimate (`hertzforge_dsp::frf::h_estimate_hv_with_coherence`).
//!
//! # Excitation scale
//!
//! The sysid drive amplitude is an ABSOLUTE knob in DRIVE units
//! ([`SysIdConfig::drive_rms`], conservative default), not a percent of full
//! level: before sysid no FRF exists, so "10% of output" is not computable.
//! The drive clip is enforced during sysid too (by the service). If the in-band
//! mean coherence comes back below the floor, sysid HOLDS AND REPORTS - the
//! operator raises `drive_rms` or adjusts the band; the level is never raised
//! silently (the same posture as the Random module).
//!
//! # Excitation shapes
//!
//! - [`Excitation::Random`]: band-limited flat-PSD random via
//!   `hertzforge_dsp::cola::ColaSynth` (variance-flat WOLA stream). The default -
//!   what commercial TWR pretests use.
//! - [`Excitation::LogChirp`]: a logarithmic sweep `f_lo -> f_hi` with cosine
//!   end ramps. Energy density falls ~1/f (flat per octave); useful when a
//!   deterministic excitation is preferred. The Welch estimator handles either.

use hertzforge_dsp::cola::ColaSynth;
use hertzforge_dsp::fft::Cplx;
use hertzforge_dsp::frf;
use hertzforge_dsp::stats;
use serde::{Deserialize, Serialize};

use crate::TwrError;

/// The ranging excitation shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Excitation {
    /// Band-limited flat-PSD random (COLA/WOLA synthesis). The default.
    Random,
    /// Logarithmic chirp `f_lo -> f_hi` with cosine end ramps.
    LogChirp,
}

/// System-identification configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SysIdConfig {
    pub excitation: Excitation,
    /// Excitation RMS in DRIVE units (absolute). Default 0.05.
    pub drive_rms: f64,
    /// Excitation record length, seconds. More seconds = more Welch averages =
    /// lower FRF variance. Default 4 s.
    pub duration_s: f64,
    /// Welch/FRF block size (power of two). df_sysid = fs / nfft. Default 2048.
    pub nfft: usize,
    /// Identification band low edge, Hz (also the correction band; > 0).
    pub f_lo_hz: f64,
    /// Identification band high edge, Hz (< fs/2).
    pub f_hi_hz: f64,
    /// Minimum acceptable IN-BAND MEAN coherence; below it sysid holds and
    /// reports (never silently proceeds). Default 0.8.
    pub min_mean_coherence: f64,
}

impl Default for SysIdConfig {
    fn default() -> Self {
        SysIdConfig {
            excitation: Excitation::Random,
            drive_rms: 0.05,
            duration_s: 4.0,
            nfft: 2048,
            f_lo_hz: 20.0,
            f_hi_hz: 2000.0,
            min_mean_coherence: 0.8,
        }
    }
}

impl SysIdConfig {
    /// Validate against a playout rate.
    pub fn validate(&self, fs_hz: f64) -> Result<(), TwrError> {
        if self.drive_rms.is_nan() || self.drive_rms <= 0.0 {
            return Err(TwrError::Config("sysid drive_rms must be positive".into()));
        }
        if self.duration_s.is_nan() || self.duration_s <= 0.0 {
            return Err(TwrError::Config("sysid duration_s must be positive".into()));
        }
        if !self.nfft.is_power_of_two() || self.nfft < 64 {
            return Err(TwrError::Config(format!(
                "sysid nfft must be a power of two >= 64 (got {})",
                self.nfft
            )));
        }
        if self.f_lo_hz.is_nan()
            || self.f_hi_hz.is_nan()
            || self.f_lo_hz <= 0.0
            || self.f_hi_hz <= self.f_lo_hz
            || self.f_hi_hz >= fs_hz / 2.0
        {
            return Err(TwrError::Config(format!(
                "sysid band [{}, {}] Hz invalid for fs {} (need 0 < f_lo < f_hi < fs/2)",
                self.f_lo_hz, self.f_hi_hz, fs_hz
            )));
        }
        if self.min_mean_coherence.is_nan()
            || self.min_mean_coherence <= 0.0
            || self.min_mean_coherence > 1.0
        {
            return Err(TwrError::Config(
                "sysid min_mean_coherence must be in (0, 1]".into(),
            ));
        }
        let n = (self.duration_s * fs_hz) as usize;
        if n < 2 * self.nfft {
            return Err(TwrError::Config(format!(
                "sysid duration too short: {n} samples < 2 * nfft ({}) - coherence needs >= 2 \
                 averaged blocks",
                self.nfft
            )));
        }
        Ok(())
    }

    /// Build the excitation record at `fs_hz` (length `duration_s * fs`),
    /// normalized to exactly `drive_rms`.
    pub fn build_excitation(&self, fs_hz: f64) -> Vec<f64> {
        let n = (self.duration_s * fs_hz).round() as usize;
        let mut x = match self.excitation {
            Excitation::Random => {
                // Flat one-sided PSD over the band whose integral is drive_rms^2:
                // S0 = rms^2 / (f_hi - f_lo).
                let nfft = self.nfft;
                let nbins = nfft / 2 + 1;
                let df = fs_hz / nfft as f64;
                let s0 = self.drive_rms * self.drive_rms / (self.f_hi_hz - self.f_lo_hz);
                let psd: Vec<f64> = (0..nbins)
                    .map(|k| {
                        let f = k as f64 * df;
                        if f >= self.f_lo_hz && f <= self.f_hi_hz {
                            s0
                        } else {
                            0.0
                        }
                    })
                    .collect();
                let mut synth = ColaSynth::with_seed(nfft, 0.5, fs_hz, 0x71E5_1D00_5EED_0001);
                let mut out = Vec::with_capacity(n + nfft);
                while out.len() < n {
                    out.extend_from_slice(&synth.next_frame(&psd));
                }
                out.truncate(n);
                out
            }
            Excitation::LogChirp => {
                // Log sweep: phase(t) = 2*pi * f_lo * T_k * (e^{t/T_k} - 1),
                // T_k = T / ln(f_hi/f_lo). Cosine ramps over 5% each end.
                let t_total = n as f64 / fs_hz;
                let k = t_total / (self.f_hi_hz / self.f_lo_hz).ln();
                let ramp = (n / 20).max(1);
                (0..n)
                    .map(|i| {
                        let t = i as f64 / fs_hz;
                        let phase =
                            std::f64::consts::TAU * self.f_lo_hz * k * ((t / k).exp() - 1.0);
                        let mut a = phase.sin();
                        if i < ramp {
                            a *= 0.5 - 0.5 * (std::f64::consts::PI * i as f64 / ramp as f64).cos();
                        }
                        if i >= n - ramp {
                            let j = n - 1 - i;
                            a *= 0.5 - 0.5 * (std::f64::consts::PI * j as f64 / ramp as f64).cos();
                        }
                        a
                    })
                    .collect()
            }
        };
        // Exact-RMS normalization (the synth gives a statistical RMS).
        let r = stats::rms(&x);
        if r > 0.0 {
            let g = self.drive_rms / r;
            for v in x.iter_mut() {
                *v *= g;
            }
        }
        x
    }

    /// Estimate the FRF + coherence from a played (drive, response) pair and
    /// apply the coherence hold gate.
    pub fn estimate(
        &self,
        drive: &[f64],
        response: &[f64],
        fs_hz: f64,
    ) -> Result<SysIdResult, TwrError> {
        let (h, coherence) = frf::h_estimate_hv_with_coherence(drive, response, fs_hz, self.nfft);
        let df = fs_hz / self.nfft as f64;
        let (mut sum, mut cnt) = (0.0f64, 0usize);
        for (k, &c) in coherence.iter().enumerate() {
            let f = k as f64 * df;
            if f >= self.f_lo_hz && f <= self.f_hi_hz {
                sum += c;
                cnt += 1;
            }
        }
        let mean = if cnt > 0 { sum / cnt as f64 } else { 0.0 };
        if mean < self.min_mean_coherence {
            return Err(TwrError::SysId(format!(
                "in-band mean coherence {mean:.3} below floor {:.3} - raise sysid drive_rms, \
                 lengthen duration_s, or adjust the band; the level is never raised silently",
                self.min_mean_coherence
            )));
        }
        Ok(SysIdResult {
            h,
            coherence,
            nfft: self.nfft,
            fs_hz,
            in_band_mean_coherence: mean,
        })
    }
}

/// The identification product: FRF + coherence on the sysid grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SysIdResult {
    /// Complex FRF drive->control, bins `0 ..= nfft/2` (the hertzforge-dsp
    /// `<X*conj(Y)>` phase convention - load-bearing, do not "fix").
    pub h: Vec<Cplx>,
    /// Ordinary coherence per bin, `[0, 1]`.
    pub coherence: Vec<f64>,
    pub nfft: usize,
    pub fs_hz: f64,
    pub in_band_mean_coherence: f64,
}

impl SysIdResult {
    /// Sysid bin spacing, Hz.
    pub fn df(&self) -> f64 {
        self.fs_hz / self.nfft as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excitation_hits_the_requested_rms_exactly() {
        let cfg = SysIdConfig {
            drive_rms: 0.125,
            duration_s: 2.0,
            ..SysIdConfig::default()
        };
        for exc in [Excitation::Random, Excitation::LogChirp] {
            let x = SysIdConfig {
                excitation: exc,
                ..cfg.clone()
            }
            .build_excitation(8192.0);
            assert_eq!(x.len(), 16384);
            let r = stats::rms(&x);
            assert!((r - 0.125).abs() < 1e-12, "{exc:?} rms {r}");
        }
    }

    #[test]
    fn random_excitation_energy_is_in_band() {
        let cfg = SysIdConfig {
            f_lo_hz: 100.0,
            f_hi_hz: 500.0,
            duration_s: 4.0,
            ..SysIdConfig::default()
        };
        let fs = 8192.0;
        let x = cfg.build_excitation(fs);
        let (freqs, psd) = hertzforge_dsp::spectrum::psd_welch(&x, fs, 1024, 0.5);
        let band: f64 = freqs
            .iter()
            .zip(psd.iter())
            .filter(|(f, _)| **f >= 100.0 && **f <= 500.0)
            .map(|(_, p)| p)
            .sum();
        let out: f64 = freqs
            .iter()
            .zip(psd.iter())
            .filter(|(f, _)| **f > 700.0)
            .map(|(_, p)| p)
            .sum();
        assert!(
            band > 50.0 * out.max(1e-30),
            "band power {band} not dominant vs out-of-band {out}"
        );
    }

    #[test]
    fn estimate_gates_on_coherence() {
        // Response = pure noise, uncorrelated with drive -> coherence ~ 0 -> hold.
        let cfg = SysIdConfig {
            duration_s: 2.0,
            nfft: 512,
            ..SysIdConfig::default()
        };
        let fs = 8192.0;
        let drive = cfg.build_excitation(fs);
        let mut seed = 1u64;
        let noise: Vec<f64> = (0..drive.len())
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((seed >> 33) as f64 / (1u64 << 31) as f64) - 1.0
            })
            .collect();
        let res = cfg.estimate(&drive, &noise, fs);
        assert!(matches!(res, Err(TwrError::SysId(_))), "{res:?}");
    }

    #[test]
    fn estimate_recovers_a_unity_plant() {
        let cfg = SysIdConfig {
            duration_s: 2.0,
            nfft: 512,
            min_mean_coherence: 0.9,
            ..SysIdConfig::default()
        };
        let fs = 8192.0;
        let drive = cfg.build_excitation(fs);
        let response = drive.clone(); // H = 1
        let r = cfg.estimate(&drive, &response, fs).unwrap();
        assert!(r.in_band_mean_coherence > 0.99);
        let df = r.df();
        for (k, &(re, im)) in r.h.iter().enumerate() {
            let f = k as f64 * df;
            if f >= cfg.f_lo_hz && f <= cfg.f_hi_hz {
                assert!((re - 1.0).abs() < 0.05 && im.abs() < 0.05, "H at {f} Hz");
            }
        }
    }

    #[test]
    fn validate_rejects_short_duration_and_bad_band() {
        let fs = 8192.0;
        let c = SysIdConfig {
            duration_s: 0.1, // < 2*nfft samples
            ..SysIdConfig::default()
        };
        assert!(c.validate(fs).is_err());
        let c = SysIdConfig {
            f_lo_hz: 0.0,
            ..SysIdConfig::default()
        };
        assert!(c.validate(fs).is_err());
        let c = SysIdConfig {
            f_hi_hz: 5000.0, // >= fs/2
            ..SysIdConfig::default()
        };
        assert!(c.validate(fs).is_err());
        assert!(SysIdConfig::default().validate(fs).is_ok());
    }
}
