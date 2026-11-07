//! The closed loop of a classical run (`ClassicalTestConfig::control` set): the
//! pulse grid, the identified plant on it, the guarded law call and the
//! checks on what a law returns.
//!
//! The service ([`super::ClassicalShockService`]) owns the run; this module
//! holds the arithmetic and the bookkeeping it runs the loop with, so the open
//! loop's pipeline in the service is left exactly as it was.
//!
//! # The grid
//!
//! Every closed-loop record is one grid: a quiet lead-in (the open loop's
//! quiet surround), the compensated pulse, and a quiet lead-out padded to a
//! power of two so a record and its spectrum are one transform apart. The
//! station plays the grid as given and measures the same number of samples.
//! The core tapers the outer half of each quiet margin to zero, so a drive
//! starts and ends at 0 V whatever the law returns: an output holds its last
//! value when a task parks.
//!
//! # Conventions
//!
//! Spectra handed to a law are in the `shock-classical` world's
//! negative-exponent convention. The identification's FRF is already in it
//! (`hertzforge_dsp::frf` returns the standard H); time waveform
//! replication's inverse is built for the positive-exponent transform, so the
//! core conjugates it.

use crate::law::{
    ClassicalControl, ClassicalLaw, ClassicalLawFactory, ClassicalLawSetup, ClassicalPlant,
    Polarity, PulseInputs, PulseOutputs,
};
use crate::service::firer::quiet_lead_frames;
use crate::service::report::IdentificationSummary;
use crate::spec::ClassicalPulseSpec;
use hertzforge_dsp::fft::Cplx;
use hertzforge_shock_common::law_guard::{
    LawCall, LawCallRecord, LawClock, LawDescriptor, LawFault, LawRecordRing, LawValue,
    MissCounter, OutputRule, MAX_LAW_VALUES,
};
use hertzforge_shock_law_math::spectrum::{abs, irfft_half, mul, rfft_half};
use hertzforge_twr::grid::RecordGrid;
use hertzforge_twr::inverse::GridInverse;
use hertzforge_twr::law::{apply_coherence_gate, build_mask, mask_energy_coverage, LawConfig};
use hertzforge_twr::sysid::SysIdResult;
use serde::{Deserialize, Serialize};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// A bin of the drive spectrum whose last magnitude is under this fraction of
/// the largest bin's is treated as that small for the per-bin rise cap: a bin
/// at numerical zero would otherwise refuse any change at all.
const BIN_RISE_FLOOR_FRACTION: f64 = 1e-3;

/// The identification drive's ends fade in and out over this long, seconds,
/// so it starts and ends at 0 V.
const SYSID_FADE_S: f64 = 0.020;

/// The pulse grid of a closed-loop run, at the adopted rate.
#[derive(Debug, Clone)]
pub(crate) struct PulseGrid {
    pub fs_hz: f64,
    /// Samples in every record (a power of two).
    pub n: usize,
    /// Quiet samples before the compensated pulse.
    pub lead: usize,
    /// The compensated pulse's length.
    pub composite_len: usize,
    /// The main pulse's first sample on the grid.
    pub main_start: usize,
    /// The main pulse's length.
    pub main_len: usize,
    /// The full-level reference on the grid, polarity applied, EU.
    pub reference: Vec<f64>,
    /// `1` for a positive pulse, `-1` for a negative one.
    pub sign: f64,
}

impl PulseGrid {
    pub fn new(spec: &ClassicalPulseSpec, fs_hz: f64, polarity: Polarity) -> Self {
        let comp = spec.synthesize(fs_hz);
        let lead = quiet_lead_frames(spec.duration_ms / 1000.0, fs_hz);
        let grid = RecordGrid::new(fs_hz, comp.waveform_g.len(), lead, lead);
        let sign = match polarity {
            Polarity::Positive => 1.0,
            Polarity::Negative => -1.0,
        };
        let mut reference = grid.window_from_section(&comp.waveform_g);
        if sign < 0.0 {
            for v in reference.iter_mut() {
                *v = -*v;
            }
        }
        PulseGrid {
            fs_hz,
            n: grid.n,
            lead,
            composite_len: comp.waveform_g.len(),
            main_start: lead + comp.pre_len,
            main_len: comp.main_len,
            reference,
            sign,
        }
    }

