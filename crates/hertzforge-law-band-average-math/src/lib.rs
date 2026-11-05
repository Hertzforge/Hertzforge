//! Brandon Zwink's band-average MIMO random-vibration control law.
//!
//! The law solves the inverse in fractional-octave space: for each band it finds
//! one flat-within-band correlated drive CPSD whose band-averaged response
//! matches the band-averaged specification, through the band cross-power
//! sensitivity `C[p, i, j, b] = < H[p, i] conj(H[p, j]) >_b`. Because the FRF is
//! averaged over each band before it is inverted, identification noise is not
//! amplified. The threshold kappa is chosen per band by a sweep at the first
//! solve and frozen. The rest is feedback and protection: gain correction, FRF
//! updates with conditioned drives, per-drive RMS limits and response limits.
//!
//! The CPSD convention is `X conj(Y)`, `S_yy = H S_xx H^H`. The control SDMs come
//! as packed stacks; everything else is flat and row-major, indexed
//! `[line][row][col]`, on the full grid including 0 Hz.
//!
//! This is a port of the Python law, and it follows its numerics and state
//! machine call for call. The factorizations are `hertzforge-sdm`'s, and the
//! bands' work is split over threads when [`BandAverage::set_threads`] says so;
//! neither changes what a call returns beyond rounding, and threads not at all.

mod bands;
mod fit;
mod linalg;
mod params;
mod sweep;

pub use bands::nth_octave_edges;
pub use hertzforge_sdm::C64;
pub use params::{KappaRule, Params, Regularization, TfUpdateBlend};

use bands::Bands;
use fit::{hermitized, psd_fit, real_dot};
use hertzforge_sdm::HermitianStack;
use linalg::{hermitian_eig, svd, HermitianEig, Svd};

/// One call's data. The two control SDMs are packed stacks, read in place; the
/// rest is flat, `[line][row][col]`, row-major. `n_lines` is the full grid
/// including 0 Hz.
///
/// `control` reads `specification` and `sysid_response_cpsd` on its first call
/// only, and `transfer_function` on its first call and on accepted FRF updates;
/// each may be absent or empty on any other call. `n_controls` counts limit
/// channels too.
pub struct Frame<'a> {
    /// `n_lines` values. Checked for length; the law's grid is `df * k`.
    pub frequencies: &'a [f64],
    /// Lines x controls x controls, at the level the drive is wanted for.
    pub specification: Option<&'a HermitianStack>,
    /// Lines x controls x drives. The system identification's on the first call,
    /// the running estimate afterwards.
    pub transfer_function: &'a [C64],
    /// Lines x controls x controls. Only `use_buzz` reads it.
    pub sysid_response_cpsd: Option<&'a HermitianStack>,
    /// Lines x controls. Only `min_coherence` reads it.
    pub multiple_coherence: Option<&'a [f64]>,
    /// CPSD frames behind the running estimates. On the first call this seeds the
    /// cumulative blend's average count; pass the system identification's count.
    pub frames: usize,
    /// The estimate is accepted only when `frames >= total_frames`.
    pub total_frames: usize,
    /// The measured response's autospectra, lines x controls. With
    /// `last_output_cpsd`, or the call is a fresh solve.
    pub last_response_asd: Option<&'a [f64]>,
    /// Lines x drives x drives.
    pub last_output_cpsd: Option<&'a [C64]>,
}

/// Which threshold a band solve uses.
#[derive(Clone, Copy)]
enum Kappa<'a> {
    Configured,
    Single(f64),
    PerBand(&'a [f64]),
}

/// A drive in band space: each band's flat `n_drives` by `n_drives` CPSD, empty
/// where the band has no drive.
type BandDrive = Vec<Vec<C64>>;

/// One band's sensitivity as [`BandAverage::band_sensitivity`] builds it.
struct BandBuilt {
    a: Vec<C64>,
    a_octave: Vec<f64>,
    c_cross: Vec<C64>,
    factored: Factored,
    /// The band was held out of the update, its conditioning load capped.
    held_band: bool,
    /// Rows the coherence gate held, and the rows it judged.
    held: usize,
    total: usize,
}

/// `f` of each of `bands`, in their order, split over up to `threads` threads.
fn over_bands<T: Send>(threads: usize, bands: &[usize], f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    if threads <= 1 || bands.len() < 2 {
        return bands.iter().map(|&b| f(b)).collect();
    }
    let chunk = bands.len().div_ceil(threads);
    let f = &f;
    std::thread::scope(|scope| {
        let workers: Vec<_> = bands
            .chunks(chunk)
            .map(|part| scope.spawn(move || part.iter().map(|&b| f(b)).collect::<Vec<T>>()))
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("a band's work does not panic"))
            .collect()
    })
}

/// One band's factored sensitivity, `k` singular values wide.
struct Factored {
    svd: Svd,
    k: usize,
}

/// The band-average control law's state.
pub struct BandAverage {
    params: Params,
    n_lines: usize,
    n_controls: usize,
    n_drives: usize,
    /// Threads the band solves are split over; 1 keeps them on the calling thread.
    threads: usize,
    log: Vec<String>,

    // Fixed at the first call, from the specification.
    initialized: bool,
    n_ctrl: usize,
    ctrl_idx: Vec<usize>,
    limit_idx: Vec<usize>,
    limit_asd: Vec<f64>,
    bands: Option<Bands>,
    /// Each band's drive lines: bin centre in the band and inside the spec's bandwidth.
    sel: Vec<Vec<usize>>,
    /// The Hz each drive line's bin covers in its band.
    wk: Vec<Vec<f64>>,
    spec_asd_octave: Vec<f64>,
    cross_pairs: Vec<(usize, usize)>,
    cross_spec_octave: Option<Vec<C64>>,

    // The model.
    built: bool,
    a_bands: Vec<Vec<C64>>,
    a_octave: Vec<Vec<f64>>,
    c_cross_octave: Vec<Vec<C64>>,
    factored: Vec<Option<Factored>>,
    h_lim: Vec<C64>,
    n_avg: f64,

    // Regularization.
    cond_number: Option<f64>,
    band_kappa: Option<Vec<f64>>,
    front: Option<Vec<Vec<f64>>>,
    front_i: usize,
    /// The RMS limit's last threshold, or last front point, once it has acted.
    kappa_rms: Option<f64>,

    // Feedback.
    gain: Option<Vec<f64>>,
    gain_solved: Vec<f64>,
    excluded: Option<Vec<bool>>,
    limit_trim: Option<Vec<f64>>,
    last_drive: Option<Vec<C64>>,
    load_capped: Vec<bool>,
    held_bands: usize,
    held_rows: f64,
    n_updates: usize,
}

impl BandAverage {
    /// A law for this grid. `n_controls` includes any limit channels.
    pub fn new(params: Params, n_lines: usize, n_controls: usize, n_drives: usize) -> Self {
        let cond_number = params.condition_number;
        BandAverage {
            params,
            n_lines,
            n_controls,
            n_drives,
            threads: 1,
            log: Vec::new(),
            initialized: false,
            n_ctrl: 0,
            ctrl_idx: Vec::new(),
            limit_idx: Vec::new(),
            limit_asd: Vec::new(),
            bands: None,
            sel: Vec::new(),
            wk: Vec::new(),
            spec_asd_octave: Vec::new(),
            cross_pairs: Vec::new(),
            cross_spec_octave: None,
            built: false,
            a_bands: Vec::new(),
            a_octave: Vec::new(),
            c_cross_octave: Vec::new(),
            factored: Vec::new(),
            h_lim: Vec::new(),
            n_avg: 0.0,
            cond_number,
            band_kappa: None,
            front: None,
            front_i: 0,
            kappa_rms: None,
            gain: None,
            gain_solved: Vec::new(),
            excluded: None,
            limit_trim: None,
            last_drive: None,
            load_capped: Vec::new(),
            held_bands: 0,
            held_rows: 0.0,
            n_updates: 0,
        }
    }

    /// One call of the Python generator's `control()`: the same state machine,
    /// returning the next drive CPSD, lines x drives x drives, at the level of
    /// the specification.
    pub fn control(&mut self, frame: &Frame) -> Result<Vec<C64>, String> {
        self.check_sizes(frame)?;
        if !self.initialized {
            self.initialize(frame)?;
        }
        if !self.built {
            if self.params.use_buzz && self.cross_spec_octave.is_none() {
                return Err(
                    "use_buzz needs a nonzero system identification response CPSD \
                            before control can run"
                        .into(),
                );
            }
            if !frame.transfer_function.iter().all(|x| x.is_finite()) {
                return Err("the first transfer function is not finite".into());
            }
            self.build_sensitivity(frame.transfer_function, false, None);
            self.built = true;
            self.n_avg = self.params.n_e_seed.unwrap_or(frame.frames as f64);
        }

        let (Some(response), Some(output)) = (frame.last_response_asd, frame.last_output_cpsd)
        else {
            let drive = self.solve_octave()?;
            self.last_drive = Some(drive.clone());
            return Ok(drive);
        };

        if !self.limit_idx.is_empty() {
            self.update_trim(response);
        }
        let mut predicted_pair = None;
        if self.params.gain_correction || self.params.use_tf_update {
            let pair = self.measured_predicted(response, output);
            self.check_channels(&pair.0, &pair.1);
            predicted_pair = Some(pair);
        }
        let resolve = match (&predicted_pair, self.params.gain_correction) {
            (Some(pair), true) => self.update_gain(&pair.0, &pair.1),
            _ => false,
        };

        if self.params.use_tf_update
            && frame.frames >= frame.total_frames
            && frame.transfer_function.iter().all(|x| x.is_finite())
        {
            self.build_sensitivity(frame.transfer_function, true, frame.multiple_coherence);
            let drive = self.solve_octave()?;
            self.last_drive = Some(drive);
            self.log_conditioning(output);
            return Ok(self.trimmed());
        }

        if resolve {
            let drive = self.solve_octave()?;
            self.last_drive = Some(drive);
        }
        if self.last_drive.is_some() {
            return Ok(self.trimmed());
        }
        Ok(output.to_vec())
    }

