//! The reference (target) waveform: import, preparation, kinematics preview.
//!
//! # Import formats (survey-scoped)
//!
//! - **CSV / TSV / pasted text** ([`ReferenceWaveform::from_delimited`]): one
//!   numeric column (sample rate must be supplied) or two columns
//!   `time, value` (sample rate derived from the time column and checked for
//!   uniformity). Header rows and `#` comment lines are skipped, RFC-4180-ish
//!   quoting is NOT needed for numeric data so it is not implemented.
//! - **WAV** ([`ReferenceWaveform::from_wav_bytes`]): RIFF/WAVE, PCM16 or IEEE
//!   float32, any channel count with an explicit channel pick. Anything else is
//!   rejected loudly (no silent conversion). PCM16 is normalized to [-1, 1);
//!   the EU scale is applied afterward with [`ReferenceWaveform::scale`].
//! - **JSON** round-trip (serde) for setups and archives.
//! - **UFF dataset 58 / RPC-III**: recognized by extension in the UI layer and
//!   rejected with a clear "not implemented in v1" message (documented stubs,
//!   same posture as eigen-channel-table's UFF export stub).
//!
//! # Preparation ops
//!
//! Field data carries DC offset and drift that would integrate to unbounded
//! shaker displacement, plus abrupt ends that would step the drive. The prep set
//! mirrors the commercial packages (Crystal EDM documents cut / rescale / taper /
//! decimate; every package detrends): [`crop`](ReferenceWaveform::crop),
//! [`scale`](ReferenceWaveform::scale), [`detrend`](ReferenceWaveform::detrend),
//! [`highpass`](ReferenceWaveform::highpass), [`taper`](ReferenceWaveform::taper),
//! [`resample`](ReferenceWaveform::resample). The service applies them in the
//! documented order: resample -> detrend -> highpass -> scale -> taper.

use eigen_dsp::integrate;
use serde::{Deserialize, Serialize};

/// Errors from importing or preparing a reference waveform.
#[derive(Debug, Clone, PartialEq)]
pub enum WaveformError {
    /// The input had no usable samples.
    Empty,
    /// A parse problem (line number + message).
    Parse { line: usize, message: String },
    /// The time column is not uniformly sampled (max deviation fraction).
    NonUniformTime { max_dev_frac: f64 },
    /// A WAV container problem.
    Wav(String),
    /// A prep-op argument was out of range.
    BadArg(String),
    /// A non-finite sample was found at `index`.
    NonFinite { index: usize },
}

impl std::fmt::Display for WaveformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WaveformError::Empty => write!(f, "no usable samples in the input"),
            WaveformError::Parse { line, message } => {
                write!(f, "parse error at line {line}: {message}")
            }
            WaveformError::NonUniformTime { max_dev_frac } => write!(
                f,
                "time column is not uniformly sampled (max deviation {:.2}% of dt)",
                max_dev_frac * 100.0
            ),
            WaveformError::Wav(s) => write!(f, "wav: {s}"),
            WaveformError::BadArg(s) => write!(f, "bad argument: {s}"),
            WaveformError::NonFinite { index } => {
                write!(f, "non-finite sample at index {index}")
            }
        }
    }
}

impl std::error::Error for WaveformError {}

/// Where a reference came from (report/honesty bookkeeping).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImportSource {
    Delimited,
    Wav,
    Json,
    Synthesized(String),
}

/// The reference (target) acceleration time history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReferenceWaveform {
    /// Operator-facing label (filename, description).
    pub label: String,
    /// Sample rate, Hz.
    pub fs_hz: f64,
    /// Samples in engineering units (`eu`).
    pub samples: Vec<f64>,
    /// Engineering unit label (display only; "g" by convention).
    pub eu: String,
    /// Provenance.
    pub source: ImportSource,
}

/// Max allowed deviation of any time-column step from the median step, as a
/// fraction of the median step, before the import is rejected as non-uniform.
const TIME_UNIFORMITY_TOL: f64 = 0.01;