    fn record_grid(&self) -> RecordGrid {
        RecordGrid {
            fs_hz: self.fs_hz,
            n: self.n,
            pad_pre: self.lead,
            section_len: self.composite_len,
        }
    }

    /// Bin `k`'s frequency, Hz.
    pub fn freq(&self, k: usize) -> f64 {
        k as f64 * self.fs_hz / self.n as f64
    }

    /// The core's at-rest taper: the outer half of each quiet margin falls to
    /// zero on a raised cosine, so the record's first and last samples are 0.
    pub fn taper(&self, d: &mut [f64]) {
        let n = d.len().min(self.n);
        let head = self.lead / 2;
        let tail = n.saturating_sub(self.lead + self.composite_len) / 2;
        for (i, v) in d[..head.min(n)].iter_mut().enumerate() {
            *v *= rise(i, head);
        }
        for (j, v) in d[..n].iter_mut().rev().take(tail).enumerate() {
            *v *= rise(j, tail);
        }
    }

    /// The judged window on the grid, `[start, end)`: the standard's quiet
    /// windows either side of the main pulse.
    pub fn judged_window(&self, pre_frac: f64, post_frac: f64) -> (usize, usize) {
        let pre = ((self.main_len as f64) * pre_frac).round() as usize;
        let post = ((self.main_len as f64) * post_frac).round() as usize;
        (
            self.main_start.saturating_sub(pre),
            (self.main_start + self.main_len + post).min(self.n),
        )
    }
}

/// `0` at `i = 0`, rising to just under `1` at `i = len - 1`.
fn rise(i: usize, len: usize) -> f64 {
    0.5 - 0.5 * (std::f64::consts::PI * i as f64 / len as f64).cos()
}

/// Fade the identification drive in and out so it starts and ends at 0 V.
pub(crate) fn fade_ends(x: &mut [f64], fs_hz: f64) {
    let n = x.len();
    let len = ((SYSID_FADE_S * fs_hz).round() as usize).min(n / 4);
    if len == 0 {
        return;
    }
    for i in 0..len {
        let w = rise(i, len);
        x[i] *= w;
        x[n - 1 - i] *= w;
    }
}

/// The largest absolute value of `x`.
pub(crate) fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}

/// The root mean square of `x`.
pub(crate) fn rms(x: &[f64]) -> f64 {
    if x.is_empty() {
        0.0
    } else {
        (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
    }
}

/// The identified plant on the pulse grid, world convention.
#[derive(Debug, Clone)]
pub(crate) struct GridPlant {
    /// `H[k]`, EU per volt.
    pub h: Vec<Cplx>,
    /// The regularized inverse, `conj(H) / (|H|^2 + eps)`.
    pub inverse: Vec<Cplx>,
    pub coherence: Vec<f64>,
    pub trusted: Vec<bool>,
    pub eps: f64,
    /// The fraction of the reference's energy the trusted bins cover.
    pub coverage: f64,
}

impl GridPlant {
    /// The plant for `sysid` (its FRF already scaled by any identification
    /// error the run asks for) on `grid`, under `control`'s trust settings.
    pub fn new(
        sysid: &SysIdResult,
        control: &ClassicalControl,
        grid: &PulseGrid,
    ) -> Result<Self, String> {
        let rg = grid.record_grid();
        let inv = GridInverse::from_sysid(
            sysid,
            &control.inverse,
            &rg,
            control.band_lo_hz,
            control.band_hi_hz,
        )
        .map_err(|e| e.to_string())?;
        // Time waveform replication's mask, on its own transform's reference
        // spectrum: the mask reads magnitudes, the same in either convention.
        let r_twr = hertzforge_twr::grid::rfft_half(&grid.reference, grid.n);
        let law = LawConfig {
            ref_floor_db: control.ref_floor_db,
            f_lo_hz: control.band_lo_hz,
            f_hi_hz: control.band_hi_hz,
            ..LawConfig::default()
        };
        let mut trusted = build_mask(&r_twr, &rg, &inv, &law);
        apply_coherence_gate(&mut trusted, &inv, control.inverse.coherence_floor);
        let coverage = mask_energy_coverage(&r_twr, &trusted);
        // H on the grid's bins, interpolated on (re, im) as the inverse is.
        let df = sysid.df();
        let last = sysid.h.len() - 1;
        let h = (0..rg.n_bins())
            .map(|k| {
                let pos = rg.freq(k) / df;
                let i0 = (pos.floor() as usize).min(last);
                let i1 = (i0 + 1).min(last);
                let fr = (pos - i0 as f64).clamp(0.0, 1.0);
                let (a, b) = (sysid.h[i0], sysid.h[i1]);
                (a.0 * (1.0 - fr) + b.0 * fr, a.1 * (1.0 - fr) + b.1 * fr)
            })
            .collect();
        Ok(GridPlant {
            h,
            inverse: inv.g.iter().map(|&(re, im)| (re, -im)).collect(),
            coherence: inv.coherence,
            trusted,
            eps: inv.eps,
            coverage,
        })
    }

    /// The response the plant predicts to `drive` (one grid record), EU.
    pub fn predict(&self, drive: &[f64]) -> Result<Vec<f64>, String> {
        let d = rfft_half(drive)?;
        let y: Vec<Cplx> = d.iter().zip(&self.h).map(|(&a, &b)| mul(a, b)).collect();
        irfft_half(&y, drive.len())
    }
}

/// One law call's record, as `law_pulses` stores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassicalCallPayload {
    /// The response the law saw, referred to full level (empty at the seed).
    pub response: Vec<f32>,
    /// The drive the law saw as played, referred to full level (empty at the
    /// seed).
    pub drive_in: Vec<f32>,
    /// The drive the law returned (empty when the call gave none).
    pub drive_out: Vec<f32>,
    /// The values the law reported.
    pub values: Vec<LawValue>,
    /// The core's judgement of the last pulse, when there was one.
    pub verdict: Option<crate::law::Verdict>,
    /// The core scaled the last pulse down.
    pub limited: bool,
    /// Drive bins over the per-pulse cap on this output.
    pub capped_bins: u64,
    /// The drive's peak was over the per-pulse cap on this output.
    pub peak_capped: bool,
}