    /// Split the band solves over up to `n` threads. Each band's solve is its own,
    /// so the drive is the same at any count. A law file has one thread.
    pub fn set_threads(&mut self, n: usize) {
        self.threads = n.max(1);
    }

    /// The log lines since the last call. The law never prints.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// The per-band thresholds the sweep froze, once it has run.
    pub fn band_kappa(&self) -> Option<&[f64]> {
        self.band_kappa.as_deref()
    }

    /// The bands' lower and upper edges [Hz], once the first call has built them.
    pub fn band_edges(&self) -> Option<(&[f64], &[f64])> {
        self.bands
            .as_ref()
            .map(|b| (b.lb.as_slice(), b.ub.as_slice()))
    }

    /// Each band's drive lines on the grid, once the first call has built them.
    pub fn band_lines(&self) -> &[Vec<usize>] {
        &self.sel
    }

    /// The threshold each band's solve uses: the sweep's, or the fixed one. `None`
    /// under the plain pseudoinverse and before the first solve.
    pub fn band_threshold(&self, b: usize) -> Option<f64> {
        self.band_kappa
            .as_ref()
            .map(|k| k[b])
            .or(match self.params.regularization {
                Regularization::Auto => self.cond_number,
                Regularization::None => None,
            })
    }

    /// The gain correction's multiplier on each band's target, bands by control
    /// channels (limit channels left out), once it has run.
    pub fn band_gain(&self) -> Option<&[f64]> {
        self.gain.as_deref()
    }

    /// The channels the law controls, in the order of `band_gain`'s columns.
    pub fn control_channels(&self) -> &[usize] {
        &self.ctrl_idx
    }

    /// Control channels the channel check is leaving out as failed gauges.
    pub fn excluded_channels(&self) -> usize {
        self.excluded
            .as_ref()
            .map_or(0, |e| e.iter().filter(|x| **x).count())
    }

    /// FRF updates accepted so far.
    pub fn frf_updates(&self) -> usize {
        self.n_updates
    }

    /// Bands left out of the last FRF update because their conditioning load was
    /// capped.
    pub fn held_bands(&self) -> usize {
        self.held_bands
    }

    /// Bands whose conditioning load is capped in the drive in force.
    pub fn capped_bands(&self) -> usize {
        self.load_capped.iter().filter(|x| **x).count()
    }

    /// The point on the error-energy front the RMS limit last used, and the
    /// front's length, when the `front` rule runs under an RMS limit.
    pub fn front_point(&self) -> Option<(usize, usize)> {
        self.front.as_ref().map(|f| (self.front_i, f.len()))
    }

    fn check_sizes(&self, frame: &Frame) -> Result<(), String> {
        let (l, c, d) = (self.n_lines, self.n_controls, self.n_drives);
        let check = |name: &str, got: usize, want: usize| {
            if got == want {
                Ok(())
            } else {
                Err(format!("{name} has {got} values, expected {want}"))
            }
        };
        if l < 2 {
            return Err("the grid needs at least two lines".into());
        }
        check("frequencies", frame.frequencies.len(), l)?;
        let stack = |name: &str, g: Option<&HermitianStack>| match g {
            Some(g) if g.lines() == l && g.n() == c => Ok(()),
            Some(g) => Err(format!(
                "{name} is {} lines of {} channels, expected {l} of {c}",
                g.lines(),
                g.n()
            )),
            None => Err(format!("{name} is missing on the first call")),
        };
        if !self.initialized {
            stack("specification", frame.specification)?;
        }
        if !self.built || self.params.use_tf_update && frame.frames >= frame.total_frames {
            check(
                "transfer_function",
                frame.transfer_function.len(),
                l * c * d,
            )?;
        }
        if self.params.use_buzz && !self.initialized {
            stack("sysid_response_cpsd", frame.sysid_response_cpsd)?;
        }
        if let Some(x) = frame.multiple_coherence {
            check("multiple_coherence", x.len(), l * c)?;
        }
        if let Some(x) = frame.last_response_asd {
            check("last_response_asd", x.len(), l * c)?;
        }
        if let Some(x) = frame.last_output_cpsd {
            check("last_output_cpsd", x.len(), l * d * d)?;
        }
        Ok(())
    }

    /// The Python constructor: splits off the limit channels, builds the bands
    /// and the band targets.
    fn initialize(&mut self, frame: &Frame) -> Result<(), String> {
        if self.params.df <= 0.0 || self.params.df.is_nan() {
            return Err("df must be positive".into());
        }
        if let Some(limit) = &self.params.drive_rms_limit {
            if limit.len() != 1 && limit.len() != self.n_drives {
                return Err(format!(
                    "drive_rms_limit has {} values for {} drives",
                    limit.len(),
                    self.n_drives
                ));
            }
        }
        let c = self.n_controls;
        self.limit_idx = self
            .params
            .limit_channel_indices
            .iter()
            .map(|i| i - 1)
            .collect();
        if let Some(i) = self.limit_idx.iter().find(|&&i| i >= c) {
            return Err(format!(
                "limit channel {} is beyond the {c} channels",
                i + 1
            ));
        }
        self.ctrl_idx = (0..c).filter(|i| !self.limit_idx.contains(i)).collect();
        self.n_ctrl = self.ctrl_idx.len();
        let spec = frame.specification.expect("checked");
        self.limit_asd = Vec::with_capacity(self.n_lines * self.limit_idx.len());
        for k in 0..self.n_lines {
            for &l in &self.limit_idx {
                self.limit_asd.push(spec.get(k, l, l).re);
            }
        }

        let bands = Bands::new(self.params.df, self.params.oct_order, self.n_lines);
        let n_bands = bands.n_bands();
        let idx = self.ctrl_idx.clone();
        let asd = |k: usize, p: usize| spec.get(k, idx[p], idx[p]).re;

        let mut octave = vec![0.0; n_bands * self.n_ctrl];
        for b in 0..n_bands {
            for &(k, hz) in &bands.overlap[b] {
                let w = hz / bands.widths[b];
                for p in 0..self.n_ctrl {
                    octave[b * self.n_ctrl + p] += w * asd(k, p);
                }
            }
        }
        // A line where no control channel specifies energy gets no drive.
        let mut sel = vec![Vec::new(); n_bands];
        let mut wk = vec![Vec::new(); n_bands];
        for k in 0..self.n_lines {
            if let Some(b) = bands.band_of[k] {
                if (0..self.n_ctrl).any(|p| asd(k, p) > 0.0) {
                    sel[b].push(k);
                    wk[b].push(bands.line_overlap(b, k));
                }
            }
        }
        self.spec_asd_octave = octave;
        self.sel = sel;
        self.wk = wk;

        if self.params.use_buzz {
            let buzz = frame.sysid_response_cpsd.expect("checked");
            let entry = |k: usize, p: usize, q: usize| buzz.get(k, idx[p], idx[q]);
            self.set_cross_targets(&bands, |k, p, q| {
                let x = entry(k, p, q);
                let den = entry(k, p, p).re * entry(k, q, q).re;
                let coh = if den > 0.0 { x.norm_sqr() / den } else { 0.0 };
                C64::cis(x.im.atan2(x.re)).scale((coh * asd(k, p) * asd(k, q)).sqrt())
            });
        } else if self.params.control_to_cross_terms {
            self.set_cross_targets(&bands, |k, p, q| spec.get(k, idx[p], idx[q]));
        }
        self.bands = Some(bands);
        self.load_capped = vec![false; n_bands];
        self.initialized = true;
        Ok(())
    }

    /// Every channel pair whose cross-spectrum is nonzero anywhere becomes a
    /// target, as the complex band averages of that cross-spectrum.
    fn set_cross_targets(&mut self, bands: &Bands, cross: impl Fn(usize, usize, usize) -> C64) {
        let n = self.n_ctrl;
        let nonzero = |x: C64| x.re.abs() > 0.0 || x.im.abs() > 0.0;
        let mut pairs = Vec::new();
        for p in 0..n {
            for q in (p + 1)..n {
                if (0..self.n_lines).any(|k| nonzero(cross(k, p, q))) {
                    pairs.push((p, q));
                }
            }
        }
        if pairs.is_empty() {
            return;
        }
        let mut octave = vec![C64::ZERO; bands.n_bands() * pairs.len()];
        for b in 0..bands.n_bands() {
            for &(k, hz) in &bands.overlap[b] {
                let w = hz / bands.widths[b];
                for (i, &(p, q)) in pairs.iter().enumerate() {
                    octave[b * pairs.len() + i] += cross(k, p, q).scale(w);
                }
            }
        }
        self.cross_pairs = pairs;
        self.cross_spec_octave = Some(octave);
    }

