//! What a Mixed Mode run draws: the spectra on the line grid (sent as steps
//! with bin edges, as Random's are), and each tone's and band's history.

use serde::{Deserialize, Serialize};

/// The line grid the spectra are on. Line `k` is centered on `f0_hz + k *
/// df_hz` and spans half a line either side, so a spectrum is drawn as steps
/// with edges `f0_hz + (k - 0.5) * df_hz`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LineGrid {
    /// The first line's center, Hz.
    pub f0_hz: f64,
    /// Line spacing, Hz.
    pub df_hz: f64,
    /// Lines.
    pub lines: usize,
}

impl LineGrid {
    /// The `lines + 1` bin edges, Hz.
    pub fn edges_hz(&self) -> Vec<f64> {
        (0..=self.lines)
            .map(|k| self.f0_hz + (k as f64 - 0.5) * self.df_hz)
            .collect()
    }
}

/// One control channel's spectra, on its real table index.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MixedDisplayChannel {
    /// The channel-table index (the trace id).
    pub channel_index: u32,
    /// The response with the tones in, g^2/Hz.
    pub control: Vec<f32>,
    /// The response with each tracked tone subtracted, g^2/Hz.
    pub residual: Vec<f32>,
}

/// One tone's history, oldest first, over the display's history window.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToneHistory {
    /// The part's id.
    pub part: String,
    /// Station time of each point, seconds since engage.
    pub t_s: Vec<f64>,
    /// Its frequency, Hz.
    pub freq_hz: Vec<f32>,
    /// Its target at the level in force, g peak.
    pub target_g: Vec<f32>,
    /// Its tracked composite amplitude, g peak (NaN before the tracker
    /// settles).
    pub measured_g: Vec<f32>,
    /// Its drive amplitude, volts peak.
    pub drive_vpk: Vec<f32>,
}

/// One band's history, oldest first, a point per floor loop.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BandHistory {
    /// The part's id.
    pub part: String,
    /// Station time of each point, seconds since engage.
    pub t_s: Vec<f64>,
    /// Its center, Hz.
    pub center_hz: Vec<f32>,
    /// Its reference at the level in force (floor plus band), g^2/Hz.
    pub reference: Vec<f32>,
    /// Its moving-frame level, g^2/Hz.
    pub measured: Vec<f32>,
    /// Degrees of freedom of each reading.
    pub dof: Vec<f32>,
}

/// The identification, on the line grid.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MixedFrf {
    /// `|H|^2`, (g / V)^2, per line.
    pub h_mag2: Vec<f32>,
    /// Ordinary coherence per line, 0 to 1.
    pub coherence: Vec<f32>,
    /// The measured pipeline lag, frames.
    pub lag_frames: usize,
    /// The band coherence the gate judged.
    pub band_coherence: f64,
    /// Per line, whether the identification is trusted there (coherence and
    /// gain over their floors).
    #[serde(default)]
    pub trusted: Vec<bool>,
    /// The share of the floor's band's lines that are trusted.
    #[serde(default)]
    pub trusted_frac: f64,
}

/// Everything a Mixed Mode display packer needs, on one line grid.
///
/// Every spectrum is empty until it exists: a packer publishes nothing rather
/// than a flat zero, which would read as "dead" rather than "not yet".
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MixedDisplay {
    /// The grid every spectrum below is on.
    pub grid: LineGrid,
    /// The control composite with the tones in, g^2/Hz.
    pub control: Vec<f32>,
    /// The composite of the residual (each tracked tone subtracted), g^2/Hz:
    /// what the floor and bands are judged on. Once the floor has had a loop
    /// this is its averaged estimate (each line over its clean frames, the
    /// neighbours' error carried on lines with none), at the level in force.
    pub residual: Vec<f32>,
    /// Per control channel.
    pub channels: Vec<MixedDisplayChannel>,
    /// The reference at the level in force: the floor plus each band where it
    /// is now, g^2/Hz.
    pub reference: Vec<f32>,
    /// Alarm lines about the reference.
    pub alarm_lo: Vec<f32>,
    /// Alarm lines about the reference.
    pub alarm_hi: Vec<f32>,
    /// Abort lines about the reference.
    pub abort_lo: Vec<f32>,
    /// Abort lines about the reference.
    pub abort_hi: Vec<f32>,
    /// The random part of the drive (floor and bands), V^2/Hz.
    pub drive: Vec<f32>,
    /// Lines left out of this loop's floor average.
    pub excluded: Vec<bool>,
    /// Each tone's history, in parts-list order.
    pub tones: Vec<ToneHistory>,
    /// Each band's history, in parts-list order.
    pub bands: Vec<BandHistory>,
    /// The identification; `None` until it has run.
    pub identification: Option<MixedFrf>,
}