/// One classical law call's record.
pub type ClassicalCallRecord = LawCallRecord<ClassicalCallPayload>;

/// The law's counts over a run.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct LawStats {
    pub calls: u64,
    pub misses: u64,
    pub refused: u64,
    pub capped_bins: u64,
    pub capped_peaks: u64,
    pub worst_call_s: f64,
}

/// What the core found on one law output.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct OutputCheck {
    pub capped_bins: u64,
    pub peak_capped: bool,
}

/// The law, its drive in force and its bookkeeping for one run.
pub(crate) struct LawRun {
    pub law: Box<dyn ClassicalLaw + Send>,
    pub descriptor: LawDescriptor,
    /// The full-level drive in force: the last usable output, tapered.
    pub drive: Vec<f64>,
    /// `|D[k]|` of the drive in force.
    pub drive_mags: Vec<f64>,
    /// The plant's predicted full-level response to the drive in force.
    pub predicted: Vec<f64>,
    pub misses: MissCounter,
    pub stats: LawStats,
    pub records: LawRecordRing<ClassicalCallRecord>,
    pub last_values: Vec<LawValue>,
    /// The call behind the drive the next ladder pulse plays.
    pub pending: Option<crate::service::report::PulseLawCall>,
    pub budget_s: f64,
    pub bin_cap_db: f64,
    pub peak_cap_db: f64,
}

/// The closed loop's state for one run.
pub(crate) struct ClosedLoop {
    pub grid: PulseGrid,
    pub plant: Option<GridPlant>,
    /// The identification as measured (its FRF unscaled), for the review view.
    pub sysid: Option<SysIdResult>,
    pub summary: Option<IdentificationSummary>,
    pub run: Option<LawRun>,
    /// The divergence tracker: the level it counts at, the last error there,
    /// the rises in a row.
    pub div_level_db: f64,
    pub div_last_err: Option<f64>,
    pub div_rises: u32,
    /// `law.json` has been written into the run folder.
    pub descriptor_written: bool,
}

impl ClosedLoop {
    pub fn new(grid: PulseGrid) -> Self {
        ClosedLoop {
            grid,
            plant: None,
            sysid: None,
            summary: None,
            run: None,
            div_level_db: f64::NAN,
            div_last_err: None,
            div_rises: 0,
            descriptor_written: false,
        }
    }