    fn n_bands(&self) -> usize {
        self.sel.len()
    }

    fn spec_row(&self, b: usize) -> &[f64] {
        &self.spec_asd_octave[b * self.n_ctrl..(b + 1) * self.n_ctrl]
    }

    /// One band's sensitivity from `h`, averaged into the band's previous one by
    /// `w` when `do_blend`, and factored. It reads nothing of another band.
    fn band_sensitivity(
        &self,
        b: usize,
        h: &[C64],
        do_blend: bool,
        w: f64,
        gate: Option<(f64, &[f64])>,
    ) -> BandBuilt {
        let (c, d) = (self.n_controls, self.n_drives);
        let n_ctrl = self.n_ctrl;
        let n_pairs = self.cross_pairs.len();
        let n2 = d * d;
        let rows = n_ctrl + n_pairs;
        let (mut held, mut total) = (0usize, 0usize);
        let sel = &self.sel[b];
        let wk = &self.wk[b];
        let width = self.bands.as_ref().expect("initialized").widths[b];
        let wk_sum: f64 = wk.iter().sum();

        // Each row's weights: its coherent lines stand in for the whole band.
        let mut row_weights: Vec<Vec<f64>> = Vec::new();
        let mut frac = Vec::new();
        if let Some((threshold, coh)) = gate {
            let ok = |line: usize, p: usize| coh[line * c + self.ctrl_idx[p]] >= threshold;
            let mut weigh = |keep: &dyn Fn(usize) -> bool| {
                let raw: Vec<f64> = sel
                    .iter()
                    .zip(wk)
                    .map(|(&l, &hz)| if keep(l) { hz } else { 0.0 })
                    .collect();
                let fraction = raw.iter().sum::<f64>() / wk_sum;
                let divide = if fraction > 0.0 { fraction } else { 1.0 };
                row_weights.push(raw.iter().map(|x| x / divide).collect());
                frac.push(fraction);
            };
            for p in 0..n_ctrl {
                weigh(&|l| ok(l, p));
            }
            for &(p, q) in &self.cross_pairs {
                weigh(&|l| ok(l, p) && ok(l, q));
            }
        }
        let weights = |row: usize| -> &[f64] {
            if gate.is_some() {
                &row_weights[row]
            } else {
                wk
            }
        };

        let mut a = vec![C64::ZERO; rows * n2];
        let (auto_rows, cross_rows) = a.split_at_mut(n_ctrl * n2);
        for (p, row) in auto_rows.chunks_exact_mut(n2).enumerate() {
            let ci = self.ctrl_idx[p];
            accumulate(row, h, sel, weights(p), c, d, ci, ci, width);
        }
        for (i, row) in cross_rows.chunks_exact_mut(n2.max(1)).enumerate() {
            let (p, q) = self.cross_pairs[i];
            let (cp, cq) = (self.ctrl_idx[p], self.ctrl_idx[q]);
            accumulate(row, h, sel, weights(n_ctrl + i), c, d, cp, cq, width);
        }

        let held_band = do_blend
            && self.params.hold_capped_bands
            && self.load_capped[b]
            && !self.a_bands[b].is_empty();
        if held_band {
            a = self.a_bands[b].clone();
        } else if do_blend && !self.a_bands[b].is_empty() {
            let prev = &self.a_bands[b];
            let mut keep = vec![false; rows];
            if let Some(excluded) = &self.excluded {
                if excluded.iter().any(|&x| x) {
                    keep[..n_ctrl].copy_from_slice(excluded);
                    for (i, &(p, q)) in self.cross_pairs.iter().enumerate() {
                        keep[n_ctrl + i] = excluded[p] || excluded[q];
                    }
                }
            }
            if gate.is_some() {
                for (row, f) in frac.iter().enumerate() {
                    keep[row] |= *f < 0.5;
                }
                held += keep.iter().filter(|&&x| x).count();
                total += keep.len();
            }
            for (row, &kept) in keep.iter().enumerate() {
                let range = row * n2..(row + 1) * n2;
                if kept {
                    a[range.clone()].copy_from_slice(&prev[range]);
                } else {
                    for (x, old) in a[range.clone()].iter_mut().zip(&prev[range]) {
                        *x = old.scale(1.0 - w) + x.scale(w);
                    }
                }
            }
        }

        BandBuilt {
            a_octave: (0..n_ctrl)
                .map(|p| trace(&a[p * n2..(p + 1) * n2], d).re)
                .collect(),
            c_cross: (n_ctrl..rows)
                .map(|row| trace(&a[row * n2..(row + 1) * n2], d))
                .collect(),
            factored: Factored {
                svd: svd(&a, rows, n2),
                k: rows.min(n2),
            },
            a,
            held_band,
            held,
            total,
        }
    }

    /// Builds and factors the per-band sensitivity from `h`, averaged into the
    /// previous one when `blend` and FRF updates are on. `coherence` gates the
    /// update's lines per row when `min_coherence` is set.
    fn build_sensitivity(&mut self, h: &[C64], blend: bool, coherence: Option<&[f64]>) {
        let n_bands = self.n_bands();
        if self.a_bands.is_empty() {
            self.a_bands = vec![Vec::new(); n_bands];
            self.a_octave = vec![Vec::new(); n_bands];
            self.c_cross_octave = vec![Vec::new(); n_bands];
        }
        let do_blend = blend && self.params.use_tf_update && self.built;
        let mut w = 0.0;
        if do_blend {
            match self.params.tf_update_blend {
                TfUpdateBlend::Cumulative => {
                    w = 1.0 / (self.n_avg + 1.0);
                    self.n_avg += 1.0;
                }
                TfUpdateBlend::Beta(beta) => w = beta,
            }
        }
        let (c, d) = (self.n_controls, self.n_drives);
        if !self.limit_idx.is_empty() {
            let mut lim = Vec::with_capacity(self.n_lines * self.limit_idx.len() * d);
            for k in 0..self.n_lines {
                for &l in &self.limit_idx {
                    lim.extend_from_slice(&h[(k * c + l) * d..(k * c + l + 1) * d]);
                }
            }
            if do_blend && !self.h_lim.is_empty() {
                for (old, new) in self.h_lim.iter_mut().zip(&lim) {
                    *old = old.scale(1.0 - w) + new.scale(w);
                }
            } else {
                self.h_lim = lim;
            }
        }
        let gate = if do_blend {
            self.params.min_coherence.zip(coherence)
        } else {
            None
        };
        let bands: Vec<usize> = (0..n_bands).filter(|&b| !self.sel[b].is_empty()).collect();
        let law = &*self;
        let built = over_bands(self.threads, &bands, |b| {
            law.band_sensitivity(b, h, do_blend, w, gate)
        });
        let mut factored: Vec<Option<Factored>> = (0..n_bands).map(|_| None).collect();
        let (mut held, mut total, mut held_bands) = (0usize, 0usize, 0usize);
        for (&b, band) in bands.iter().zip(built) {
            self.a_octave[b] = band.a_octave;
            self.c_cross_octave[b] = band.c_cross;
            self.a_bands[b] = band.a;
            factored[b] = Some(band.factored);
            held += band.held;
            total += band.total;
            held_bands += usize::from(band.held_band);
        }
        self.factored = factored;
        self.held_bands = held_bands;
        if gate.is_some() {
            self.held_rows = held as f64 / total.max(1) as f64;
        }
    }

    /// Band `b`'s ASD targets: the specification, times the gain correction when active.
    fn target(&self, b: usize) -> Vec<f64> {
        let spec = self.spec_row(b);
        match &self.gain {
            None => spec.to_vec(),
            Some(g) => {
                let row = &g[b * self.n_ctrl..(b + 1) * self.n_ctrl];
                spec.iter().zip(row).map(|(s, g)| s * g).collect()
            }
        }
    }

    /// The regularized fit to the target: Hermitian, not always PSD.
    fn fit_band(&self, f: &Factored, filter: &[f64], target: &[C64]) -> Vec<C64> {
        let d = self.n_drives;
        let n2 = d * d;
        let mut g = vec![C64::ZERO; f.k];
        for (i, t) in target.iter().enumerate() {
            if *t == C64::ZERO {
                continue;
            }
            let row = &f.svd.u[i * f.k..(i + 1) * f.k];
            for (gj, u) in g.iter_mut().zip(row) {
                *gj += u.conj() * *t;
            }
        }
        for (gj, s) in g.iter_mut().zip(filter) {
            *gj = gj.scale(*s);
        }
        let mut p = vec![C64::ZERO; n2];
        for (x, v) in p.iter_mut().zip(f.svd.v.chunks_exact(f.k)) {
            for (vj, gj) in v.iter().zip(&g) {
                *x += *vj * *gj;
            }
        }
        hermitized(&p, d)
    }