impl ReferenceWaveform {
    /// Build directly from samples (tests, synthesized references).
    pub fn from_samples(label: &str, fs_hz: f64, samples: Vec<f64>) -> Result<Self, WaveformError> {
        let w = ReferenceWaveform {
            label: label.to_string(),
            fs_hz,
            samples,
            eu: "g".to_string(),
            source: ImportSource::Synthesized(label.to_string()),
        };
        w.validate()?;
        Ok(w)
    }

    /// Import delimited text (CSV, TSV, pasted). `fs_hz` is REQUIRED for
    /// single-column data and IGNORED (derived from the time column, which wins)
    /// for two-column `time, value` data. Header rows (any non-numeric first
    /// field) and lines starting with `#` are skipped.
    pub fn from_delimited(text: &str, fs_hz: Option<f64>) -> Result<Self, WaveformError> {
        let mut times: Vec<f64> = Vec::new();
        let mut values: Vec<f64> = Vec::new();
        let mut two_column = None::<bool>;

        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line
                .split(|c: char| c == ',' || c == '\t' || c == ';' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .collect();
            if fields.is_empty() {
                continue;
            }
            let parsed: Option<Vec<f64>> = fields.iter().map(|s| s.parse::<f64>().ok()).collect();
            let Some(nums) = parsed else {
                // Non-numeric row: a header if we have no data yet, an error after.
                if values.is_empty() {
                    continue;
                }
                return Err(WaveformError::Parse {
                    line: i + 1,
                    message: format!("non-numeric row after data began: {line:?}"),
                });
            };
            let cols = match two_column {
                Some(t) => t,
                None => {
                    let t = nums.len() >= 2;
                    two_column = Some(t);
                    t
                }
            };
            if cols {
                if nums.len() < 2 {
                    return Err(WaveformError::Parse {
                        line: i + 1,
                        message: "expected two columns (time, value)".to_string(),
                    });
                }
                times.push(nums[0]);
                values.push(nums[1]);
            } else {
                values.push(nums[0]);
            }
        }

        if values.is_empty() {
            return Err(WaveformError::Empty);
        }

        let fs = if two_column == Some(true) {
            if times.len() < 2 {
                return Err(WaveformError::Empty);
            }
            let mut steps: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
            steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = steps[steps.len() / 2];
            if median <= 0.0 {
                return Err(WaveformError::Parse {
                    line: 0,
                    message: "time column is not increasing".to_string(),
                });
            }
            let max_dev = steps
                .iter()
                .map(|s| (s - median).abs() / median)
                .fold(0.0f64, f64::max);
            if max_dev > TIME_UNIFORMITY_TOL {
                return Err(WaveformError::NonUniformTime {
                    max_dev_frac: max_dev,
                });
            }
            1.0 / median
        } else {
            fs_hz.ok_or_else(|| {
                WaveformError::BadArg(
                    "single-column data needs an explicit sample rate".to_string(),
                )
            })?
        };

        let w = ReferenceWaveform {
            label: "imported".to_string(),
            fs_hz: fs,
            samples: values,
            eu: "g".to_string(),
            source: ImportSource::Delimited,
        };
        w.validate()?;
        Ok(w)
    }