    /// The setup a law is built for, borrowing the grid and the plant.
    pub fn setup<'a>(
        &'a self,
        plant: &'a GridPlant,
        drive_ceiling_v: f64,
        call_budget_s: f64,
        band: (f64, f64),
    ) -> ClassicalLawSetup<'a> {
        ClassicalLawSetup {
            fs_hz: self.grid.fs_hz,
            record_len: self.grid.n,
            band_lo_hz: band.0,
            band_hi_hz: band.1,
            reference: &self.grid.reference,
            main_start: self.grid.main_start,
            main_len: self.grid.main_len,
            plant: ClassicalPlant {
                frf: &plant.h,
                coherence: &plant.coherence,
                inverse: &plant.inverse,
                trusted: &plant.trusted,
                epsilon: plant.eps,
            },
            drive_ceiling_v,
            call_budget_s,
        }
    }

    /// Count a pulse's error toward divergence: true when `pulses` errors in a
    /// row at one level have each risen by more than `rise_db` over the last.
    pub fn diverging(&mut self, level_db: f64, error_pct: f64, rise_db: f64, pulses: u32) -> u32 {
        if self.div_level_db.is_nan() || (level_db - self.div_level_db).abs() > 1e-9 {
            self.div_level_db = level_db;
            self.div_last_err = None;
            self.div_rises = 0;
        }
        if let Some(last) = self.div_last_err {
            if last > 0.0 && error_pct > 0.0 && 20.0 * (error_pct / last).log10() > rise_db {
                self.div_rises += 1;
            } else {
                self.div_rises = 0;
            }
        }
        self.div_last_err = Some(error_pct);
        if self.div_rises >= pulses {
            self.div_rises
        } else {
            0
        }
    }
}

/// Build the law with `factory`, contained: a panic is a crash, not the
/// service's.
pub(crate) fn build_law(
    factory: &dyn ClassicalLawFactory,
    setup: &ClassicalLawSetup<'_>,
) -> Result<Box<dyn ClassicalLaw + Send>, LawFault> {
    match catch_unwind(AssertUnwindSafe(|| factory.build(setup))) {
        Ok(r) => r,
        Err(p) => Err(LawFault::Crashed(panic_text(p))),
    }
}

/// Call the law once, timed on `clock` and contained. A call that returns an
/// output after its budget is over budget; a call that fails keeps its own
/// reason.
pub(crate) fn guarded_call(
    law: &mut (dyn ClassicalLaw + Send),
    which: LawCall,
    inputs: &PulseInputs<'_>,
    clock: &dyn LawClock,
    budget_s: f64,
) -> (Result<PulseOutputs, LawFault>, f64) {
    let t0 = clock.now_s();
    let r = catch_unwind(AssertUnwindSafe(|| match which {
        LawCall::Seed => law.seed(inputs),
        LawCall::Iterate => law.iterate(inputs),
    }));
    let took_s = (clock.now_s() - t0).max(0.0);
    let r = match r {
        Ok(Ok(_)) if took_s > budget_s => Err(LawFault::OverBudget { took_s, budget_s }),
        Ok(r) => r,
        Err(p) => Err(LawFault::Crashed(panic_text(p))),
    };
    (r, took_s)
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic with no message".into()
    }
}

/// Check a law's output: its length, that every number is finite, how many
/// values it reports, and (against the drive in force) the per-pulse rise caps
/// on its peak and on every bin. `candidate` is the output already tapered.
pub(crate) fn check_output(
    out: &PulseOutputs,
    candidate: &[f64],
    n: usize,
    last: Option<(&[f64], &[f64])>,
    peak_cap_db: f64,
    bin_cap_db: f64,
) -> Result<(OutputCheck, Vec<f64>), (LawFault, OutputCheck)> {
    let none = OutputCheck::default();
    if out.drive.len() != n {
        return Err((LawFault::BadOutput(OutputRule::WrongLength), none));
    }
    if out.drive.iter().any(|v| !v.is_finite()) || out.values.iter().any(|v| !v.value.is_finite()) {
        return Err((LawFault::BadOutput(OutputRule::NonFinite), none));
    }
    if out.values.len() > MAX_LAW_VALUES {
        return Err((LawFault::BadOutput(OutputRule::TooManyValues), none));
    }
    let spec =
        rfft_half(candidate).map_err(|_| (LawFault::BadOutput(OutputRule::WrongLength), none))?;
    let mags: Vec<f64> = spec.iter().map(|&c| abs(c)).collect();
    let Some((last_drive, last_mags)) = last else {
        return Ok((none, mags));
    };
    let mut check = OutputCheck::default();
    let peak_grow = 10f64.powf(peak_cap_db / 20.0);
    let last_peak = peak(last_drive);
    if peak(candidate) > last_peak.max(f64::MIN_POSITIVE) * peak_grow {
        check.peak_capped = true;
    }
    let bin_grow = 10f64.powf(bin_cap_db / 20.0);
    let floor = BIN_RISE_FLOOR_FRACTION * last_mags.iter().fold(0.0f64, |m, &v| m.max(v));
    for (new, old) in mags.iter().zip(last_mags) {
        if *new > old.max(floor) * bin_grow {
            check.capped_bins += 1;
        }
    }
    if check.peak_capped {
        return Err((LawFault::BadOutput(OutputRule::PeakRise), check));
    }
    if check.capped_bins > 0 {
        return Err((LawFault::BadOutput(OutputRule::BinRise), check));
    }
    Ok((check, mags))
}