    /// The target as the fit sees it: ASDs, then the band's complex cross terms.
    fn full_target(&self, b: usize, asd: &[f64], load: f64) -> Vec<C64> {
        let mut t: Vec<C64> = asd.iter().map(|&x| C64::new(x, 0.0)).collect();
        if let Some(cross) = &self.cross_spec_octave {
            let n_pairs = self.cross_pairs.len();
            let row = &cross[b * n_pairs..(b + 1) * n_pairs];
            if load == 0.0 {
                t.extend_from_slice(row);
            } else {
                t.extend(
                    row.iter()
                        .zip(&self.c_cross_octave[b])
                        .map(|(x, y)| *x - y.scale(load)),
                );
            }
        }
        t
    }

    fn diag_load(&self, lmax: f64, tr: f64, spec: &[f64], a: &[f64]) -> (f64, bool) {
        let mut d = (lmax / self.params.drive_condition_number)
            .max(self.params.min_drive_fraction * tr / self.n_drives as f64);
        let mut capped = false;
        let mut least = f64::INFINITY;
        for (s, x) in spec.iter().zip(a) {
            if *s > 0.0 && *x > 0.0 {
                least = least.min(s / x);
            }
        }
        if least.is_finite() {
            let cap = 0.5 * least;
            capped = cap < d;
            d = d.min(cap);
        }
        (d, capped)
    }

    /// The realizable (PSD) drive for band `b`. The fit is Hermitian but not
    /// always PSD; where dropping its negative eigenvalues moves some channel's
    /// predicted response by more than 0.1 dB, that response is re-fit over PSD
    /// drives instead. Always from this solve's own projection: a warm start from
    /// the previous frame's drive lets the drive energy ratchet up over re-solves.
    fn realizable(
        &self,
        b: usize,
        p_fit: &[C64],
        p_proj: Vec<C64>,
        w: Vec<f64>,
    ) -> Result<(Vec<C64>, Vec<f64>), String> {
        let least = w.iter().copied().fold(f64::INFINITY, f64::min);
        if !self.params.constrained_solve || self.cross_spec_octave.is_some() || least >= 0.0 {
            return Ok((p_proj, w));
        }
        let (n_ctrl, d) = (self.n_ctrl, self.n_drives);
        let a = &self.a_bands[b][..n_ctrl * d * d];
        let r_fit = auto_response(a, d, p_fit);
        let r_prj = auto_response(a, d, &p_proj);
        let mut gap = f64::NEG_INFINITY;
        for (f, p) in r_fit.iter().zip(&r_prj) {
            if *f > 0.0 {
                gap = gap.max((10.0 * (p.max(1e-300) / f).log10()).abs());
            }
        }
        if gap == f64::NEG_INFINITY || gap <= 0.1 {
            return Ok((p_proj, w));
        }
        let t: Vec<f64> = r_fit.iter().map(|x| x.max(0.0)).collect();
        let p = hermitized(&psd_fit(a, n_ctrl, d, &t, &p_proj)?, d);
        let values = hermitian_eig(&p, d).values;
        Ok((p, values))
    }

    fn solve_bands(&mut self, kappa: Kappa) -> Result<BandDrive, String> {
        let mut out = vec![Vec::new(); self.n_bands()];
        self.load_capped = vec![false; self.n_bands()];
        let bands: Vec<usize> = (0..self.n_bands())
            .filter(|&b| self.factored[b].is_some())
            .collect();
        let law = &*self;
        let solved = over_bands(self.threads, &bands, |b| law.solve_band(b, kappa));
        for (&b, result) in bands.iter().zip(solved) {
            let (p, capped) = result?;
            out[b] = p;
            self.load_capped[b] = capped;
        }
        Ok(out)
    }

    /// One band's drive and whether its conditioning load hit the cap. With FRF
    /// updates on, a diagonal load `d I` conditions the drive and the correlated
    /// part is re-solved against the deflated target, so compliance is unaffected.
    fn solve_band(&self, b: usize, kappa: Kappa) -> Result<(Vec<C64>, bool), String> {
        let d = self.n_drives;
        let f = self.factored[b].as_ref().expect("the band has a drive");
        let target = self.target(b);
        let filter = self.filter(b, f, kappa);
        let p_fit = self.fit_band(f, &filter, &self.full_target(b, &target, 0.0));
        let (w, p_proj) = projected(&p_fit, d);
        if w.iter().any(|x| !x.is_finite()) {
            return Err(format!("band {b}: the fit is not finite"));
        }
        if !self.params.use_tf_update {
            let (p, _) = self.realizable(b, &p_fit, p_proj, w)?;
            return Ok((p, false));
        }

        let a_b = &self.a_octave[b];
        let positive = |values: &[f64]| {
            let lmax = values.iter().fold(0.0f64, |m, x| m.max(x.max(0.0)));
            let tr: f64 = values.iter().map(|x| x.max(0.0)).sum();
            (lmax, tr)
        };
        let (lmax, tr) = positive(&w);
        let (mut load, mut capped) = self.diag_load(lmax, tr, &target, a_b);
        if load <= 0.0 || load.is_nan() {
            let (p, _) = self.realizable(b, &p_fit, p_proj, w)?;
            return Ok((p, capped));
        }
        // The re-solved drive can be larger than the first, so the load is re-sized
        // from it and the re-solve repeated until the load stops growing.
        let mut p;
        let mut it = 0;
        loop {
            let t2: Vec<f64> = target
                .iter()
                .zip(a_b)
                .map(|(s, a)| (s - load * a).max(0.0))
                .collect();
            let p2 = self.fit_band(f, &filter, &self.full_target(b, &t2, load));
            let (w2, proj2) = projected(&p2, d);
            let (solved, values) = self.realizable(b, &p2, proj2, w2)?;
            p = solved;
            let (lmax2, tr2) = positive(&values);
            let (load2, capped2) = self.diag_load(lmax2, tr2, &target, a_b);
            if load2 <= load * 1.001 || it == 9 {
                break;
            }
            load = load2;
            capped = capped2;
            it += 1;
        }
        for i in 0..d {
            p[i * d + i].re += load;
        }
        Ok((p, capped))
    }

    /// The band's singular-value filter `S / (S^2 + lambda)`, or the plain
    /// pseudoinverse's under `regularization:none`.
    fn filter(&self, b: usize, f: &Factored, kappa: Kappa) -> Vec<f64> {
        let s = &f.svd.s;
        let top = s.first().copied().unwrap_or(0.0);
        let kappa_b = match kappa {
            Kappa::Configured => self.band_kappa.as_ref().map(|k| k[b]),
            Kappa::Single(k) => Some(k),
            Kappa::PerBand(k) => Some(k[b]),
        };
        let threshold = kappa_b.or(if self.params.regularization == Regularization::None {
            None
        } else {
            self.cond_number
        });
        match threshold {
            Some(k) => {
                let lam = if top > 0.0 {
                    (top / k) * (top / k)
                } else {
                    0.0
                };
                s.iter().map(|&x| x / (x * x + lam)).collect()
            }
            None => {
                let rows = self.n_ctrl + self.cross_pairs.len();
                let floor = f64::EPSILON * rows.max(self.n_drives * self.n_drives) as f64 * top;
                s.iter()
                    .map(|&x| if x > floor { 1.0 / x } else { 0.0 })
                    .collect()
            }
        }
    }

    fn solve_octave(&mut self) -> Result<Vec<C64>, String> {
        if self.params.regularization == Regularization::Auto && self.cond_number.is_none() {
            self.resolve_auto_kappa()?;
        }
        let drive = if self.params.drive_rms_limit.is_none() {
            self.solve_bands(Kappa::Configured)?
        } else {
            self.rms_limited()?
        };
        let mut lines = self.expand(&drive);
        if !self.limit_idx.is_empty() {
            self.apply_limits(&mut lines);
        }
        Ok(lines)
    }

    /// A band drive on the full grid: flat within each band.
    fn expand(&self, drive: &BandDrive) -> Vec<C64> {
        let n2 = self.n_drives * self.n_drives;
        let mut out = vec![C64::ZERO; self.n_lines * n2];
        for (p, sel) in drive.iter().zip(&self.sel) {
            if !p.is_empty() {
                for &k in sel {
                    out[k * n2..(k + 1) * n2].copy_from_slice(p);
                }
            }
        }
        out
    }

    fn drive_rms(&self, drive: &BandDrive) -> Vec<f64> {
        let d = self.n_drives;
        let mut power = vec![0.0; d];
        for (p, sel) in drive.iter().zip(&self.sel) {
            if !p.is_empty() {
                for (i, x) in power.iter_mut().enumerate() {
                    *x += p[i * d + i].re * sel.len() as f64;
                }
            }
        }
        power.iter().map(|x| (x * self.params.df).sqrt()).collect()
    }

    fn rms_limit(&self, i: usize) -> f64 {
        let limit = self
            .params
            .drive_rms_limit
            .as_ref()
            .expect("the RMS limit is on");
        if limit.len() == 1 {
            limit[0]
        } else {
            limit[i]
        }
    }