    /// Import a WAV file (RIFF/WAVE). PCM16 and IEEE float32 only; pick one
    /// channel (`channel`, 0-based). PCM16 is normalized to [-1, 1); apply the
    /// engineering-unit scale afterward with [`Self::scale`].
    pub fn from_wav_bytes(bytes: &[u8], channel: usize) -> Result<Self, WaveformError> {
        let err = |m: &str| WaveformError::Wav(m.to_string());
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(err("not a RIFF/WAVE file"));
        }
        let rd_u16 = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let rd_u32 =
            |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);

        let mut pos = 12usize;
        let mut fmt: Option<(u16, u16, u32, u16)> = None; // (format, channels, rate, bits)
        let mut data: Option<(usize, usize)> = None; // (offset, len)
        while pos + 8 <= bytes.len() {
            let id = &bytes[pos..pos + 4];
            let sz = rd_u32(pos + 4) as usize;
            let body = pos + 8;
            if body + sz > bytes.len() {
                return Err(err("truncated chunk"));
            }
            match id {
                b"fmt " => {
                    if sz < 16 {
                        return Err(err("fmt chunk too short"));
                    }
                    fmt = Some((
                        rd_u16(body),
                        rd_u16(body + 2),
                        rd_u32(body + 4),
                        rd_u16(body + 14),
                    ));
                }
                b"data" => data = Some((body, sz)),
                _ => {}
            }
            pos = body + sz + (sz & 1); // chunks are word-aligned
        }
        let (format, n_ch, rate, bits) = fmt.ok_or_else(|| err("missing fmt chunk"))?;
        let (doff, dlen) = data.ok_or_else(|| err("missing data chunk"))?;
        let n_ch = n_ch as usize;
        if n_ch == 0 {
            return Err(err("zero channels"));
        }
        if channel >= n_ch {
            return Err(WaveformError::Wav(format!(
                "channel {channel} out of range (file has {n_ch})"
            )));
        }

        let samples: Vec<f64> = match (format, bits) {
            (1, 16) => {
                let frame = 2 * n_ch;
                let n = dlen / frame;
                (0..n)
                    .map(|i| {
                        let o = doff + i * frame + 2 * channel;
                        i16::from_le_bytes([bytes[o], bytes[o + 1]]) as f64 / 32768.0
                    })
                    .collect()
            }
            (3, 32) => {
                let frame = 4 * n_ch;
                let n = dlen / frame;
                (0..n)
                    .map(|i| {
                        let o = doff + i * frame + 4 * channel;
                        f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
                            as f64
                    })
                    .collect()
            }
            _ => {
                return Err(WaveformError::Wav(format!(
                    "unsupported format (tag {format}, {bits}-bit); PCM16 and IEEE float32 only"
                )))
            }
        };

        let w = ReferenceWaveform {
            label: "wav import".to_string(),
            fs_hz: rate as f64,
            samples,
            eu: "g".to_string(),
            source: ImportSource::Wav,
        };
        w.validate()?;
        Ok(w)
    }

    /// JSON round-trip (setups, archives).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ReferenceWaveform serializes")
    }

    /// Parse the JSON produced by [`Self::to_json`].
    pub fn from_json(s: &str) -> Result<Self, WaveformError> {
        let w: ReferenceWaveform = serde_json::from_str(s).map_err(|e| WaveformError::Parse {
            line: 0,
            message: e.to_string(),
        })?;
        w.validate()?;
        Ok(w)
    }

    /// Structural validation: non-empty, positive rate, all samples finite.
    pub fn validate(&self) -> Result<(), WaveformError> {
        if self.samples.is_empty() {
            return Err(WaveformError::Empty);
        }
        if self.fs_hz.is_nan() || self.fs_hz <= 0.0 {
            return Err(WaveformError::BadArg(format!(
                "sample rate must be positive (got {})",
                self.fs_hz
            )));
        }
        if let Some(i) = self.samples.iter().position(|v| !v.is_finite()) {
            return Err(WaveformError::NonFinite { index: i });
        }
        Ok(())
    }

    /// Duration in seconds.
    pub fn duration_s(&self) -> f64 {
        self.samples.len() as f64 / self.fs_hz
    }

    // ---- prep ops (each returns a new waveform; the source is not mutated) ----

    /// Keep samples `[start, end)`.
    pub fn crop(&self, start: usize, end: usize) -> Result<Self, WaveformError> {
        if start >= end || end > self.samples.len() {
            return Err(WaveformError::BadArg(format!(
                "crop [{start}, {end}) out of range (len {})",
                self.samples.len()
            )));
        }
        let mut w = self.clone();
        w.samples = self.samples[start..end].to_vec();
        Ok(w)
    }

    /// Multiply every sample by `factor` (EU rescale).
    pub fn scale(&self, factor: f64) -> Result<Self, WaveformError> {
        if !factor.is_finite() {
            return Err(WaveformError::BadArg("scale factor not finite".into()));
        }
        let mut w = self.clone();
        for v in w.samples.iter_mut() {
            *v *= factor;
        }
        Ok(w)
    }

    /// Remove the mean (`linear == false`) or the least-squares linear trend
    /// (`linear == true`).
    pub fn detrend(&self, linear: bool) -> Self {
        let n = self.samples.len();
        let mut w = self.clone();
        if n == 0 {
            return w;
        }
        if !linear || n < 2 {
            let mean = self.samples.iter().sum::<f64>() / n as f64;
            for v in w.samples.iter_mut() {
                *v -= mean;
            }
            return w;
        }
        // Least-squares line y = a + b*x over x = 0..n-1.
        let nf = n as f64;
        let sx = (nf - 1.0) * nf / 2.0;
        let sxx = (nf - 1.0) * nf * (2.0 * nf - 1.0) / 6.0;
        let sy: f64 = self.samples.iter().sum();
        let sxy: f64 = self
            .samples
            .iter()
            .enumerate()
            .map(|(i, &y)| i as f64 * y)
            .sum();
        let den = nf * sxx - sx * sx;
        let b = if den.abs() > 0.0 {
            (nf * sxy - sx * sy) / den
        } else {
            0.0
        };
        let a = (sy - b * sx) / nf;
        for (i, v) in w.samples.iter_mut().enumerate() {
            *v -= a + b * i as f64;
        }
        w
    }

    /// First-order Butterworth high-pass at `fc_hz` (bilinear transform - the
    /// same design `eigen_dsp::integrate` uses for drift banding). Causal
    /// forward pass; phase lead near the corner is the documented trade for
    /// staying causal. Choose `fc_hz` at or below the control band's low edge.
    pub fn highpass(&self, fc_hz: f64) -> Result<Self, WaveformError> {
        if fc_hz.is_nan() || fc_hz <= 0.0 || fc_hz >= self.fs_hz / 2.0 {
            return Err(WaveformError::BadArg(format!(
                "highpass corner {fc_hz} Hz outside (0, fs/2)"
            )));
        }
        let mut w = self.clone();
        let k = (std::f64::consts::PI * fc_hz / self.fs_hz).tan();
        let a = 1.0 / (1.0 + k);
        let b = (1.0 - k) / (1.0 + k);
        let mut x_prev = w.samples[0];
        let mut y_prev = 0.0f64;
        w.samples[0] = 0.0;
        for slot in w.samples.iter_mut().skip(1) {
            let x = *slot;
            let y = a * (x - x_prev) + b * y_prev;
            *slot = y;
            x_prev = x;
            y_prev = y;
        }
        Ok(w)
    }

    /// Cosine (Tukey-style) taper: half-cosine ramps over `frac` of the record
    /// length at EACH end (`frac` in (0, 0.5]). Ends the record at rest so the
    /// playout does not step the drive.
    pub fn taper(&self, frac: f64) -> Result<Self, WaveformError> {
        if frac.is_nan() || frac <= 0.0 || frac > 0.5 {
            return Err(WaveformError::BadArg(format!(
                "taper fraction {frac} outside (0, 0.5]"
            )));
        }
        let n = self.samples.len();
        let ramp = ((n as f64 * frac).round() as usize).max(1).min(n / 2);
        let mut w = self.clone();
        for i in 0..ramp {
            let g = 0.5 - 0.5 * (std::f64::consts::PI * i as f64 / ramp as f64).cos();
            w.samples[i] *= g;
            w.samples[n - 1 - i] *= g;
        }
        Ok(w)
    }

    /// Linear-interpolation resample to `new_fs_hz`. UPWARD ONLY.
    ///
    /// # Downward resampling REFUSES
    ///
    /// This is linear interpolation with NO anti-alias filter. Decimating
    /// 51 200 -> 8533.33 folds everything above 4266 Hz down into the band, and
    /// the fold is invisible afterwards: the convergence metric is computed
    /// against the RESAMPLED target, so the loop faithfully reproduces the
    /// alias, every number agrees, the report says "converged", and the UUT saw
    /// a test that was never in the field data. A documented import-path
    /// limitation is one thing; promoting it into a mandatory run-path step is
    /// another, so the run path refuses and names both rates.
    ///
    /// Upward interpolation is allowed: it invents no energy the record did not
    /// have, it only misrepresents the band above the ORIGINAL Nyquist as
    /// empty, which it genuinely is.
    pub fn resample(&self, new_fs_hz: f64) -> Result<Self, WaveformError> {
        if new_fs_hz.is_nan() || new_fs_hz <= 0.0 {
            return Err(WaveformError::BadArg(format!(
                "resample rate must be positive (got {new_fs_hz})"
            )));
        }
        if (new_fs_hz - self.fs_hz).abs() < 1e-9 {
            return Ok(self.clone());
        }
        if new_fs_hz < self.fs_hz {
            return Err(WaveformError::BadArg(format!(
                "refusing to resample the reference DOWN from {} Hz to {new_fs_hz} Hz: this \
                 resampler is linear interpolation with no anti-alias filter, so content \
                 above {:.1} Hz would fold into the band and the run would converge on an \
                 aliased target that reads as a clean pass. Decimate the record upstream \
                 with a proper filter, or run the station at or above the record's rate",
                self.fs_hz,
                new_fs_hz / 2.0
            )));
        }
        let n_in = self.samples.len();
        let n_out = ((n_in as f64) * new_fs_hz / self.fs_hz).round() as usize;
        if n_out < 2 {
            return Err(WaveformError::BadArg(
                "resample would produce fewer than 2 samples".into(),
            ));
        }
        let ratio = self.fs_hz / new_fs_hz;
        let mut out = Vec::with_capacity(n_out);
        for i in 0..n_out {
            let x = i as f64 * ratio;
            let i0 = x.floor() as usize;
            if i0 + 1 >= n_in {
                out.push(self.samples[n_in - 1]);
            } else {
                let fr = x - i0 as f64;
                out.push(self.samples[i0] * (1.0 - fr) + self.samples[i0 + 1] * fr);
            }
        }
        let mut w = self.clone();
        w.fs_hz = new_fs_hz;
        w.samples = out;
        Ok(w)
    }

    /// Kinematics preview at FULL level: peak acceleration (EU, nominally g),
    /// peak velocity (in/s), and peak-to-peak displacement (in), using the
    /// banded streaming integrators (`hp_hz` bands the drift; choose it at or
    /// below the control band low edge). Assumes the EU is g; the UI labels it.
    pub fn kinematics(&self, hp_hz: f64) -> WaveformKinematics {
        let peak_accel = self.samples.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        let vel = integrate::accel_to_velocity(&self.samples, self.fs_hz, hp_hz);
        let peak_velocity = vel.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        let disp = integrate::velocity_to_displacement(&vel, self.fs_hz, hp_hz);
        let (dmin, dmax) = disp
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
                (lo.min(v), hi.max(v))
            });
        WaveformKinematics {
            peak_accel,
            peak_velocity_ips: peak_velocity,
            disp_pkpk_in: if dmax >= dmin { dmax - dmin } else { 0.0 },
            rms: eigen_dsp::stats::rms(&self.samples),
        }
    }
}