/// What identification found, as the status shows it.
pub(crate) fn summarize(
    drive: &[f64],
    sysid: &SysIdResult,
    plant: &GridPlant,
    control: &ClassicalControl,
) -> IdentificationSummary {
    IdentificationSummary {
        drive_vrms: rms(drive),
        drive_vpk: peak(drive),
        coherence_mean: sysid.in_band_mean_coherence,
        min_mean_coherence: control.sysid.min_mean_coherence,
        trusted_bins: plant.trusted.iter().filter(|&&t| t).count(),
        band_energy: plant.coverage,
        passed: sysid.in_band_mean_coherence >= control.sysid.min_mean_coherence
            && plant.coverage >= control.min_band_energy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ClassicalShape;
    use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};

    fn spec() -> ClassicalPulseSpec {
        let mut s = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
        s.compensation = CompensationConfig {
            shape: CompensationShape::Rectangular,
            pre_frac: 4.0,
            post_frac: 4.0,
            symmetric: true,
        };
        s
    }

    #[test]
    fn the_grid_holds_the_pulse_in_its_quiet_margins_on_a_power_of_two() {
        let g = PulseGrid::new(&spec(), 51_200.0, Polarity::Positive);
        assert!(g.n.is_power_of_two());
        assert_eq!(g.n, 8192, "an 11 ms half-sine at 51.2 kHz");
        assert!(g.reference[..g.lead].iter().all(|&v| v == 0.0));
        assert!(g.reference[g.lead + g.composite_len..]
            .iter()
            .all(|&v| v == 0.0));
        let main_peak = g.reference[g.main_start..g.main_start + g.main_len]
            .iter()
            .fold(0.0f64, |m, &v| m.max(v));
        assert!((main_peak - 20.0).abs() < 0.01);
        let neg = PulseGrid::new(&spec(), 51_200.0, Polarity::Negative);
        assert!(neg
            .reference
            .iter()
            .zip(&g.reference)
            .all(|(a, b)| *a == -*b));
    }

    #[test]
    fn the_taper_brings_both_ends_to_zero_and_leaves_the_pulse_alone() {
        let g = PulseGrid::new(&spec(), 5120.0, Polarity::Positive);
        let mut d = vec![1.0; g.n];
        g.taper(&mut d);
        assert_eq!(d[0], 0.0);
        assert!(d[g.n - 1].abs() < 1e-3, "{}", d[g.n - 1]);
        assert!(d[g.lead / 2..g.lead + g.composite_len]
            .iter()
            .all(|&v| v == 1.0));
        assert!(d.windows(2).take(g.lead / 2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn divergence_counts_rises_in_a_row_at_one_level() {
        let mut c = ClosedLoop::new(PulseGrid::new(&spec(), 5120.0, Polarity::Positive));
        assert_eq!(c.diverging(-12.0, 10.0, 1.0, 3), 0);
        assert_eq!(c.diverging(-12.0, 12.0, 1.0, 3), 0);
        assert_eq!(c.diverging(-12.0, 14.0, 1.0, 3), 0);
        assert_eq!(c.diverging(-12.0, 16.0, 1.0, 3), 3);
        // A fall starts the count again, and so does a new level.
        assert_eq!(c.diverging(-12.0, 10.0, 1.0, 3), 0);
        assert_eq!(c.diverging(-12.0, 12.0, 1.0, 3), 0);
        assert_eq!(c.diverging(-9.0, 15.0, 1.0, 3), 0);
        assert_eq!(c.div_rises, 0);
    }
}