    /// Scales each drive over its RMS limit down to it, a per-drive congruence
    /// that keeps coherence and phase. Returns the drive and the scale factors.
    pub(crate) fn limit_scaled(&self, drive: &BandDrive) -> (BandDrive, Vec<f64>) {
        let d = self.n_drives;
        let rms = self.drive_rms(drive);
        let sc: Vec<f64> = rms
            .iter()
            .enumerate()
            .map(|(i, &r)| (self.rms_limit(i) / if r > 0.0 { r } else { 1.0 }).min(1.0))
            .collect();
        let scaled = drive
            .iter()
            .map(|p| {
                let mut out = p.clone();
                if !out.is_empty() {
                    for i in 0..d {
                        for j in 0..d {
                            out[i * d + j] = out[i * d + j].scale(sc[i] * sc[j]);
                        }
                    }
                }
                out
            })
            .collect();
        (scaled, sc)
    }

    /// Mean |dB| band error of a drive against the current targets, through the
    /// band sensitivity.
    pub(crate) fn predicted_error(&self, drive: &BandDrive) -> f64 {
        let d = self.n_drives;
        let (mut e, mut n) = (0.0, 0usize);
        for (b, p) in drive.iter().enumerate() {
            if self.factored[b].is_none() {
                continue;
            }
            let t = self.target(b);
            if !t.iter().any(|&x| x > 0.0) {
                continue;
            }
            let a = &self.a_bands[b][..self.n_ctrl * d * d];
            let pred = auto_response(a, d, p);
            for (x, t) in pred.iter().zip(&t) {
                if *t > 0.0 {
                    e += (10.0 * (x.max(1e-300) / t).log10()).abs();
                    n += 1;
                }
            }
        }
        e / n.max(1) as f64
    }

    fn rms_limited(&mut self) -> Result<BandDrive, String> {
        if self.front.is_some() {
            return self.rms_limited_front();
        }
        let drive = self.solve_bands(Kappa::Configured)?;
        let rms = self.drive_rms(&drive);
        if rms.iter().enumerate().all(|(i, &r)| r <= self.rms_limit(i)) {
            self.kappa_rms = None;
            return Ok(drive);
        }
        // Searched afresh each time the limit binds, so the threshold can rise again
        // when the targets fall. Reusing the last threshold only ever lowered it, and
        // the error grew frame by frame under the gain correction.
        let hi = self.cond_number.unwrap_or(1e8);
        let count = (2.0 * hi.log10().ceil() + 1.0).clamp(3.0, 11.0) as usize;
        let mut grid = sweep::logspace(0.0, hi.log10(), count);
        if let Some(k) = self.kappa_rms {
            if !grid.contains(&k) {
                grid.push(k);
            }
        }
        let mut best: Option<(f64, f64, BandDrive, Vec<f64>)> = None;
        for kappa in grid {
            let solved = self.solve_bands(Kappa::Single(kappa))?;
            let (out, sc) = self.limit_scaled(&solved);
            let e = self.predicted_error(&out);
            if best.as_ref().is_none_or(|x| e < x.0) {
                best = Some((e, kappa, out, sc));
            }
        }
        let (err, kappa, out, sc) = best.expect("the grid is not empty");
        let moved = self.kappa_rms.is_none_or(|k| (kappa / k).ln().abs() > 0.1);
        if moved || sc.iter().any(|&s| s < 1.0) {
            self.log.push(format!(
                "band_average: drive RMS limit -> condition_number {}{} (predicted mean band error {err:.2} dB)",
                sweep::format_g(kappa, 3),
                scaled_note(&sc)
            ));
        }
        self.kappa_rms = Some(kappa);
        Ok(out)
    }

    /// The RMS limit with the `front` rule: tries last frame's point on the front
    /// and its two neighbours, and moves at most one step, so the choice follows
    /// the gain correction and the FRF updates without a search every frame.
    fn rms_limited_front(&mut self) -> Result<BandDrive, String> {
        let front = self.front.take().expect("the front was traced");
        let n = front.len();
        let j0 = self.front_i;
        let mut best: Option<(f64, usize, BandDrive, Vec<f64>)> = None;
        for (j, point) in front
            .iter()
            .enumerate()
            .take(j0 + 2)
            .skip(j0.saturating_sub(1))
        {
            let solved = self.solve_bands(Kappa::PerBand(point))?;
            let (out, sc) = self.limit_scaled(&solved);
            let e = self.predicted_error(&out);
            if best.as_ref().is_none_or(|x| e < x.0) {
                best = Some((e, j, out, sc));
            }
        }
        self.front = Some(front);
        let (e, j, out, sc) = best.expect("the point itself is on the front");
        if j != j0 || self.kappa_rms.is_none() {
            let d = self.n_drives;
            let mut total = 0.0;
            for (p, sel) in out.iter().zip(&self.sel) {
                if !p.is_empty() {
                    total += (0..d).map(|i| p[i * d + i].re).sum::<f64>() * sel.len() as f64;
                }
            }
            self.log.push(format!(
                "band_average: drive RMS limit -> front point {} of {n} (predicted band error {e:.2} dB, drive {}){}",
                j + 1,
                sweep::format_g(total * self.params.df, 3),
                scaled_note(&sc)
            ));
        }
        self.front_i = j;
        self.kappa_rms = Some(j as f64);
        Ok(out)
    }

    /// Prediction-based response limiting: scales the drive CPSD down, all drives
    /// together, at every line where a limit channel's predicted response exceeds
    /// its limit. NaN or <= 0 limits constrain nothing.
    fn apply_limits(&self, lines: &mut [C64]) {
        let d = self.n_drives;
        let n_lim = self.limit_idx.len();
        for k in 0..self.n_lines {
            let p = &mut lines[k * d * d..(k + 1) * d * d];
            if p.iter().all(|x| *x == C64::ZERO) {
                continue;
            }
            let mut s = f64::INFINITY;
            for l in 0..n_lim {
                let h = &self.h_lim[(k * n_lim + l) * d..(k * n_lim + l + 1) * d];
                let mut y = 0.0;
                for i in 0..d {
                    for j in 0..d {
                        y += (h[i] * p[i * d + j] * h[j].conj()).re;
                    }
                }
                let limit = self.limit_asd[k * n_lim + l];
                if limit > 0.0 && y > 0.0 {
                    s = s.min(limit / y);
                }
            }
            let s = s.min(1.0);
            for x in p.iter_mut() {
                *x = x.scale(s);
            }
        }
    }

    /// Where the measured limit-channel ASD exceeds its limit, steps the per-line
    /// trim down by half the exceedance in dB, damped because the measurement is a
    /// lagged multi-frame average. Where it is back under, relaxes up 0.1 dB per
    /// call toward 1, so noise-driven over-trim recovers.
    fn update_trim(&mut self, response: &[f64]) {
        let c = self.n_controls;
        let n_lim = self.limit_idx.len();
        let mut trim = self
            .limit_trim
            .take()
            .unwrap_or_else(|| vec![1.0; self.n_lines]);
        let relax = 10f64.powf(0.01);
        for (k, t) in trim.iter_mut().enumerate() {
            let mut r = f64::INFINITY;
            for (l, &ci) in self.limit_idx.iter().enumerate() {
                let measured = response[k * c + ci];
                let limit = self.limit_asd[k * n_lim + l];
                if limit > 0.0 && measured > 0.0 {
                    r = r.min(limit / measured);
                }
            }
            if r < 1.0 {
                *t *= r.sqrt();
            } else {
                *t = (*t * relax).min(1.0);
            }
        }
        self.limit_trim = Some(trim);
    }

    /// The commanded drive: the solved drive with the measured trim on top.
    fn trimmed(&self) -> Vec<C64> {
        let mut out = self.last_drive.clone().expect("a drive was solved");
        if let Some(trim) = &self.limit_trim {
            if trim.iter().any(|&t| t < 1.0) {
                let n2 = self.n_drives * self.n_drives;
                for (p, &t) in out.chunks_exact_mut(n2).zip(trim) {
                    for x in p {
                        *x = x.scale(t);
                    }
                }
            }
        }
        out
    }

    /// Every control channel's measured band response and the model's prediction
    /// from the measured drive, both from the caller's same CPSD window. Each is
    /// `n_bands` by `n_ctrl`, zero where a band has no drive.
    fn measured_predicted(&self, response: &[f64], output: &[C64]) -> (Vec<f64>, Vec<f64>) {
        let (c, d, n_ctrl) = (self.n_controls, self.n_drives, self.n_ctrl);
        let n2 = d * d;
        let bands = self.bands.as_ref().expect("initialized");
        let mut measured = vec![0.0; self.n_bands() * n_ctrl];
        let mut predicted = vec![0.0; self.n_bands() * n_ctrl];
        for b in 0..self.n_bands() {
            for &(k, hz) in &bands.overlap[b] {
                let w = hz / bands.widths[b];
                for (p, &ci) in self.ctrl_idx.iter().enumerate() {
                    measured[b * n_ctrl + p] += w * response[k * c + ci];
                }
            }
            if self.factored[b].is_none() {
                continue;
            }
            let mut mean = vec![C64::ZERO; n2];
            for (&k, &hz) in self.sel[b].iter().zip(&self.wk[b]) {
                for (x, y) in mean.iter_mut().zip(&output[k * n2..(k + 1) * n2]) {
                    *x += y.scale(hz);
                }
            }
            let total: f64 = self.wk[b].iter().sum();
            for x in &mut mean {
                *x = x.scale(1.0 / total);
            }
            let row = auto_response(&self.a_bands[b][..n_ctrl * n2], d, &mean);
            predicted[b * n_ctrl..(b + 1) * n_ctrl].copy_from_slice(&row);
        }
        (measured, predicted)
    }