/// The kinematics preview ([`ReferenceWaveform::kinematics`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WaveformKinematics {
    /// Peak |acceleration| in the waveform EU (nominally g).
    pub peak_accel: f64,
    /// Peak |velocity|, in/s (banded integral).
    pub peak_velocity_ips: f64,
    /// Peak-to-peak displacement, inches (banded double integral).
    pub disp_pkpk_in: f64,
    /// Waveform RMS (EU).
    pub rms: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimited_single_column_needs_fs() {
        let text = "1.0\n2.0\n3.0\n";
        assert!(ReferenceWaveform::from_delimited(text, None).is_err());
        let w = ReferenceWaveform::from_delimited(text, Some(100.0)).unwrap();
        assert_eq!(w.samples, vec![1.0, 2.0, 3.0]);
        assert_eq!(w.fs_hz, 100.0);
    }

    #[test]
    fn delimited_two_column_derives_fs_and_checks_uniformity() {
        let text = "# comment\ntime,accel\n0.000,0.1\n0.001,0.2\n0.002,0.3\n0.003,0.4\n";
        let w = ReferenceWaveform::from_delimited(text, None).unwrap();
        assert_eq!(w.samples.len(), 4);
        assert!((w.fs_hz - 1000.0).abs() < 1e-6);

        let bad = "0.000,0.1\n0.001,0.2\n0.005,0.3\n";
        assert!(matches!(
            ReferenceWaveform::from_delimited(bad, None),
            Err(WaveformError::NonUniformTime { .. })
        ));
    }

    #[test]
    fn delimited_tsv_and_whitespace_work() {
        let w = ReferenceWaveform::from_delimited("0\t1.5\n0.5\t2.5\n1.0\t3.5\n", None).unwrap();
        assert_eq!(w.samples, vec![1.5, 2.5, 3.5]);
        assert!((w.fs_hz - 2.0).abs() < 1e-9);
    }

    fn tiny_wav_pcm16(rate: u32, ch: u16, frames: &[(i16, i16)]) -> Vec<u8> {
        let data_len = frames.len() * 2 * ch as usize;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&ch.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * ch as u32 * 2).to_le_bytes());
        b.extend_from_slice(&(ch * 2).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data_len as u32).to_le_bytes());
        for &(l, r) in frames {
            b.extend_from_slice(&l.to_le_bytes());
            if ch == 2 {
                b.extend_from_slice(&r.to_le_bytes());
            }
        }
        b
    }

    #[test]
    fn wav_pcm16_stereo_channel_pick() {
        let bytes = tiny_wav_pcm16(8192, 2, &[(16384, -16384), (0, 32767)]);
        let l = ReferenceWaveform::from_wav_bytes(&bytes, 0).unwrap();
        let r = ReferenceWaveform::from_wav_bytes(&bytes, 1).unwrap();
        assert_eq!(l.fs_hz, 8192.0);
        assert!((l.samples[0] - 0.5).abs() < 1e-4);
        assert!((r.samples[0] + 0.5).abs() < 1e-4);
        assert!(ReferenceWaveform::from_wav_bytes(&bytes, 2).is_err());
    }

    #[test]
    fn wav_rejects_unsupported_formats() {
        let mut bytes = tiny_wav_pcm16(8192, 1, &[(1, 0)]);
        // Corrupt the format tag to 7 (mu-law).
        bytes[20] = 7;
        assert!(matches!(
            ReferenceWaveform::from_wav_bytes(&bytes, 0),
            Err(WaveformError::Wav(_))
        ));
    }

    #[test]
    fn detrend_removes_mean_and_line() {
        let w = ReferenceWaveform::from_samples(
            "t",
            100.0,
            (0..100).map(|i| 3.0 + 0.25 * i as f64).collect(),
        )
        .unwrap();
        let m = w.detrend(false);
        assert!((m.samples.iter().sum::<f64>()).abs() < 1e-9);
        let l = w.detrend(true);
        let resid = l.samples.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        assert!(resid < 1e-9, "linear detrend residual {resid}");
    }

    #[test]
    fn taper_zeroes_the_ends_and_keeps_the_middle() {
        let w = ReferenceWaveform::from_samples("t", 100.0, vec![1.0; 100]).unwrap();
        let t = w.taper(0.1).unwrap();
        assert_eq!(t.samples[0], 0.0);
        assert_eq!(t.samples[99], 0.0);
        assert_eq!(t.samples[50], 1.0);
    }

    #[test]
    fn resample_up_keeps_shape_and_down_refuses_by_name() {
        let w = ReferenceWaveform::from_samples(
            "t",
            1000.0,
            (0..1000)
                .map(|i| (std::f64::consts::TAU * 10.0 * i as f64 / 1000.0).sin())
                .collect(),
        )
        .unwrap();
        // UP is allowed and keeps the waveform.
        let r = w.resample(2000.0).unwrap();
        assert_eq!(r.fs_hz, 2000.0);
        assert!((r.samples.len() as i64 - 2000).abs() <= 1);
        let pk = r.samples.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        assert!(pk > 0.99 && pk <= 1.0 + 1e-9);

        // DOWN refuses, naming both rates and the reason. This
        // used to be the test that PROVED halving worked - the operation was
        // right for an import preview and wrong as a run-path step.
        let e = w.resample(500.0).unwrap_err().to_string();
        assert!(e.contains("1000"), "names the record rate: {e}");
        assert!(e.contains("500"), "names the requested rate: {e}");
        assert!(e.contains("anti-alias"), "names the reason: {e}");
        // ...including the 6x decimation the chassis would ask for.
        let field = ReferenceWaveform::from_samples("field", 51_200.0, vec![0.1; 4096]).unwrap();
        assert!(field.resample(8533.33).is_err());
        // Same rate is a no-op, not a refusal.
        assert_eq!(w.resample(1000.0).unwrap().fs_hz, 1000.0);
    }

    #[test]
    fn highpass_kills_dc_keeps_band() {
        let n = 4000;
        let fs = 1000.0;
        let w = ReferenceWaveform::from_samples(
            "t",
            fs,
            (0..n)
                .map(|i| 5.0 + (std::f64::consts::TAU * 50.0 * i as f64 / fs).sin())
                .collect(),
        )
        .unwrap();
        let h = w.highpass(2.0).unwrap();
        // Settled tail: DC gone, the 50 Hz tone survives near unit amplitude.
        let tail = &h.samples[n / 2..];
        let mean = tail.iter().sum::<f64>() / tail.len() as f64;
        assert!(mean.abs() < 0.05, "residual DC {mean}");
        let pk = tail.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        assert!(pk > 0.9 && pk < 1.1, "band tone peak {pk}");
    }

    #[test]
    fn kinematics_of_a_sine_matches_analytic() {
        // 1 g at 100 Hz: v_peak = G_TO_IN_S2/(2*pi*100) ~ 0.6145 ips;
        // d_pkpk = 2 * v_peak/(2*pi*100) ~ 1.956 mil. The sine is end-TAPERED:
        // a suddenly-starting sine carries a real (1-cos)/w startup transient
        // that peaks at 2x the steady-state velocity - which is exactly why the
        // taper prep op exists for playable references.
        let fs = 8192.0;
        let n = 32768;
        let w = ReferenceWaveform::from_samples(
            "sine",
            fs,
            (0..n)
                .map(|i| (std::f64::consts::TAU * 100.0 * i as f64 / fs).sin())
                .collect(),
        )
        .unwrap()
        .taper(0.25)
        .unwrap();
        let k = w.kinematics(2.0);
        assert!((k.peak_accel - 1.0).abs() < 1e-6);
        let v_expect = integrate::G_TO_IN_S2 / (std::f64::consts::TAU * 100.0);
        assert!(
            (k.peak_velocity_ips - v_expect).abs() / v_expect < 0.05,
            "v {} vs {}",
            k.peak_velocity_ips,
            v_expect
        );
        let d_expect = 2.0 * v_expect / (std::f64::consts::TAU * 100.0);
        assert!(
            (k.disp_pkpk_in - d_expect).abs() / d_expect < 0.10,
            "d {} vs {}",
            k.disp_pkpk_in,
            d_expect
        );
    }

    #[test]
    fn json_round_trip() {
        let w = ReferenceWaveform::from_samples("rt", 512.0, vec![0.5, -0.25, 0.125]).unwrap();
        let j = w.to_json();
        let back = ReferenceWaveform::from_json(&j).unwrap();
        assert_eq!(w, back);
    }

    #[test]
    fn crop_bounds_checked() {
        let w = ReferenceWaveform::from_samples("c", 100.0, vec![1.0, 2.0, 3.0, 4.0]).unwrap();
        assert_eq!(w.crop(1, 3).unwrap().samples, vec![2.0, 3.0]);
        assert!(w.crop(3, 3).is_err());
        assert!(w.crop(0, 5).is_err());
    }

    #[test]
    fn non_finite_rejected() {
        assert!(matches!(
            ReferenceWaveform::from_samples("nf", 100.0, vec![1.0, f64::NAN]),
            Err(WaveformError::NonFinite { index: 1 })
        ));
    }
}