    /// Flags control channels whose measured response over the specification band
    /// is more than `channel_check_db` from the model's prediction, a failed,
    /// disconnected or saturated gauge. Reports each change once.
    fn check_channels(&mut self, measured: &[f64], predicted: &[f64]) {
        let n_ctrl = self.n_ctrl;
        let was = self.excluded.take().unwrap_or_else(|| vec![false; n_ctrl]);
        let Some(limit) = self.params.channel_check_db else {
            self.excluded = Some(was);
            return;
        };
        let mut bad = vec![false; n_ctrl];
        for p in 0..n_ctrl {
            let (mut meas, mut pred) = (0.0, 0.0);
            for b in 0..self.n_bands() {
                if self.spec_asd_octave[b * n_ctrl + p] > 0.0 {
                    meas += measured[b * n_ctrl + p];
                    pred += predicted[b * n_ctrl + p];
                }
            }
            let level = if pred > 0.0 {
                10.0 * (meas.max(1e-300) / pred).log10()
            } else {
                0.0
            };
            bad[p] = pred > 0.0 && level.abs() > limit;
            let number = self.ctrl_idx[p] + 1;
            if bad[p] && !was[p] {
                self.log.push(format!(
                    "band_average: control channel {number} reads {level:+.0} dB from its prediction; \
                     excluded from the gain correction and the FRF update"
                ));
            }
            if !bad[p] && was[p] {
                self.log.push(format!(
                    "band_average: control channel {number} back within {} dB of its prediction; used again",
                    sweep::format_g(limit, 6)
                ));
            }
        }
        self.excluded = Some(bad);
    }

    /// Per-band, per-channel gain correction: the inverse of measured over predicted
    /// from the measured drive, smoothed and capped at +/-6 dB, multiplies the band's
    /// target. Measured over predicted rather than over spec, so intentional drive
    /// changes are in the prediction and never fought, and no integrator winds up on
    /// a lagged measurement. True when a multiplier moved enough to re-solve.
    fn update_gain(&mut self, measured: &[f64], predicted: &[f64]) -> bool {
        let n_ctrl = self.n_ctrl;
        let n = self.n_bands() * n_ctrl;
        let gain = self.gain.take().unwrap_or_else(|| {
            self.gain_solved = vec![1.0; n];
            vec![1.0; n]
        });
        let cap = (10f64.powf(0.6)).ln();
        let rate = self.params.gain_correction_rate;
        let mut lg: Vec<f64> = gain.iter().map(|g| g.ln()).collect();
        for b in 0..self.n_bands() {
            if self.factored[b].is_none() {
                continue;
            }
            for p in 0..n_ctrl {
                let i = b * n_ctrl + p;
                if self.spec_asd_octave[i] > 0.0 && predicted[i] > 0.0 && measured[i] > 0.0 {
                    lg[i] = (1.0 - rate) * lg[i] + rate * (predicted[i] / measured[i]).ln();
                }
            }
        }
        if let Some(excluded) = &self.excluded {
            for (p, _) in excluded.iter().enumerate().filter(|x| *x.1) {
                for b in 0..self.n_bands() {
                    lg[b * n_ctrl + p] = 0.0;
                }
            }
        }
        let gain: Vec<f64> = lg.iter().map(|x| x.clamp(-cap, cap).exp()).collect();
        let moved = gain
            .iter()
            .zip(&self.gain_solved)
            .map(|(g, s)| (10.0 * (g / s).log10()).abs())
            .fold(f64::NEG_INFINITY, f64::max);
        let resolve = moved > 0.05;
        if resolve {
            self.gain_solved = gain.clone();
        }
        self.gain = Some(gain);
        resolve
    }

    /// One log line per accepted update: how well conditioned the drive was for
    /// identification, commanded (what the load bounds) and measured. A 12 x 12 CPSD
    /// estimated from 20 averages is far worse conditioned than the one it estimates.
    fn log_conditioning(&mut self, output: &[C64]) {
        let d = self.n_drives;
        let n2 = d * d;
        let commanded = self.last_drive.as_ref().expect("just solved");
        let mut cmd = Vec::new();
        let mut med = Vec::new();
        let mut worst: Option<(f64, usize)> = None;
        for b in 0..self.n_bands() {
            if self.factored[b].is_none() {
                continue;
            }
            let per_line: Vec<(f64, f64)> = self.sel[b]
                .iter()
                .map(|&k| {
                    let v = hermitian_eig(&output[k * n2..(k + 1) * n2], d).values;
                    (v[0], v[d - 1])
                })
                .collect();
            let top = per_line.iter().fold(f64::NEG_INFINITY, |m, x| m.max(x.0));
            let ratios: Vec<f64> = per_line
                .iter()
                .map(|(hi, lo)| hi / lo.max(1e-300 * top))
                .collect();
            let m = median(&ratios);
            let first = self.sel[b][0];
            let c = hermitian_eig(&commanded[first * n2..(first + 1) * n2], d).values;
            let command = c[0] / c[d - 1].max(1e-300 * c[0]);
            if m.is_finite() {
                med.push(m);
                cmd.push(command);
            }
            if !m.is_nan() && worst.is_none_or(|w| m > w.0) {
                worst = Some((m, b));
            }
        }
        self.n_updates += 1;
        let Some((worst_cond, worst_band)) = worst else {
            return;
        };
        let bands = self.bands.as_ref().expect("initialized");
        let worst_hz = (bands.lb[worst_band] * bands.ub[worst_band]).sqrt();
        let bound = self.params.drive_condition_number + 1.0;
        let over = cmd.iter().filter(|&&x| x > bound).count();
        let capped = self.load_capped.iter().filter(|&&x| x).count();
        let mut line = format!(
            "band_average: update {}: commanded drive cond median {:.0}, worst {:.0}; measured median {:.0}, \
             worst {:.0} at {:.0} Hz; {}/{} bands commanded over {}, load capped in {}",
            self.n_updates,
            median(&cmd),
            cmd.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            median(&med),
            worst_cond,
            worst_hz,
            over,
            med.len(),
            sweep::format_g(bound, 6),
            capped
        );
        if self.params.hold_capped_bands {
            line += &format!(", held {} capped bands", self.held_bands);
        }
        if self.params.min_coherence.is_some() {
            line += &format!(
                ", coherence gate held {:.0} % of rows",
                100.0 * self.held_rows
            );
        }
        self.log.push(line);
    }
}

/// Adds `w H[p, i] conj(H[q, j]) / width` over the band's lines into a row of the sensitivity.
#[allow(clippy::too_many_arguments)]
fn accumulate(
    row: &mut [C64],
    h: &[C64],
    sel: &[usize],
    weights: &[f64],
    c: usize,
    d: usize,
    p: usize,
    q: usize,
    width: f64,
) {
    for (&line, &w) in sel.iter().zip(weights) {
        if w == 0.0 {
            continue;
        }
        let hp = &h[(line * c + p) * d..(line * c + p + 1) * d];
        let hq = &h[(line * c + q) * d..(line * c + q + 1) * d];
        for (i, x) in hp.iter().enumerate() {
            let (re, im) = (w * x.re, w * x.im);
            for (out, y) in row[i * d..(i + 1) * d].iter_mut().zip(hq) {
                out.re += re * y.re + im * y.im;
                out.im += im * y.re - re * y.im;
            }
        }
    }
    for x in row.iter_mut() {
        *x = x.scale(1.0 / width);
    }
}

fn trace(row: &[C64], d: usize) -> C64 {
    let mut t = C64::ZERO;
    for i in 0..d {
        t += row[i * d + i];
    }
    t
}

/// `Re(A vec(p))` over the rows of `a`, each `d * d` long.
fn auto_response(a: &[C64], d: usize, p: &[C64]) -> Vec<f64> {
    a.chunks_exact(d * d).map(|row| real_dot(row, p)).collect()
}

/// The eigenvalues of a Hermitian `p` and its nearest PSD matrix (negatives dropped).
fn projected(p: &[C64], d: usize) -> (Vec<f64>, Vec<C64>) {
    let HermitianEig { values, vectors } = hermitian_eig(p, d);
    let mut out = vec![C64::ZERO; d * d];
    for (r, &w) in values.iter().enumerate() {
        if w <= 0.0 {
            continue;
        }
        for i in 0..d {
            let x = vectors[i * d + r].scale(w);
            for j in 0..d {
                out[i * d + j] += x * vectors[j * d + r].conj();
            }
        }
    }
    (values, out)
}

pub(crate) fn median(x: &[f64]) -> f64 {
    if x.is_empty() || x.iter().any(|v| v.is_nan()) {
        return f64::NAN;
    }
    let mut s = x.to_vec();
    s.sort_by(f64::total_cmp);
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        0.5 * (s[n / 2 - 1] + s[n / 2])
    }
}

fn scaled_note(sc: &[f64]) -> String {
    if sc.iter().any(|&s| s < 0.999) {
        let db: Vec<String> = sc
            .iter()
            .map(|s| format!("{:.2}", 20.0 * s.log10()))
            .collect();
        format!(", drives scaled by [{}] dB", db.join(" "))
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `a b`: `a` is `m` by `k`, `b` is `k` by `n`.
    fn mul(a: &[C64], b: &[C64], m: usize, k: usize, n: usize) -> Vec<C64> {
        let mut out = vec![C64::ZERO; m * n];
        for i in 0..m {
            for l in 0..k {
                for j in 0..n {
                    out[i * n + j] += a[i * k + l] * b[l * n + j];
                }
            }
        }
        out
    }

    /// The upper triangles of `lines` full `n` by `n` matrices as a packed stack,
    /// kept for the test's life as the frames' other borrowed data is.
    fn packed(full: &[C64], lines: usize, n: usize) -> &'static HermitianStack {
        let mut g = HermitianStack::zeros(lines, n);
        for k in 0..lines {
            for r in 0..n {
                for c in r..n {
                    g.set(k, r, c, full[(k * n + r) * n + c]);
                }
            }
        }
        Box::leak(Box::new(g))
    }

    /// The conjugate transpose of the `m` by `n` matrix `a`.
    fn conj_transpose(a: &[C64], m: usize, n: usize) -> Vec<C64> {
        let mut out = vec![C64::ZERO; m * n];
        for i in 0..m {
            for j in 0..n {
                out[j * m + i] = a[i * n + j].conj();
            }
        }
        out
    }

    /// A small seeded plant: decoupled-ish smooth FRFs on a short grid, so the
    /// tests run in a blink and need no files.
    struct Plant {
        lines: usize,
        controls: usize,
        drives: usize,
        h: Vec<C64>,
        spec: Vec<C64>,
        frequencies: Vec<f64>,
    }

    fn plant(lines: usize, controls: usize, drives: usize, level: f64) -> Plant {
        let mut seed = 88172645463325252u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        let shapes: Vec<f64> = (0..controls * drives * 2).map(|_| next()).collect();
        let mut h = vec![C64::ZERO; lines * controls * drives];
        for k in 0..lines {
            let phase = k as f64 * 0.001;
            for c in 0..controls {
                for d in 0..drives {
                    let i = (c * drives + d) * 2;
                    h[(k * controls + c) * drives + d] = C64::new(
                        shapes[i] + 0.1 * (phase * (d + 1) as f64).sin(),
                        shapes[i + 1],
                    );
                }
            }
        }
        let mut spec = vec![C64::ZERO; lines * controls * controls];
        for k in 20..lines.min(400) {
            for c in 0..controls {
                spec[(k * controls + c) * controls + c] = C64::new(level, 0.0);
            }
        }
        let frequencies = (0..lines).map(|k| k as f64).collect();
        Plant {
            lines,
            controls,
            drives,
            h,
            spec,
            frequencies,
        }
    }

    impl Plant {
        fn frame<'a>(
            &'a self,
            h: &'a [C64],
            response: Option<&'a [C64]>,
            output: Option<&'a [C64]>,
            frames: usize,
        ) -> Frame<'a> {
            Frame {
                frequencies: &self.frequencies,
                specification: Some(packed(&self.spec, self.lines, self.controls)),
                transfer_function: h,
                sysid_response_cpsd: Some(packed(&self.spec, self.lines, self.controls)),
                multiple_coherence: None,
                frames,
                total_frames: 10,
                // The tests hold whole response matrices; the law reads the diagonals.
                last_response_asd: response.map(|r| {
                    let c = (r.len() / self.frequencies.len()).isqrt();
                    let asd: Vec<f64> = (0..r.len() / (c * c))
                        .flat_map(|k| (0..c).map(move |i| r[(k * c + i) * c + i].re))
                        .collect();
                    &*Box::leak(asd.into_boxed_slice())
                }),
                last_output_cpsd: output,
            }
        }

        fn law(&self, text: &str) -> BandAverage {
            BandAverage::new(
                Params::parse(text).unwrap(),
                self.lines,
                self.controls,
                self.drives,
            )
        }

        /// `H P H^H`, per line.
        fn response(&self, h: &[C64], drive: &[C64]) -> Vec<C64> {
            let (c, d) = (self.controls, self.drives);
            let mut out = vec![C64::ZERO; self.lines * c * c];
            for k in 0..self.lines {
                let hk = &h[k * c * d..(k + 1) * c * d];
                let pk = &drive[k * d * d..(k + 1) * d * d];
                let y = mul(&mul(hk, pk, c, d, d), &conj_transpose(hk, c, d), c, d, c);
                out[k * c * c..(k + 1) * c * c].copy_from_slice(&y);
            }
            out
        }
    }

    fn rms(drive: &[C64], d: usize) -> Vec<f64> {
        (0..d)
            .map(|i| {
                drive
                    .chunks_exact(d * d)
                    .map(|p| p[i * d + i].re)
                    .sum::<f64>()
                    .sqrt()
            })
            .collect()
    }

    fn scaled(values: &[C64], s: f64) -> Vec<C64> {
        values.iter().map(|x| x.scale(s)).collect()
    }

    const BASE: &str = "df:1.0\ndrive_rms_limit:none\ncondition_number:1000";

    #[test]
    fn the_first_solve_meets_the_band_targets_and_is_psd_and_flat_in_band() {
        let plant = plant(512, 6, 3, 2e-3);
        let mut law = plant.law(BASE);
        let drive = law.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
        let d = 3;
        assert_eq!(drive.len(), 512 * 9);
        // Inside the spec's bandwidth the drive is PSD, and outside it there is none.
        assert!(drive[..20 * 9].iter().all(|x| *x == C64::ZERO));
        assert!(drive[400 * 9..].iter().all(|x| *x == C64::ZERO));
        for k in 20..400 {
            let e = hermitian_eig(&drive[k * 9..(k + 1) * 9], d);
            assert!(e.values.iter().all(|&v| v >= -1e-12 * e.values[0]));
        }
        let bands = Bands::new(1.0, 6, 512);
        let response = plant.response(&plant.h, &drive);
        for b in 0..bands.n_bands() {
            let lines: Vec<usize> = (20..400).filter(|&k| bands.band_of[k] == Some(b)).collect();
            for w in lines.windows(2) {
                assert_eq!(
                    drive[w[0] * 9..(w[0] + 1) * 9],
                    drive[w[1] * 9..(w[1] + 1) * 9]
                );
            }
            if bands.ub[b] > 400.0 || bands.lb[b] < 25.0 || lines.is_empty() {
                continue;
            }
            // Six channels on three drives' nine parameters is solvable: the band's own
            // lines, through their bin overlaps, meet the band's integrated target.
            for p in 0..6 {
                let y: f64 = lines
                    .iter()
                    .map(|&k| bands.line_overlap(b, k) * response[(k * 6 + p) * 6 + p].re)
                    .sum();
                let want: f64 = bands.overlap[b].iter().map(|&(_, hz)| hz * 2e-3).sum();
                assert!(
                    (y / want - 1.0).abs() < 0.05,
                    "band {b} channel {p}: {y} against {want}"
                );
            }
        }
    }

    #[test]
    fn the_law_holds_the_drive_without_gain_correction_or_updates() {
        let plant = plant(256, 4, 2, 1e-3);
        let mut law = plant.law(&format!("{BASE}\ngain_correction:False"));
        let first = law.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
        let response = plant.response(&plant.h, &first);
        let next = law
            .control(&plant.frame(&plant.h, Some(&response), Some(&first), 10))
            .unwrap();
        assert_eq!(first, next);
    }

    #[test]
    fn the_gain_correction_is_blind_to_a_common_level_and_answers_a_model_error() {
        let plant = plant(256, 4, 2, 1e-3);
        let mut a = plant.law(BASE);
        let mut b = plant.law(BASE);
        let first = a.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
        let _ = b.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
        // The article reads 2 dB high on every channel.
        let response = scaled(&plant.response(&plant.h, &first), 10f64.powf(0.2));
        let next_a = a
            .control(&plant.frame(&plant.h, Some(&response), Some(&first), 10))
            .unwrap();
        // The same measurement at a tenth of the level.
        let (low_response, low_output) = (scaled(&response, 0.01), scaled(&first, 0.01));
        let next_b = b
            .control(&plant.frame(&plant.h, Some(&low_response), Some(&low_output), 10))
            .unwrap();
        for (x, y) in next_a.chunks_exact(4).zip(next_b.chunks_exact(4)) {
            let size: f64 = x.iter().map(|v| v.norm_sqr()).sum();
            let diff: f64 = x.iter().zip(y).map(|(u, v)| (*u - *v).norm_sqr()).sum();
            assert!(diff <= 1e-12 * size, "{diff} against {size}");
        }
        // A quarter of a 2 dB error comes off the targets at the default rate.
        let trace = |d: &[C64]| -> f64 { d.chunks_exact(4).map(|p| p[0].re + p[3].re).sum() };
        let ratio = trace(&next_a) / trace(&first);
        assert!((10.0 * ratio.log10() + 0.5).abs() < 0.05, "ratio {ratio}");
    }

    #[test]
    fn updates_wait_for_a_full_finite_window_and_are_blended_in() {
        let plant = plant(256, 4, 2, 1e-3);
        let text = format!("{BASE}\nuse_tf_update:True\ngain_correction:False");
        let mut law = plant.law(&text);
        let first = law.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
        let response = plant.response(&plant.h, &first);
        let doubled = scaled(&plant.h, 2.0);

        let partial = law
            .control(&plant.frame(&doubled, Some(&response), Some(&first), 9))
            .unwrap();
        assert_eq!(partial, first);
        let mut poisoned = doubled.clone();
        poisoned[500].re = f64::NAN;
        let held = law
            .control(&plant.frame(&poisoned, Some(&response), Some(&first), 10))
            .unwrap();
        assert_eq!(held, first);
        assert!(law.take_log().is_empty());

        let updated = law
            .control(&plant.frame(&doubled, Some(&response), Some(&first), 10))
            .unwrap();
        assert_ne!(updated, first);
        let log = law.take_log();
        assert_eq!(log.len(), 1);
        assert!(
            log[0].starts_with("band_average: update 1: commanded drive cond"),
            "{}",
            log[0]
        );
        // One frame of eleven with a plant reading 4 times the power: drive down by
        // (10 + 4) / 11 = 14/11 in power.
        let power = |d: &[C64]| -> f64 { d.chunks_exact(4).map(|p| p[0].re + p[3].re).sum() };
        assert!((power(&first) / power(&updated) - 14.0 / 11.0).abs() < 0.01);
    }

    #[test]
    fn the_conditioning_load_floors_every_drive_and_bounds_the_condition() {
        let plant = plant(256, 6, 3, 1e-3);
        let mut law = plant.law(&format!("{BASE}\nuse_tf_update:True"));
        let drive = law.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
        let k = 100;
        let e = hermitian_eig(&drive[k * 9..(k + 1) * 9], 3);
        assert!(e.values[2] > 0.0);
        assert!(e.values[0] / e.values[2] <= 101.0 + 1e-9, "{:?}", e.values);
    }

    #[test]
    fn the_rms_limit_is_held_in_the_solve() {
        let plant = plant(256, 4, 2, 1e-3);
        let mut free = plant.law("df:1.0\ndrive_rms_limit:none\nkappa_rule:knee");
        let unlimited = free
            .control(&plant.frame(&plant.h, None, None, 10))
            .unwrap();
        let top = rms(&unlimited, 2).into_iter().fold(0.0, f64::max);
        for rule in ["knee", "front"] {
            let mut law = plant.law(&format!(
                "df:1.0\nkappa_rule:{rule}\ndrive_rms_limit:{}",
                0.5 * top
            ));
            let drive = law.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
            for r in rms(&drive, 2) {
                assert!(
                    r <= 0.5 * top * (1.0 + 1e-12),
                    "{rule}: {r} against {}",
                    0.5 * top
                );
            }
            assert!(law.take_log().iter().any(|l| l.contains("drive RMS limit")));
        }
    }

    #[test]
    fn a_limit_channel_scales_the_drive_and_is_left_out_of_the_solve() {
        let mut plant = plant(256, 5, 2, 1e-3);
        // Channel 5's "specification" is a limit a tenth of what it would read.
        let mut free = plant.law(BASE);
        let baseline = free
            .control(&plant.frame(&plant.h, None, None, 10))
            .unwrap();
        let y = plant.response(&plant.h, &baseline);
        let c = plant.controls;
        for k in 0..plant.lines {
            let i = (k * c + 4) * c + 4;
            plant.spec[i] = y[i].scale(0.1);
        }
        let mut law = plant.law(&format!("{BASE}\nlimit_channel_indices:5"));
        let drive = law.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
        let response = plant.response(&plant.h, &drive);
        for k in 0..plant.lines {
            let i = (k * c + 4) * c + 4;
            let limit = plant.spec[i].re;
            if limit > 0.0 {
                assert!(response[i].re <= limit * (1.0 + 1e-9), "line {k}");
            }
        }
        // The measured trim takes half the exceedance in dB off the next drive.
        let high: Vec<C64> = scaled(&response, 4.0);
        let next = law
            .control(&plant.frame(&plant.h, Some(&high), Some(&drive), 10))
            .unwrap();
        let k = 100;
        let i = k * 4;
        assert!(next[i].re > 0.0);
        assert!(next[i].re < drive[i].re);
    }

    #[test]
    fn the_cross_terms_are_approached_when_asked_for() {
        let mut plant = plant(256, 4, 3, 1e-3);
        // A specification the plant can make: H S H^H from a correlated drive.
        let d = 3;
        let mut s = vec![C64::ZERO; 9];
        s[0] = C64::new(2e-3, 0.0);
        s[1] = C64::new(1e-3, 5e-4);
        s[3] = s[1].conj();
        s[4] = C64::new(2e-3, 0.0);
        s[8] = C64::new(1e-3, 0.0);
        let c = plant.controls;
        for k in 20..256 {
            let hk = &plant.h[k * c * d..(k + 1) * c * d];
            let y = mul(&mul(hk, &s, c, d, d), &conj_transpose(hk, c, d), c, d, c);
            plant.spec[k * c * c..(k + 1) * c * c].copy_from_slice(&y);
        }
        let mut with = plant.law(&format!("{BASE}\ncontrol_to_cross_terms:True"));
        let mut without = plant.law(BASE);
        let k = 100;
        let error = |law: &mut BandAverage, plant: &Plant| {
            let drive = law.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
            let y = plant.response(&plant.h, &drive);
            let (mut e, mut n) = (0.0, 0.0);
            for p in 0..c {
                for q in (p + 1)..c {
                    e += (y[(k * c + p) * c + q] - plant.spec[(k * c + p) * c + q]).norm_sqr();
                    n += plant.spec[(k * c + p) * c + q].norm_sqr();
                }
            }
            (e / n).sqrt()
        };
        let with_error = error(&mut with, &plant);
        let without_error = error(&mut without, &plant);
        assert!(
            with_error < 0.5 * without_error,
            "{with_error} against {without_error}"
        );
    }

    #[test]
    fn a_failed_gauge_is_named_and_recovers() {
        let plant = plant(256, 4, 2, 1e-3);
        let mut law = plant.law(BASE);
        let first = law.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
        let healthy = plant.response(&plant.h, &first);
        let mut failed = healthy.clone();
        for k in 0..plant.lines {
            failed[(k * 4 + 2) * 4 + 2] = failed[(k * 4 + 2) * 4 + 2].scale(1e-4);
        }
        let _ = law
            .control(&plant.frame(&plant.h, Some(&failed), Some(&first), 10))
            .unwrap();
        let log = law.take_log();
        assert!(
            log.iter()
                .any(|l| l.starts_with("band_average: control channel 3 reads -40 dB")),
            "{log:?}"
        );
        let _ = law
            .control(&plant.frame(&plant.h, Some(&healthy), Some(&first), 10))
            .unwrap();
        assert!(law
            .take_log()
            .iter()
            .any(|l| l.contains("control channel 3 back within 20 dB")));
    }

    #[test]
    fn a_bad_frame_is_an_error_and_not_a_panic() {
        let plant = plant(64, 2, 2, 1e-3);
        let mut law = plant.law(BASE);
        assert!(law
            .control(&plant.frame(&plant.h[1..], None, None, 10))
            .is_err());
        let mut unset = BandAverage::new(Params::default(), 64, 2, 2);
        assert!(unset
            .control(&plant.frame(&plant.h, None, None, 10))
            .is_err());
        let mut buzz = plant.law("df:1.0\nuse_buzz:True\ndrive_rms_limit:none");
        let zero = HermitianStack::zeros(64, 2);
        let mut frame = plant.frame(&plant.h, None, None, 10);
        frame.sysid_response_cpsd = Some(&zero);
        assert!(buzz.control(&frame).is_err());
    }

    /// Each band's solve is its own, so splitting the bands over threads changes
    /// no bit of the drive, through the sweep, the gain correction and an update.
    #[test]
    fn the_drive_is_the_same_on_any_number_of_threads() {
        let plant = plant(256, 6, 3, 1e-3);
        let run = |threads: usize| {
            let mut law = plant.law("df:1.0\ndrive_rms_limit:none\nuse_tf_update:True");
            law.set_threads(threads);
            let first = law.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
            let response = plant.response(&plant.h, &scaled(&first, 1.2));
            let second = law
                .control(&plant.frame(&plant.h, Some(&response), Some(&first), 10))
                .unwrap();
            (first, second)
        };
        let one = run(1);
        for threads in [2, 5, 64] {
            assert!(run(threads) == one, "{threads} threads");
        }
    }

    #[test]
    fn the_sweep_freezes_a_kappa_for_each_band_within_its_grid() {
        let plant = plant(256, 6, 3, 1e-3);
        for rule in ["front", "knee", "lcurve"] {
            let mut law = plant.law(&format!("df:1.0\ndrive_rms_limit:none\nkappa_rule:{rule}"));
            law.control(&plant.frame(&plant.h, None, None, 10)).unwrap();
            let kappa = law.band_kappa().unwrap();
            assert!(kappa.iter().all(|&k| (1.0..=1e5).contains(&k)), "{rule}");
            let log = law.take_log();
            assert!(log[0].contains(&format!("per band ({rule})")), "{log:?}");
        }
    }
}
